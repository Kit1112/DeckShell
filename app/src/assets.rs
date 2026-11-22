//! Steam assets: extracted SVGs, packaged Settings and controller glyphs, and localized strings.
use slint::{ComponentHandle, Image};

use crate::{AppWindow, Icons};

thread_local! {
    /// rendered text -> loc token, recorded by t()/t_or()/tu() so the tree export (dumptree.rs) can report `loc_token`.
    static SEEN: std::cell::RefCell<std::collections::HashMap<String, String>> = Default::default();
}
fn seen(text: &str, token: &str) {
    if token.starts_with('#') && !text.is_empty() && text != token {
        SEEN.with(|s| {
            s.borrow_mut().entry(text.to_string()).or_insert_with(|| token.to_string());
        });
    }
}
/// Loc token that produced `text` (exact or upper-cased), if it was localized through t()/t_or()/tu().
pub fn token_for(text: &str) -> Option<String> {
    SEEN.with(|s| s.borrow().get(text).cloned())
}

/// Localize a token (`#Token`). Falls back to English then the bare token (steam-assets behaviour).
pub fn t(token: &str) -> String {
    let r = steam_assets::loc(token);
    seen(&r, token);
    r
}
/// Localize, or `fallback` when the token does not exist (use "" to hide).
pub fn t_or(token: &str, fallback: &str) -> String {
    if steam_assets::has_loc(token) { t(token) } else { fallback.to_string() }
}
pub fn tf(token: &str, args: &[&str]) -> String {
    steam_assets::loc_fmt(token, args)
}
/// Upper-cased like Steam's CSS `text-transform: uppercase` labels.
pub fn tu(token: &str) -> String {
    let r = t(token).to_uppercase();
    seen(&r, token);
    r
}

/// Icons ship as `currentColor` SVGs; rasterise them white (Slint `colorize` re-tints flat ones).
fn load_svg(path: &std::path::Path) -> Option<Image> {
    let txt = std::fs::read_to_string(path).ok()?;
    let txt = txt.replace("currentColor", "#ffffff");
    Image::load_from_svg_data(txt.as_bytes()).ok()
}

// A fixed set used by the global icons, Steam menu, QAM and on-screen keyboard.
// The experiment uses the same extracted SVG, rasterized on the asset worker.
const RASTER_NAMES: &[&str] = &[
    "home", "library", "store", "friends", "media", "downloads", "settings", "power", "search", "sdcard", "bell", "wifi",
    "battery.100", "play", "star", "controller", "info", "chat", "qam.performance", "dots", "refresh", "Desktop", "pause", "trash", "check", "volume", "volume.muted", "bluetooth", "brightness",
    "Battery.5", "Battery.20", "Battery.50", "Battery.80", "Battery.100", "VolumeMuted", "AudioVolumeIcon", "Microphone", "Brightness", "Settings", "Globe",
    // QAM Friends tabs and header buttons (Steam asset names; the cache keys are case-sensitive)
    "Star", "Friends", "Groups", "Recent", "PendingInvites", "AddFriend", "CreateGroup", "HardDrive",
    "Video", "Screenshot", "Achievement", "SteamLogo", "SteamControllerHollow", "ThumbsUp", "Clock", "SteamDeck", "Display", "Keyboard", "CustomizeSteamDeck", "Accessibility", "Notifications",
];
pub const CONTROLLER_MODE_ICONS: &[&str] = &[
    "Mouse", "GuideButton", "SteamInputModeScroll2D", "SteamInputModeScrollWheel",
    "TouchTap", "RadialMenu", "TouchMenu", "HotbarMenu",
];
const MAX_RASTER_SIDE: u32 = 256;
type RasterIcons = std::collections::HashMap<String, slint::SharedPixelBuffer<slint::Rgba8Pixel>>;
fn raster_cache() -> &'static std::sync::Mutex<RasterIcons> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<RasterIcons>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(RasterIcons::new()))
}
// The Deck femtovg path can fragment valid SVG geometry. The bounded asset-worker
// raster cache was verified on-device; keep an explicit vector A/B escape hatch.
fn raster_enabled() -> bool { std::env::var("DECKSHELL_RASTER_ICONS").as_deref() != Ok("0") }
// Bundled SVGs are generated from the gamepad components at build time.
macro_rules! bundled_svgs {
    ($($name:literal),+ $(,)?) => {
        &[$(($name, include_str!(concat!(env!("OUT_DIR"), "/", $name, ".svg")))),+]
    };
}
const CONTROLLER_SVGS: &[(&str, &str)] = bundled_svgs![
    "ControllerType.Deck", "ControllerType.Xbox", "ControllerType.Steam", "ControllerType.Triton",
    "ControllerType.PS4", "ControllerType.PS5", "ControllerType.Switch", "ControllerType.Hori",
    "ControllerType.Mobile", "ControllerType.Frame", "ControllerType.Generic",
    "Mouse", "GuideButton", "SteamInputModeScroll2D", "SteamInputModeScrollWheel",
    "TouchTap", "RadialMenu", "TouchMenu", "HotbarMenu", "User", "Edit",
    "Star", "SteamLogo", "ThumbsUp", "Clock", "Search", "Download", "Checkmark", "Caution",
];
const INTERNET_SVGS: &[(&str, &str)] = bundled_svgs![
    "LockClosed", "WirelessNetwork-0", "WirelessNetwork-1", "WirelessNetwork-2",
    "WirelessNetwork-3", "WirelessNetwork-4", "SteamPal", "WiredNetwork", "Checkmark",
    "WirelessDisabled", "WirelessOfflineMode", "WirelessOfflineModeFocused", "Close", "Caution", "WiFi", "Spinner",
    "FeatureVisible", "FeatureHidden", "CaratDown", "AirplaneMode", "Add", "NightMode",
    // Settings rail (stock page table, tools/extract-assets/internet.mjs)
    "Rail-SteamDeck", "Rail-Lock", "Rail-ExclamationPoint", "Rail-Display", "Rail-Lightning", "Rail-AudioVolumeIcon",
    "Rail-Bluetooth", "Rail-ControllerStatus", "Rail-Keyboard", "Rail-CustomizeSteamDeck", "Rail-Friends", "Rail-Download",
    "Rail-CloudSync", "Rail-FamilySharing", "Rail-RemotePlay", "Rail-HardDrive", "Rail-Home", "Rail-Library", "Rail-Shader",
    "Rail-SteamLogo", "Rail-ContactInfo", "Rail-InGame", "Rail-Music", "Rail-Broadcast", "Rail-Microphone", "Rail-Security",
    "Rail-Accessibility", "Rail-ShoppingCart", "Rail-GameRecording",
    // Bluetooth device icons (stock `D(A)`)
    "Rail-Headphones", "Rail-Mobile", "Rail-Volume100",
];

