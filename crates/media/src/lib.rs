//! Cheap animated covers: decode once into a small on-disk frame cache, play by swapping textures.
//! CONTRACT shared with `app`.
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::UNIX_EPOCH;

use fast_image_resize as fir;
use image::AnimationDecoder;

#[derive(Clone)]
pub struct Frame { pub width: u32, pub height: u32, pub rgba: std::sync::Arc<Vec<u8>>, pub delay_ms: u32 }

/// Decoded, downscaled animation (fits in `max_w`x`max_h`, keeps aspect).
pub struct Animation { pub frames: Vec<Frame> }

impl Animation {
    pub fn total_ms(&self) -> u64 { self.frames.iter().map(|f| f.delay_ms.max(1) as u64).sum() }
    pub fn byte_size(&self) -> usize { self.frames.iter().map(|f| f.rgba.len()).sum() }
}

const MAX_SECS: u64 = 10;
const MAX_FRAMES: usize = 240;

// ---------------------------------------------------------------- paths / keys

fn cache_root() -> PathBuf {
    if let Some(d) = std::env::var_os("DECKSHELL_CACHE") { return PathBuf::from(d); }
    let base = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("deckshell")
}

fn fnv(data: &[u8]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in data { h ^= *b as u64; h = h.wrapping_mul(0x100000001b3); }
    h
}

fn cache_key(path: &Path, w: u32, h: u32) -> Result<String, String> {
    let md = fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mt = md.modified().ok().and_then(|m| m.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0);
    let s = format!("{}|{}|{}|{}x{}", path.display(), mt, md.len(), w, h);
    Ok(format!("{:016x}", fnv(s.as_bytes())))
}

fn write_atomic(path: &Path, data: &[u8]) {
    if let Some(p) = path.parent() { let _ = fs::create_dir_all(p); }
    let tmp = path.with_extension(format!("tmp{}", std::process::id() as u64 ^ fnv(format!("{:?}", std::thread::current().id()).as_bytes())));
    if fs::write(&tmp, data).is_ok() && fs::rename(&tmp, path).is_err() { let _ = fs::remove_file(&tmp); }
}

// ---------------------------------------------------------------- resizing

fn fit(w: u32, h: u32, mw: u32, mh: u32) -> (u32, u32) {
    if w <= mw && h <= mh { return (w, h); }
    let s = (mw as f64 / w as f64).min(mh as f64 / h as f64);
    (((w as f64 * s).round() as u32).max(1), ((h as f64 * s).round() as u32).max(1))
}

fn resize_rgba(rgba: Vec<u8>, w: u32, h: u32, mw: u32, mh: u32, rz: &mut fir::Resizer) -> Result<(u32, u32, Vec<u8>), String> {
    let (nw, nh) = fit(w, h, mw.max(1), mh.max(1));
    if (nw, nh) == (w, h) { return Ok((w, h, rgba)); }
    let src = fir::images::Image::from_vec_u8(w, h, rgba, fir::PixelType::U8x4).map_err(|e| e.to_string())?;
    let mut dst = fir::images::Image::new(nw, nh, fir::PixelType::U8x4);
    let opts = fir::ResizeOptions::new().resize_alg(fir::ResizeAlg::Convolution(fir::FilterType::Bilinear));
    rz.resize(&src, &mut dst, &opts).map_err(|e| e.to_string())?;
    Ok((nw, nh, dst.into_vec()))
}

/// Decode by content, not extension (Steam grid files are often JPEGs named .png).
fn open_sniffed(path: &Path) -> Result<image::RgbaImage, String> {
    let e = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let r = image::ImageReader::open(path).map_err(|x| e(&x))?.with_guessed_format().map_err(|x| e(&x))?;
    Ok(r.decode().map_err(|x| e(&x))?.into_rgba8())
}

// ---------------------------------------------------------------- thumbnails

const THUMB_MAGIC: &[u8; 4] = b"DST1";

/// Decode a still image downscaled (for fast grid thumbnails), cached on disk.
pub fn load_thumbnail(path: &Path, max_w: u32, max_h: u32) -> Result<Frame, String> {
    let key = cache_key(path, max_w, max_h)?;
    let cpath = cache_root().join("thumbs").join(format!("{key}.bin"));
    if let Some(f) = read_thumb(&cpath) { return Ok(f); }
    let img = open_sniffed(path)?;
    let (w, h) = img.dimensions();
    let (nw, nh, buf) = resize_rgba(img.into_raw(), w, h, max_w, max_h, &mut fir::Resizer::new())?;
    let mut out = Vec::with_capacity(buf.len() + 12);
    out.extend_from_slice(&buf);
    out.extend_from_slice(&nw.to_le_bytes());
    out.extend_from_slice(&nh.to_le_bytes());
    out.extend_from_slice(THUMB_MAGIC);
    write_atomic(&cpath, &out);
    Ok(Frame { width: nw, height: nh, rgba: Arc::new(buf), delay_ms: 0 })
}

