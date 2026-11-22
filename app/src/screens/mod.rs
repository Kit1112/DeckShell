//! One module per screen. Each owns its `State` (held as a field of `App`), its `impl App { .. }` handlers, its
//! localized strings (`init_strings`) and the wiring of its Slint `<Name>State` global callbacks (`wire`).
//! The matching Slint file is `ui/screens/<name>.slint`; keep cross-screen coupling to `App` accessors only.
pub mod account_auth;
pub mod account_request;
pub mod account_switch;
pub mod anim_page;
pub mod capture;
pub mod chrome;
pub mod controller;
#[allow(dead_code)] // UI integration follows the Steam Input data contract.
mod controller_browser;
#[allow(dead_code)] // Provisional until stock focus walk and editor integration.
pub(crate) mod controller_editor;
mod controller_bind_write;
mod controller_live;
pub(crate) mod controller_gyro;
mod controller_pages;
mod controller_pad;
mod controller_quick;
#[allow(dead_code)]
mod text_wrap;
mod controller_commands;
mod controller_layout_actions;
pub mod downloads;
pub mod downloads_menu;
pub mod downloads_page;
pub mod game;
pub mod install;
pub mod bluetooth;
pub mod internet;
pub mod home;
pub mod library;
pub mod manage;
pub mod manage_rules;
pub mod media;
pub mod osk;
pub mod osk_model;
mod osk_emoji_img;
pub mod properties;
pub mod qam;
pub mod search;
pub mod settings;
pub mod steam_menu;

#[path = "../hotkeys/mod.rs"]
pub mod hotkeys;

use crate::AppWindow;
use slint::ComponentHandle;

/// Connect every screen's Slint callbacks to the UI-thread application (called once from main).
pub fn wire_all(ui: &AppWindow) {
    account_switch::wire(ui);
    install::wire(ui);
    home::wire(ui);
    library::wire(ui);
    game::wire(ui);
    downloads::wire(ui);
    downloads_menu::wire(ui);
    steam_menu::wire(ui);
    qam::wire(ui);
    settings::wire(ui);
    chrome::wire(ui);
    properties::wire(ui);
    controller::wire(ui);
    controller_gyro::wire(ui);
    ui.global::<crate::HotkeysState>().on_power_click(|i| defer(move |a| a.hk_power_pick(i as usize)));
    ui.global::<crate::HotkeysState>().on_close(|| defer(|a| a.hk_close_pub()));
}

/// Run `f` on the UI thread against the application state (Slint callbacks are already on it; this defers to avoid
/// re-entrancy while Slint is still evaluating the callback).
pub fn defer(f: impl FnOnce(&mut crate::app::App) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || {
        crate::with_app(f);
    });
}

pub mod controller_sticks;

/// Prefer Steam's English label while keeping the existing Russian screen copy.
pub(crate) fn stock_label(token: &str, key: &str) -> String {
    let fallback = steam_assets::i18n::tr(key);
    if steam_assets::i18n::current().code() == "ru" {
        fallback
    } else {
        crate::assets::t_or(token, &fallback)
    }
}

/// Return the token itself so the footer retains Steam localization metadata.
pub(crate) fn stock_hint(token: &'static str, key: &'static str) -> &'static str {
    if steam_assets::i18n::current().code() != "ru" && steam_assets::has_loc(token) {
        token
    } else {
        steam_assets::i18n::tr_ref(key)
    }
}

#[cfg(test)]
pub(crate) fn expected_text<'a>(russian: &'a str, english: &'a str) -> &'a str {
    if steam_assets::i18n::current().code() == "ru" { russian } else { english }
}