/// Stock settings rail icon (asset name) for a page id: the `icon:` of each `#Settings_Page_*` entry in sp.js.
pub fn settings_rail_icon(page: &str) -> Option<&'static str> {
    Some(match page.to_ascii_lowercase().as_str() {
        "system" => "Rail-SteamDeck", "internet" => "WirelessNetwork-4", "storage" => "Rail-HardDrive", "bluetooth" => "Rail-Bluetooth",
        "display" | "interface" => "Rail-Display", "power" => "Rail-Lightning", "audio" => "Rail-AudioVolumeIcon",
        "controller" => "Rail-ControllerStatus", "keyboard" => "Rail-Keyboard", "customization" => "Rail-CustomizeSteamDeck",
        "accessibility" => "Rail-Accessibility", "notifications" => "Rail-ExclamationPoint", "friends" => "Rail-Friends",
        "downloads" => "Rail-Download", "cloud" => "Rail-CloudSync", "family" => "Rail-FamilySharing", "remoteplay" => "Rail-RemotePlay",
        "home" => "Rail-Home", "library" => "Rail-Library", "store" => "Rail-ShoppingCart", "ingame" => "Rail-InGame",
        "compatibility" | "internal" => "Rail-SteamLogo", "gamerecording" => "Rail-GameRecording", "developer" => "Rail-Shader",
        "account" => "Rail-ContactInfo", "music" => "Rail-Music", "broadcast" => "Rail-Broadcast", "voice" | "ingamevoice" => "Rail-Microphone",
        "security" => "Rail-Security",
        _ => return None,
    })
}

/// Asset names of the Bluetooth device icons by `ui/screens/internet.slint` index: stock `D(A)` (sp.js:345501).
pub const BT_DEVICE_ICONS: [&str; 7] = ["Rail-Bluetooth", "Rail-Headphones", "Rail-Volume100", "Rail-ControllerStatus", "Rail-Mobile", "Rail-Display", "Rail-Keyboard"];
/// Index into `BT_DEVICE_ICONS` for a stock device type (`name` is the stock special case `SteamController`).
pub fn bt_device_icon(etype: u8, name: &str) -> i32 {
    if name == "SteamController" { return 3; }
    match etype { 4 | 5 => 1, 6 | 7 => 2, 9 | 10 => 3, 2 => 4, 3 => 5, 11 => 6, _ => 0 }
}

