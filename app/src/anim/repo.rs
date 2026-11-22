//! steamdeckrepo.com catalog for the animation page (docs/animation-changer-analysis-2026-11-18.md §3).
//!
//! * `Post` / `parse_catalog`: `GET /api/posts/all` -> boot / suspend posts, mapped like the plugin's `get_steamdeckrepo`
//!   (plus duration, devices and creation date, which the plugin drops).
//! * `Query` / `filter`: the plugin's search (name substring, case-insensitive; DeckShell also matches the author),
//!   its five sort orders and the All / Boot / Suspend filter; DeckShell adds "Steam Deck only" and "not downloaded".
//! * `Net`: HTTP over `ureq` + rustls (webpki roots, no system TLS). Only `https://steamdeckrepo.com`; a redirect may only go to
//!   `*.steamdeckrepo.com` or the site's own storage bucket `steamdeckrepo.s3.<region>.backblazeb2.com` (the download link
//!   answers 302 there). Redirects are followed by hand so every hop is checked. `force_ipv4` = IPv4-only resolver.
//! * Downloads stream into `<dest>.part` with progress, cancellation, a size cap, a free-space check and a WebM signature
//!   check, then rename atomically. Nothing here runs unless the user asked for it (the page calls it).
use steam_assets::i18n::tr_ref;
use serde_json::{json, Value};
use steam_assets::i18n;
use std::io::{Read, Write};
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub const BASE: &str = "https://steamdeckrepo.com";
/// The catalog is ~8 MB (8903 posts on 2026-11-18); refuse absurd answers.
const MAX_CATALOG: u64 = 64 << 20;
/// Largest download accepted (the biggest of the owner's 31 files is 13 MB).
pub const MAX_VIDEO: u64 = 300 << 20;
pub const MAX_THUMB: u64 = 8 << 20;
/// The plugin's REQUEST_RETRIES.
const RETRIES: usize = 5;
const MAX_HOPS: usize = 5;

// ---------------------------------------------------------------- catalog model

#[derive(Clone, Debug, PartialEq)]
pub struct Post {
    pub id: String,
    pub name: String,
    pub author: String,
    pub description: String,
    pub thumbnail: String,
    pub video: String,
    /// seconds (`video_duration`), absent for one post
    pub duration: Option<f64>,
    pub created: String,
    pub updated: String,
    pub url: String,
    pub likes: i64,
    pub downloads: i64,
    /// "boot" | "suspend" (plugin: `suspend_video` -> suspend, else boot)
    pub target: &'static str,
    pub devices: Vec<String>,
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

impl Post {
    pub fn from_json(v: &Value) -> Option<Post> {
        let target = match v.get("type")?.as_str()? {
            "boot_video" => "boot",
            "suspend_video" => "suspend",
            _ => return None,
        };
        let id = s(v, "id");
        // ids become file names: keep them to the site's alphabet
        if id.is_empty() || id.len() > 32 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            return None;
        }
        Some(Post {
            name: s(v, "title").trim().to_string(),
            author: v.get("user").map(|u| s(u, "steam_name")).unwrap_or_default(),
            description: s(v, "content").replace("\r\n", "\n").trim().to_string(),
            thumbnail: s(v, "thumbnail"),
            video: s(v, "video"),
            duration: v.get("video_duration").and_then(|d| d.as_f64()).filter(|d| d.is_finite() && *d >= 0.0),
            created: s(v, "created_at"),
            updated: s(v, "updated_at"),
            url: s(v, "url"),
            likes: v.get("likes").and_then(|x| x.as_i64()).unwrap_or(0),
            downloads: v.get("downloads").and_then(|x| x.as_i64()).unwrap_or(0),
            target,
            devices: v.get("devices").and_then(|d| d.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
            id,
        })
    }

    pub fn download_url(&self, base: &str) -> String {
        format!("{base}/post/download/{}", self.id)
    }

    /// True when the post is meant for the Steam Deck (posts with no device list are treated as Deck posts).
    pub fn for_deck(&self) -> bool {
        self.devices.is_empty() || self.devices.iter().any(|d| d == "steam_deck")
    }