// Trailer layout (header at the end so the pixel Vec needs no copy): [rgba][w][h][magic]
fn read_thumb(p: &Path) -> Option<Frame> {
    let mut v = fs::read(p).ok()?;
    let n = v.len();
    if n < 12 || &v[n - 4..] != THUMB_MAGIC { return None; }
    let w = u32::from_le_bytes(v[n - 12..n - 8].try_into().ok()?);
    let h = u32::from_le_bytes(v[n - 8..n - 4].try_into().ok()?);
    if (w as usize) * (h as usize) * 4 != n - 12 { return None; }
    v.truncate(n - 12);
    Some(Frame { width: w, height: h, rgba: Arc::new(v), delay_ms: 0 })
}

// ---------------------------------------------------------------- animations

const ANIM_MAGIC: &[u8; 4] = b"DSA1";

/// Decode (or load from cache in ~/.cache/deckshell/anim) an animated file.
/// Supports webp/apng/gif natively; webm/mp4 via ffmpeg if available. Blocking — call off the UI thread.
pub fn load_animation(path: &Path, max_w: u32, max_h: u32) -> Result<Animation, String> {
    let key = cache_key(path, max_w, max_h)?;
    let cpath = cache_root().join("anim").join(format!("{key}.bin"));
    if let Some(a) = read_anim(&cpath) { return Ok(a); }
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let anim = match ext.as_str() {
        "webm" | "mp4" | "mkv" | "mov" | "m4v" | "avi" => decode_video(path, max_w, max_h)?,
        // ffmpeg decodes animated WebP ~20x faster than the `image` crate, with real per-frame timing. Old ffmpeg builds only
        // know the first frame of an animated WebP: then (or on any error) fall back to the crate decoder.
        "webp" if find_ffmpeg().is_some() => match decode_video(path, max_w, max_h) {
            Ok(a) if a.frames.len() > 1 => a,
            _ => decode_image_anim(path, max_w, max_h)?,
        },
        _ => match decode_image_anim(path, max_w, max_h) {
            Ok(a) if !a.frames.is_empty() => a,
            err => if find_ffmpeg().is_some() { decode_video(path, max_w, max_h)? } else { err? },
        },
    };
    if anim.frames.is_empty() { return Err("no frames decoded".into()); }
    write_anim(&cpath, &anim);
    Ok(anim)
}

fn push_limited(frames: &mut Vec<Frame>, elapsed: &mut u64, f: Frame) -> bool {
    if frames.len() >= MAX_FRAMES || *elapsed >= MAX_SECS * 1000 { return false; }
    *elapsed += f.delay_ms.max(1) as u64;
    frames.push(f);
    true
}

fn decode_image_anim(path: &Path, mw: u32, mh: u32) -> Result<Animation, String> {
    let err = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let rd = || fs::File::open(path).map(std::io::BufReader::new).map_err(|e| err(&e));
    let mut head = [0u8; 12];
    let n = fs::File::open(path).map_err(|e| err(&e))?.read(&mut head).map_err(|e| err(&e))?;
    let head = &head[..n];
    let frames_iter: Option<image::Frames> = if head.starts_with(b"GIF8") {
        Some(image::codecs::gif::GifDecoder::new(rd()?).map_err(|e| err(&e))?.into_frames())
    } else if head.starts_with(b"\x89PNG") {
        let d = image::codecs::png::PngDecoder::new(rd()?).map_err(|e| err(&e))?;
        if d.is_apng().map_err(|e| err(&e))? { Some(d.apng().map_err(|e| err(&e))?.into_frames()) } else { None }
    } else if head.len() >= 12 && &head[0..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        let d = image::codecs::webp::WebPDecoder::new(rd()?).map_err(|e| err(&e))?;
        if d.has_animation() { Some(d.into_frames()) } else { None }
    } else { None };

    let mut rz = fir::Resizer::new();
    let mut frames = Vec::new();
    let mut elapsed = 0u64;
    match frames_iter {
        Some(it) => {
            for fr in it {
                let fr = fr.map_err(|e| err(&e))?;
                let (num, den) = fr.delay().numer_denom_ms();
                let delay = if den == 0 { 100 } else { (num / den).max(10) }; // browsers clamp tiny delays
                let img = fr.into_buffer();
                let (w, h) = img.dimensions();
                let (nw, nh, buf) = resize_rgba(img.into_raw(), w, h, mw, mh, &mut rz)?;
                let f = Frame { width: nw, height: nh, rgba: Arc::new(buf), delay_ms: delay };
                if !push_limited(&mut frames, &mut elapsed, f) { break; }
            }
        }
        None => {
            let img = open_sniffed(path)?;
            let (w, h) = img.dimensions();
            let (nw, nh, buf) = resize_rgba(img.into_raw(), w, h, mw, mh, &mut rz)?;
            frames.push(Frame { width: nw, height: nh, rgba: Arc::new(buf), delay_ms: 1000 });
        }
    }
    Ok(Animation { frames })
}