/// Sets the root element's size, replacing a `width`/`height` the stock markup already carries (a duplicate attribute fails to parse).
fn with_square_size(svg: &str, px: u32) -> String {
    let end = svg.find('>').unwrap_or(svg.len());
    let (head, rest) = svg.split_at(end);
    let mut head = head.to_string();
    for attr in [" width=\"", " height=\""] {
        if let Some(i) = head.find(attr) {
            if let Some(len) = head[i + attr.len()..].find('"') { head.replace_range(i..i + attr.len() + len + 1, ""); }
        }
    }
    format!("{head} width=\"{px}\" height=\"{px}\"{rest}")
}

fn internet_svg(name: &str, raster: bool) -> Option<Image> {
    let (_, svg) = INTERNET_SVGS.iter().find(|(key, _)| *key == name)?;
    let svg = svg.replace("currentColor", "#ffffff");
    // Sample large dialog glyphs at 2× their 96px maximum display size.
    let svg = if raster { with_square_size(&svg, 192) } else { svg };
    Image::load_from_svg_data(svg.as_bytes()).ok()
}

fn rasterize_icon(name: &str) -> Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>> {
    if let Some(image) = internet_svg(name, true) {
        return rasterize_image(image);
    }
    let path = steam_assets::icon(name)?;
    if name == "SteamControllerHollow" { rasterize_icon_file_sized(&path, true) } else { rasterize_icon_file(&path) }
}
fn rasterize_icon_file(path: &std::path::Path) -> Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>> {
    rasterize_icon_file_sized(path, false)
}
fn rasterize_icon_file_sized(path: &std::path::Path, controller_thumbnail: bool) -> Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>> {
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(2 * 1024 * 1024 + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() > 2 * 1024 * 1024 { return None; }
    let text = std::str::from_utf8(&bytes).ok()?.replace("currentColor", "#ffffff");
    // This Steam glyph has a 315x221 viewBox; keep the cache size cap and
    // rasterize the chooser thumbnail at its actual 36px display size.
    let text = if controller_thumbnail { text.replacen("<svg", "<svg width=\"36\" height=\"36\"", 1) } else { text };
    rasterize_image(Image::load_from_svg_data(text.as_bytes()).ok()?)
}
fn rasterize_image(image: Image) -> Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>> {
    let size = image.size();
    if size.width == 0 || size.height == 0 || size.width > MAX_RASTER_SIDE || size.height > MAX_RASTER_SIDE { return None; }
    image.to_rgba8()
}
fn prepare_raster_icons() -> RasterIcons {
    prepare_raster_icons_with(rasterize_icon)
}
fn prepare_raster_icons_with(mut load: impl FnMut(&str) -> Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>) -> RasterIcons {
    RASTER_NAMES.iter().copied().chain(INTERNET_SVGS.iter().map(|(name, _)| *name))
        .filter_map(|name| load(name).map(|pixels| (name.to_string(), pixels))).collect()
}

/// Steam icon by name/alias. Raster experiment returns only prepared local pixels;
/// missing assets remain unavailable and are never decoded on the UI thread.
pub fn try_icon_image(name: &str) -> Option<Image> {
    if raster_enabled() {
        raster_cache().lock().unwrap().get(name).cloned().map(Image::from_rgba8)
    } else {
        internet_svg(name, false).or_else(|| steam_assets::icon(name).and_then(|p| load_svg(&p)))
    }
}
pub fn icon_image(name: &str) -> Image {
    try_icon_image(name).unwrap_or_default()
}

#[derive(Debug, PartialEq)]
pub struct BatteryVisual {
    pub percent: i32,
    pub full: bool,
    pub bolt: bool,
    pub color: &'static str,
}

pub fn battery_visual(level: Option<f64>, online: bool, full: bool) -> BatteryVisual {
    match level.filter(|n| n.is_finite()) {
        None => BatteryVisual { percent: -1, full: false, bolt: false, color: "#59BF40" },
        Some(n) => BatteryVisual {
            percent: (n.clamp(0.0, 1.0) * 100.0).round().min(99.0) as i32,
            full, bolt: online && !full,
            color: if !full && n <= 0.2 { "#E01D79" } else { "#59BF40" },
        },
    }
}

#[cfg(test)]
#[test]
fn battery_state_mapping() {
    for (name, level, online, full, percent, bolt, color) in [
        ("full", Some(1.0), true, true, 99, false, "#59BF40"),
        ("80", Some(0.8), false, false, 80, false, "#59BF40"),
        ("20", Some(0.2), false, false, 20, false, "#E01D79"),
        ("10", Some(0.1), false, false, 10, false, "#E01D79"),
        ("charging", Some(0.5), true, false, 50, true, "#59BF40"),
        ("not charging", Some(0.5), false, false, 50, false, "#59BF40"),
        ("unknown", None, false, false, -1, false, "#59BF40"),
    ] {
        assert_eq!(battery_visual(level, online, full), BatteryVisual { percent, full, bolt, color }, "{name}");
    }
}

/// Stock Steam battery SVG: continuous fill and AC bolt / full check.
pub fn battery_icon(level: f64, charging: bool, full: bool) -> Image {
    battery_icon_ink(level, charging, full, "white")
}

/// Same icon with a dark outline and mark, drawn on the white block of a focused header item.
pub fn battery_icon_dark(level: f64, charging: bool, full: bool) -> Image {
    battery_icon_ink(level, charging, full, "#23262e")
}

fn battery_icon_ink(level: f64, charging: bool, full: bool, ink: &str) -> Image {
    let level = if level.is_finite() { level.clamp(0.0, 1.0) } else { 0.0 };
    let visual = battery_visual(Some(level), charging, full);
    let color = visual.color;
    let mark = if full { r#"<path d="M23.591 13.3334L18.3713 18.8147L16.0198 16.355L14.1665 18.2756L18.3713 22.6667L25.4443 15.254Z" fill="INK"/>"# } else if visual.bolt { r#"<path d="M16 20L21 11V16H26L21 25V20H16Z" fill="INK"/>"# } else { "" }.replace("INK", ink);
    let width = if full { 27.0 } else { 27.0 * level };
    let svg = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="52" height="36" viewBox="-8 0 52 36"><path fill-rule="evenodd" d="M39 6H0V30H39V22H42V14H39V6ZM36 9H3V27H36V9Z" fill="{ink}"/><rect x="6" y="12" width="{width}" height="12" fill="{color}"/>{mark}</svg>"#);
    Image::load_from_svg_data(svg.as_bytes()).ok().and_then(rasterize_image).map(Image::from_rgba8).unwrap_or_default()
}

include!(concat!(env!("OUT_DIR"), "/controller_glyphs.rs"));

thread_local! {
    // Keep decoded images stable across focus changes and repeated model updates.
    static CONTROLLER_IMAGES: std::cell::RefCell<std::collections::HashMap<(*const u8, usize), Image>> =
        Default::default();
}

fn cached_controller_image(bytes: &'static [u8], load: impl FnOnce() -> Option<Image>) -> Option<Image> {
    let key = (bytes.as_ptr(), bytes.len());
    CONTROLLER_IMAGES.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(image) = cache.get(&key) {
            return Some(image.clone());
        }
        let image = load()?;
        cache.insert(key, image.clone());
        Some(image)
    })
}

