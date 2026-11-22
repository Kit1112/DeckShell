//! `clipinfo <file> [w h]`: build the preview clip of a video / animated image and print frames, timing, size, time taken.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (w, h) = (a.get(2).and_then(|v| v.parse().ok()).unwrap_or(190), a.get(3).and_then(|v| v.parse().ok()).unwrap_or(285));
    let t = std::time::Instant::now();
    let c = media::open_clip(std::path::Path::new(&a[1]), w, h, 0).unwrap();
    let t1 = t.elapsed();
    let t = std::time::Instant::now();
    let mut buf = vec![0u8; c.raw_len(0)];
    for i in 0..c.len() { c.frame_into(i, &mut buf).unwrap(); }
    println!("{}: {} frames {}x{} total {} ms (avg {:.1} ms = {:.1} fps), built in {:?}, read all frames in {:?} ({:.2} ms/frame)",
        a[1], c.len(), c.width, c.height, c.total_ms(), c.total_ms() as f64 / c.len() as f64, 1000.0 * c.len() as f64 / c.total_ms() as f64, t1, t.elapsed(), t.elapsed().as_secs_f64() * 1000.0 / c.len() as f64);
}