pub fn find_ffmpeg() -> Option<PathBuf> {
    static F: OnceLock<Option<PathBuf>> = OnceLock::new();
    F.get_or_init(|| {
        if let Some(h) = std::env::var_os("HOME") {
            let p = PathBuf::from(h).join(".local/share/deckshell/bin/ffmpeg");
            if p.is_file() { return Some(p); }
        }
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths).map(|d| d.join("ffmpeg")).find(|p| p.is_file())
        })
    }).clone()
}

/// (width, height) of the first video stream, from `ffmpeg -i` banner.
fn probe_size(ff: &Path, path: &Path) -> Result<(u32, u32), String> {
    let out = Command::new(ff).args(["-hide_banner", "-nostdin", "-i"]).arg(path)
        .stdin(Stdio::null()).output().map_err(|e| e.to_string())?;
    let s = String::from_utf8_lossy(&out.stderr);
    for line in s.lines().filter(|l| l.contains("Video:")) {
        for tok in line.split(|c: char| c == ',' || c.is_whitespace()) {
            if let Some((a, b)) = tok.split_once('x') {
                if let (Ok(w), Ok(h)) = (a.parse::<u32>(), b.parse::<u32>()) {
                    if w >= 16 && h >= 16 { return Ok((w, h)); }
                }
            }
        }
    }
    Err("could not determine video size".into())
}

/// Decodes a video / animated WebP through ffmpeg at the SOURCE timing (no fps filter, `-fps_mode passthrough`): per-frame
/// delays come from the `showinfo` pts, so VFR sources and 30/50/60 fps files play at their real speed. Returns
/// (width, height, [(rgba, delay_ms)]). Blocking; a watchdog kills a stuck ffmpeg after 60 s.
fn ffmpeg_frames(path: &Path, mw: u32, mh: u32, max_secs: u64, max_frames: usize) -> Result<(u32, u32, Vec<(Vec<u8>, u32)>), String> {
    use std::sync::atomic::{AtomicBool, Ordering};
    let ff = find_ffmpeg().ok_or_else(|| "ffmpeg not found".to_string())?;
    let (sw, sh) = probe_size(&ff, path)?;
    let (w, h) = fit(sw, sh, mw.max(1), mh.max(1));
    let vf = format!("scale={w}:{h}:flags=bilinear,showinfo");
    let mut child = Command::new(&ff)
        .args(["-v", "info", "-nostdin", "-t", &max_secs.to_string(), "-i"]).arg(path)
        .args(["-an", "-sn", "-fps_mode", "passthrough", "-vf", &vf, "-frames:v", &max_frames.to_string(), "-f", "rawvideo", "-pix_fmt", "rgba", "-"])
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().map_err(|e| format!("ffmpeg: {e}"))?;
    let mut out = child.stdout.take().unwrap();
    let mut err = child.stderr.take().unwrap();
    let done = Arc::new(AtomicBool::new(false));
    let pid = child.id();
    {
        let done = done.clone();
        std::thread::spawn(move || {
            for _ in 0..600 {
                std::thread::sleep(std::time::Duration::from_millis(100));
                if done.load(Ordering::Relaxed) { return; }
            }
            let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
        });
    }
    // showinfo prints one line per frame to stderr: "[Parsed_showinfo_1 @ ..] n:   0 pts:   0 pts_time:0 ..."
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = err.read_to_string(&mut text);
        text.lines().filter(|l| l.contains("Parsed_showinfo") && l.contains(" n:"))
            .filter_map(|l| l.split("pts_time:").nth(1)?.split_whitespace().next()?.parse::<f64>().ok()).collect::<Vec<f64>>()
    });
    let fsz = (w * h * 4) as usize;
    let mut bufs: Vec<Vec<u8>> = Vec::new();
    loop {
        let mut buf = vec![0u8; fsz];
        if out.read_exact(&mut buf).is_err() { break; }
        bufs.push(buf);
        if bufs.len() >= max_frames { break; }
    }
    drop(out);
    let _ = child.kill();
    let _ = child.wait();
    done.store(true, Ordering::Relaxed);
    let pts = reader.join().unwrap_or_default();
    let count = bufs.len();
    if count == 0 { return Err("ffmpeg produced no frames".into()); }
    let known: Vec<u32> = pts.windows(2).map(|p| ((p[1] - p[0]) * 1000.0).round().max(1.0) as u32).collect();
    let mean = if known.is_empty() { 33 } else { (known.iter().sum::<u32>() / known.len() as u32).max(1) };
    let frames = bufs.into_iter().enumerate().map(|(i, b)| {
        let d = if pts.len() >= count && i + 1 < count { ((pts[i + 1] - pts[i]) * 1000.0).round().max(1.0) as u32 } else { mean };
        (b, d.clamp(1, 5000))
    }).collect();
    Ok((w, h, frames))
}