/// Original Steam Input glyph embedded in the DeckShell binary.
pub fn controller_glyph(name: &str, variant: &str) -> Option<Image> {
    let bytes = controller_glyph_bytes(variant, name)?;
    cached_controller_image(bytes, || {
        if variant == "knockout" {
            return Image::load_from_svg_data(bytes).ok();
        }
        let rgba = image::load_from_memory_with_format(bytes, image::ImageFormat::Png).ok()?.into_rgba8();
        Some(Image::from_rgba8(slint::SharedPixelBuffer::clone_from_slice(
            rgba.as_raw(), rgba.width(), rgba.height(),
        )))
    })
}

/// Packaged Steam UI icon for the controller tabs, layouts and mode choices.
pub fn controller_icon(name: &str) -> Option<Image> {
    let (_, svg) = CONTROLLER_SVGS.iter().find(|(key, _)| *key == name)?;
    cached_controller_image(svg.as_bytes(), || {
        let svg = svg.replace("currentColor", "#ffffff");
        if !raster_enabled() {
            return Image::load_from_svg_data(svg.as_bytes()).ok();
        }
        let svg = if matches!(name, "ControllerType.Frame" | "ControllerType.Hori") {
            with_square_size(&svg, 36)
        } else {
            svg
        };
        rasterize_image(Image::load_from_svg_data(svg.as_bytes()).ok()?).map(Image::from_rgba8)
    })
}

