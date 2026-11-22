//! Background avatar loader and cache for QAM friends list.
//! Fetches medium avatars (64x64) from Steam CDN, validates and caches them on disk (~/.cache/deckshell/avatars),
//! bounds decoding memory, recovers corrupt cache, enforces negative-caching backoff, and decodes off the UI thread.
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Instant;

use slint::{Rgba8Pixel, SharedPixelBuffer};

pub const DEFAULT_AVATAR_URL: &str =
    "https://avatars.steamstatic.com/fef49e7fa7e1997310d705b2a6158ff8dc1cdfeb_medium.jpg";

pub const MAX_AVATAR_BYTES: u64 = 1_048_576; // 1 MB transfer & file limit
pub const MAX_DISK_CACHE_BYTES: u64 = 10 * 1024 * 1024; // 10 MB budget
pub const MAX_DISK_CACHE_FILES: usize = 300;
pub const FAILURE_BACKOFF_SECS: u64 = 60;

pub fn is_trusted_avatar_host(host: &str) -> bool {
    matches!(
        host,
        "avatars.steamstatic.com"
            | "avatars.akamai.steamstatic.com"
            | "avatars.cloudflare.steamstatic.com"
            | "steamcdn-a.akamaihd.net"
    )
}

pub fn is_trusted_avatar_url(url: &str) -> bool {
    if url.bytes().any(|b| b.is_ascii_whitespace() || b.is_ascii_control() || b == b'\\') {
        return false;
    }
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let Some((authority, _path)) = rest.split_once('/') else {
        return false;
    };
    // Validate the entire authority. Truncating at ':' accepts a trusted-looking
    // username in https://trusted:443@untrusted/path. Only default HTTPS is used.
    let host = authority.strip_suffix(":443").unwrap_or(authority);
    is_trusted_avatar_host(host)
}

pub fn is_valid_avatar_hash(hash: &str) -> bool {
    hash.len() == 40 && hash.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Resolve the avatar URL for a friend. Validates that the input is a trusted Steam HTTPS URL
/// or a strictly 40-character hex hash. Returns the confirmed Steam fallback URL for empty,
/// default, untrusted, or arbitrary non-hash strings without inventing CDN URLs.
pub fn resolve_avatar_url(raw_url: &str) -> String {
    let s = raw_url.trim();
    if s.is_empty()
        || s.contains("0000000000000000000000000000000000000000")
        || s.contains("fef49e7fa7e1997310d705b2a6158ff8dc1cdfeb")
    {
        return DEFAULT_AVATAR_URL.to_string();
    }

    if s.starts_with("https://") {
        if is_trusted_avatar_url(s) {
            return s.to_string();
        }
        return DEFAULT_AVATAR_URL.to_string();
    }

    // Only strictly 40-character hex hashes are normalized to Steam CDN URLs
    let candidate = s.strip_suffix(".jpg").unwrap_or(s);
    if is_valid_avatar_hash(candidate) {
        let lower = candidate.to_ascii_lowercase();
        return format!("https://avatars.steamstatic.com/{lower}_medium.jpg");
    }

    // Do NOT invent URLs for arbitrary non-http / non-hash strings
    DEFAULT_AVATAR_URL.to_string()
}

/// Computes a stable disk cache filename for a given avatar URL.
pub fn cache_filename(url: &str) -> String {
    let mut h = 0xcbf29ce484222325u64;
    for b in url.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}.jpg", h)
}