fn decode_video(path: &Path, mw: u32, mh: u32) -> Result<Animation, String> {
    let (width, height, raw) = ffmpeg_frames(path, mw, mh, MAX_SECS, MAX_FRAMES)?;
    Ok(Animation { frames: raw.into_iter().map(|(b, d)| Frame { width, height, rgba: Arc::new(b), delay_ms: d }).collect() })
}

/// Keep the cache directory of `file` under `max` bytes: delete the oldest files (by mtime). Cheap guard: at most one directory
/// scan per directory per 30 s. The caches used to grow without bound (anim/ reached 1.7 GB).
fn prune_dir(file: &Path, max: u64) {
    static LAST: Mutex<Vec<(PathBuf, std::time::Instant)>> = Mutex::new(Vec::new());
    let Some(dir) = file.parent() else { return };
    {
        let mut g = LAST.lock().unwrap();
        let now = std::time::Instant::now();
        match g.iter_mut().find(|e| e.0 == dir) {
            Some(e) if now.duration_since(e.1).as_secs() < 30 => return,
            Some(e) => e.1 = now,
            None => g.push((dir.to_path_buf(), now)),
        }
    }
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = rd.filter_map(|e| {
        let e = e.ok()?;
        let m = e.metadata().ok()?;
        m.is_file().then(|| (m.modified().unwrap_or(UNIX_EPOCH), m.len(), e.path()))
    }).collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    if total <= max { return; }
    files.sort();
    for (_, len, p) in files {
        if total <= max * 9 / 10 { break; }
        if p != file && fs::remove_file(&p).is_ok() { total -= len; }
    }
}

fn write_anim(p: &Path, a: &Animation) {
    let mut out = Vec::new();
    out.extend_from_slice(ANIM_MAGIC);
    out.extend_from_slice(&(a.frames.len() as u32).to_le_bytes());
    for f in &a.frames {
        let c = lz4_flex::block::compress(&f.rgba);
        for v in [f.width, f.height, f.delay_ms, f.rgba.len() as u32, c.len() as u32] { out.extend_from_slice(&v.to_le_bytes()); }
        out.extend_from_slice(&c);
    }
    write_atomic(p, &out);
    prune_dir(p, ANIM_CACHE_BYTES);
}

fn read_anim(p: &Path) -> Option<Animation> {
    let v = fs::read(p).ok()?;
    if v.len() < 8 || &v[..4] != ANIM_MAGIC { return None; }
    let n = u32::from_le_bytes(v[4..8].try_into().ok()?) as usize;
    let mut pos = 8;
    let u = |pos: &mut usize| -> Option<u32> {
        let x = u32::from_le_bytes(v.get(*pos..*pos + 4)?.try_into().ok()?); *pos += 4; Some(x)
    };
    let mut frames = Vec::with_capacity(n);
    for _ in 0..n {
        let (w, h, d, raw, cl) = (u(&mut pos)?, u(&mut pos)?, u(&mut pos)?, u(&mut pos)? as usize, u(&mut pos)? as usize);
        let c = v.get(pos..pos + cl)?; pos += cl;
        if raw != (w as usize) * (h as usize) * 4 { return None; }
        let mut buf = vec![0u8; raw];
        if lz4_flex::block::decompress_into(c, &mut buf).ok()? != raw { return None; }
        frames.push(Frame { width: w, height: h, rgba: Arc::new(buf), delay_ms: d });
    }
    if frames.is_empty() { None } else { Some(Animation { frames }) }
}


// ---------------------------------------------------------------- clips (long animated banners: random-access on-disk frames)

const CLIP_MAGIC: &[u8; 4] = b"DSC1";
const CLIP_MAX_MS: u64 = 30_000;
const CLIP_MAX_FRAMES: usize = 300;
const CLIP_CACHE_BYTES: u64 = 512 << 20;
const ANIM_CACHE_BYTES: u64 = 512 << 20;