/// Deck controller glyph for a footer hint key (`a b x y menu view steam qam l1 ...`).
pub fn glyph_image(key: &str) -> Option<Image> {
    let name = steam_assets::hint_glyph_name(key);
    controller_glyph(&name, "knockout").or_else(|| controller_glyph(&name, "knockout.md"))
}

/// Fill the `Icons` global. Cheap (~30 small SVGs) but runs `extract.mjs` once if the cache is missing, so in that case
/// it is deferred to a worker thread and applied when ready.
pub fn init_icons(ui: &AppWindow) {
    apply_controller_icons(ui);
    // Validation and any extraction run off the UI thread, including incomplete caches.
    let w = ui.as_weak();
    std::thread::spawn(move || {
        steam_assets::ensure_extracted();
        if raster_enabled() {
            *raster_cache().lock().unwrap() = prepare_raster_icons();
        }
        crate::screens::osk::prepare_icons();
        let _ = slint::invoke_from_event_loop(move || {
            crate::with_app(|a| a.osk_icons_ready());
            if let Some(ui) = w.upgrade() {
                apply_icons(&ui);
                crate::with_app(|a| {
                    a.menu_icons_ready();
                    a.build_settings_rail();
                    a.qam_icons_ready();
                });
            }
        });
    });
}

/// Test helper: load the bundled and cached Steam icons synchronously.
#[cfg(test)]
pub(crate) fn load_icons_for_test(ui: &AppWindow) {
    *raster_cache().lock().unwrap() = prepare_raster_icons();
    apply_controller_icons(ui);
    apply_icons(ui);
}

fn apply_controller_icons(ui: &AppWindow) {
    let g = ui.global::<Icons>();
    // Set before the icon properties: the pure callback must exist when the bindings that use it evaluate.
    g.on_tinted(tint_image);
    g.set_configurator_deck(controller_icon("ControllerType.Deck").unwrap_or_default());
    g.set_configurator_xbox(controller_icon("ControllerType.Xbox").unwrap_or_default());
}

fn apply_icons(ui: &AppWindow) {
    let g = ui.global::<Icons>();
    g.set_home(icon_image("home"));
    g.set_library(icon_image("library"));
    g.set_store(icon_image("store"));
    g.set_friends(icon_image("friends"));
    g.set_media(icon_image("media"));
    g.set_downloads(icon_image("downloads"));
    g.set_settings(icon_image("settings"));
    g.set_power(icon_image("power"));
    g.set_search(icon_image("search"));
    g.set_sdcard(icon_image("sdcard"));
    // Steam's internal-drive glyph for storage device cards (task 145; stock is HardDrive, not a placeholder)
    g.set_harddrive(icon_image("HardDrive"));
    g.set_bell(icon_image("bell"));
    g.set_wifi(icon_image("WirelessNetwork-4"));
    g.set_wifi_off(icon_image("WirelessDisabled"));
    g.set_wifi_offline(icon_image("WirelessOfflineMode"));
    g.set_wifi_offline_focused(icon_image("WirelessOfflineModeFocused"));
    g.set_battery(icon_image("battery.100"));
    g.set_lock(icon_image("LockClosed"));
    g.set_deck(icon_image("SteamPal"));
    g.set_globe(icon_image("WiredNetwork"));
    g.set_checkmark(icon_image("Checkmark"));
    g.set_close(icon_image("Close"));
    g.set_caution(icon_image("Caution"));
    g.set_spinner(icon_image("Spinner"));
    g.set_feature_visible(icon_image("FeatureVisible"));
    g.set_feature_hidden(icon_image("FeatureHidden"));
    g.set_carat_down(icon_image("CaratDown"));
    g.set_wifi_levels(std::rc::Rc::new(slint::VecModel::from(
        (0..=4).map(|level| icon_image(&format!("WirelessNetwork-{level}"))).collect::<Vec<_>>()
    )).into());
    g.set_play(icon_image("play"));
    g.set_star(icon_image("star"));
    g.set_controller(icon_image("controller"));
    g.set_info(icon_image("info"));
    g.set_chat(icon_image("chat"));
    g.set_performance(icon_image("qam.performance"));
    g.set_plugin(icon_image("dots"));
    g.set_sleep(icon_image("power"));
    g.set_restart(icon_image("refresh"));
    g.set_desktop(icon_image("Desktop"));
    g.set_pause(icon_image("pause"));
    g.set_trash(icon_image("trash"));
    g.set_check(icon_image("check"));
    g.set_volume(icon_image("volume"));
    g.set_volume_muted(icon_image("volume.muted"));
    g.set_bluetooth(icon_image("bluetooth"));
    g.set_brightness(icon_image("brightness"));
    g.set_device_icons(std::rc::Rc::new(slint::VecModel::from(BT_DEVICE_ICONS.iter().map(|n| icon_image(n)).collect::<Vec<_>>())).into());
}

