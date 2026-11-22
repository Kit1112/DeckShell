use steam_control::*;
fn main() {
    let pages = [SteamPage::Store, SteamPage::StoreApp(220), SteamPage::Friends, SteamPage::Downloads, SteamPage::Settings,
        SteamPage::Achievements(220), SteamPage::Properties(220), SteamPage::Screenshots, SteamPage::Keyboard];
    for p in &pages { println!("{:?} -> {}", p, url_for(p)); }
    println!("steam_running: {}", steam_running());
    println!("running_game: {:?}", running_game());
    // Build a bare Library (steam_root only) so the probe works independent of steam-data's loader.
    let root = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share/Steam")).unwrap_or_default();
    let lib = steam_data::Library { steam_root: root, ..Default::default() };
    println!("downloads: {:#?}", downloads(&lib));
}