/// A decimated, downscaled animated image kept on disk (lz4 per frame, random access). Playback keeps ONE frame in memory:
/// animated hero banners are 1920x620 with hundreds of frames, far beyond the RAM budget of `Animation`.
pub struct Clip { handle: fs::File, cum: Vec<u64> /* end time of each frame */, pub width: u32, pub height: u32, entries: Vec<(u64, u32, u32, u32)> /* offset, delay, raw len, comp len */ }

impl Clip {
    pub fn len(&self) -> usize { self.entries.len() }
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }
    pub fn delay_ms(&self, i: usize) -> u32 { self.entries[i % self.entries.len()].1 }
    pub fn total_ms(&self) -> u64 { self.entries.iter().map(|e| e.1 as u64).sum() }
    /// (frame on screen at `ms` since the start of a looping playback, ms until the next frame starts). Time-based, so many
    /// clips driven by one clock change frames together (one redraw for all of them) and a late timer skips instead of drifting.
    pub fn at(&self, ms: u64) -> (usize, u64) {
        let total = self.cum.last().copied().unwrap_or(1).max(1);
        let t = ms % total;
        let i = self.cum.partition_point(|&e| e <= t).min(self.cum.len() - 1);
        (i, self.cum[i].saturating_sub(t).max(1))
    }
    /// Decode frame `i` (blocking pread + lz4, ~1 ms for 1280x413).
    pub fn frame(&self, i: usize) -> Result<Frame, String> {
        let mut buf = vec![0u8; self.raw_len(i)];
        self.frame_into(i, &mut buf)?;
        Ok(Frame { width: self.width, height: self.height, rgba: Arc::new(buf), delay_ms: self.delay_ms(i) })
    }
    pub fn raw_len(&self, i: usize) -> usize { self.entries[i % self.entries.len()].2 as usize }
    /// Decode frame `i` straight into `out` (`width*height*4` bytes): no intermediate copy, e.g. into a Slint pixel buffer.
    pub fn frame_into(&self, i: usize, out: &mut [u8]) -> Result<(), String> {
        use std::os::unix::fs::FileExt;
        let (off, _, raw, cl) = self.entries[i % self.entries.len()];
        if out.len() != raw as usize { return Err("clip: wrong buffer size".into()); }
        let mut c = vec![0u8; cl as usize];
        self.handle.read_exact_at(&mut c, off).map_err(|e| e.to_string())?;
        if lz4_flex::block::decompress_into(&c, out).map_err(|e| e.to_string())? != raw as usize { return Err("clip: short frame".into()); }
        Ok(())
    }
}

fn scan_clip(file: &Path) -> Option<Clip> {
    use std::io::{Seek, SeekFrom};
    let mut f = std::io::BufReader::new(fs::File::open(file).ok()?);
    let mut h = [0u8; 16];
    f.read_exact(&mut h).ok()?;
    if &h[..4] != CLIP_MAGIC { return None; }
    let u = |b: &[u8]| u32::from_le_bytes(b.try_into().unwrap());
    let (n, w, hh) = (u(&h[4..8]) as usize, u(&h[8..12]), u(&h[12..16]));
    let mut entries = Vec::with_capacity(n);
    let mut pos = 16u64;
    for _ in 0..n {
        let mut e = [0u8; 12];
        f.read_exact(&mut e).ok()?;
        let (delay, raw, cl) = (u(&e[0..4]), u(&e[4..8]), u(&e[8..12]));
        if raw as u64 != w as u64 * hh as u64 * 4 { return None; }
        entries.push((pos + 12, delay, raw, cl));
        pos += 12 + cl as u64;
        f.seek(SeekFrom::Start(pos)).ok()?;
    }
    if entries.is_empty() || fs::metadata(file).ok()?.len() != pos { return None; }
    let mut acc = 0u64;
    let cum = entries.iter().map(|e: &(u64, u32, u32, u32)| { acc += e.1.max(1) as u64; acc }).collect();
    Some(Clip { handle: fs::File::open(file).ok()?, cum, width: w, height: hh, entries })
}