    /// Entry of the plugin's `downloads[]` list (same keys, so the plugin could read our config), plus DeckShell extras.
    pub fn to_entry(&self, base: &str) -> Value {
        json!({
            "id": self.id, "name": self.name, "preview_image": self.thumbnail, "preview_video": self.video, "author": self.author,
            "description": self.description, "last_changed": self.updated, "source": self.url, "download_url": self.download_url(base),
            "likes": self.likes, "downloads": self.downloads, "version": "", "target": self.target, "manifest_version": 1,
            "duration": self.duration, "devices": self.devices, "created_at": self.created,
        })
    }
}

pub struct Catalog {
    pub posts: Vec<Post>,
    /// `last_cached_at` of the site (unix seconds); kept for diagnostics, the page dates the catalog by its own fetch time
    #[allow(dead_code)]
    pub site_cached_at: Option<i64>,
}

pub fn parse_catalog(bytes: &[u8]) -> Result<Catalog, NetErr> {
    let v: Value = serde_json::from_slice(bytes).map_err(|e| NetErr::BadData(format!("JSON: {e}")))?;
    let arr = v.get("posts").and_then(|p| p.as_array()).ok_or_else(|| NetErr::BadData(tr_ref("anim_repo.missing_posts").into()))?;
    let mut seen = std::collections::HashSet::new();
    let posts: Vec<Post> = arr.iter().filter_map(Post::from_json).filter(|p| seen.insert(p.id.clone())).collect();
    Ok(Catalog { posts, site_cached_at: v.get("last_cached_at").and_then(|x| x.as_i64()) })
}

// ---------------------------------------------------------------- search / sort / filter

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Sort {
    #[default]
    Newest,
    Oldest,
    Alpha,
    Popular,
    Liked,
}

impl Sort {
    pub const ALL: [Sort; 5] = [Sort::Newest, Sort::Oldest, Sort::Alpha, Sort::Popular, Sort::Liked];
    pub fn label(self) -> &'static str {
        match self {
            Sort::Newest => i18n::tr_ref("anim_repo.newest"),
            Sort::Oldest => i18n::tr_ref("anim_repo.oldest"),
            Sort::Alpha => i18n::tr_ref("anim_repo.name"),
            Sort::Popular => i18n::tr_ref("anim_repo.popular"),
            Sort::Liked => i18n::tr_ref("anim_repo.liked"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Kind {
    #[default]
    All,
    Boot,
    Suspend,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::All, Kind::Boot, Kind::Suspend];
    pub fn label(self) -> &'static str {
        match self {
            Kind::All => i18n::tr_ref("anim_repo.all_types"),
            Kind::Boot => i18n::tr_ref("anim_repo.startup"),
            Kind::Suspend => i18n::tr_ref("anim_repo.sleep"),
        }
    }
    pub fn accepts(self, target: &str) -> bool {
        match self {
            Kind::All => true,
            Kind::Boot => target == "boot",
            Kind::Suspend => target == "suspend",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Query {
    pub text: String,
    pub sort: Sort,
    pub kind: Kind,
    pub deck_only: bool,
}

/// Lower-case haystack match of every whitespace-separated word of `text` (name or author).
pub fn matches_text(text: &str, name: &str, author: &str) -> bool {
    let hay = format!("{} {}", name.to_lowercase(), author.to_lowercase());
    text.to_lowercase().split_whitespace().all(|w| hay.contains(w))
}

/// Indices of `posts` passing the query, in display order. Ties keep the catalog order (stable sort).
pub fn filter(posts: &[Post], q: &Query) -> Vec<usize> {
    let mut idx: Vec<usize> = posts
        .iter()
        .enumerate()
        .filter(|(_, p)| q.kind.accepts(p.target) && (!q.deck_only || p.for_deck()) && matches_text(&q.text, &p.name, &p.author))
        .map(|(i, _)| i)
        .collect();
    match q.sort {
        Sort::Newest => idx.sort_by(|a, b| posts[*b].updated.cmp(&posts[*a].updated)),
        Sort::Oldest => idx.sort_by(|a, b| posts[*a].updated.cmp(&posts[*b].updated)),
        Sort::Alpha => idx.sort_by_cached_key(|i| posts[*i].name.to_lowercase()),
        Sort::Popular => idx.sort_by(|a, b| posts[*b].downloads.cmp(&posts[*a].downloads)),
        Sort::Liked => idx.sort_by(|a, b| posts[*b].likes.cmp(&posts[*a].likes)),
    }
    idx
}

/// "3 года назад" style age of an ISO-8601 timestamp (`2022-10-06T16:34:50.000000Z`).
pub fn relative_age(iso: &str, now_unix: i64) -> String {
    let Ok(t) = chrono::DateTime::parse_from_rfc3339(iso) else { return String::new() };
    let d = (now_unix - t.timestamp()).max(0);
    let (key, count) = if d < 3600 {
        return i18n::tr("anim_repo.just_now");
    } else if d < 86_400 {
        ("anim_repo.hour_ago", d / 3600)
    } else if d < 30 * 86_400 {
        ("anim_repo.day_ago", d / 86_400)
    } else if d < 365 * 86_400 {
        ("anim_repo.month_ago", d / (30 * 86_400))
    } else {
        ("anim_repo.year_ago", d / (365 * 86_400))
    };
    i18n::tr_plural(key, count, &[])
}

/// Compact download and like counts.
pub fn short_count(n: i64) -> String {
    let format = |v: f64, key| {
        let number = crate::app::comma(format!("{v:.1}").trim_end_matches(".0").to_owned());
        i18n::tr_args(key, &[("number", &number)])
    };
    if n >= 1_000_000 {
        format(n as f64 / 1e6, "anim_repo.million")
    } else if n >= 10_000 {
        format(n as f64 / 1e3, "anim_repo.thousand")
    } else {
        n.to_string()
    }
}

pub fn size_label(bytes: u64) -> String {
    let mb = bytes as f64 / (1024.0 * 1024.0);
    let (key, number) = if bytes >= 1 << 30 {
        ("anim_repo.gb", crate::app::comma(format!("{:.1}", mb / 1024.0)))
    } else if mb >= 10.0 {
        ("anim_repo.mb", format!("{mb:.0}"))
    } else if bytes >= 1 << 20 {
        ("anim_repo.mb", crate::app::comma(format!("{mb:.1}")))
    } else {
        ("anim_repo.kb", bytes.div_ceil(1024).to_string())
    };
    i18n::tr_args(key, &[("number", &number)])
}

// ---------------------------------------------------------------- network

#[derive(Clone, Debug, PartialEq)]
pub enum NetErr {
    /// no route / DNS / refused
    Offline(String),
    Timeout,
    RateLimited,
    Status(u16),
    HostNotAllowed(String),
    Tls(String),
    Cancelled,
    TooLarge,
    NoSpace { need: u64, free: u64 },
    NotVideo,
    Io(String),
    BadData(String),
}

impl NetErr {
    /// Localized message for the animation page.
    pub fn message(&self) -> String {
        match self {
            NetErr::Offline(_) => i18n::tr_ref("anim_repo.offline").into(),
            NetErr::Timeout => i18n::tr_ref("anim_repo.timeout").into(),
            NetErr::RateLimited => i18n::tr_ref("anim_repo.too_many_requests").into(),
            NetErr::Status(c) if *c >= 500 => i18n::tr_args("anim_repo.unavailable", &[("code", &c.to_string())]),
            NetErr::Status(404) => i18n::tr_ref("anim_repo.not_found").into(),
            NetErr::Status(c) => i18n::tr_args("anim_repo.rejected", &[("code", &c.to_string())]),
            NetErr::HostNotAllowed(_) => i18n::tr_ref("anim_repo.redirect").into(),
            NetErr::Tls(_) => i18n::tr_ref("anim_repo.tls").into(),
            NetErr::Cancelled => i18n::tr_ref("anim_repo.cancelled").into(),
            NetErr::TooLarge => i18n::tr_ref("anim_repo.too_large").into(),
            NetErr::NoSpace { need, free } => i18n::tr_args("anim_repo.no_space", &[("need", &size_label(*need)), ("free", &size_label(*free))]),
            NetErr::NotVideo => i18n::tr_ref("anim_repo.not_webm").into(),
            NetErr::Io(_) => i18n::tr_ref("anim_repo.disk_write").into(),
            NetErr::BadData(_) => i18n::tr_ref("anim_repo.bad_data").into(),
        }
    }
    /// Worth retrying (the plugin retries any non-200 status; 429 is final there too).
    fn retryable(&self) -> bool {
        matches!(self, NetErr::Status(c) if *c >= 500) || matches!(self, NetErr::Timeout)
    }
}

/// (scheme, host, port) of an absolute URL; None for anything odd (userinfo, missing host, unknown scheme).
pub fn split_url(url: &str) -> Option<(&str, String, Option<u16>)> {
    let (scheme, rest) = url.split_once("://")?;
    if scheme != "https" && scheme != "http" {
        return None;
    }
    let auth = rest.split(['/', '?', '#']).next()?;
    if auth.is_empty() || auth.contains('@') {
        return None;
    }
    let (host, port) = match auth.rsplit_once(':') {
        Some((h, p)) if !h.contains(']') || h.ends_with(']') => (h, Some(p.parse::<u16>().ok()?)),
        _ => (auth, None),
    };
    if host.is_empty() {
        return None;
    }
    Some((scheme, host.to_ascii_lowercase(), port))
}

pub struct Net {
    pub base: String,
    pub ipv4: bool,
    /// extra `host:port` allowed over plain http (local test servers only; empty in the app)
    pub test_hosts: Vec<String>,
    pub timeout: Duration,
}

impl Net {
    pub fn new(ipv4: bool) -> Net {
        Net { base: BASE.into(), ipv4, test_hosts: vec![], timeout: Duration::from_secs(30) }
    }

    pub fn allowed(&self, url: &str) -> bool {
        let Some((scheme, host, port)) = split_url(url) else { return false };
        if !self.test_hosts.is_empty() {
            let hp = format!("{host}:{}", port.unwrap_or(80));
            if self.test_hosts.iter().any(|t| *t == hp) {
                return true;
            }
        }
        scheme == "https"
            && port.is_none_or(|p| p == 443)
            && (host == "steamdeckrepo.com"
                || host.ends_with(".steamdeckrepo.com")
                || (host.starts_with("steamdeckrepo.s3.") && host.ends_with(".backblazeb2.com")))
    }

    fn agent(&self) -> ureq::Agent {
        let mut b = ureq::AgentBuilder::new()
            .redirects(0)
            .timeout_connect(Duration::from_secs(10))
            .timeout_read(self.timeout)
            .timeout_write(self.timeout)
            .user_agent("DeckShell-AnimationPage/1");
        if self.ipv4 {
            b = b.resolver(|addr: &str| -> std::io::Result<Vec<SocketAddr>> { Ok(addr.to_socket_addrs()?.filter(|a| a.is_ipv4()).collect()) });
        }
        b.build()
    }

    /// GET with hand-followed, host-checked redirects. Returns the final 2xx response.
    fn get(&self, url: &str, cancel: Option<&AtomicBool>) -> Result<ureq::Response, NetErr> {
        let agent = self.agent();
        let mut url = url.to_string();
        for _ in 0..=MAX_HOPS {
            if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
                return Err(NetErr::Cancelled);
            }
            if !self.allowed(&url) {
                return Err(NetErr::HostNotAllowed(split_url(&url).map(|u| u.1).unwrap_or_default()));
            }
            let resp = match agent.get(&url).call() {
                Ok(r) => r,
                Err(ureq::Error::Status(429, _)) => return Err(NetErr::RateLimited),
                Err(ureq::Error::Status(c, _)) => return Err(NetErr::Status(c)),
                Err(ureq::Error::Transport(t)) => return Err(transport(&t)),
            };
            match resp.status() {
                200..=299 => return Ok(resp),
                301 | 302 | 303 | 307 | 308 => {
                    let loc = resp.header("location").unwrap_or("").to_string();
                    url = if loc.starts_with('/') {
                        let (scheme, host, port) = split_url(&url).ok_or_else(|| NetErr::BadData(tr_ref("anim_repo.invalid_address").into()))?;
                        format!("{scheme}://{host}{}{loc}", port.map(|p| format!(":{p}")).unwrap_or_default())
                    } else {
                        loc
                    };
                }
                c => return Err(NetErr::Status(c)),
            }
        }
        Err(NetErr::BadData(tr_ref("anim_repo.redirect_limit").into()))
    }

    /// Whole body into memory, at most `max` bytes.
    pub fn fetch(&self, url: &str, max: u64, cancel: Option<&AtomicBool>) -> Result<Vec<u8>, NetErr> {
        let resp = self.get(url, cancel)?;
        if resp.header("content-length").and_then(|v| v.parse::<u64>().ok()).is_some_and(|n| n > max) {
            return Err(NetErr::TooLarge);
        }
        let mut out = Vec::new();
        let mut rd = resp.into_reader().take(max + 1);
        let mut buf = [0u8; 64 * 1024];
        loop {
            if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
                return Err(NetErr::Cancelled);
            }
            let n = rd.read(&mut buf).map_err(|e| io_err(&e))?;
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
        }
        if out.len() as u64 > max {
            return Err(NetErr::TooLarge);
        }
        Ok(out)
    }

    /// The catalog JSON, retried like the plugin (5 attempts; 429 stops at once).
    pub fn fetch_catalog(&self) -> Result<Vec<u8>, NetErr> {
        let url = format!("{}/api/posts/all", self.base);
        let mut last = NetErr::Timeout;
        for attempt in 0..RETRIES {
            match self.fetch(&url, MAX_CATALOG, None) {
                Ok(b) => return Ok(b),
                Err(e) if e.retryable() && attempt + 1 < RETRIES => {
                    eprintln!("[animrepo] catalog attempt {} failed: {e:?}", attempt + 1);
                    std::thread::sleep(Duration::from_millis(400 * (attempt as u64 + 1)));
                    last = e;
                }
                Err(e) => return Err(e),
            }
        }
        Err(last)
    }

    /// Stream `url` into `dest` (via `dest.part`, atomic rename). `progress(done, total)` is called per chunk.
    /// `free_space` returns the free bytes of `dest`'s file system (injected for tests).
    pub fn download(
        &self,
        url: &str,
        dest: &Path,
        cancel: &AtomicBool,
        free_space: &dyn Fn(&Path) -> Option<u64>,
        progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<u64, NetErr> {
        let dir = dest.parent().ok_or_else(|| NetErr::Io(tr_ref("anim_repo.missing_directory").into()))?;
        std::fs::create_dir_all(dir).map_err(|e| io_err(&e))?;
        let resp = self.get(url, Some(cancel))?;
        let total = resp.header("content-length").and_then(|v| v.parse::<u64>().ok());
        if total.is_some_and(|t| t > MAX_VIDEO) {
            return Err(NetErr::TooLarge);
        }
        // keep 64 MB spare for the frame cache and the system
        let reserve = 64 << 20;
        if let (Some(t), Some(free)) = (total, free_space(dir)) {
            if t + reserve > free {
                return Err(NetErr::NoSpace { need: t, free: free.saturating_sub(reserve) });
            }
        }
        let part = part_path(dest);
        let res = (|| {
            let mut f = std::fs::File::create(&part).map_err(|e| io_err(&e))?;
            let mut rd = resp.into_reader();
            let mut buf = vec![0u8; 128 * 1024];
            let mut done = 0u64;
            let mut head: Vec<u8> = Vec::with_capacity(4);
            progress(0, total);
            loop {
                if cancel.load(Ordering::Relaxed) {
                    return Err(NetErr::Cancelled);
                }
                let n = rd.read(&mut buf).map_err(|e| io_err(&e))?;
                if n == 0 {
                    break;
                }
                if head.len() < 4 {
                    head.extend(buf[..n].iter().take(4 - head.len()));
                    if head.len() == 4 && head != [0x1Au8, 0x45, 0xDF, 0xA3] {
                        return Err(NetErr::NotVideo);
                    }
                }
                done += n as u64;
                if done > MAX_VIDEO {
                    return Err(NetErr::TooLarge);
                }
                f.write_all(&buf[..n]).map_err(|e| io_err(&e))?;
                progress(done, total);
            }
            if head.len() < 4 {
                return Err(NetErr::NotVideo);
            }
            if total.is_some_and(|t| t != done) {
                return Err(NetErr::BadData(tr_ref("anim_repo.incomplete_download").into()));
            }
            f.sync_all().map_err(|e| io_err(&e))?;
            Ok(done)
        })();
        match res {
            Ok(n) => {
                std::fs::rename(&part, dest).map_err(|e| io_err(&e))?;
                Ok(n)
            }
            Err(e) => {
                let _ = std::fs::remove_file(&part);
                Err(e)
            }
        }
    }
}

pub fn part_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(".part");
    PathBuf::from(s)
}

fn io_err(e: &std::io::Error) -> NetErr {
    match e.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => NetErr::Timeout,
        _ if e.raw_os_error() == Some(libc::ENOSPC) => NetErr::NoSpace { need: 0, free: 0 },
        _ => NetErr::Io(e.to_string()),
    }
}

fn transport(t: &ureq::Transport) -> NetErr {
    let msg = t.to_string();
    let low = msg.to_lowercase();
    match t.kind() {
        _ if low.contains("timed out") || low.contains("timeout") => NetErr::Timeout,
        ureq::ErrorKind::Dns | ureq::ErrorKind::ConnectionFailed => NetErr::Offline(msg),
        _ if low.contains("certificate") || low.contains("tls") || low.contains("handshake") => NetErr::Tls(msg),
        ureq::ErrorKind::Io => NetErr::Offline(msg),
        _ => NetErr::BadData(msg),
    }
}

/// Free bytes of the file system holding `p` (statvfs).
pub fn free_space(p: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(p.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    (unsafe { libc::statvfs(c.as_ptr(), &mut st) } == 0).then(|| st.f_bavail as u64 * st.f_frsize as u64)
}

// ---------------------------------------------------------------- disk cache of the catalog / thumbnails / previews

pub struct RepoCache {
    pub dir: PathBuf,
}

impl RepoCache {
    pub fn catalog_path(&self) -> PathBuf {
        self.dir.join("posts.json")
    }
    pub fn thumb_path(&self, id: &str) -> PathBuf {
        self.dir.join("thumbs").join(format!("{id}.png"))
    }
    pub fn preview_path(&self, id: &str) -> PathBuf {
        self.dir.join("previews").join(format!("{id}.webm"))
    }
    /// Cached catalog and its age in seconds.
    pub fn load_catalog(&self) -> Option<(Catalog, u64)> {
        let p = self.catalog_path();
        let age = p.metadata().ok()?.modified().ok()?.elapsed().map(|d| d.as_secs()).unwrap_or(0);
        let bytes = std::fs::read(&p).ok()?;
        parse_catalog(&bytes).ok().map(|c| (c, age))
    }
    pub fn store_catalog(&self, bytes: &[u8]) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let tmp = self.dir.join("posts.json.tmp");
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(tmp, self.catalog_path())
    }
    /// Thumbnails and previews over `budget` bytes are removed oldest first.
    pub fn prune(&self, budget: u64) {
        let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = vec![];
        for sub in ["thumbs", "previews"] {
            if let Ok(rd) = std::fs::read_dir(self.dir.join(sub)) {
                for e in rd.flatten() {
                    if let Ok(m) = e.metadata() {
                        files.push((m.modified().unwrap_or(std::time::UNIX_EPOCH), m.len(), e.path()));
                    }
                }
            }
        }
        let mut total: u64 = files.iter().map(|f| f.1).sum();
        files.sort();
        for (_, len, p) in files {
            if total <= budget {
                break;
            }
            if std::fs::remove_file(&p).is_ok() {
                total -= len;
            }
        }
    }
    pub fn clear_media(&self) {
        let _ = std::fs::remove_dir_all(self.dir.join("thumbs"));
        let _ = std::fs::remove_dir_all(self.dir.join("previews"));
    }
}

pub fn dir_size(p: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(p) else { return 0 };
    rd.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

/// Fetch a thumbnail, shrink it to at most `w`x`h` and store it as PNG (the site's PNGs are 1280x800, ~400 KB).
pub fn fetch_thumb(net: &Net, cache: &RepoCache, id: &str, url: &str, w: u32, h: u32) -> Result<PathBuf, NetErr> {
    let dest = cache.thumb_path(id);
    if dest.is_file() {
        return Ok(dest);
    }
    let bytes = net.fetch(url, MAX_THUMB, None)?;
    let img = image::load_from_memory(&bytes).map_err(|e| NetErr::BadData(e.to_string()))?;
    let img = if img.width() > w || img.height() > h { img.thumbnail(w, h) } else { img };
    std::fs::create_dir_all(dest.parent().unwrap()).map_err(|e| io_err(&e))?;
    let tmp = dest.with_extension("png.tmp");
    img.to_rgba8().save_with_format(&tmp, image::ImageFormat::Png).map_err(|e| NetErr::Io(e.to_string()))?;
    std::fs::rename(&tmp, &dest).map_err(|e| io_err(&e))?;
    Ok(dest)
}

// ---------------------------------------------------------------- tests (local fixtures, no internet)

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::screens::expected_text;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use std::sync::Arc;

    pub const FIXTURE: &str = include_str!("../../tests/fixtures/steamdeckrepo-posts.json");

    #[test]
    fn parses_the_site_format_like_the_plugin() {
        let c = parse_catalog(FIXTURE.as_bytes()).unwrap();
        assert_eq!(c.site_cached_at, Some(1794952819));
        // the fixture has 8 posts: one unknown type and one duplicate id are dropped
        assert_eq!(c.posts.len(), 6);
        let p = &c.posts[0];
        assert_eq!((p.id.as_str(), p.name.as_str(), p.author.as_str(), p.target), ("BootA", "Starfield Intro", "Fixture Creator A", "boot"));
        assert_eq!(p.duration, Some(10.0));
        assert_eq!(p.download_url(BASE), "https://steamdeckrepo.com/post/download/BootA");
        let s = c.posts.iter().find(|p| p.id == "Sleep").unwrap();
        assert_eq!(s.target, "suspend");
        assert!(s.description.is_empty(), "null content becomes empty");
        let e = p.to_entry(BASE);
        for k in ["id", "name", "preview_image", "preview_video", "author", "description", "last_changed", "source", "download_url", "likes", "downloads", "version", "target", "manifest_version"] {
            assert!(e.get(k).is_some(), "plugin key {k}");
        }
        assert_eq!(e["last_changed"], json!(p.updated));
    }

    #[test]
    fn rejects_broken_catalogs() {
        assert!(matches!(parse_catalog(b"<html>"), Err(NetErr::BadData(_))));
        assert!(matches!(parse_catalog(b"{\"data\":[]}"), Err(NetErr::BadData(_))));
        let bad_id = br#"{"posts":[{"id":"../x","type":"boot_video","title":"t"}]}"#;
        assert_eq!(parse_catalog(bad_id).unwrap().posts.len(), 0);
    }

    #[test]
    fn search_sort_and_type_filters() {
        let c = parse_catalog(FIXTURE.as_bytes()).unwrap();
        let ids = |q: &Query| filter(&c.posts, q).into_iter().map(|i| c.posts[i].id.clone()).collect::<Vec<_>>();
        let mut q = Query::default();
        assert_eq!(ids(&q).len(), 6);
        q.sort = Sort::Popular;
        assert_eq!(ids(&q)[0], "BootB");
        q.sort = Sort::Liked;
        assert_eq!(ids(&q)[0], "BootB");
        q.sort = Sort::Oldest;
        assert_eq!(ids(&q)[0], "BootA");
        q.sort = Sort::Newest;
        assert_eq!(ids(&q)[0], "Mach1");
        q.sort = Sort::Alpha;
        assert_eq!(ids(&q).first().map(String::as_str), Some("BootC")); // "90's ..." sorts before letters
        q.kind = Kind::Suspend;
        assert_eq!(ids(&q), vec!["Sleep"]);
        q.kind = Kind::All;
        q.text = "STAR".into();
        assert_eq!(ids(&q), vec!["BootA"]);
        q.text = "creator b".into(); // author
        assert_eq!(ids(&q), vec!["BootB"]);
        q.text = "заставка".into(); // Cyrillic title, case-insensitive
        assert_eq!(ids(&q), vec!["RuZ01"]);
        q.text.clear();
        q.deck_only = true;
        assert!(!ids(&q).contains(&"Mach1".to_string()));
    }

    #[test]
    fn russian_numbers() {
        let ru = i18n::by_code("ru").unwrap();
        assert_eq!(ru.tr_plural("anim_repo.year_ago", 1, &[]), "1 год назад");
        assert_eq!(ru.tr_plural("anim_repo.year_ago", 3, &[]), "3 года назад");
        assert_eq!(ru.tr_plural("anim_repo.year_ago", 11, &[]), "11 лет назад");
        assert_eq!(ru.tr_plural("anim_repo.day_ago", 22, &[]), "22 дня назад");
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-18T00:00:00Z").unwrap().timestamp();
        assert_eq!(relative_age("2022-10-06T16:34:50.000000Z", now), expected_text("4 года назад", "4 years ago"));
        assert_eq!(relative_age("2026-11-15T00:00:00Z", now), expected_text("3 дня назад", "3 days ago"));
        assert_eq!(relative_age("bad", now), "");
        assert_eq!(short_count(35674), expected_text("35,7 тыс.", "35.7K"));
        assert_eq!(short_count(980), "980");
        assert_eq!(short_count(2_000_000), expected_text("2 млн", "2M"));
        assert_eq!(size_label(13_083_545), expected_text("12 МБ", "12 MB"));
        assert_eq!(size_label(2_036_735), expected_text("1,9 МБ", "1.9 MB"));
        assert_eq!(size_label(25_859), expected_text("26 КБ", "26 KB"));
    }

    #[test]
    fn only_steamdeckrepo_hosts_are_reachable() {
        let n = Net::new(false);
        assert!(n.allowed("https://steamdeckrepo.com/api/posts/all"));
        assert!(n.allowed("https://cdn.steamdeckrepo.com/thumbnails/x.png"));
        assert!(n.allowed("https://steamdeckrepo.s3.us-west-004.backblazeb2.com/videos/x.webm?X-Amz=1"));
        assert!(!n.allowed("http://steamdeckrepo.com/api/posts/all"), "plain http");
        assert!(!n.allowed("https://i.imgur.com/1DGagAC.mp4"));
        assert!(!n.allowed("https://github.com/TheLogicMaster/SDH-AnimationChanger"));
        assert!(!n.allowed("https://evil.backblazeb2.com/x"));
        assert!(!n.allowed("https://steamdeckrepo.com.evil.org/x"));
        assert!(!n.allowed("https://steamdeckrepo.com@evil.org/x"), "userinfo trick");
        assert!(!n.allowed("https://steamdeckrepo.com:8443/x"));
        assert!(!n.allowed("file:///etc/passwd"));
        assert!(!n.allowed("127.0.0.1"));
    }

    // ------------------------------------------------ tiny local HTTP server for fixtures

    pub struct Fixture {
        pub base: String,
        pub hits: Arc<std::sync::Mutex<Vec<String>>>,
    }

    /// Serves `routes(path) -> (status, headers, body, delay per 16 KB chunk)` on 127.0.0.1 until the test ends.
    pub fn serve(routes: impl Fn(&str, &str) -> (u16, Vec<(String, String)>, Vec<u8>, u64) + Send + Sync + 'static) -> Fixture {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        let hits = Arc::new(std::sync::Mutex::new(vec![]));
        let (h2, b2) = (hits.clone(), base.clone());
        let routes = Arc::new(routes);
        std::thread::spawn(move || {
            for s in l.incoming().flatten() {
                let (routes, hits, base) = (routes.clone(), h2.clone(), b2.clone());
                std::thread::spawn(move || {
                    let mut r = BufReader::new(s.try_clone().unwrap());
                    let mut line = String::new();
                    if r.read_line(&mut line).is_err() {
                        return;
                    }
                    loop {
                        let mut h = String::new();
                        if r.read_line(&mut h).is_err() || h == "\r\n" || h.is_empty() {
                            break;
                        }
                    }
                    let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
                    hits.lock().unwrap().push(path.clone());
                    let (code, headers, body, delay) = routes(&path, &base);
                    let mut s = s;
                    let mut head = format!("HTTP/1.1 {code} X\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
                    for (k, v) in headers {
                        head.push_str(&format!("{k}: {v}\r\n"));
                    }
                    head.push_str("\r\n");
                    if s.write_all(head.as_bytes()).is_err() {
                        return;
                    }
                    for c in body.chunks(16 * 1024) {
                        if delay > 0 {
                            std::thread::sleep(Duration::from_millis(delay));
                        }
                        if s.write_all(c).is_err() {
                            return;
                        }
                    }
                });
            }
        });
        Fixture { base, hits }
    }

    pub fn test_net(f: &Fixture) -> Net {
        let hp = f.base.trim_start_matches("http://").to_string();
        Net { base: f.base.clone(), ipv4: true, test_hosts: vec![hp], timeout: Duration::from_secs(5) }
    }

    pub fn webm(len: usize) -> Vec<u8> {
        let mut v = vec![0x1A, 0x45, 0xDF, 0xA3];
        v.resize(len, 7);
        v
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("animrepo-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn catalog_retries_server_errors_and_stops_on_rate_limit() {
        let n = std::sync::atomic::AtomicUsize::new(0);
        let f = serve(move |p, _| {
            if p == "/api/posts/all" && n.fetch_add(1, Ordering::SeqCst) < 2 {
                return (503, vec![], b"busy".to_vec(), 0);
            }
            (200, vec![], FIXTURE.as_bytes().to_vec(), 0)
        });
        let bytes = test_net(&f).fetch_catalog().unwrap();
        assert_eq!(parse_catalog(&bytes).unwrap().posts.len(), 6);
        assert_eq!(f.hits.lock().unwrap().len(), 3);

        let f = serve(|_, _| (429, vec![], vec![], 0));
        assert_eq!(test_net(&f).fetch_catalog(), Err(NetErr::RateLimited));
        assert_eq!(f.hits.lock().unwrap().len(), 1, "429 is not retried");
        assert!(NetErr::RateLimited.message().contains(expected_text("минуту", "minute")));
    }

    #[test]
    fn offline_is_reported_without_retry_storm() {
        // a port nobody listens on
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        drop(l);
        let net = Net { base: format!("http://{addr}"), ipv4: false, test_hosts: vec![addr.to_string()], timeout: Duration::from_secs(2) };
        let e = net.fetch_catalog().unwrap_err();
        assert!(matches!(e, NetErr::Offline(_)), "{e:?}");
        assert!(e.message().contains(expected_text("интернет", "internet")));
    }

    #[test]
    fn download_follows_redirect_reports_progress_and_renames() {
        let body = webm(200_000);
        let b2 = body.clone();
        let f = serve(move |p, base| match p {
            "/post/download/BootB" => (302, vec![("Location".into(), format!("{base}/videos/BootB.webm?sig=1"))], vec![], 0),
            "/videos/BootB.webm?sig=1" => (200, vec![], b2.clone(), 0),
            _ => (404, vec![], vec![], 0),
        });
        let dir = tmp("dl");
        let dest = dir.join("BootB.webm");
        let cancel = AtomicBool::new(false);
        let mut seen = vec![];
        let n = test_net(&f).download(&format!("{}/post/download/BootB", f.base), &dest, &cancel, &|_| Some(10 << 30), &mut |d, t| seen.push((d, t))).unwrap();
        assert_eq!(n, 200_000);
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert!(!part_path(&dest).exists());
        assert_eq!(seen.first(), Some(&(0, Some(200_000))));
        assert_eq!(seen.last(), Some(&(200_000, Some(200_000))));
        assert!(seen.windows(2).all(|w| w[0].0 <= w[1].0));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn redirect_to_a_foreign_host_is_refused() {
        let f = serve(|_, _| (302, vec![("Location".into(), "https://evil.example.org/x.webm".into())], vec![], 0));
        let dir = tmp("evil");
        let dest = dir.join("x.webm");
        let e = test_net(&f).download(&format!("{}/post/download/x", f.base), &dest, &AtomicBool::new(false), &|_| Some(1 << 40), &mut |_, _| {}).unwrap_err();
        assert!(matches!(e, NetErr::HostNotAllowed(ref h) if h == "evil.example.org"), "{e:?}");
        assert!(!dest.exists() && !part_path(&dest).exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn cancel_mid_download_leaves_nothing() {
        let f = serve(|_, _| (200, vec![], webm(400_000), 30));
        let dir = tmp("cancel");
        let dest = dir.join("c.webm");
        let cancel = Arc::new(AtomicBool::new(false));
        let c2 = cancel.clone();
        let mut chunks = 0;
        let e = test_net(&f)
            .download(&format!("{}/v", f.base), &dest, &cancel, &|_| Some(1 << 40), &mut |d, _| {
                chunks += 1;
                if d > 50_000 {
                    c2.store(true, Ordering::SeqCst);
                }
            })
            .unwrap_err();
        assert_eq!(e, NetErr::Cancelled);
        assert!(chunks >= 2);
        assert!(!dest.exists() && !part_path(&dest).exists(), "no partial file is left behind");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn errors_html_space_and_status() {
        let f = serve(|p, _| match p {
            "/html" => (200, vec![], b"<!DOCTYPE html><html>".to_vec(), 0),
            "/big" => (200, vec![], webm(300_000), 0),
            _ => (404, vec![], vec![], 0),
        });
        let dir = tmp("errs");
        let net = test_net(&f);
        let none = AtomicBool::new(false);
        let dl = |p: &str, free: u64| net.download(&format!("{}{p}", f.base), &dir.join("e.webm"), &none, &move |_| Some(free), &mut |_, _| {});
        assert_eq!(dl("/html", 1 << 40), Err(NetErr::NotVideo));
        assert!(matches!(dl("/big", 1 << 20), Err(NetErr::NoSpace { need: 300_000, .. })));
        assert_eq!(dl("/missing", 1 << 40), Err(NetErr::Status(404)));
        assert!(!dir.join("e.webm").exists());
        assert!(NetErr::Status(404).message().contains("404"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
