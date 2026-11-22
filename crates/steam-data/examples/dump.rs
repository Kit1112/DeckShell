use std::time::Instant;
fn main() {
    let root = std::env::args().nth(1).map(std::path::PathBuf::from).or_else(steam_data::default_root).expect("no steam root");
    let t = Instant::now();
    let lib = steam_data::load(&root).expect("load");
    let dt = t.elapsed();
    let steam = lib.games.iter().filter(|g| g.kind == steam_data::GameKind::Steam).count();
    let inst = lib.games.iter().filter(|g| g.installed).count();
    let anim = lib.games.iter().filter(|g| g.art.capsule_animated.is_some()).count();
    let cap = lib.games.iter().filter(|g| g.art.capsule.is_some()).count();
    println!("root {}", root.display());
    println!("users {} active {:?}", lib.users.len(), lib.active_user);
    println!("games {} (steam {}, shortcuts {}, installed {}, with capsule {}, animated {})", lib.games.len(), steam, lib.games.len() - steam, inst, cap, anim);
    println!("collections {}: {:?}", lib.collections.len(), lib.collections);
    println!("load time: {:.2} ms", dt.as_secs_f64() * 1000.0);
    for g in lib.games.iter().take(10) {
        println!("{:>10} {:?} {:<40} inst={} pt={}m lp={:?} {}MB upd={} col={:?} cap={}", g.app_id, g.kind, g.name, g.installed, g.playtime_minutes, g.last_played, g.size_on_disk / 1_000_000, g.update_pending, g.collections, g.art.capsule.is_some());
    }
}
