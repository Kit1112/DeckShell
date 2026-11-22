//! Print the resolved art for appids given on the command line (read-only).
use steam_bridge::*;
fn main() {
    let ids: Vec<u32> = std::env::args().skip(1).filter_map(|a| a.parse().ok()).collect();
    let b = Bridge::connect().expect("steam");
    let root = steam_data::default_root().unwrap();
    let urls = b.art_urls(&ids).unwrap();
    let r = ArtResolver::new(&root, b.active_account().unwrap());
    for u in urls {
        println!("{:#?}\n{:#?}", u, r.resolve(&u));
    }
}