thread_local! {
    /// Tinted copies keyed by (source pixel buffer address, colour). Each entry keeps its source alive, so an
    /// address cannot be reused by another icon while cached.
    static TINTED: std::cell::RefCell<std::collections::HashMap<(usize, u32), (slint::SharedPixelBuffer<slint::Rgba8Pixel>, slint::SharedPixelBuffer<slint::Rgba8Pixel>)>> = Default::default();
}

/// Flat colour for an icon. Icons are white alpha masks (see `load_svg`), so the colour replaces RGB and
/// scales alpha: the same result the renderer's `colorize` gave, but as new pixels. Every (icon, colour) pair
/// owns its buffer, so no two icons, nor one icon in two colours, ever share a recoloured texture.
/// White stays the source itself, so untinted icons keep their original texture.
pub fn tint_image(src: Image, color: slint::Color) -> Image {
    if color.alpha() == 255 && color.red() == 255 && color.green() == 255 && color.blue() == 255 {
        return src;
    }
    // Borrowed textures can lack accessible pixels.
    let Some(source) = src.to_rgba8() else { return src };
    let key = (source.as_slice().as_ptr() as usize, color.as_argb_encoded());
    TINTED.with(|cache| {
        let mut cache = cache.borrow_mut();
        let entry = cache.entry(key).or_insert_with(|| (source.clone(), tint_pixels(&source, color)));
        Image::from_rgba8(entry.1.clone())
    })
}

fn tint_pixels(source: &slint::SharedPixelBuffer<slint::Rgba8Pixel>, color: slint::Color) -> slint::SharedPixelBuffer<slint::Rgba8Pixel> {
    let mut out = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(source.width(), source.height());
    for (dst, src) in out.make_mut_slice().iter_mut().zip(source.as_slice()) {
        *dst = slint::Rgba8Pixel {
            r: color.red(),
            g: color.green(),
            b: color.blue(),
            a: ((src.a as u32 * color.alpha() as u32 + 127) / 255) as u8,
        };
    }
    out
}

#[cfg(test)]
mod icon_tint_tests {
    use super::*;

    fn mask(w: u32, h: u32, alpha: u8) -> slint::SharedPixelBuffer<slint::Rgba8Pixel> {
        let mut buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(w, h);
        for p in buf.make_mut_slice() {
            *p = slint::Rgba8Pixel { r: 255, g: 255, b: 255, a: alpha };
        }
        buf
    }

    #[test]
    fn tint_keeps_the_alpha_mask_and_replaces_colour() {
        let src = mask(2, 1, 128);
        let out = tint_pixels(&src, slint::Color::from_argb_u8(255, 0x23, 0x26, 0x2e));
        assert_eq!(out.as_slice()[0], slint::Rgba8Pixel { r: 0x23, g: 0x26, b: 0x2e, a: 128 });
        // The source is never written.
        assert_eq!(src.as_slice()[0], slint::Rgba8Pixel { r: 255, g: 255, b: 255, a: 128 });
    }

    #[test]
    fn same_icon_in_two_colours_gets_two_buffers_and_the_source_stays_white() {
        let src = Image::from_rgba8(mask(4, 4, 255));
        let dark = tint_image(src.clone(), slint::Color::from_argb_u8(255, 0x23, 0x26, 0x2e));
        let light = tint_image(src.clone(), slint::Color::from_argb_u8(255, 0xdc, 0xde, 0xdf));
        let dark_again = tint_image(src.clone(), slint::Color::from_argb_u8(255, 0x23, 0x26, 0x2e));
        let dark_px = dark.to_rgba8().unwrap();
        let light_px = light.to_rgba8().unwrap();
        assert_eq!(dark_px.as_slice()[0].r, 0x23);
        assert_eq!(light_px.as_slice()[0].r, 0xdc);
        assert_ne!(dark_px.as_slice().as_ptr(), light_px.as_slice().as_ptr());
        assert_eq!(dark_again.to_rgba8().unwrap().as_slice().as_ptr(), dark_px.as_slice().as_ptr(), "cached per colour");
        assert_eq!(src.to_rgba8().unwrap().as_slice()[0].r, 255, "untinted source unchanged");
    }

