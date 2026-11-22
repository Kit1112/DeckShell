use std::path::Path;
use std::time::Instant;

fn main() {
    let p = std::env::args().nth(1).expect("usage: bench <path> [w h]");
    let w = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(300);
    let h = std::env::args().nth(3).and_then(|s| s.parse().ok()).unwrap_or(450);
    let p = Path::new(&p);
    // Use a private cache so "cold" is truly cold.
    let c = std::env::temp_dir().join(format!("deckshell-bench-{}", std::process::id()));
    std::env::set_var("DECKSHELL_CACHE", &c);
    for i in 0..2 {
        let t = Instant::now();
        let f = media::load_thumbnail(p, w, h);
        let d = t.elapsed();
        match f { Ok(f) => println!("thumb {} {}x{}: {:?}", if i == 0 { "cold  " } else { "cached" }, f.width, f.height, d), Err(e) => { println!("thumb err: {e}"); break } }
    }
    for i in 0..2 {
        let t = Instant::now();
        let a = media::load_animation(p, w, h);
        let d = t.elapsed();
        match a { Ok(a) => println!("anim  {} frames={} {}ms total, {:?}", if i == 0 { "cold  " } else { "cached" }, a.frames.len(), a.total_ms(), d), Err(e) => { println!("anim err: {e}"); break } }
    }
    let _ = std::fs::remove_dir_all(c);
}
