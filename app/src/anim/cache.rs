//! ffmpeg -> frame cache ("DSA1"): probe, decode (video as RGBA at display size, audio as s16le 48 kHz stereo), and a
//! random-access, lz4-per-frame file so playback never decodes video on the fly:
//!
//! ```text
//! header 64 B : "DSA1" ver w h nframes rate channels pad | audio_off audio_samples index_off duration_us
//! frames      : lz4 blocks, each either a key frame or the bytewise difference to the previous frame (index clen bit 31);
//!               identical consecutive frames are stored once (index clen = 0)
//! audio       : i16 little endian, interleaved
//! index       : nframes * { u64 offset, u32 clen, u32 pts_us }
//! ```
//! Boot animations are mostly flat graphics: 6-40x compression, so decoding a frame back is ~1-2 ms of lz4.
use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::FileExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const MAGIC: &[u8; 4] = b"DSA1";
const VERSION: u32 = 3;
const DELTA: u32 = 1 << 31;
const KEY_EVERY: u32 = 240;
pub const AUDIO_RATE: u32 = 48_000;
pub const AUDIO_CH: u16 = 2;

#[derive(Clone, Debug, Default)]
pub struct Probe {
    pub w: u32,
    pub h: u32,
    pub fps: Option<f64>,
    pub duration_s: f64,
    pub v_start_s: f64,
    pub has_audio: bool,
    pub color_tagged: bool,
}

fn ffmpeg() -> Result<PathBuf, String> {
    media::find_ffmpeg().ok_or_else(|| "ffmpeg not found".to_string())
}

fn parse_start(line: &str) -> Option<f64> {
    let i = line.find(" start ")?;
    line[i + 7..].split(|c: char| !(c.is_ascii_digit() || c == '.')).next()?.parse().ok()
}

pub fn probe(path: &Path) -> Result<Probe, String> {
    let out = Command::new(ffmpeg()?).args(["-hide_banner", "-nostdin", "-i"]).arg(path).stdin(Stdio::null()).output().map_err(|e| e.to_string())?;
    parse_probe(&String::from_utf8_lossy(&out.stderr))
}

pub fn parse_probe(s: &str) -> Result<Probe, String> {
    let mut p = Probe::default();
    for line in s.lines() {
        let l = line.trim();
        if let Some(r) = l.strip_prefix("Duration: ") {
            let t = r.split(',').next().unwrap_or("");
            let mut it = t.split(':').map(|x| x.trim().parse::<f64>().unwrap_or(0.0));
            let (h, m, sec) = (it.next().unwrap_or(0.0), it.next().unwrap_or(0.0), it.next().unwrap_or(0.0));
            p.duration_s = h * 3600.0 + m * 60.0 + sec;
        } else if l.contains("Video:") && p.w == 0 && !l.contains("attached pic") {
            for tok in l.split(',') {
                let t = tok.trim();
                let first = t.split_whitespace().next().unwrap_or("");
                if let Some((a, b)) = first.split_once('x') {
                    if let (Ok(w), Ok(h)) = (a.parse::<u32>(), b.parse::<u32>()) {
                        if w >= 16 && h >= 16 {
                            p.w = w;
                            p.h = h;
                        }
                    }
                }
                if let Some(f) = t.strip_suffix(" fps") {
                    p.fps = f.trim().parse().ok();
                }
            }
            p.v_start_s = parse_start(l).unwrap_or(0.0);
            p.color_tagged = ["bt709", "bt470", "smpte", "bt2020", "bt601"].iter().any(|k| l.contains(k));
        } else if l.contains("Audio:") {
            p.has_audio = true;
        }
    }
    if p.w == 0 {
        return Err("no video stream".into());
    }
    Ok(p)
}

/// Snap NTSC-ish rates to their exact rational value so frame times do not drift over 30 s.
pub fn exact_fps(f: f64) -> f64 {
    for ex in [24000.0 / 1001.0, 30000.0 / 1001.0, 60000.0 / 1001.0] {
        if (f - ex).abs() < 0.02 {
            return ex;
        }
    }
    f
}

pub fn fit(w: u32, h: u32, maxw: u32, maxh: u32) -> (u32, u32) {
    if w <= maxw && h <= maxh {
        return (w & !1, h & !1);
    }
    let s = (maxw as f64 / w as f64).min(maxh as f64 / h as f64);
    ((((w as f64 * s) as u32) & !1).max(2), (((h as f64 * s) as u32) & !1).max(2))
}

#[derive(Clone, Debug)]
pub struct Opts {
    pub maxw: u32,
    pub maxh: u32,
    /// background build: nice 19, 2 threads
    pub background: bool,
}

