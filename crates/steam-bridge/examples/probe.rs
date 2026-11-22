//! Read-only probe: prints counts and timings. Never calls any action.
use std::time::Instant;
use steam_bridge::*;

fn main() {
    let t = Instant::now();
    let b = match Bridge::connect() {
        Ok(b) => b,
        Err(e) => return eprintln!("connect failed: {e}"),
    };
    println!("connect: {:?}", t.elapsed());
    let t = Instant::now();
    let apps = b.apps().expect("apps");
    println!("apps(): {:?}  total={} installed={} games={} shortcuts={}", t.elapsed(), apps.len(),
        apps.iter().filter(|a| a.installed).count(), apps.iter().filter(|a| a.is_game()).count(),
        apps.iter().filter(|a| a.is_shortcut).count());
    let t = Instant::now();
    let _ = b.apps();
    println!("apps() again: {:?}", t.elapsed());
    for a in apps.iter().filter(|a| a.installed && a.app_type == 1).take(3) {
        println!("  {} {} size={} play={}m last={} status={}", a.appid, a.display_name, a.size_on_disk, a.playtime_minutes, a.last_played, a.display_status);
    }
    let t = Instant::now();
    for c in b.collections().expect("collections") {
        println!("collection {:?} ({}) -> {}", c.name, c.id, c.apps.len());
    }
    println!("collections(): {:?}", t.elapsed());
    println!("recent: {:?}", b.recent_apps().map(|r| r.into_iter().take(5).collect::<Vec<_>>()));
    let t = Instant::now();
    let d = b.downloads().expect("downloads");
    println!("downloads(): {:?} items={} state={} net={}", t.elapsed(), d.items.len(), d.overview.state, d.overview.net_bytes_per_sec);
    for i in &d.items {
        println!("  dl {} paused={} {}% active={}", i.appid, i.paused, i.percent, i.active);
    }
    let n = b.notifications().expect("notifications");
    println!("notifications: unviewed={} tray={}", n.unviewed, n.items.len());
    let s = b.system().expect("system");
    println!("user: {:?} avatar={:?}", s.user.persona_name, s.user.avatar_path);
    println!("network: {:?}\nbattery: {:?}", s.network, s.battery);
    println!("audio: {:?}", b.audio_devices());
    println!("running: {:?}", b.running_apps());
    if let Some(a) = apps.iter().find(|a| a.installed && a.app_type == 1) {
        let t = Instant::now();
        let d = b.app_details(a.appid).expect("details");
        println!("app_details({}): {:?} compat={}", a.appid, t.elapsed(), d["compat_tool_name"]);
    }
    let rx = b.subscribe();
    let t = Instant::now();
    println!("event: {:?} after {:?}", rx.recv_timeout(std::time::Duration::from_secs(3)), t.elapsed());
}
