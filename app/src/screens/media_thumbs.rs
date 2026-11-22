//! Bounded, local-only reader for Steam's native clip thumbnails.
use std::ffi::CString;
use std::fs::File;
use std::io::{Cursor, Read};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use slint::{Rgba8Pixel, SharedPixelBuffer};

const MAX_BYTES: u64 = 16 * 1024 * 1024;
const MAX_PIXELS: u64 = 8_000_000;

fn component(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn valid_url(url: &str, clip_id: &str) -> bool {
    if !component(clip_id) { return false; }
    let Some(path) = url.strip_prefix("https://steamloopback.host/") else { return false };
    let parts: Vec<_> = path.split('/').collect();
    let rest = match parts.as_slice() {
        ["gamerecordings", "clips", _, "thumbnail.jpg"] => &parts[..],
        [prefix, "gamerecordings", "clips", _, "thumbnail.jpg"] if component(prefix) => &parts[1..],
        _ => return false,
    };
    rest[2] == clip_id
}

// Every component below the canonical root is opened relative to the preceding
// directory descriptor without following symlinks, avoiding path-check races.
fn open_child(parent: &File, name: &str, directory: bool) -> Option<File> {
    let name = CString::new(name).ok()?;
    let flags = libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK
        | if directory { libc::O_DIRECTORY } else { 0 };
    let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    if fd < 0 { return None; }
    // SAFETY: openat returned a new owned descriptor.
    Some(unsafe { File::from_raw_fd(fd) })
}

fn read_bytes(root: &Path, account: u32, clip_id: &str) -> Option<Vec<u8>> {
    let canonical = root.canonicalize().ok()?;
    let name = CString::new(canonical.as_os_str().as_bytes()).ok()?;
    let fd = unsafe { libc::open(name.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY) };
    if fd < 0 { return None; }
    // SAFETY: open returned a new owned descriptor.
    let mut dir = unsafe { File::from_raw_fd(fd) };
    for part in ["userdata", &account.to_string(), "gamerecordings", "clips", clip_id] {
        dir = open_child(&dir, part, true)?;
    }
    let file = open_child(&dir, "thumbnail.jpg", false)?;
    if !file.metadata().ok()?.is_file() { return None; }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_BYTES { return None; }
    Some(bytes)
}

fn decode(bytes: &[u8]) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    let format = image::guess_format(bytes).ok()?;
    if !matches!(format, image::ImageFormat::Jpeg | image::ImageFormat::Png) { return None; }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(48 * 1024 * 1024);
    let mut header = image::ImageReader::with_format(Cursor::new(bytes), format);
    header.limits(limits.clone());
    let (w, h) = header.into_dimensions().ok()?;
    if w == 0 || h == 0 || w > 4096 || h > 4096 || u64::from(w) * u64::from(h) > MAX_PIXELS { return None; }
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), format);
    reader.limits(limits);
    let img = reader.decode().ok()?.resize(w.min(640), h.min(400), image::imageops::FilterType::Triangle).to_rgba8();
    Some(SharedPixelBuffer::clone_from_slice(img.as_raw(), img.width(), img.height()))
}

// The paths come from the screenshot scan. Open regular files without following
// the final symlink; cap compressed bytes and decoded dimensions for each source.
fn read_image_file(path: &Path) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(path).ok()?;
    if !file.metadata().ok()?.is_file() { return None; }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_BYTES { return None; }
    decode(&bytes)
}

pub fn read_screenshot(full: &Path, thumb: Option<&Path>) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    let thumb = thumb.filter(|p| *p != full).and_then(read_image_file);
    let full = read_image_file(full);
    match (full, thumb) {
        (Some(a), Some(b)) => if u64::from(a.width()) * u64::from(a.height()) >= u64::from(b.width()) * u64::from(b.height()) { Some(a) } else { Some(b) },
        (a, b) => a.or(b),
    }
}