/// Open (building on first use, blocking: call off the UI thread) the on-disk clip of an animated image: frames are
/// streamed through the decoder one at a time, scaled to fit `max_w`x`max_h` and decimated so frames are >= `min_ms` apart
/// (delays accumulate, so total duration is kept). Errors if `path` is not animated.
pub fn open_clip(path: &Path, max_w: u32, max_h: u32, min_ms: u32) -> Result<Clip, String> {
    let key = cache_key(path, max_w, max_h ^ (min_ms << 16))?;
    let cpath = cache_root().join("clips").join(format!("{key}.bin"));
    if let Some(c) = scan_clip(&cpath) { return Ok(c); }
    let err = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if matches!(ext.as_str(), "webm" | "mp4" | "mkv" | "mov" | "m4v" | "avi") {
        // video preview (SteamGridDB thumbs): ffmpeg at source timing, kept frames re-timed by the same >= min_ms rule
        let (w, h, raw) = ffmpeg_frames(path, max_w, max_h, CLIP_MAX_MS / 1000, CLIP_MAX_FRAMES)?;
        let mut body: Vec<u8> = Vec::new();
        let (mut count, mut acc) = (0u32, 0u32);
        for (buf, d) in &raw {
            acc += d;
            if acc + 5 >= min_ms {
                let c = lz4_flex::block::compress(buf);
                for v in [acc, buf.len() as u32, c.len() as u32] { body.extend_from_slice(&v.to_le_bytes()); }
                body.extend_from_slice(&c);
                count += 1;
                acc = 0;
            }
        }
        if acc > 0 && count > 0 {
            // leftover time goes to the last kept frame (patch its delay in place)
            let mut pos = 0usize;
            let mut last = 0usize;
            for _ in 0..count { last = pos; let cl = u32::from_le_bytes(body[pos + 8..pos + 12].try_into().unwrap()) as usize; pos += 12 + cl; }
            let d = u32::from_le_bytes(body[last..last + 4].try_into().unwrap()) + acc;
            body[last..last + 4].copy_from_slice(&d.to_le_bytes());
        }
        if count == 0 { return Err("no frames decoded".into()); }
        let mut out = Vec::with_capacity(body.len() + 16);
        out.extend_from_slice(CLIP_MAGIC);
        for v in [count, w, h] { out.extend_from_slice(&v.to_le_bytes()); }
        out.extend_from_slice(&body);
        write_atomic(&cpath, &out);
        prune_dir(&cpath, CLIP_CACHE_BYTES);
        return scan_clip(&cpath).ok_or_else(|| "clip: cache write failed".to_string());
    }
    let mut head = [0u8; 12];
    let n = fs::File::open(path).map_err(|e| err(&e))?.read(&mut head).map_err(|e| err(&e))?;
    let head = &head[..n];
    let rd = || fs::File::open(path).map(std::io::BufReader::new).map_err(|e| err(&e));
    let it: image::Frames = if head.starts_with(b"GIF8") {
        image::codecs::gif::GifDecoder::new(rd()?).map_err(|e| err(&e))?.into_frames()
    } else if head.starts_with(b"\x89PNG") {
        let d = image::codecs::png::PngDecoder::new(rd()?).map_err(|e| err(&e))?;
        if !d.is_apng().map_err(|e| err(&e))? { return Err("not animated".into()); }
        d.apng().map_err(|e| err(&e))?.into_frames()
    } else if head.len() >= 12 && &head[0..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        let d = image::codecs::webp::WebPDecoder::new(rd()?).map_err(|e| err(&e))?;
        if !d.has_animation() { return Err("not animated".into()); }
        d.into_frames()
    } else { return Err("not animated".into()) };

    let mut rz = fir::Resizer::new();
    let mut body: Vec<u8> = Vec::new();
    let (mut count, mut acc, mut elapsed) = (0u32, 0u32, 0u64);
    let (mut cw, mut ch) = (0u32, 0u32);
    let mut emit = |buf: &Vec<u8>, w: u32, h: u32, delay: u32, body: &mut Vec<u8>, count: &mut u32| {
        let c = lz4_flex::block::compress(buf);
        for v in [delay, buf.len() as u32, c.len() as u32] { body.extend_from_slice(&v.to_le_bytes()); }
        body.extend_from_slice(&c);
        *count += 1; (cw, ch) = (w, h);
    };
    for fr in it {
        let fr = fr.map_err(|e| err(&e))?;
        let (num, den) = fr.delay().numer_denom_ms();
        let delay = if den == 0 { 100 } else { (num / den).max(10) };
        elapsed += delay as u64;
        acc += delay;
        // scaling is the expensive part besides decoding: only for frames we keep
        if acc + 5 >= min_ms {
            let img = fr.into_buffer();
            let (w, h) = img.dimensions();
            let (nw, nh, buf) = resize_rgba(img.into_raw(), w, h, max_w, max_h, &mut rz)?;
            emit(&buf, nw, nh, acc, &mut body, &mut count);
            acc = 0;
        }
        if count as usize >= CLIP_MAX_FRAMES || elapsed >= CLIP_MAX_MS { break; }
    }
    if count == 0 { return Err("no frames decoded".into()); }
    let mut out = Vec::with_capacity(body.len() + 16);
    out.extend_from_slice(CLIP_MAGIC);
    for v in [count, cw, ch] { out.extend_from_slice(&v.to_le_bytes()); }
    out.extend_from_slice(&body);
    write_atomic(&cpath, &out);
    prune_dir(&cpath, CLIP_CACHE_BYTES);
    scan_clip(&cpath).ok_or_else(|| "clip: cache write failed".to_string())
}