pub fn cache_dir() -> PathBuf {
    std::env::var_os("DECKSHELL_AVATAR_CACHE")
        .map(PathBuf::from)
        .or_else(|| {
            let base = std::env::var_os("XDG_CACHE_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
                .unwrap_or_else(|| PathBuf::from("/tmp"));
            Some(base.join("deckshell/avatars"))
        })
        .unwrap_or_else(|| PathBuf::from("/tmp/deckshell-avatars"))
}

/// Validates decoded image bytes with bounded allocation limits and converts to RGBA8 64x64 SharedPixelBuffer.
pub fn decode_avatar_bytes(bytes: &[u8]) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    if bytes.len() < 64 || bytes.len() as u64 > MAX_AVATAR_BYTES {
        return None;
    }
    // Reject HTML/XML/JSON error payloads
    if bytes.starts_with(b"<!DOCTYPE")
        || bytes.starts_with(b"<html")
        || bytes.starts_with(b"<?xml")
        || bytes.starts_with(b"{\n")
        || bytes.starts_with(b"{\"error\"")
    {
        return None;
    }

    let mut limits = image::Limits::default();
    limits.max_image_width = Some(1024);
    limits.max_image_height = Some(1024);
    limits.max_alloc = Some(4 * 1024 * 1024); // max 4MB memory allocation

    // Read format and dimensions from header without allocating full image
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    reader.limits(limits.clone());
    let (w, h) = reader.into_dimensions().ok()?;
    if w == 0 || h == 0 || w > 1024 || h > 1024 {
        return None;
    }

    // Now decode using reader with strict allocation limits
    let mut reader2 = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    reader2.limits(limits);
    let img = reader2.decode().ok()?;
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 || w > 1024 || h > 1024 {
        return None;
    }

    let rgba = if w == 64 && h == 64 {
        img.to_rgba8()
    } else {
        img.resize_exact(64, 64, image::imageops::FilterType::Triangle).to_rgba8()
    };
    Some(SharedPixelBuffer::clone_from_slice(rgba.as_raw(), 64, 64))
}

pub fn prune_disk_cache(dir: &Path, max_bytes: u64, max_files: usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut files = Vec::new();
    let mut total_bytes: u64 = 0;

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let fname = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        // Never prune in-flight temporary files
        if fname.starts_with("tmp_") {
            continue;
        }
        if let Ok(meta) = entry.metadata() {
            let sz = meta.len();
            let mtime = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            total_bytes += sz;
            files.push((path, sz, mtime));
        }
    }

    if files.len() <= max_files && total_bytes <= max_bytes {
        return;
    }

    // Sort by mtime ascending (oldest files first)
    files.sort_by_key(|(_, _, mtime)| *mtime);

    let mut current_files = files.len();
    for (path, sz, _) in files {
        if total_bytes <= max_bytes && current_files <= max_files {
            break;
        }
        if fs::remove_file(&path).is_ok() {
            total_bytes = total_bytes.saturating_sub(sz);
            current_files = current_files.saturating_sub(1);
        }
    }
}