fn fnv(s: &str) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

pub fn cache_path(dir: &Path, src: &Path, o: &Opts) -> PathBuf {
    let canon = std::fs::canonicalize(src).unwrap_or_else(|_| src.to_path_buf());
    let md = std::fs::metadata(&canon).ok();
    let key = format!(
        "{}|{}|{}|{}x{}|v{VERSION}",
        canon.display(),
        md.as_ref().map(|m| m.len()).unwrap_or(0),
        md.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0),
        o.maxw,
        o.maxh
    );
    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("anim");
    dir.join(format!("{stem}-{:016x}.dsa", fnv(&key)))
}

const H8: u64 = 0x8080_8080_8080_8080;

/// out[i] = a[i] - b[i] (wrapping, per byte), 8 bytes at a time.
fn sub_bytes(a: &[u8], b: &[u8], out: &mut Vec<u8>) {
    out.clear();
    out.reserve(a.len());
    let (ca, cb) = (a.chunks_exact(8), b.chunks_exact(8));
    let (ra, rb) = (ca.remainder(), cb.remainder());
    for (x, y) in ca.zip(cb) {
        let (x, y) = (u64::from_le_bytes(x.try_into().unwrap()), u64::from_le_bytes(y.try_into().unwrap()));
        let d = ((x | H8).wrapping_sub(y & !H8)) ^ ((x ^ !y) & H8);
        out.extend_from_slice(&d.to_le_bytes());
    }
    out.extend(ra.iter().zip(rb).map(|(x, y)| x.wrapping_sub(*y)));
}

/// a[i] += b[i] (wrapping, per byte)
fn add_bytes(a: &mut [u8], b: &[u8]) {
    let mut ca = a.chunks_exact_mut(8);
    let cb = b.chunks_exact(8);
    let rb = cb.remainder();
    for (x, y) in (&mut ca).zip(cb) {
        let (xv, yv) = (u64::from_le_bytes((&*x).try_into().unwrap()), u64::from_le_bytes(y.try_into().unwrap()));
        let d = ((xv & !H8).wrapping_add(yv & !H8)) ^ ((xv ^ yv) & H8);
        x.copy_from_slice(&d.to_le_bytes());
    }
    for (x, y) in ca.into_remainder().iter_mut().zip(rb) {
        *x = x.wrapping_add(*y);
    }
}

fn lower_priority(c: &mut Command) {
    unsafe {
        c.pre_exec(|| {
            libc::setpriority(libc::PRIO_PROCESS, 0, 19);
            Ok(())
        });
    }
}

// ---------------------------------------------------------------- decode

pub fn decode_audio(src: &Path, o: &Opts) -> Result<Vec<i16>, String> {
    let mut c = Command::new(ffmpeg()?);
    c.args(["-v", "error", "-nostdin"]);
    if o.background {
        c.args(["-threads", "2"]);
    }
    c.arg("-i").arg(src).args(["-map", "0:a:0", "-vn", "-sn", "-dn", "-af", "aresample=async=1:first_pts=0", "-ac", "2", "-ar", "48000", "-f", "s16le", "-"]);
    c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    if o.background {
        lower_priority(&mut c);
    }
    let out = c.output().map_err(|e| format!("ffmpeg audio: {e}"))?;
    Ok(out.stdout.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect())
}

/// Decode all video frames in order: `emit(index, pts_us, frame, dup_of_previous)`; return false to stop.
pub fn decode_video(src: &Path, p: &Probe, o: &Opts, mut emit: impl FnMut(usize, u64, &SharedPixelBuffer<Rgba8Pixel>, bool) -> bool) -> Result<(), String> {
    let (w, h) = fit(p.w, p.h, o.maxw, o.maxh);
    let cfr = p.fps.map(|f| f > 1.0 && f <= 120.0).unwrap_or(false);
    let fps = if cfr { exact_fps(p.fps.unwrap()) } else { 60.0 };
    let mut vf = String::new();
    if !cfr {
        vf.push_str("fps=60,");
    }
    // Chromium (Steam's video element) treats untagged HD YUV as BT.709, swscale defaults to BT.601
    let matrix = if !p.color_tagged && p.h >= 720 { ":in_color_matrix=bt709" } else { "" };
    vf.push_str(&format!("scale={w}:{h}:flags=bilinear{matrix},format=rgba"));
    let mut c = Command::new(ffmpeg()?);
    c.args(["-v", "error", "-nostdin"]);
    if o.background {
        c.args(["-threads", "2"]);
    }
    c.arg("-i").arg(src).args(["-map", "0:v:0", "-an", "-sn", "-dn", "-vf", &vf, "-fps_mode", "passthrough", "-f", "rawvideo", "-pix_fmt", "rgba", "-"]);
    c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    if o.background {
        lower_priority(&mut c);
    }
    let mut child = c.spawn().map_err(|e| format!("ffmpeg video: {e}"))?;
    let mut out = child.stdout.take().unwrap();
    let mut prev: Option<SharedPixelBuffer<Rgba8Pixel>> = None;
    let mut i = 0usize;
    loop {
        let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
        if out.read_exact(buf.make_mut_bytes()).is_err() {
            break;
        }
        let pts = ((p.v_start_s + i as f64 / fps) * 1e6) as u64;
        let dup = prev.as_ref().map(|q| q.as_bytes() == buf.as_bytes()).unwrap_or(false);
        let cur = if dup { prev.clone().unwrap() } else { buf };
        let go = emit(i, pts, &cur, dup);
        prev = Some(cur);
        i += 1;
        if !go {
            break;
        }
    }
    drop(out);
    let _ = child.kill();
    let _ = child.wait();
    if i == 0 {
        return Err("ffmpeg produced no frames".into());
    }
    Ok(())
}