    #[test]
    fn white_is_the_source_itself() {
        let src = Image::from_rgba8(mask(2, 2, 255));
        let white = tint_image(src.clone(), slint::Color::from_argb_u8(255, 255, 255, 255));
        assert_eq!(white.to_rgba8().unwrap().as_slice().as_ptr(), src.to_rgba8().unwrap().as_slice().as_ptr());
    }

    /// Regression guard for the owner's icon corruption: the renderer must never recolour an icon itself.
    /// Every flat colour goes through `Icons.tinted`, so no `colorize` may appear in the UI sources.
    #[test]
    fn no_slint_colorize_anywhere_in_the_ui() {
        fn scan(dir: &std::path::Path, hits: &mut Vec<String>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    scan(&p, hits);
                } else if p.extension().is_some_and(|x| x == "slint") {
                    let text = std::fs::read_to_string(&p).unwrap();
                    for (i, line) in text.lines().enumerate() {
                        if line.contains("colorize") {
                            hits.push(format!("{}:{}", p.display(), i + 1));
                        }
                    }
                }
            }
        }
        let mut hits = vec![];
        scan(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui"), &mut hits);
        assert!(hits.is_empty(), "runtime colorize is forbidden, use Icons.tinted: {hits:?}");
    }
}

#[cfg(test)]
mod raster_icon_tests {
    use super::*;
    #[test]
    fn issue_14_icon_regression() {
        // Synthetic shapes exercise paths, holes, non-square viewports and currentColor.
        // They do not certify the appearance of Steam's actual icons or Deck glyphs.
        let fixtures = [
            (r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><rect x="4" y="4" width="8" height="8" fill="currentColor"/></svg>"#, 16, 16, (5, 5), (0, 0)),
            (r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 20"><path fill="currentColor" fill-rule="evenodd" d="M2 2H30V18H2Z M10 6H22V14H10Z"/></svg>"#, 32, 20, (4, 4), (15, 10)),
            (r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="40"><circle cx="12" cy="20" r="8" fill="currentColor"/></svg>"#, 24, 40, (12, 20), (0, 0)),
        ];
        let expected = [
            "home", "library", "store", "friends", "media", "downloads", "settings", "power", "search", "sdcard", "bell", "wifi",
            "battery.100", "play", "star", "controller", "info", "chat", "qam.performance", "dots", "refresh", "Desktop", "pause", "trash", "check", "volume", "volume.muted", "bluetooth", "brightness",
            "Battery.5", "Battery.20", "Battery.50", "Battery.80", "Battery.100", "VolumeMuted", "AudioVolumeIcon", "Microphone", "Brightness", "Settings", "Globe",
            "Star", "Friends", "Groups", "Recent", "PendingInvites", "AddFriend", "CreateGroup", "HardDrive",
            "Video", "Screenshot", "Achievement", "SteamLogo", "SteamControllerHollow", "ThumbsUp", "Clock", "SteamDeck", "Display", "Keyboard", "CustomizeSteamDeck", "Accessibility", "Notifications",
        ];
        assert_eq!(RASTER_NAMES, expected);
        let expected = expected.into_iter().chain(INTERNET_SVGS.iter().map(|(name, _)| *name)).collect::<Vec<_>>();
        let dir = std::env::temp_dir().join(format!("deckshell-issue14-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
        let _cleanup = Cleanup(dir.clone());
        for (i, (svg, ..)) in fixtures.iter().enumerate() {
            std::fs::write(dir.join(format!("{i}.svg")), svg).unwrap();
        }
        let icons = prepare_raster_icons_with(|name| {
            let index = expected.iter().position(|candidate| *candidate == name)? % fixtures.len();
            rasterize_icon_file(&dir.join(format!("{index}.svg")))
        });
        assert_eq!(icons.len(), expected.len());
        for (i, name) in expected.iter().enumerate() {
            let (_, width, height, solid, clear) = fixtures[i % fixtures.len()];
            let pixels = icons.get(*name).unwrap_or_else(|| panic!("missing {name}"));
            assert_eq!((pixels.width(), pixels.height()), (width, height), "{name}");
            assert!(width <= MAX_RASTER_SIDE && height <= MAX_RASTER_SIDE);
            let pixel = |(x, y): (u32, u32)| {
                let offset = ((y * width + x) * 4) as usize;
                &pixels.as_bytes()[offset..offset + 4]
            };
            assert_eq!(pixel(solid), &[255, 255, 255, 255], "content: {name}");
            assert_eq!(pixel(clear)[3], 0, "transparency: {name}");
        }
        let oversized = dir.join("oversized.svg");
        std::fs::write(&oversized, r#"<svg xmlns="http://www.w3.org/2000/svg" width="257" height="2"><rect width="257" height="2"/></svg>"#).unwrap();
        assert!(rasterize_icon_file(&oversized).is_none());
        assert!(rasterize_icon_file(&dir.join("missing.svg")).is_none());
        let malformed = dir.join("malformed.svg");
        std::fs::write(&malformed, "not svg").unwrap();
        assert!(rasterize_icon_file(&malformed).is_none());
    }
    #[test]
    fn rasterizes_same_svg_with_pixels_and_bounded_dimensions() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 36 36"><path fill="#fff" d="M3 3H33V33H3Z"/></svg>"##;
        let image = Image::load_from_svg_data(svg).unwrap();
        let pixels = rasterize_image(image).unwrap();
        assert_eq!((pixels.width(), pixels.height()), (36, 36));
        assert!(pixels.as_bytes().chunks_exact(4).any(|p| p[3] != 0));
        let too_large = br##"<svg xmlns="http://www.w3.org/2000/svg" width="257" height="1"><path d="M0 0H1V1Z"/></svg>"##;
        assert!(rasterize_image(Image::load_from_svg_data(too_large).unwrap()).is_none());
        assert!(Image::load_from_svg_data(b"not svg").is_err());
    }
    #[test]
    fn controller_thumbnail_keeps_large_viewbox_inside_the_raster_limit() {
        let dir = std::env::temp_dir().join(format!("deckshell-controller-glyph-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("controller.svg");
        std::fs::write(&path, r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 315 221"><path fill="#fff" d="M0 0H315V221H0Z"/></svg>"##).unwrap();
        assert!(rasterize_icon_file(&path).is_none());
        let thumbnail = rasterize_icon_file_sized(&path, true).unwrap();
        assert_eq!((thumbnail.width(), thumbnail.height()), (36, 36));
        assert!(thumbnail.as_bytes().chunks_exact(4).any(|p| p[3] != 0));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn internet_wireless_glyphs_keep_five_signal_levels() {
        // Interior samples of the four Steam paths in the 36×36 source viewport,
        // ordered from the innermost segment outwards.
        let samples = [(5, 31), (5, 20), (5, 12), (5, 5)];
        for level in 0..=4 {
            let name = format!("WirelessNetwork-{level}");
            let size = internet_svg(&name, false).unwrap().size();
            assert_eq!((size.width, size.height), (36, 36));
            let pixels = rasterize_icon(&name).unwrap();
            let width = pixels.width() as usize;
            for (index, (x, y)) in samples.into_iter().enumerate() {
                let pixel = (x * width / 36) + (y * width / 36) * width;
                let expected = if index < level { 255 } else { 64 };
                assert_eq!(pixels.as_slice()[pixel].a, expected, "{name}, segment {index}");
            }
        }
    }

    #[test]
    fn raster_cache_set_is_fixed_and_covers_experiment_icons() {
        assert!(RASTER_NAMES.len() + INTERNET_SVGS.len() <= 120);
        assert_eq!(CONTROLLER_MODE_ICONS.len(), 8);
        for name in ["settings", "power", "search", "home", "Settings"] {
            assert!(RASTER_NAMES.contains(&name));
        }
        assert!(INTERNET_SVGS.iter().any(|(name, _)| *name == "WiFi"));
    }

    #[test]
    fn packaged_controller_images_decode() {
        let size = |image: Option<Image>| image.expect("packaged controller image").size();
        for key in ["a", "menu", "view", "steam", "qam", "l1", "r5", "shared_dpad_up"] {
            assert!(size(glyph_image(key)).width > 0, "{key}");
        }
        for (name, variant) in [
            ("sd_ltrackpad_swipe", "knockout.md"),
            ("sd_r2_half", "knockout.md"),
            ("shared_button_a", "light.md"),
            ("shared_button_a", "dark.md"),
            ("shared_mouse_scroll_up", "dark.md"),
        ] {
            assert!(size(controller_glyph(name, variant)).width > 0, "{name}/{variant}");
        }
        for (name, _) in CONTROLLER_SVGS {
            let image = controller_icon(name).unwrap_or_else(|| panic!("invalid packaged controller icon {name}"));
            assert!(image.to_rgba8().unwrap().as_slice().iter().any(|pixel| pixel.a != 0), "{name}");
        }
        assert!(controller_glyph("unknown", "knockout.md").is_none());
        assert!(controller_icon("").is_none());
    }
}