// ---------------------------------------------------------------- player

/// Maps elapsed time to a frame index (looping).
#[derive(Clone)]
pub struct Player { anim: Arc<Animation>, total: u64 }

impl Player {
    pub fn new(anim: Arc<Animation>) -> Self { let total = anim.total_ms().max(1); Player { anim, total } }
    pub fn animation(&self) -> &Arc<Animation> { &self.anim }
    pub fn frame_index(&self, elapsed_ms: u64) -> usize {
        if self.anim.frames.len() <= 1 { return 0; }
        let mut t = elapsed_ms % self.total;
        for (i, f) in self.anim.frames.iter().enumerate() {
            let d = f.delay_ms.max(1) as u64;
            if t < d { return i; }
            t -= d;
        }
        self.anim.frames.len() - 1
    }
    pub fn frame(&self, elapsed_ms: u64) -> &Frame { &self.anim.frames[self.frame_index(elapsed_ms)] }
}

// ---------------------------------------------------------------- memory LRU

type Key = (PathBuf, u32, u32);

struct Inner { map: HashMap<Key, (Arc<Animation>, usize, u64)>, bytes: usize, tick: u64 }

/// Memory-budgeted LRU of decoded animations. Share via `Arc<AnimCache>`; all methods take `&self`.
pub struct AnimCache { budget: usize, inner: Mutex<Inner> }

impl Default for AnimCache { fn default() -> Self { Self::new(256 * 1024 * 1024) } }

impl AnimCache {
    pub fn new(budget_bytes: usize) -> Self {
        AnimCache { budget: budget_bytes, inner: Mutex::new(Inner { map: HashMap::new(), bytes: 0, tick: 0 }) }
    }
    pub fn bytes(&self) -> usize { self.inner.lock().unwrap().bytes }
    pub fn len(&self) -> usize { self.inner.lock().unwrap().map.len() }
    pub fn is_empty(&self) -> bool { self.len() == 0 }

    pub fn peek(&self, path: &Path, mw: u32, mh: u32) -> Option<Arc<Animation>> {
        let mut g = self.inner.lock().unwrap();
        g.tick += 1;
        let t = g.tick;
        g.map.get_mut(&(path.to_path_buf(), mw, mh)).map(|e| { e.2 = t; e.0.clone() })
    }

    /// Memory hit, else disk cache / decode (blocking, lock not held while decoding).
    pub fn get(&self, path: &Path, mw: u32, mh: u32) -> Result<Arc<Animation>, String> {
        if let Some(a) = self.peek(path, mw, mh) { return Ok(a); }
        let a = Arc::new(load_animation(path, mw, mh)?);
        let sz = a.byte_size();
        let mut g = self.inner.lock().unwrap();
        g.tick += 1;
        let t = g.tick;
        if let Some(old) = g.map.insert((path.to_path_buf(), mw, mh), (a.clone(), sz, t)) { g.bytes -= old.1; }
        g.bytes += sz;
        while g.bytes > self.budget && g.map.len() > 1 {
            let victim = g.map.iter().filter(|(_, e)| !Arc::ptr_eq(&e.0, &a)).min_by_key(|(_, e)| e.2).map(|(k, _)| k.clone());
            match victim { Some(k) => { if let Some(e) = g.map.remove(&k) { g.bytes -= e.1; } } None => break }
        }
        Ok(a)
    }