/// Test helper for the write-then-rename pattern. Production promotes downloads in `download_avatar_curl`.
#[cfg(test)]
pub fn atomic_write_avatar(cdir: &Path, dest_path: &Path, bytes: &[u8]) -> bool {
    let _ = fs::create_dir_all(cdir);
    let tmp_path = cdir.join(format!(
        "tmp_{}_{:016x}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));

    if fs::write(&tmp_path, bytes).is_err() {
        let _ = fs::remove_file(&tmp_path);
        return false;
    }

    // Atomic move to permanent path
    if fs::rename(&tmp_path, dest_path).is_err() {
        let _ = fs::remove_file(&tmp_path);
        return false;
    }

    true
}

fn download_avatar_curl(url: &str, dest_path: &Path) -> Option<Vec<u8>> {
    if !is_trusted_avatar_url(url) {
        return None;
    }

    let cdir = dest_path.parent()?;
    let _ = fs::create_dir_all(cdir);
    let tmp_path = cdir.join(format!(
        "tmp_{}_{:016x}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));

    // curl with --fail, restricted https proto, and strict 1MB transfer limit
    let status = Command::new("curl")
        .args([
            "-sS",
            "--fail",
            "--proto",
            "=https",
            "--max-filesize",
            "1048576",
            "--max-time",
            "5",
            "--connect-timeout",
            "3",
            "-o",
            tmp_path.to_str()?,
            url,
        ])
        .status()
        .ok()?;

    if !status.success() {
        let _ = fs::remove_file(&tmp_path);
        return None;
    }

    let meta = fs::metadata(&tmp_path).ok()?;
    let sz = meta.len();
    if sz < 64 || sz > MAX_AVATAR_BYTES {
        let _ = fs::remove_file(&tmp_path);
        return None;
    }

    use std::io::Read;
    let Ok(mut file) = fs::File::open(&tmp_path) else {
        let _ = fs::remove_file(&tmp_path);
        return None;
    };
    let mut bytes = Vec::with_capacity(sz as usize);
    if file.take(MAX_AVATAR_BYTES + 1).read_to_end(&mut bytes).is_err() || bytes.len() as u64 != sz {
        let _ = fs::remove_file(&tmp_path);
        return None;
    }

    // Validate image format before promoting temporary file to permanent cache
    if decode_avatar_bytes(&bytes).is_none() {
        let _ = fs::remove_file(&tmp_path);
        return None;
    }

    // Atomic move to permanent cache path; never fallback to non-atomic partial overwrite
    if fs::rename(&tmp_path, dest_path).is_err() {
        let _ = fs::remove_file(&tmp_path);
        return None;
    }

    Some(bytes)
}

// Bound the read itself: metadata can become stale before a cache file is read.
fn read_avatar_cache(reader: impl std::io::Read) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    reader.take(MAX_AVATAR_BYTES + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() < 64 || bytes.len() as u64 > MAX_AVATAR_BYTES {
        return None;
    }
    Some(bytes)
}

pub fn fetch_and_decode_avatar(
    url: &str,
    cdir: &Path,
    fetch_fn: &dyn Fn(&str, &Path) -> Option<Vec<u8>>,
) -> (Option<SharedPixelBuffer<Rgba8Pixel>>, bool) {
    let cpath = cdir.join(cache_filename(url));

    if cpath.exists() {
        if let Ok(file) = fs::File::open(&cpath) {
            if let Some(bytes) = read_avatar_cache(file) {
                if let Some(buf) = decode_avatar_bytes(&bytes) {
                    return (Some(buf), true);
                }
            }
        }
        // Cache exists but is corrupt or invalid!
        // Evict corrupted file immediately to recover and avoid permanent failure locks
        let _ = fs::remove_file(&cpath);
    }

    // Perform ONE controlled fetch
    let bytes = fetch_fn(url, &cpath);
    if let Some(ref b) = bytes {
        if let Some(buf) = decode_avatar_bytes(b) {
            prune_disk_cache(cdir, MAX_DISK_CACHE_BYTES, MAX_DISK_CACHE_FILES);
            return (Some(buf), false);
        }
    }

    (None, false)
}

pub type FetcherFn = Arc<dyn Fn(&str, &Path) -> Option<Vec<u8>> + Send + Sync>;

struct WorkerState {
    queue: VecDeque<String>,
    in_flight: HashSet<String>,
    failed: HashMap<String, Instant>,
}

pub struct AvatarLoader {
    cache_dir: PathBuf,
    fetch_fn: FetcherFn,
    state: Mutex<WorkerState>,
    cv: Condvar,
}

static LOADER: OnceLock<Arc<AvatarLoader>> = OnceLock::new();

pub fn get_loader() -> Arc<AvatarLoader> {
    LOADER
        .get_or_init(|| {
            let loader = Arc::new(AvatarLoader::new(
                cache_dir(),
                Arc::new(|url, path| download_avatar_curl(url, path)),
            ));

            let worker_loader = loader.clone();
            std::thread::Builder::new()
                .name("qam-avatar-worker".into())
                .spawn(move || worker_loader.worker_loop())
                .expect("spawn qam avatar worker");

            loader
        })
        .clone()
}

impl AvatarLoader {
    pub fn new(cache_dir: PathBuf, fetch_fn: FetcherFn) -> Self {
        AvatarLoader {
            cache_dir,
            fetch_fn,
            state: Mutex::new(WorkerState {
                queue: VecDeque::new(),
                in_flight: HashSet::new(),
                failed: HashMap::new(),
            }),
            cv: Condvar::new(),
        }
    }

    pub fn request(&self, raw_url: &str) {
        let url = resolve_avatar_url(raw_url);
        if url.is_empty() {
            return;
        }

        let mut st = self.state.lock().unwrap();
        // 1. If currently in-flight or waiting for UI to commit to cache, deduplicate
        if st.in_flight.contains(&url) {
            return;
        }

        // 2. If recently failed, enforce negative-caching backoff to prevent retry storms
        if let Some(failed_at) = st.failed.get(&url) {
            if failed_at.elapsed().as_secs() < FAILURE_BACKOFF_SECS {
                return;
            }
        }

        st.failed.remove(&url);
        st.in_flight.insert(url.clone());
        st.queue.push_back(url);
        self.cv.notify_one();
    }

    pub fn complete_success(&self, url: &str) {
        let mut st = self.state.lock().unwrap();
        st.in_flight.remove(url);
        st.failed.remove(url);
    }

    pub fn complete_failure(&self, url: &str) {
        let mut st = self.state.lock().unwrap();
        st.in_flight.remove(url);
        st.failed.insert(url.to_string(), Instant::now());

        if st.failed.len() > 300 {
            let expired: Vec<String> = st
                .failed
                .iter()
                .filter(|(_, t)| t.elapsed().as_secs() >= FAILURE_BACKOFF_SECS)
                .map(|(k, _)| k.clone())
                .collect();
            for k in expired {
                st.failed.remove(&k);
            }
            if st.failed.len() > 300 {
                if let Some(oldest_key) = st
                    .failed
                    .iter()
                    .min_by_key(|(_, t)| *t)
                    .map(|(k, _)| k.clone())
                {
                    st.failed.remove(&oldest_key);
                }
            }
        }
    }

    #[cfg(test)]
    pub fn queue_len(&self) -> usize {
        self.state.lock().unwrap().queue.len()
    }

    #[cfg(test)]
    pub fn is_in_flight(&self, url: &str) -> bool {
        self.state.lock().unwrap().in_flight.contains(url)
    }

    #[cfg(test)]
    pub fn is_failed(&self, url: &str) -> bool {
        self.state.lock().unwrap().failed.contains_key(url)
    }

    fn worker_loop(self: &Arc<Self>) {
        loop {
            let url = {
                let mut st = self.state.lock().unwrap();
                loop {
                    if let Some(u) = st.queue.pop_front() {
                        break u;
                    }
                    st = self.cv.wait(st).unwrap();
                }
            };

            let t0 = Instant::now();
            let (buf_opt, from_disk) = fetch_and_decode_avatar(&url, &self.cache_dir, &*self.fetch_fn);
            let elapsed_ms = t0.elapsed().as_millis() as u64;

            if let Some(buf) = buf_opt {
                let target_url = url.clone();
                let loader = self.clone();
                let scheduled = slint::invoke_from_event_loop(move || {
                    crate::with_app(|a| {
                        a.on_friend_avatar_ready(&target_url, buf, from_disk, elapsed_ms);
                    });
                    // Only release in_flight AFTER UI has inserted the image into cache
                    loader.complete_success(&target_url);
                });
                if scheduled.is_err() {
                    self.complete_failure(&url);
                }
            } else {
                self.complete_failure(&url);
                if crate::diag::enabled() {
                    crate::diag::log(
                        "qam_avatar_fetch_failed",
                        serde_json::json!({
                            "from_disk": from_disk,
                            "duration_ms": elapsed_ms
                        }),
                    );
                }
            }
        }
    }
}

/// Decides whether rows on the QAM Friends tab should be rebuilt when an avatar becomes ready.
/// Returns false if QAM is closed, or if another tab is active, or if the friend list
/// no longer contains a friend with the arrived avatar URL (changed friend or stale response).
pub fn should_rebuild_on_avatar_ready<'a>(
    qam_open: bool,
    tab: usize,
    friends_avatar_urls: impl IntoIterator<Item = &'a str>,
    arrived_url: &str,
) -> bool {
    if !qam_open || tab != 1 {
        return false;
    }
    friends_avatar_urls
        .into_iter()
        .any(|raw| resolve_avatar_url(raw) == arrived_url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_avatar_authority_rejects_userinfo_ports_and_ambiguous_urls() {
        for url in [
            "https://avatars.steamstatic.com:443@untrusted.invalid/avatar.jpg",
            "https://avatars.steamstatic.com:1234/avatar.jpg",
            "https://avatars.steamstatic.com@untrusted.invalid/avatar.jpg",
            "https://avatars.steamstatic.com\\@untrusted.invalid/avatar.jpg",
            "https://avatars.steamstatic.com\t/avatar.jpg",
            "https://avatars.steamstatic.com:443:443/avatar.jpg",
        ] {
            assert!(!is_trusted_avatar_url(url), "accepted ambiguous URL: {url}");
            assert_eq!(resolve_avatar_url(url), DEFAULT_AVATAR_URL);
        }
        assert!(is_trusted_avatar_url("https://avatars.steamstatic.com:443/avatar.jpg"));
    }

    #[test]
    fn test_resolve_avatar_url_validation() {
        assert_eq!(resolve_avatar_url(""), DEFAULT_AVATAR_URL);
        assert_eq!(
            resolve_avatar_url("   0000000000000000000000000000000000000000  "),
            DEFAULT_AVATAR_URL
        );
        // Valid 40-char lowercase hex hash
        let valid_hash = "20713cfb832dbab43f6a3644f38c171dfceb89cf";
        assert_eq!(
            resolve_avatar_url(valid_hash),
            format!("https://avatars.steamstatic.com/{valid_hash}_medium.jpg")
        );
        // Valid synthetic safe 40-char hex hash
        let synthetic_hash = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(
            resolve_avatar_url(synthetic_hash),
            format!("https://avatars.steamstatic.com/{synthetic_hash}_medium.jpg")
        );
        // Trusted full HTTPS URL
        let sample = "https://avatars.steamstatic.com/20713cfb832dbab43f6a3644f38c171dfceb89cf_medium.jpg";
        assert_eq!(resolve_avatar_url(sample), sample);

        // Arbitrary non-http / non-hash string must NOT invent CDN URLs
        assert_eq!(resolve_avatar_url("not_a_hash"), DEFAULT_AVATAR_URL);
        assert_eq!(resolve_avatar_url("random_username_123"), DEFAULT_AVATAR_URL);
        assert_eq!(resolve_avatar_url("20713cfb832dbab4"), DEFAULT_AVATAR_URL); // too short

        // Untrusted HTTPS host or insecure HTTP must fallback to default
        assert_eq!(resolve_avatar_url("https://evil.example.com/avatar.jpg"), DEFAULT_AVATAR_URL);
        assert_eq!(resolve_avatar_url("http://avatars.steamstatic.com/avatar.jpg"), DEFAULT_AVATAR_URL);
    }

    #[test]
    fn test_cache_filename_deterministic() {
        let f1 = cache_filename("https://avatars.steamstatic.com/test1.jpg");
        let f2 = cache_filename("https://avatars.steamstatic.com/test1.jpg");
        let f3 = cache_filename("https://avatars.steamstatic.com/test2.jpg");
        assert_eq!(f1, f2);
        assert_ne!(f1, f3);
        assert!(f1.ends_with(".jpg"));
        assert!(!f1.contains('/'));
        assert!(!f1.contains('\\'));
    }

    #[test]
    fn test_decode_avatar_bytes_rejections() {
        assert!(decode_avatar_bytes(b"").is_none());
        assert!(decode_avatar_bytes(b"short").is_none());
        // HTML error page (e.g. 404/500 from CDN or captive portal)
        assert!(decode_avatar_bytes(b"<!DOCTYPE html><html><body>Error 404</body></html>").is_none());
        assert!(decode_avatar_bytes(b"<html><head><title>502 Bad Gateway</title></head></html>").is_none());
        assert!(decode_avatar_bytes(b"<?xml version=\"1.0\"?><Error><Code>AccessDenied</Code></Error>").is_none());
        assert!(decode_avatar_bytes(b"{\"error\":\"file not found\"}").is_none());
        let garbage = vec![0x42; 200];
        assert!(decode_avatar_bytes(&garbage).is_none());
    }

    #[test]
    fn test_hostile_header_dimension_rejection_without_allocation() {
        // Construct a synthetic PNG with IHDR advertising 65536 x 65536 dimensions in only 33 bytes
        let mut hostile_png = Vec::new();
        // PNG magic
        hostile_png.extend_from_slice(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]);
        // IHDR chunk: length 13
        hostile_png.extend_from_slice(&[0x00, 0x00, 0x00, 0x0D]);
        // Chunk type IHDR
        hostile_png.extend_from_slice(b"IHDR");
        // Width: 65536 (0x00010000)
        hostile_png.extend_from_slice(&[0x00, 0x01, 0x00, 0x00]);
        // Height: 65536 (0x00010000)
        hostile_png.extend_from_slice(&[0x00, 0x01, 0x00, 0x00]);
        // Bit depth 8, ColorType 6 (RGBA), Compression 0, Filter 0, Interlace 0
        hostile_png.extend_from_slice(&[0x08, 0x06, 0x00, 0x00, 0x00]);
        // CRC 4 bytes
        hostile_png.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        // Pad to > 64 bytes
        hostile_png.resize(80, 0);

        // Reader checks into_dimensions / limits before full decode, rejecting instantly without allocating gigabytes
        assert!(decode_avatar_bytes(&hostile_png).is_none());
    }

    #[test]
    fn test_decode_valid_image_normalizes_to_64x64() {
        let img = image::RgbImage::new(128, 128);
        let mut cursor = std::io::Cursor::new(Vec::new());
        img.write_to(&mut cursor, image::ImageFormat::Png).unwrap();
        let bytes = cursor.into_inner();
        let buf = decode_avatar_bytes(&bytes);
        assert!(buf.is_some());
        let b = buf.unwrap();
        assert_eq!(b.width(), 64);
        assert_eq!(b.height(), 64);
        assert_eq!(b.as_slice().len(), 64 * 64);
    }

    #[test]
    fn test_loader_queue_dedup_and_failure_backoff() {
        let temp_dir = std::env::temp_dir().join(format!("deckshell_test_loader_{}", std::process::id()));
        let _ = fs::create_dir_all(&temp_dir);

        let loader = AvatarLoader::new(
            temp_dir.clone(),
            Arc::new(|_url, _path| None), // Mock fetcher returns None
        );

        let url = "https://avatars.steamstatic.com/0123456789abcdef0123456789abcdef01234567_medium.jpg";
        assert_eq!(loader.queue_len(), 0);

        // 1. Initial request puts in queue and marks in-flight
        loader.request(url);
        assert_eq!(loader.queue_len(), 1);
        assert!(loader.is_in_flight(url));

        // 2. Duplicate request while in-flight is deduplicated
        loader.request(url);
        assert_eq!(loader.queue_len(), 1);

        // 3. Complete with failure: in_flight is cleared, failure backoff is recorded
        loader.complete_failure(url);
        assert!(!loader.is_in_flight(url));
        assert!(loader.is_failed(url));

        // 4. Repeated request during failure backoff window is suppressed to prevent retry storms
        loader.request(url);
        assert_eq!(loader.queue_len(), 1); // queue not increased

        // 5. Complete with success clears failure state
        loader.complete_success(url);
        assert!(!loader.is_failed(url));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_cache_read_stops_at_limit_even_for_an_unbounded_reader() {
        // A growing file must not make read_to_end allocate until EOF.
        assert!(read_avatar_cache(std::io::repeat(0xEE)).is_none());
        let mut source = std::io::Cursor::new(vec![0xEE; MAX_AVATAR_BYTES as usize + 100]);
        assert!(read_avatar_cache(&mut source).is_none());
        assert_eq!(source.position(), MAX_AVATAR_BYTES + 1);
        assert!(read_avatar_cache(std::io::Cursor::new(vec![0; 63])).is_none());
    }

    #[test]
    fn test_corrupt_cache_recovery_and_atomic_promotion() {
        let temp_dir = std::env::temp_dir().join(format!("deckshell_test_corrupt_{}", std::process::id()));
        let _ = fs::create_dir_all(&temp_dir);

        let url = "https://avatars.steamstatic.com/20713cfb832dbab43f6a3644f38c171dfceb89cf_medium.jpg";
        let cpath = temp_dir.join(cache_filename(url));

        // Create a corrupt cache file (>64 bytes of invalid garbage)
        fs::write(&cpath, vec![0xEE; 128]).unwrap();
        assert!(cpath.exists());

        // Create valid 64x64 PNG bytes
        let img = image::RgbImage::new(64, 64);
        let mut cursor = std::io::Cursor::new(Vec::new());
        img.write_to(&mut cursor, image::ImageFormat::Png).unwrap();
        let valid_png = cursor.into_inner();

        let valid_png_clone = valid_png.clone();
        let fetch_called = Arc::new(Mutex::new(0));
        let fetch_called_clone = fetch_called.clone();

        let mock_fetch = move |_u: &str, dest: &Path| {
            let mut count = fetch_called_clone.lock().unwrap();
            *count += 1;
            atomic_write_avatar(dest.parent().unwrap(), dest, &valid_png_clone);
            Some(valid_png_clone.clone())
        };

        // First call: detects corrupt cache, deletes corrupt file, calls fetch_fn once
        let (buf, from_disk) = fetch_and_decode_avatar(url, &temp_dir, &mock_fetch);
        assert!(buf.is_some());
        assert!(!from_disk);
        assert_eq!(*fetch_called.lock().unwrap(), 1);

        // Second call: now loads valid image from disk cache without invoking fetcher
        let (buf2, from_disk2) = fetch_and_decode_avatar(url, &temp_dir, &mock_fetch);
        assert!(buf2.is_some());
        assert!(from_disk2);
        assert_eq!(*fetch_called.lock().unwrap(), 1); // fetch count remains 1

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_failed_atomic_promotion_preserves_original() {
        let temp_dir = std::env::temp_dir().join(format!("deckshell_test_atomic_{}", std::process::id()));
        let _ = fs::create_dir_all(&temp_dir);

        let valid_dest = temp_dir.join("original.jpg");
        fs::write(&valid_dest, b"ORIGINAL_VALID_CONTENT_HERE").unwrap();

        // Attempt to atomically write into an invalid destination (e.g. directory as destination)
        let invalid_dest_dir = temp_dir.join("dir_as_target");
        let _ = fs::create_dir_all(&invalid_dest_dir);

        let ok = atomic_write_avatar(&temp_dir, &invalid_dest_dir, b"NEW_CONTENT");
        assert!(!ok);

        // Verify original remains untouched and no stray tmp_ files left
        assert_eq!(fs::read(&valid_dest).unwrap(), b"ORIGINAL_VALID_CONTENT_HERE");
        let tmps: Vec<_> = fs::read_dir(&temp_dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("tmp_"))
            .collect();
        assert!(tmps.is_empty());

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_bounded_disk_cache_pruning() {
        let temp_dir = std::env::temp_dir().join(format!("deckshell_test_prune_{}", std::process::id()));
        let _ = fs::create_dir_all(&temp_dir);

        // Create 5 files with differing timestamps
        for i in 1..=5 {
            let p = temp_dir.join(format!("avatar_{i}.jpg"));
            fs::write(&p, vec![0xAA; 1000]).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        // Create an in-flight tmp file that must NOT be pruned
        let tmp_file = temp_dir.join("tmp_worker_123.tmp");
        fs::write(&tmp_file, vec![0xBB; 2000]).unwrap();

        // Prune to max 3 files and 4000 bytes
        prune_disk_cache(&temp_dir, 4000, 3);

        let remaining: Vec<String> = fs::read_dir(&temp_dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();

        // Oldest avatars (avatar_1, avatar_2) should be evicted; newer (3, 4, 5) kept
        assert!(!remaining.contains(&"avatar_1.jpg".to_string()));
        assert!(!remaining.contains(&"avatar_2.jpg".to_string()));
        assert!(remaining.contains(&"avatar_4.jpg".to_string()));
        assert!(remaining.contains(&"avatar_5.jpg".to_string()));
        // In-flight tmp file must still exist
        assert!(remaining.contains(&"tmp_worker_123.tmp".to_string()));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_late_response_qam_filtering_and_state_preservation() {
        let friend_avatar = "20713cfb832dbab43f6a3644f38c171dfceb89cf";
        let target_url = resolve_avatar_url(friend_avatar);

        // Case 1: QAM is closed when late response arrives -> DO NOT rebuild rows
        assert!(!should_rebuild_on_avatar_ready(false, 1, [friend_avatar], &target_url));

        // Case 2: QAM is open on a different tab (e.g. Tab 0: Notifs, Tab 2: Quick, Tab 4: Settings) -> DO NOT rebuild
        assert!(!should_rebuild_on_avatar_ready(true, 0, [friend_avatar], &target_url));
        assert!(!should_rebuild_on_avatar_ready(true, 4, [friend_avatar], &target_url));

        // Case 3: QAM is open on Tab 1 (Friends), but friend list changed (friend removed / different friend) -> DO NOT rebuild
        assert!(!should_rebuild_on_avatar_ready(true, 1, ["other_friend_avatar_hash"], &target_url));

        // Case 4: QAM is open on Tab 1 (Friends) and friend is still present -> REBUILD in-place
        assert!(should_rebuild_on_avatar_ready(true, 1, [friend_avatar], &target_url));
    }
}