// ---------------------------------------------------------------- DSA writer / reader

pub struct Writer {
    f: BufWriter<File>,
    tmp: PathBuf,
    dst: PathBuf,
    pos: u64,
    index: Vec<(u64, u32, u32)>,
    w: u32,
    h: u32,
    last_pts: u64,
    prev: Vec<u8>,
    scratch: Vec<u8>,
    since_key: u32,
}

impl Writer {
    pub fn create(dst: &Path, w: u32, h: u32) -> std::io::Result<Writer> {
        if let Some(d) = dst.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = dst.with_extension(format!("dsa.tmp{}", std::process::id()));
        let mut f = BufWriter::with_capacity(1 << 20, File::create(&tmp)?);
        f.write_all(&[0u8; 64])?;
        Ok(Writer { f, tmp, dst: dst.to_path_buf(), pos: 64, index: vec![], w, h, last_pts: 0, prev: vec![], scratch: vec![], since_key: 0 })
    }
    pub fn push(&mut self, pts_us: u64, frame: Option<&[u8]>) -> std::io::Result<()> {
        self.last_pts = pts_us;
        match frame {
            Some(raw) => {
                let key = self.prev.len() != raw.len() || self.since_key == 0;
                let (c, flag) = if key {
                    (lz4_flex::block::compress(raw), 0)
                } else {
                    let mut d = std::mem::take(&mut self.scratch);
                    sub_bytes(raw, &self.prev, &mut d);
                    let c = lz4_flex::block::compress(&d);
                    self.scratch = d;
                    (c, DELTA)
                };
                self.since_key = (self.since_key + 1) % KEY_EVERY;
                self.prev.clear();
                self.prev.extend_from_slice(raw);
                self.f.write_all(&c)?;
                self.index.push((self.pos, c.len() as u32 | flag, pts_us as u32));
                self.pos += c.len() as u64;
            }
            None => self.index.push((0, 0, pts_us as u32)),
        }
        Ok(())
    }
    pub fn finish(mut self, audio: &[i16], duration_us: u64) -> std::io::Result<PathBuf> {
        let audio_off = self.pos;
        let mut bytes = Vec::with_capacity(audio.len() * 2);
        for s in audio {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        self.f.write_all(&bytes)?;
        let index_off = audio_off + bytes.len() as u64;
        for (off, clen, pts) in &self.index {
            self.f.write_all(&off.to_le_bytes())?;
            self.f.write_all(&clen.to_le_bytes())?;
            self.f.write_all(&pts.to_le_bytes())?;
        }
        self.f.flush()?;
        let mut hdr = Vec::with_capacity(64);
        hdr.extend_from_slice(MAGIC);
        for v in [VERSION, self.w, self.h, self.index.len() as u32, AUDIO_RATE, AUDIO_CH as u32, 0] {
            hdr.extend_from_slice(&v.to_le_bytes());
        }
        for v in [audio_off, audio.len() as u64, index_off, duration_us.max(self.last_pts)] {
            hdr.extend_from_slice(&v.to_le_bytes());
        }
        let file = self.f.get_mut();
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&hdr)?;
        file.sync_data().ok();
        drop(self.f);
        std::fs::rename(&self.tmp, &self.dst)?;
        Ok(self.dst)
    }
    pub fn abort(self) {
        let tmp = self.tmp.clone();
        drop(self.f);
        let _ = std::fs::remove_file(tmp);
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Entry {
    pub off: u64,
    pub clen: u32,
    pub pts_us: u32,
}

pub struct Dsa {
    f: File,
    pub path: PathBuf,
    pub w: u32,
    pub h: u32,
    pub index: Vec<Entry>,
    audio_off: u64,
    audio_samples: u64,
    pub duration_us: u64,
}

impl Dsa {
    pub fn open(path: &Path) -> Option<Dsa> {
        let f = File::open(path).ok()?;
        let mut hd = [0u8; 64];
        f.read_exact_at(&mut hd, 0).ok()?;
        if &hd[..4] != MAGIC {
            return None;
        }
        let u32_at = |o: usize| u32::from_le_bytes(hd[o..o + 4].try_into().unwrap());
        let u64_at = |o: usize| u64::from_le_bytes(hd[o..o + 8].try_into().unwrap());
        if u32_at(4) != VERSION || u32_at(20) != AUDIO_RATE || u32_at(24) != AUDIO_CH as u32 {
            return None;
        }
        let (w, h, n) = (u32_at(8), u32_at(12), u32_at(16) as usize);
        let (audio_off, audio_samples, index_off, duration_us) = (u64_at(32), u64_at(40), u64_at(48), u64_at(56));
        if n == 0 || index_off == 0 {
            return None;
        }
        let mut raw = vec![0u8; n * 16];
        f.read_exact_at(&mut raw, index_off).ok()?;
        let index = raw
            .chunks_exact(16)
            .map(|c| Entry { off: u64::from_le_bytes(c[..8].try_into().unwrap()), clen: u32::from_le_bytes(c[8..12].try_into().unwrap()), pts_us: u32::from_le_bytes(c[12..16].try_into().unwrap()) })
            .collect();
        Some(Dsa { f, path: path.to_path_buf(), w, h, index, audio_off, audio_samples, duration_us })
    }
    pub fn has_audio(&self) -> bool {
        self.audio_samples > 0
    }
    pub fn audio(&self) -> Vec<i16> {
        let mut raw = vec![0u8; self.audio_samples as usize * 2];
        if self.f.read_exact_at(&mut raw, self.audio_off).is_err() {
            return vec![];
        }
        raw.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect()
    }
    pub fn is_dup(&self, i: usize) -> bool {
        self.index[i].clen == 0
    }
    /// Decompress frame `i` (must not be a dup) into `out` (w*h*4 bytes). `prev` = the previous decoded frame (needed for delta frames).
    pub fn frame_into(&self, i: usize, tmp: &mut Vec<u8>, out: &mut [u8], prev: Option<&[u8]>) -> bool {
        let e = self.index[i];
        let len = (e.clen & !DELTA) as usize;
        tmp.resize(len, 0);
        if self.f.read_exact_at(tmp, e.off).is_err() || !lz4_flex::block::decompress_into(tmp, out).map(|n| n == out.len()).unwrap_or(false) {
            return false;
        }
        if e.clen & DELTA != 0 {
            match prev {
                Some(p) if p.len() == out.len() => add_bytes(out, p),
                _ => return false,
            }
        }
        true
    }
}

// ---------------------------------------------------------------- build / ensure / gc

/// Make sure a valid cache file exists for `src` (blocking, meant for the background worker). Returns its path.
pub fn ensure(dir: &Path, src: &Path, o: &Opts) -> Result<PathBuf, String> {
    let dst = cache_path(dir, src, o);
    if Dsa::open(&dst).is_some() {
        if let Ok(f) = File::options().write(true).open(&dst) {
            let _ = f.set_modified(std::time::SystemTime::now()); // LRU
        }
        return Ok(dst);
    }
    let t0 = std::time::Instant::now();
    let p = probe(src)?;
    let (w, h) = fit(p.w, p.h, o.maxw, o.maxh);
    let audio = if p.has_audio { decode_audio(src, o).unwrap_or_default() } else { vec![] };
    let mut wr = Writer::create(&dst, w, h).map_err(|e| e.to_string())?;
    let mut err = None;
    let mut last = 0u64;
    let r = decode_video(src, &p, o, |_, pts, buf, dup| {
        last = pts;
        if let Err(e) = wr.push(pts, if dup { None } else { Some(buf.as_bytes()) }) {
            err = Some(e.to_string());
            return false;
        }
        true
    });
    if let Some(e) = err.or(r.err()) {
        wr.abort();
        return Err(e);
    }
    let dur = ((p.duration_s * 1e6) as u64).max(last);
    let path = wr.finish(&audio, dur).map_err(|e| e.to_string())?;
    let sz = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    eprintln!("[anim] cache built {} ({} MB, {:.1} s)", path.file_name().and_then(|n| n.to_str()).unwrap_or(""), sz >> 20, t0.elapsed().as_secs_f32());
    Ok(path)
}

/// Delete least recently used cache files until the directory is below `budget` bytes (files in `keep` survive).
pub fn gc(dir: &Path, budget: u64, keep: &[PathBuf]) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = rd
        .flatten()
        .filter(|e| e.path().extension().map(|x| x == "dsa").unwrap_or(false))
        .filter_map(|e| e.metadata().ok().map(|m| (m.modified().unwrap_or(std::time::UNIX_EPOCH), m.len(), e.path())))
        .collect();
    // caches of an older format are useless: remove them first
    files.retain(|(_, _, p)| {
        let ok = File::open(p).ok().and_then(|f| {
            let mut h = [0u8; 8];
            f.read_exact_at(&mut h, 0).ok()?;
            Some(&h[..4] == MAGIC && u32::from_le_bytes(h[4..8].try_into().unwrap()) == VERSION)
        });
        if ok != Some(true) && !keep.contains(p) {
            let _ = std::fs::remove_file(p);
            return false;
        }
        true
    });
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    files.sort();
    for (_, len, p) in files {
        if total <= budget {
            break;
        }
        if keep.contains(&p) {
            continue;
        }
        if std::fs::remove_file(&p).is_ok() {
            total -= len;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn probe_parse() {
        let s = "  Duration: 00:00:01.02, start: 0.000000, bitrate: 202 kb/s\n  Stream #0:0(eng): Video: vp9 (Profile 0), yuv420p(tv, progressive), 1280x800, 60 fps, 60 tbr, 1k tbn (default), start 0.024000\n  Stream #0:1(eng): Audio: opus, 48000 Hz, stereo, fltp\n";
        let p = parse_probe(s).unwrap();
        assert_eq!((p.w, p.h, p.fps, p.has_audio), (1280, 800, Some(60.0), true));
        assert!((p.duration_s - 1.02).abs() < 1e-6);
        assert!((p.v_start_s - 0.024).abs() < 1e-6);
        assert!(!p.color_tagged);
        let v = parse_probe("Duration: 00:00:09.18\n Stream #0:0: Video: vp9 (Profile 0), yuv420p(tv, bt709), 1280x800, 1k tbr, 1k tbn\n").unwrap();
        assert_eq!(v.fps, None);
        assert!(v.color_tagged);
    }
    #[test]
    fn fit_and_fps() {
        assert_eq!(fit(1280, 800, 1280, 800), (1280, 800));
        assert_eq!(fit(1920, 1080, 1280, 800), (1280, 720));
        assert!((exact_fps(29.97) - 30000.0 / 1001.0).abs() < 1e-9);
        assert_eq!(exact_fps(30.0), 30.0);
    }
    #[test]
    fn swar_bytes() {
        let a: Vec<u8> = (0..37u32).map(|i| (i * 53 + 200) as u8).collect();
        let b: Vec<u8> = (0..37u32).map(|i| (i * 91 + 7) as u8).collect();
        let mut d = vec![];
        sub_bytes(&a, &b, &mut d);
        for i in 0..a.len() {
            assert_eq!(d[i], a[i].wrapping_sub(b[i]));
        }
        let mut r = d.clone();
        add_bytes(&mut r, &b);
        assert_eq!(r, a);
    }
    #[test]
    fn dsa_roundtrip() {
        let dir = std::env::temp_dir().join(format!("dsa-test-{}", std::process::id()));
        let dst = dir.join("t.dsa");
        let mut w = Writer::create(&dst, 4, 2).unwrap();
        let a = vec![7u8; 32];
        let b: Vec<u8> = (0..32).collect();
        w.push(0, Some(&a)).unwrap();
        w.push(16_000, None).unwrap();
        w.push(33_000, Some(&b)).unwrap();
        w.finish(&[1, -2, 3, 4], 50_000).unwrap();
        let d = Dsa::open(&dst).unwrap();
        assert_eq!((d.w, d.h, d.index.len(), d.duration_us), (4, 2, 3, 50_000));
        assert!(d.is_dup(1) && !d.is_dup(0));
        assert_eq!(d.audio(), vec![1, -2, 3, 4]);
        let (mut tmp, mut out) = (vec![], vec![0u8; 32]);
        assert!(d.frame_into(0, &mut tmp, &mut out, None));
        assert_eq!(out, a);
        let prev = out.clone();
        assert!(d.frame_into(2, &mut tmp, &mut out, Some(&prev)));
        assert_eq!(out, b);
        let _ = std::fs::remove_dir_all(dir);
    }
}