    pub fn clear(&self) { let mut g = self.inner.lock().unwrap(); g.map.clear(); g.bytes = 0; }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{codecs::gif::GifEncoder, Delay, Rgba, RgbaImage};

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("media-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }
    fn setup(name: &str) -> PathBuf {
        let d = tmpdir(name);
        std::env::set_var("DECKSHELL_CACHE", d.join("cache"));
        d
    }
    fn solid(c: u8) -> RgbaImage { RgbaImage::from_pixel(40, 60, Rgba([c, 0, 255 - c, 255])) }

    #[test]
    fn gif_roundtrip_and_cache() {
        let d = setup("gif");
        let p = d.join("a.gif");
        let mut enc = GifEncoder::new(fs::File::create(&p).unwrap());
        for i in 0..3u8 {
            enc.encode_frame(image::Frame::from_parts(solid(i * 80), 0, 0, Delay::from_numer_denom_ms(50, 1))).unwrap();
        }
        drop(enc);
        let a = load_animation(&p, 20, 20).unwrap();
        assert_eq!(a.frames.len(), 3);
        assert_eq!((a.frames[0].width, a.frames[0].height), (13, 20));
        assert_eq!(a.frames[0].delay_ms, 50);
        let b = load_animation(&p, 20, 20).unwrap(); // from disk cache
        assert_eq!(b.frames.len(), 3);
        assert_eq!(a.frames[2].rgba, b.frames[2].rgba);
        let pl = Player::new(Arc::new(b));
        assert_eq!(pl.frame_index(0), 0);
        assert_eq!(pl.frame_index(60), 1);
        assert_eq!(pl.frame_index(149), 2);
        assert_eq!(pl.frame_index(150), 0);
    }

    #[test]
    fn apng_roundtrip() {
        let d = setup("apng");
        let p = d.join("a.png");
        let mut e = png::Encoder::new(std::io::BufWriter::new(fs::File::create(&p).unwrap()), 40, 60);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        e.set_animated(3, 0).unwrap();
        e.set_frame_delay(1, 20).unwrap();
        let mut w = e.write_header().unwrap();
        for i in 0..3u8 { w.write_image_data(solid(i * 90).as_raw()).unwrap(); }
        w.finish().unwrap();
        let a = load_animation(&p, 20, 30).unwrap();
        assert_eq!(a.frames.len(), 3);
        assert_eq!((a.frames[0].width, a.frames[0].height), (20, 30));
        assert_eq!(a.frames[0].delay_ms, 50);
        assert_eq!(load_animation(&p, 20, 30).unwrap().frames.len(), 3);
    }

    #[test]
    fn thumbnail_cached() {
        let d = setup("thumb");
        let p = d.join("t.png");
        solid(7).save(&p).unwrap();
        let a = load_thumbnail(&p, 20, 20).unwrap();
        assert_eq!((a.width, a.height), (13, 20));
        let b = load_thumbnail(&p, 20, 20).unwrap();
        assert_eq!(a.rgba, b.rgba);
        assert_eq!(b.rgba.len(), 13 * 20 * 4);
    }

    #[test]
    fn lru_budget() {
        let d = setup("lru");
        let mut paths = vec![];
        for i in 0..3 {
            let p = d.join(format!("{i}.gif"));
            let mut enc = GifEncoder::new(fs::File::create(&p).unwrap());
            for c in 0..2u8 { enc.encode_frame(image::Frame::from_parts(solid(c * 90), 0, 0, Delay::from_numer_denom_ms(40, 1))).unwrap(); }
            paths.push(p);
        }
        let one = 40 * 60 * 4 * 2;
        let c = AnimCache::new(one * 2 + 10);
        for p in &paths { c.get(p, 100, 100).unwrap(); }
        assert_eq!(c.len(), 2);
        assert!(c.peek(&paths[0], 100, 100).is_none());
        assert!(c.peek(&paths[2], 100, 100).is_some());
    }

    /// 1 s of 30 fps video: all frames are kept at ~33 ms (source timing, no 24 fps cap), clip frames read back via pread.
    #[test]
    fn video_keeps_source_fps() {
        let Some(ff) = find_ffmpeg() else { return };
        let d = setup("vid30");
        let p = d.join("t.webm");
        let ok = Command::new(&ff).args(["-v", "error", "-f", "lavfi", "-i", "testsrc=size=64x96:rate=30:duration=1", "-c:v", "libvpx", "-y"]).arg(&p)
            .status().map(|s| s.success()).unwrap_or(false);
        if !ok { return; }
        let a = load_animation(&p, 64, 96).unwrap();
        assert_eq!(a.frames.len(), 30);
        assert!(a.frames.iter().all(|f| (32..=34).contains(&f.delay_ms)), "{:?}", a.frames.iter().map(|f| f.delay_ms).collect::<Vec<_>>());
        let c = open_clip(&p, 32, 48, 0).unwrap();
        assert_eq!((c.len(), c.width, c.height), (30, 32, 48));
        let mut buf = vec![0u8; c.raw_len(3)];
        c.frame_into(3, &mut buf).unwrap();
        assert!((c.total_ms() as i64 - 1000).abs() < 40);
    }

    #[test]
    fn video_without_ffmpeg_errors() {
        let d = setup("vid");
        let p = d.join("x.webm");
        fs::write(&p, b"nope").unwrap();
        if find_ffmpeg().is_none() { assert_eq!(load_animation(&p, 10, 10).err().unwrap(), "ffmpeg not found"); }
    }
}