/// Read a thumbnail from Steam's local cache. Call from a worker, never the UI thread.
pub fn read_thumbnail(steam_root: &Path, account: u32, clip_id: &str, thumbnail_url: &str) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    if !valid_url(thumbnail_url, clip_id) { return None; }
    decode(&read_bytes(steam_root, account, clip_id)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    const ID: &str = "test_clip-1";
    const URL: &str = "https://steamloopback.host/opaque/gamerecordings/clips/test_clip-1/thumbnail.jpg";
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!("deckshell-thumb-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            std::fs::create_dir(&p).unwrap(); Self(p)
        }
        fn clip(&self) -> PathBuf { self.0.join("userdata/42/gamerecordings/clips").join(ID) }
        fn setup(&self) { std::fs::create_dir_all(self.clip()).unwrap(); }
        fn read(&self, url: &str) -> Option<SharedPixelBuffer<Rgba8Pixel>> { read_thumbnail(&self.0, 42, ID, url) }
    }
    impl Drop for Temp { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::new(w, h)).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }
    #[test]
    fn url_allowlist_and_missing_file() {
        let t = Temp::new();
        assert!(valid_url(URL, ID));
        assert!(valid_url("https://steamloopback.host/gamerecordings/clips/test_clip-1/thumbnail.jpg", ID));
        for url in ["http://steamloopback.host/gamerecordings/clips/test_clip-1/thumbnail.jpg", "https://steamloopback.host.evil/gamerecordings/clips/test_clip-1/thumbnail.jpg", "https://user@steamloopback.host/gamerecordings/clips/test_clip-1/thumbnail.jpg", "https://steamloopback.host:443/gamerecordings/clips/test_clip-1/thumbnail.jpg", "https://steamloopback.host/../gamerecordings/clips/test_clip-1/thumbnail.jpg", "https://steamloopback.host/%2e/gamerecordings/clips/test_clip-1/thumbnail.jpg", "https://steamloopback.host/gamerecordings/clips/test_clip-1/thumbnail.jpg?x", "https://steamloopback.host/gamerecordings/clips/test_clip-1/thumbnail.jpg#x", "https://steamloopback.host/gamerecordings/clips/test_clip-1\\/thumbnail.jpg", "https://steamloopback.host/gamerecordings/clips/other/thumbnail.jpg"] { assert!(!valid_url(url, ID), "{url}"); }
        assert!(!valid_url(URL, "../bad"));
        assert!(!valid_url(URL, &"a".repeat(129)));
        assert!(t.read(URL).is_none());
    }
    #[test]
    fn rejects_symlinks_and_fifo_without_blocking() {
        let t = Temp::new(); t.setup();
        let target = t.0.join("target"); std::fs::write(&target, png(10, 10)).unwrap();
        let file = t.clip().join("thumbnail.jpg"); symlink(&target, &file).unwrap();
        assert!(t.read(URL).is_none()); std::fs::remove_file(&file).unwrap();
        let name = CString::new(file.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(t.read(URL).is_none()); std::fs::remove_file(&file).unwrap();
        std::fs::remove_dir(t.clip()).unwrap(); symlink(&t.0, t.clip()).unwrap();
        assert!(t.read(URL).is_none());
    }
    #[test]
    fn bounds_and_decoding() {
        let t = Temp::new(); t.setup(); let file = t.clip().join("thumbnail.jpg");
        std::fs::write(&file, vec![0; MAX_BYTES as usize + 1]).unwrap(); assert!(t.read(URL).is_none());
        std::fs::write(&file, b"not an image").unwrap(); assert!(t.read(URL).is_none());
        std::fs::write(&file, png(4097, 1)).unwrap(); assert!(t.read(URL).is_none());
        std::fs::write(&file, png(3000, 3000)).unwrap(); assert!(t.read(URL).is_none());
        std::fs::write(&file, png(640, 320)).unwrap(); let image = t.read(URL).unwrap();
        assert_eq!((image.width(), image.height()), (640, 320));
        let mut jpeg = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::new(100, 100)).write_to(&mut jpeg, image::ImageFormat::Jpeg).unwrap();
        std::fs::write(&file, jpeg.into_inner()).unwrap();
        assert!(t.read(URL).is_some());
        let alias = t.0.with_extension("alias"); symlink(&t.0, &alias).unwrap();
        assert!(read_thumbnail(&alias, 42, ID, URL).is_some());
        std::fs::remove_file(alias).unwrap();
    }
    #[test]
    #[ignore = "requires private saved native-thumbnail.jpg; never committed"]
    fn private_saved_native_thumbnail_decodes_without_playback() {
        if let Some(metadata)=std::env::var_os("DECKSHELL_TEST_NATIVE_METADATA") {
            let mut bytes=Vec::new();File::open(metadata).unwrap().take(1024*1024).read_to_end(&mut bytes).unwrap();
            let value:serde_json::Value=serde_json::from_slice(&bytes).unwrap();
            let clip=&value.as_array().unwrap()[0];
            assert!(valid_url(clip["thumbnail_url"].as_str().unwrap(),clip["clip_id"].as_str().unwrap()));
        }
        let source=std::env::var_os("DECKSHELL_TEST_NATIVE_THUMBNAIL").expect("set private JPEG path");
        let source=File::open(source).unwrap();
        let mut bytes=Vec::new(); source.take(MAX_BYTES+1).read_to_end(&mut bytes).unwrap();
        assert!(bytes.len() as u64<=MAX_BYTES);
        let t=Temp::new();t.setup();std::fs::write(t.clip().join("thumbnail.jpg"),bytes).unwrap();
        let pixels=t.read(URL).expect("actual native JPEG accepted");
        assert!(pixels.width() <= 640 && pixels.height() <= 400);
    }

}
