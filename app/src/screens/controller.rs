//! Native "Настройки контроллера" page (Steam Deck, gamepad icon on the game page). Spec: context/reference/controller/SPEC.md.
//! Rows come from the layout state exactly as sp.js `ControllerConfigurationQuickSettings` derives them (a row exists only
//! when the source's mode supports that setting key). The layout itself arrives from Steam only as protobuf messages
//! (RegisterForControllerConfigInfoMessages) into the internal `v3` store. The root reads active metadata from
//! Steam and available local Deck VDF settings. The Buttons editor reads the native editing copy; Undo restores raw bindings.
use std::cell::RefCell;
use std::cell::Cell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, ModelRc, VecModel};

use crate::app::App;
use crate::assets::t_or;
use crate::input::Action;
use crate::{AppWindow, CRow, ControllerState, SummaryRow, ControllerBrowserTab, ControllerConfigCard};
use steam_bridge::controller_list::{community_allowed, ActiveLayout, LayoutDetails, LayoutEntry};
use super::controller_layout_actions::{self as lact, Button as LButton, Plan};
use super::controller_browser::{Availability, Browser, Focus, Input, Outcome, Tab};
use super::controller_live as live;
use super::controller_bind_write as bind;
use super::controller_pages as pages;
use super::controller_quick as quick;
use super::controller_pad as pad;
use super::controller_sticks as sticks;
use super::controller_gyro::{self, Owner};
use super::settings::controller_devices::{self, Identity};
use super::controller_editor::{self as editor, Column, Direction, RowControls, EditorFocus};
use crate::{ChoiceItem, ControllerEditorRow, ControllerEditorSection, ControllerPickerKey, ControllerSettingRow, PadRow};

// UI routing and its snapshot belong to the same thread as ST.
thread_local! {
    static OPEN: Cell<bool> = const { Cell::new(false) };
    static SIG: Cell<i64> = const { Cell::new(0) };
    /// Звук, который обработчик назначил явно (тумблер, ползунок, конец списка вкладок); см. `nav_sound`.
    static SAY: Cell<Option<crate::sound::Snd>> = const { Cell::new(None) };
}

/// Явно заданный звук действия; первый заданный за одно нажатие выигрывает.
fn say(sound: crate::sound::Snd) {
    SAY.with(|c| if c.get().is_none() { c.set(Some(sound)); });
}

/// Снимок всего, что в Steam меняет звук навигации: фокус на любой странице, вкладки/наборы, открытые диалоги и сама страница.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NavSnap { open: bool, page: u8, modals: u8, tabs: String, focus: String, busy: bool }

pub fn nav_snap() -> NavSnap {
    SAY.with(|c| c.set(None));
    let s = st();
    let page = if s.menu_page > 0 { 1 } else if s.stick_settings.is_some() { 2 } else if s.pad.is_some() { 3 }
        else if s.summary_open { 4 } else if s.editor.as_ref().is_some_and(|f| f.picker_open) { 6 } else if s.editor.is_some() { 5 }
        else if s.browser_preview.is_some() { 8 } else if s.browser_open { 7 } else { 0 };
    let modals = u8::from(controller_gyro::active(Owner::Editor)) + u8::from(s.help_page > 0)
        + u8::from(s.menu_page > 0) + u8::from(s.quick_dialog.is_some()) + u8::from(s.command_dialog.is_some()) + u8::from(s.editor_set_prompt.is_some())
        + u8::from(s.pad_number.is_some()) + u8::from(s.pad.as_ref().is_some_and(|p| p.choice.is_some()))
        + u8::from(s.browser_delete_pending.is_some()) + u8::from(matches!(s.menu_page, 2 | 3 | 5 | 6 | 7))
        + u8::from(s.menu_page == 7 && s.bind_activation_parent == 5);
    let tabs = format!("{}|{}|{}|{:?}", s.tab, s.editor_set_index, s.picker_tab, s.browser.as_ref().map(|b| b.active));
    let focus = format!("{}|{}|{}|{}|{:?}|{:?}|{}|{}|{:?}|{:?}|{:?}", s.foc, s.quick_gear,
        s.menu_focus, s.help_page, s.browser.as_ref().map(|b| b.focus), s.editor,
        s.picker_cursors[s.picker_tab.min(7)], s.summary_focus, s.stick_settings, s.quick_dialog,
        s.command_dialog.as_ref().map(|d| d.cursor));
    let pad = s.pad.as_ref().map(|p| format!("|{}|{}|{}|{:?}|{}", p.focus, p.rail, p.slider, p.choice, p.fine)).unwrap_or_default();
    NavSnap { open: s.open, page, modals, tabs, focus: focus + &pad, busy: s.editor_busy }
}

/// Звук после нажатия `act` по правилам стока (docs/engine/sounds.json): явный звук обработчика; открытие/закрытие диалога —
/// ShowModal/HideModal; L1/R1 — ChangeTabs; сдвиг фокуса — BasicNav; A без перемены — DefaultOk; D-pad без перемены — FailedNav.
pub fn nav_sound(act: Action, before: &NavSnap) -> Option<crate::sound::Snd> {
    use crate::sound::Snd;
    let said = SAY.with(|c| c.take());
    let now = nav_snap();
    if before.busy { return None; }
    if said.is_some() { return said; }
    if now.modals > before.modals { return Some(Snd::ShowModal); }
    if now.modals < before.modals { return Some(Snd::HideModal); }
    if now.page != before.page || now.open != before.open {
        return (act == Action::A).then_some(Snd::DefaultOk);
    }
    if now.tabs != before.tabs { return Some(Snd::ChangeTabs); }
    if now.focus != before.focus { return Some(Snd::BasicNav); }
    match act {
        Action::A => Some(Snd::DefaultOk),
        Action::Up | Action::Down | Action::Left | Action::Right => Some(Snd::FailedNav),
        _ => None,
    }
}

// Root focus order, from Gaming Mode captures root/03, 05, 06, 40m. Quick-settings rows start at F_ROW0.
const F_DECK_TAB: i32 = 0;
const F_X360_TAB: i32 = 1;
const F_HELP: i32 = 2;
const F_CARD: i32 = 3;
const F_VIEW: i32 = 4;
const F_EDIT: i32 = 5;
const F_GEAR: i32 = 6;
const F_ROW0: i32 = 7;
/// Bottom edge of the quick-settings viewport (footer starts at y 740).
const LIST_BOTTOM: f32 = 738.0;
/// Gap between quick-settings rows (stock: toggle row 494..563, next row 572).
const ROW_GAP: f32 = 9.0;
pub fn is_open() -> bool {
    OPEN.get()
}
pub fn sig() -> i64 {
    SIG.get()
}

/// A numeric setting (`humanDisplayMin/Max` of the bundle `SW` table) with its unit.
#[derive(Clone, Debug, PartialEq)]
pub struct Val {
    pub key: &'static str,
    pub v: f32,
    pub min: f32,
    pub max: f32,
    pub step: f32,
    pub unit: Unit,
    /// false: key missing in the VDF (value shown as dash); int_min/int_max come from the Steam client, not the bundle, so min/max are only humanDisplay bounds
    pub known: bool,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Unit {
    Mult,
    Pixels,
}
/// One source (gyro / right trackpad / right stick) of the active layout. `None` fields = the mode does not support the key.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Src {
    pub mode: String,
    pub sens: Option<Val>,
    pub natural: Option<Val>,
    pub pixels: Option<Val>,
    pub invert: Option<bool>,
    pub min_out: Option<Val>,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    pub name: String,
    pub author: String,
    pub grips: Option<bool>,
    pub gyro: Option<Src>,
    pub rpad: Option<Src>,
    pub rstick: Option<Src>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    Grips(bool),
    Mode { src: &'static str, label: String, value: String },
    Slider(Val),
    Toggle { key: &'static str, src: &'static str, label: String, on: bool },
    /// A row read from the live editing copy of the layout (see `controller_quick`).
    Live(quick::QRow),
}

/// Vertical layout of the root page in device px, measured on the owner's stock frames of 20.11
/// (no controller tabs: «Текущая раскладка» at y 99, card at 135, buttons 15 px under the card or 16 under the hint plate).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Geo { title_y: f32, card_y: f32, card_h: f32, banner_y: f32, btn_y: f32, band_end: f32, head_y: f32, rows_y: f32, list_h: f32 }
fn geometry(tabs: bool, author: bool, title_lines: u32, banner: bool) -> Geo {
    let dy = if tabs { 88.0 } else { 0.0 };
    let card_y = 135.0 + dy;
    let card_h = if author { 84.0 } else { 80.0 } + 30.0 * (title_lines.max(1) - 1) as f32;
    let banner_y = card_y + card_h + 1.0;
    let btn_y = if banner { banner_y + 60.0 + 16.0 } else { card_y + card_h + 15.0 };
    let btn_bottom = btn_y + 60.0;
    let rows_y = btn_bottom + 113.0;
    Geo { title_y: 82.0 + dy, card_y, card_h, banner_y, btn_y, band_end: btn_bottom + 44.0, head_y: btn_bottom + 68.0, rows_y, list_h: (LIST_BOTTOM - rows_y).max(96.0) }
}
/// Author line and layout name shown on the card.
fn root_texts(s: &St) -> (String, String) {
    if let Some(active) = s.root_active.as_ref().filter(|v| !v.title.is_empty()) { return (active.author_name.clone(), layout_title(&active.title)); }
    match &s.layout {
        Some(l) => (l.author.clone(), l.name.clone()),
        None if s.editor_busy => (String::new(), "Чтение раскладки Steam…".into()),
        None => (String::new(), "Раскладка Steam недоступна".into()),
    }
}
fn root_geo(s: &St) -> Geo {
    let (author, name) = root_texts(s);
    let lines = ((name.chars().count() as f32 * 11.6 / 600.0).ceil() as u32).clamp(1, 3);
    // The hint plate belongs to games without an official layout: the current layout is a stock template.
    let banner = s.root_active.as_ref().is_some_and(|a| a.url.starts_with("template://"));
    geometry(s.x360_connected, !author.is_empty(), lines, banner)
}
fn has_banner(s: &St) -> bool { s.root_active.as_ref().is_some_and(|a| a.url.starts_with("template://")) }

pub fn fmt_val(v: &Val) -> String {
    if !v.known {
        return "—".into();
    }
    match v.unit {
        Unit::Mult => format!("{:.1}x", v.v),
        Unit::Pixels => format!("{}пикс.", v.v.round() as i64),
    }
}

/// Rows in the order of sp.js:356426-356707. Stock's quick settings have no calibration row (it lives on the
/// Gyro editor page, `editor-gyro/20`); an unreadable layout (`None`) therefore yields no rows at all.
pub fn rows(l: Option<&Layout>) -> Vec<Row> {
    let mut r = vec![];
    if let Some(l) = l {
        if let Some(g) = l.grips {
            r.push(Row::Grips(g));
        }
        if let Some(s) = &l.gyro {
            r.push(Row::Mode { src: "gyro", label: t_or("#ControllerConfigurator_SourceMode_Group_Behavior", "Функции: Гироскоп"), value: s.mode.clone() });
            for v in [&s.sens, &s.natural, &s.pixels].into_iter().flatten() {
                r.push(Row::Slider(v.clone()));
            }
            if let Some(on) = s.invert {
                r.push(Row::Toggle { key: "invert_y", src: "gyro", label: "Инвертировать ось Y гироскопа".into(), on });
            }
            if let Some(v) = &s.min_out {
                r.push(Row::Slider(v.clone()));
            }
        }
        if let Some(s) = &l.rpad {
            r.push(Row::Mode { src: "rpad", label: "Функции: Правый трекпад".into(), value: s.mode.clone() });
            if let Some(v) = &s.sens {
                r.push(Row::Slider(v.clone()));
            }
            if let Some(on) = s.invert {
                r.push(Row::Toggle { key: "invert_y", src: "rpad", label: "Инвертировать ось Y правого трекпада".into(), on });
            }
        }
        if let Some(s) = &l.rstick {
            if let Some(v) = &s.sens {
                r.push(Row::Slider(v.clone()));
            }
            if let Some(on) = s.invert {
                r.push(Row::Toggle { key: "invert_y", src: "rstick", label: "Инвертировать ось Y правого джойстика".into(), on });
            }
        }
    }
    r
}

fn label_of(v: &Val) -> String {
    match v.key {
        "gyro_natural" => "Чувствительность гироскопа (в °)".into(),
        "gyro_pixels" => "Точек на 360°".into(),
        "gyro_sens" => "Чувствительность гироскопа".into(),
        "min_out" => "Минимальный выход гироскопа в джойстик".into(),
        k => k.into(),
    }
}

/// Layout read from Steam's own autosave: `<steam>/steamapps/common/Steam Controller Configs/<acct>/config/<key>/controller_neptune.vdf`
/// (key = appid, or the lowercased name for non-Steam shortcuts). Read-only: Steam owns these files.
fn read_layout(root: &std::path::Path, acct: u32, key: &str, lang: &str) -> Option<Layout> {
    let p = root.join("steamapps/common/Steam Controller Configs").join(acct.to_string()).join("config").join(key).join("controller_neptune.vdf");
    match std::fs::read_to_string(&p) {
        Ok(t) => Some(layout_from_vdf(&t, lang)),
        Err(e) => {
            eprintln!("[controller] no layout file {} ({e}): shown as not found, no defaults assumed", p.display());
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum V {
    S(String),
    M(Vec<(String, V)>),
}
impl V {
    fn get(&self, k: &str) -> Option<&V> {
        match self {
            V::M(m) => m.iter().find(|(a, _)| a == k).map(|(_, v)| v),
            _ => None,
        }
    }
    fn all<'a>(&'a self, k: &'a str) -> impl Iterator<Item = &'a V> {
        let m: &[(String, V)] = match self { V::M(m) => m, _ => &[] };
        m.iter().filter(move |(a, _)| a == k).map(|(_, v)| v)
    }
    fn s(&self, k: &str) -> Option<&str> {
        match self.get(k) { Some(V::S(s)) => Some(s), _ => None }
    }
}

/// Minimal Valve KeyValues parser (quoted strings, braces).
pub fn parse_vdf(t: &str) -> V {
    let mut toks: Vec<String> = vec![];
    let mut it = t.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '"' => {
                let mut s = String::new();
                while let Some(c) = it.next() {
                    match c {
                        '\\' => if let Some(n) = it.next() { s.push(match n { 'n' => '\n', 't' => '\t', o => o }) },
                        '"' => break,
                        c => s.push(c),
                    }
                }
                toks.push(s);
            }
            '{' => toks.push("\u{1}{".into()),
            '}' => toks.push("\u{1}}".into()),
            '/' if it.peek() == Some(&'/') => { while let Some(&c) = it.peek() { if c == '\n' { break; } it.next(); } }
            _ => {}
        }
    }
    fn block(toks: &[String], i: &mut usize) -> V {
        let mut m = vec![];
        while *i < toks.len() {
            let k = toks[*i].clone();
            *i += 1;
            if k == "\u{1}}" { break; }
            if *i < toks.len() && toks[*i] == "\u{1}{" {
                *i += 1;
                m.push((k, block(toks, i)));
            } else if *i < toks.len() {
                m.push((k, V::S(toks[*i].clone())));
                *i += 1;
            }
        }
        V::M(m)
    }
    let mut i = 0;
    block(&toks, &mut i)
}

fn mode_label(m: &str) -> String {
    match m {
        // Stock root (root/40m, quick-settings/09): a source without an active group shows «Не выбрано».
        "" => "Не выбрано".into(),
        "gyro_to_mouse" => t_or("#ControllerConfigurator_SourceMode_Gyro_To_Mouse_Title", "Гироскоп как мышь"),
        "mouse_joystick" => t_or("#ControllerConfigurator_SourceMode_MouseJoystick_Title", "Как джойстик"),
        o => o.to_string(),
    }
}

/// Active group of a source in preset 0 (`group_source_bindings`: "<group id>" "<source> active|inactive").
fn active_group<'a>(root: &'a V, source: &str) -> Option<&'a V> {
    let want = format!("{source} active");
    let gid = root.get("preset")?.get("group_source_bindings").and_then(|b| match b { V::M(m) => m.iter().find(|(_, v)| matches!(v, V::S(s) if *s == want)).map(|(k, _)| k.clone()), _ => None })?;
    root.all("group").find(|g| g.s("id") == Some(gid.as_str()))
}

fn val(g: &V, vkey: &'static str, raw: &str, scale: f32, hmax: f32, step: f32, unit: Unit) -> Val {
    let r = g.get("settings").and_then(|s| s.s(raw)).and_then(|x| x.parse::<f32>().ok()).map(|x| x * scale);
    Val { key: vkey, v: r.unwrap_or(0.0), min: 0.0, max: hmax, step, unit, known: r.is_some() }
}

/// Config fields to rows (sp.js:356461-356707): gyro_to_mouse (n.qL, sp.js:369375) offers natural sensitivity, pixels per 360 and invert Y.
/// Other modes' key tables were not derived (int_min/int_max live in the Steam client): only their mode row is shown.
pub fn layout_from_vdf(t: &str, lang: &str) -> Layout {
    let root = parse_vdf(t);
    let root = root.get("controller_mappings").cloned().unwrap_or(root);
    let title = root.s("title").unwrap_or("");
    let loc = root.get("localization");
    let lt = |l: &str| loc.and_then(|x| x.get(l)).and_then(|x| x.s("title")).map(String::from);
    let name = if title.starts_with('#') { lt(lang).or_else(|| lt("english")).unwrap_or_else(|| title.to_string()) } else { title.to_string() };
    // Stock lists the gyro and right-trackpad mode rows even when the source has no active group («Не выбрано»).
    let src = |source: &str| {
        let group = active_group(&root, source);
        let mode = group.and_then(|g| g.s("mode")).unwrap_or("").to_string();
        let mut s = Src { mode: mode_label(&mode), ..Default::default() };
        if let (Some(g), "gyro_to_mouse") = (group, mode.as_str()) {
            s.natural = Some(val(g, "gyro_natural", "gyro_natural_sensitivity", 0.01, 30.0, 0.1, Unit::Mult));
            s.pixels = Some(val(g, "gyro_pixels", "gyro_angles_to_pixels", 1.0, 1800.0, 1.0, Unit::Pixels));
            let inv = g.get("settings").and_then(|x| x.s("gyro_invert_y")).map(|x| x == "1");
            s.invert = Some(inv.unwrap_or(false));
        }
        Some(s)
    };
    let grips = active_group(&root, "switch").map(|g| {
        g.get("inputs").map(|i| match i { V::M(m) => m.iter().any(|(k, v)| (k.contains("back") || k.contains("grip")) && format!("{v:?}").contains("binding")), _ => false }).unwrap_or(false)
    });
    Layout { name, author: String::new(), grips, gyro: src("gyro"), rpad: src("right_trackpad"), rstick: None }
}

#[derive(Default)]
struct St {
    open: bool,
    app_id: u32,
    name: String,
    layout: Option<Layout>,
    rows: Vec<Row>,
    /// See F_DECK_TAB..F_GEAR; F_ROW0 + i = rows[i].
    foc: i32,
    browser_open: bool,
    browser: Option<Browser>,
    browser_query: String,
    browser_entries: Option<Result<Vec<LayoutEntry>, String>>,
    browser_generation: u64,
    browser_active: Option<ActiveLayout>,
    browser_controller_type: u32,
    browser_controller_index: Option<u32>,
    browser_connected: bool,
    browser_details: HashMap<u64, LayoutDetails>,
    browser_votes: HashMap<u64, bool>,
    /// URL of a personal layout awaiting confirmation.
    browser_delete_pending: Option<String>,
    browser_preview: Option<LayoutEntry>,
    editor: Option<EditorFocus>,
    stick_settings: Option<editor::StickFocus>,
    picker_tab: usize,
    picker_cursors: [usize; 8],
    editing: Option<(u32, Vec<u8>, Vec<live::BindingRow>)>,
    editor_gyro_target: Option<Identity>,
    editor_busy: bool,
    editor_status: String,
    editor_dirty: bool,
    editor_uncertain: bool,
    editor_set_index: usize,
    editor_set_prompt: Option<(String, Option<String>, String)>,
    /// «Добавить виртуальное меню»: the name typed in the dialog, waiting for the style choice (`quick::VMENU` list).
    vmenu_name: Option<String>,
    /// «Сохранить как…»: the title typed so far (the layout is exported as Personal Local).
    export_prompt: Option<String>,
    command_dialog: Option<live::CommandDialog>,
    editor_undo: Vec<sticks::Undo>,
    stick_index: usize,
    summary_open: bool,
    summary_focus: usize,
    /// Editing copy read for the quick settings (controller index, raw layout); the root owns this session.
    quick: Option<(u32, Vec<u8>)>,
    quick_error: Option<String>,
    /// Open mode list of a quick-settings drop-down: (source, cursor).
    quick_dialog: Option<(i64, usize)>,
    /// The mode list was opened from a «Функции» row of the editor (writes go to the editing copy, not the root's).
    mode_list_editor: bool,
    /// Страница настроек источника («Настройки Как джойстик»), кадры controls/05–11.
    pad: Option<Pad>,
    pad_number: Option<(String, bool)>,
    /// Focus sits on the gear of a quick-settings row.
    quick_gear: bool,
    /// Native value being adjusted on the focused quick-settings slider.
    quick_slider: Option<i64>,
    menu_page: u8,
    menu_focus: usize,
    menu_error: String,
    bind_name: String,
    /// Address of the command whose «Настройки» window is open (activator index = the row's activator).
    bind_target: Option<steam_bridge::controller_edit::BindingTarget>,
    /// «Способ включения» list: the activation types on offer and the cursor is `menu_focus`.
    bind_activations: Vec<i64>,
    /// Page that opened the activation list, so cancellation restores its control.
    bind_activation_parent: u8,
    menu_info: Option<steam_bridge::controller_list::ActiveLayout>,
    menu_description: String,
    menu_author: String,
    menu_link_copied: bool,
    root_active: Option<ActiveLayout>,
    root_controller_index: Option<u32>,
    root_generation: u64,
    menu_generation: u64,
    help_page: u8,
    /// Selected controller type tab: 0 Steam Deck, 1 X360.
    tab: u8,
    x360_connected: bool,
}
/// Состояние страницы настроек источника: фокус в списке строк или в левом списке разделов, режим ползунка, открытый список значений.
#[derive(Clone, Debug, Default)]
struct Pad { source: i64, editor_set: Option<usize>, shift: bool, focus: usize, rail: bool, slider: bool, fine: bool, scroll: f32, choice: Option<usize>, dirty: bool, uncertain: bool }
thread_local! { static ST: &'static RefCell<St> = Box::leak(Box::new(RefCell::new(St::default()))); }
fn st() -> std::cell::RefMut<'static, St> {
    ST.with(|s| s.borrow_mut())
}

pub fn wire(ui: &AppWindow) {
    ui.global::<ControllerState>().on_menu_selected(|i| super::defer(move |a| { st().menu_focus = i.max(0) as usize; if st().menu_page >= 4 { binding_menu_handle(a, Action::A) } else { menu_activate(a) } }));
    ui.global::<ControllerState>().on_clicked(|i| super::defer(move |a| {
        if st().quick_slider.is_some() { return handle_quick_slider(a, Action::A); }
        { let mut s = st(); if s.editor_busy { return; } s.foc = i; s.quick_gear = false; }
        activate(a);
    }));
    ui.global::<ControllerState>().on_quick_gear_clicked(|i| super::defer(move |a| {
        if st().quick_slider.is_some() { return handle_quick_slider(a, Action::A); }
        { let mut s = st(); if s.editor_busy { return; } s.foc = i; s.quick_gear = true; }
        activate(a);
    }));
    ui.global::<ControllerState>().on_pad_number_clicked(|apply| super::defer(move |a| {
        if apply { pad_number_enter(a); } else {
            super::osk::detach(a, PAD_NUMBER_SINK);
            pad_number_cancel(a);
        }
    }));
    ui.global::<ControllerState>().on_summary_clicked(|i| super::defer(move |a| { st().summary_focus = i.max(0) as usize; sync(a); }));
    ui.global::<ControllerState>().on_browser_tab_clicked(|i| super::defer(move |a| {
        let mut s = st();
        if s.editor_busy { return; }
        s.browser_delete_pending = None;
        if let Some(browser) = s.browser.as_mut() {
            if let Some(tab) = browser.availability.tabs().get(i.max(0) as usize).copied() { browser.select_tab(tab); }
        }
        drop(s); sync(a);
    }));
    ui.global::<ControllerState>().on_browser_card_clicked(|i| super::defer(move |a| {
        let mut s = st();
        if s.editor_busy { return; }
        s.browser_delete_pending = None;
        let count = s.browser.as_ref().map(|b| visible_layouts(&s, b).len()).unwrap_or(0);
        if i >= 0 && (i as usize) < count { if let Some(b) = s.browser.as_mut() { b.focus = Focus::Card(i as usize); } }
        drop(s);
        if i >= 0 && (i as usize) < count { layout_card_action(a, LButton::A); }
    }));
    ui.global::<ControllerState>().on_browser_preview_action(|apply| super::defer(move |a| {
        handle(a, if apply { Action::X } else { Action::B });
    }));
    ui.global::<ControllerState>().on_browser_download_clicked(|| super::defer(download_browser_layout));
    ui.global::<ControllerState>().on_browser_search_clicked(|| super::defer(|a| {
        if st().editor_busy { return; }
        st().browser_delete_pending = None;
        if let Some(browser) = st().browser.as_mut() { browser.focus = Focus::SearchField; }
        open_browser_keyboard(a);
    }));
    ui.global::<ControllerState>().on_editor_section_clicked(|i| super::defer(move |a| {
        let i = (i.max(0) as usize).min(7);
        if let Some(f) = st().editor.as_mut() { f.section = (i.max(0) as usize).min(7); f.row = 0; f.column = Column::Rail; }
        sync(a);
    }));
    ui.global::<ControllerState>().on_editor_row_clicked(|i, gear| super::defer(move |a| {
        let mut s = st();
        if s.editor_busy || s.editor_set_prompt.is_some() { return; }
        let Some(section) = s.editor.as_ref().map(|f| f.section) else { return; };
        let Some(row) = focusable_of(&s, section).iter().position(|&index| index as i32 == i) else { return; };
        if let Some(f) = s.editor.as_mut() { f.row = row; f.column = if gear { Column::Gear } else { Column::Value }; }
        drop(s);
        editor_activate(a);
    }));
    ui.global::<ControllerState>().on_setting_section_clicked(|i| super::defer(move |a| {
        if st().editor_busy {return;}
        if let Some(f) = st().stick_settings.as_mut() { f.section = (i.max(0) as usize).min(4); f.row = 0; f.content = false; } sync(a);
    }));
    ui.global::<ControllerState>().on_setting_row_clicked(|i| super::defer(move |a| {
        if st().editor_busy {return;}
        if let Some(f) = st().stick_settings.as_mut() { f.row = i.max(0) as usize; f.content = true; }
        step_stick(a,1);
    }));
    ui.global::<ControllerState>().on_picker_tab_clicked(|i| super::defer(move |a| { {let mut s=st();s.picker_tab=*picker_tabs(&s).get(i.max(0) as usize).unwrap_or(&0);} sync(a); }));
    ui.global::<ControllerState>().on_picker_key_clicked(|i| super::defer(move |a| {
        let mut s = st(); let tab = s.picker_tab; s.picker_cursors[tab] = (i.max(0) as usize).min(picker_keys(&s).len().saturating_sub(1)); drop(s);
        apply_picker(a);
    }));
    ui.global::<ControllerState>().on_command_dialog_select(|i|super::defer(move|a| {if let Some(d)=st().command_dialog.as_mut(){d.cursor=i.max(0) as usize;}command_dialog_handle(a,Action::A);}));
    ui.global::<ControllerState>().on_command_dialog_step(|i,dir|super::defer(move|a| {if let Some(d)=st().command_dialog.as_mut(){if let Some(p)=d.parameters.get_mut(i.max(0) as usize){p.step(dir.signum() as i64);}}sync(a);}));
    ui.global::<ControllerState>().on_choice_selected(|i| super::defer(move |a| {
        // Временное значение `st()` не должно жить в теле `if let`: обработчики берут состояние заново.
        let quick_open = { let mut s = st(); if let Some(q) = s.quick_dialog.as_mut() { q.1 = i.max(0) as usize; true } else { false } };
        if quick_open { return quick_dialog_handle(a, Action::A); }
        let pad_open = { let mut s = st(); if let Some(p) = s.pad.as_mut() { p.choice = Some(i.max(0) as usize); true } else { false } };
        if pad_open { pad_handle(a, Action::A); }
    }));
    ui.global::<ControllerState>().on_pad_row_clicked(|i| super::defer(move |a| {
        let ready = { let mut s = st(); match s.pad.as_mut() { Some(p) if p.choice.is_none() => { if p.focus != i.max(0) as usize { p.slider = false; } p.focus = i.max(0) as usize; p.rail = false; true } _ => false } };
        if ready { pad_handle(a, Action::A); }
    }));
    ui.global::<ControllerState>().on_pad_section_clicked(|i| super::defer(move |a| {
        let rows = pad_rows(&st());
        let section = pad::present_sections(&rows).get(i.max(0) as usize).copied();
        if let (Some(section), Some(p)) = (section, st().pad.as_mut()) {
            if let Some(row) = pad::first_row_of(&rows, section) { p.focus = row; p.rail = true; p.slider = false; p.scroll = pad::scroll_for(&rows, row, p.scroll, pad_view_height(p, &rows), pad_content(&rows)); }
        }
        sync(a);
    }));
    ui.global::<ControllerState>().on_editor_undo(|| super::defer(undo_editor));
    ui.global::<ControllerState>().on_back(|| super::defer(|a| handle(a, Action::B)));
}

// Steam creates a virtual Xbox 360 pad for the Deck itself. It must not be
// mistaken for an attached physical controller when deciding which tabs to show.
fn is_physical_x360(path: &std::path::Path, name: &str) -> bool {
    (!path.starts_with("/sys/devices/virtual") || name.trim().ends_with("(vpad)"))
        && (name.to_ascii_lowercase().contains("x-box 360") || name.to_ascii_lowercase().contains("xbox 360"))
}

fn x360_connected() -> bool {
    let Ok(entries) = std::fs::read_dir("/sys/class/input") else { return false };
    entries.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("event")).any(|e| {
        let device = e.path().join("device");
        let Ok(path) = device.canonicalize() else { return false };
        let Ok(name) = std::fs::read_to_string(device.join("name")) else { return false };
        is_physical_x360(&path, &name)
    })
}

/// Key of the game's local layout file: the app id, or the lower-cased name for a non-Steam shortcut.
fn app_key(a: &App, id: u32) -> String {
    let name = a.gidx.get(&id).map(|&gi| a.games()[gi].name.clone()).unwrap_or_default();
    let kind_shortcut = a.gidx.get(&id).map(|&gi| a.games()[gi].kind == steam_data::GameKind::Shortcut).unwrap_or(false);
    if kind_shortcut { name.to_lowercase().chars().filter(|c| !"<>:\"/\\|?*".contains(*c)).collect::<String>() } else { id.to_string() }
}

// SteamUI's controller types (sp.js module 43251), distinct from ISteamInput's API enum.
const DECK_CONTROLLER_TYPE: i32 = 4;
const X360_CONTROLLER_TYPE: i32 = 31;

#[derive(serde::Deserialize)]
struct ControllerDevice {
    index: i64,
    kind: i32,
}

#[derive(serde::Deserialize)]
struct ControllerDevices {
    active: Option<i64>,
    devices: Vec<ControllerDevice>,
}

impl ControllerDevices {
    fn for_tab(&self, tab: u8) -> Option<u32> {
        let kind = if tab == 0 { DECK_CONTROLLER_TYPE } else { X360_CONTROLLER_TYPE };
        let mut matches = self.devices.iter()
            .filter(|device| device.kind == kind)
            .filter_map(|device| u32::try_from(device.index).ok());
        let first = matches.next()?;
        matches.find(|index| Some(i64::from(*index)) == self.active).or(Some(first))
    }
}

fn selected_controller_index(bridge: &steam_bridge::Bridge, tab: u8) -> steam_bridge::Result<u32> {
    let value = bridge.eval_json(r#"({
        active: window.SteamUIStore?.MostRecentlyActiveControllerIndex ?? null,
        devices: (window.ControllerStore?.GetControllers() ?? []).map(controller => ({
            index: controller.nControllerIndex, kind: controller.eControllerType
        }))
    })"#)?;
    let devices: ControllerDevices = serde_json::from_value(value)
        .map_err(|err| steam_bridge::Error::Protocol(format!("Steam: список контроллеров: {err}")))?;
    devices.for_tab(tab)
        .ok_or_else(|| steam_bridge::Error::Protocol("Выбранный контроллер не найден".into()))
}

pub fn open(a: &mut App, id: u32) {
    if controller_gyro::active(Owner::Editor) || controller_gyro::active(Owner::Settings) { return; }
    if st().editor_busy {a.toast("Дождитесь завершения операции с раскладкой");return;}
    if st().editing.is_some() {stop_editor(a);return;}
    let name = if id == 413080 {
        crate::assets::t_or("#Settings_Controller_DesktopConfig", "Раскладка для режима рабочего стола")
    } else {
        a.gidx.get(&id).map(|&gi| a.games()[gi].name.clone()).unwrap_or_default()
    };
    let key = app_key(a, id);
    let connected = x360_connected();
    // The local controller_neptune file is a Deck layout, never an X360 layout.
    let layout = if !connected && id != 413080 { read_layout(&a.base.steam_root, a.base.active_user.unwrap_or(0), &key, &steam_assets::language()) } else { None };
    let old_index = {
        let mut s = st();
        let old_index = s.quick.take().map(|(index, _)| index);
        *s = St { open: true, app_id: id, name, rows: rows(layout.as_ref()), layout, foc: if connected { F_X360_TAB } else { F_VIEW }, browser_generation: s.browser_generation.wrapping_add(1), root_generation: s.root_generation.wrapping_add(1), tab: if connected { 1 } else { 0 }, x360_connected: connected, ..Default::default() };
        old_index
    };
    request_root_active(id);
    request_quick_after_stop(old_index);
    a.ui().global::<ControllerState>().set_vy(0.0);
    sync(a);
}

fn select_controller_tab(a: &mut App, tab: u8) {
    let (app_id, old_index) = {
        let mut s = st();
        if !s.x360_connected || s.tab == tab || s.editor_busy { return; }
        let old_index = s.quick.take().map(|(index, _)| index);
        s.tab = tab;
        s.quick_gear = false;
        s.layout = if tab == 0 && s.app_id != 413080 {
            read_layout(&a.base.steam_root, a.base.active_user.unwrap_or(0), &app_key(a, s.app_id), &steam_assets::language())
        } else { None };
        s.rows = rows(s.layout.as_ref());
        s.foc = controller_tab_focus(s.foc, tab, s.rows.len());
        s.root_active = None;
        s.root_controller_index = None;
        s.browser_entries = None;
        s.browser_generation = s.browser_generation.wrapping_add(1);
        (s.app_id, old_index)
    };
    a.ui().global::<ControllerState>().set_vy(0.);
    request_root_active(app_id);
    request_quick_after_stop(old_index);
    sync(a);
    ensure_visible(a);
}

fn controller_tab_focus(focus: i32, tab: u8, rows: usize) -> i32 {
    match focus {
        F_DECK_TAB | F_X360_TAB => i32::from(tab),
        row if row >= F_ROW0 && (row - F_ROW0) as usize >= rows => F_VIEW,
        _ => focus,
    }
}

// The active configuration is independent of the editor's temporary editing copy.
fn request_root_active(id: u32) {
    let Some(bridge) = crate::actions::bridge() else { return };
    let (generation, tab) = { let mut s = st(); s.root_generation = s.root_generation.wrapping_add(1); (s.root_generation, s.tab) };
    std::thread::spawn(move || {
        let result = selected_controller_index(&bridge, tab)
            .and_then(|idx| bridge.layout_active_with_author(id, idx).map(|active| (idx, active)));
        if let Err(e) = &result { eprintln!("[controller] active layout: {e}"); }
        super::defer(move |a| {
            let mut s = st();
            if !s.open || s.app_id != id || s.root_generation != generation { return; }
            let (index, active) = match result { Ok((index, active)) => (Some(index), active), Err(_) => (None, None) };
            s.root_controller_index = index;
            s.root_active = active;
            drop(s);
            sync(a);
        });
    });
}

// Re-evaluate hotplug while the page is open; the Steam virtual pad remains excluded.
pub fn refresh_connection(a: &mut App) {
    let (is_open, id, was_connected) = { let s = st(); (s.open && !s.editor_busy && s.editor.is_none(), s.app_id, s.x360_connected) };
    if is_open && x360_connected() != was_connected {
        open(a, id);
    }
}

pub fn close(a: &mut App) {
    if st().editor_busy {a.toast("Дождитесь завершения операции с раскладкой");return;}
    controller_gyro::close(a, Owner::Editor);
    if st().editing.is_some() {stop_editor(a);}
    super::osk::detach(a, ACTION_SET_SINK);
    super::osk::detach(a, PAD_NUMBER_SINK);
    super::osk::detach(a, EXPORT_SINK);
    let session = { let mut s = st(); s.open = false; s.export_prompt = None; s.menu_page = 0; s.quick_dialog = None; s.pad = None; s.pad_number = None; s.summary_open = false; s.quick.take().map(|q| q.0) };
    if let (Some(idx), Some(b)) = (session, crate::actions::bridge()) { stop_session(b, idx); }
    sync(a);
}

/// Close the root's own editing session (nothing is saved by this call).
fn stop_session(b: steam_bridge::Bridge, idx: u32) {
    std::thread::spawn(move || { if let Err(e) = b.eval_json_once(&format!("SteamClient.Input.StopEditingControllerConfiguration({idx})")) { eprintln!("[controller] Stop: {e}"); } });
}

pub fn hints() -> Vec<(&'static str, &'static str, i32, &'static str, Action)> {
    if controller_gyro::active(Owner::Editor) { return controller_gyro::hints(); }
    {
        let s = st();
        if s.help_page > 0 {
            let next = if s.help_page == help_pages(s.tab) { "#Generic_Close" } else { "#ControllerConfigurator_Interstitial_NextPage" };
            return vec![("a", next, 1, "Enter", Action::A), ("b", "#ActionButtonLabelBack", 1, "Esc", Action::B)];
        }
    }
    let (foc, browser_open, show_all, loading) = { let s = st(); (s.foc, s.browser_open, s.browser.as_ref().is_some_and(|b| b.show_all), s.browser_entries.is_none()) };
    if browser_open {
        if st().browser_preview.is_some() {
            let mut hints = Vec::new();
            if !fixture_catalog() { hints.push(("x", "Применить раскладку", 1, "X", Action::X)); }
            hints.push(("b", "#ActionButtonLabelBack", 1, "Esc", Action::B));
            return hints;
        }
        let mut hints = Vec::new();
        let s = st();
        if !loading && !s.editor_busy {
            if browser_download_entry(&s).is_some() && !fixture_catalog() {
                hints.push(("x", "#AppControllerConfiguration_Download", 1, "X", Action::X));
            } else {
                let token = if show_all { "#ControllerConfigurator_ChooseConfiguration_ShowCompatibleConfigs" }
                    else { "#ControllerConfigurator_ChooseConfiguration_ShowAllConfigs" };
                hints.push(("x", token, 1, "X", Action::X));
            }
            if s.browser.as_ref().is_some_and(|b| b.focus == Focus::SearchField) && !s.browser_query.is_empty() {
                hints.push(("y", "#Clear", 1, "F4", Action::Y));
            } else if let Some(entry) = focused_browser_entry(&s).filter(|_| !fixture_catalog()) {
                if let Some((_, voted_up)) = browser_vote(&s) {
                    let token = if voted_up { "#ControllerConfigurator_ChooseConfiguration_DownVote" }
                        else { "#ControllerConfigurator_ChooseConfiguration_UpVote" };
                    hints.push(("y", token, 1, "F4", Action::Y));
                } else if entry.sections & 8 != 0 && steam_bridge::controller_list::check_personal_url(&entry.url).is_ok() {
                    hints.push(("menu", "#ControllerConfigurator_ChooseConfiguration_Delete", 2, "F9", Action::Menu));
                } else if entry.url.starts_with("template://") {
                    hints.push(("y", "Создать копию", 1, "F4", Action::Y));
                }
            }
            if focused_browser_entry(&s).is_none_or(|entry| selectable(&s, entry) || fixture_catalog()) {
                hints.push(("a", "#ActionButtonLabelSelect", 1, "Enter", Action::A));
            }
        }
        hints.push(("b", "#ActionButtonLabelBack", 1, "Esc", Action::B));
        return hints;
    }
    if st().summary_open { return vec![("b", "#ActionButtonLabelBack", 1, "Esc", Action::B)]; }
    let pad_state = st().pad.clone();
    if let Some(p) = pad_state {
        if p.uncertain { return vec![("b", "#ActionButtonLabelBack", 1, "Esc", Action::B)]; }
        let rows = pad_rows(&st());
        let slider = pad_slider_focused(&p, &rows) && st().pad_number.is_none();
        let mut h = Vec::new();
        if slider && p.choice.is_none() {
            h.push(("y", if p.fine { "Обычный сдвиг" } else { "Плавный сдвиг" }, 1, "F4", Action::Y));
            h.push(("menu", "Изменить значение", 2, "F9", Action::Menu));
            h.push(("a", if p.slider { "Готово" } else { "Изменить положение ползунка" }, 1, "Enter", Action::A));
        } else {
            h.push(("a", "#ActionButtonLabelSelect", 1, "Enter", Action::A));
        }
        h.push(("b", "#ActionButtonLabelBack", 1, "Esc", Action::B));
        return h;
    }
    if st().editor.is_some() {
        // Stock editor: Y swaps left/right on pair pages, A selects, B goes back; «Отменить» only while there is something to undo.
        let (pair, undo, save, sets) = { let s = st(); let main = s.editor.as_ref().is_some_and(|f| !f.picker_open) && s.stick_settings.is_none(); (main && focused_swap_pair(&s).is_some(), !s.editor_undo.is_empty(), main && s.editor_dirty, main && s.editing.as_ref().and_then(|e| live::msg(&e.1, 8).ok()).is_some_and(|sets| sets.len() > 1)) };
        let mut h = Vec::new();
        if save { h.push(("menu", "Сохранить", 2, "F9", Action::Menu)); }
        if sets { h.push(("r1", "Следующий набор", 1, "R1", Action::RB)); }
        if pair { h.push(("y", "Поменять левую кнопку на правую", 1, "F4", Action::Y)); }
        if undo { h.push(("x", "Отменить", 1, "X", Action::X)); }
        if st().editor.as_ref().is_some_and(|f| !f.picker_open) && st().stick_settings.is_none() {
            h.push(("view", "Предпросмотр", 2, "F10", Action::View));
        }
        h.push(("a", "#ActionButtonLabelSelect", 1, "Enter", Action::A));
        h.push(("b", "#ActionButtonLabelBack", 1, "Esc", Action::B));
        return h;
    }
    let select = {
        let s = st();
        if s.quick_slider.is_some() { "Готово" }
        else if focused_quick_slider(&s).is_some() { "Изменить положение ползунка" }
        else if foc == F_CARD { "Загрузить раскладку" }
        else if foc == F_HELP { "Помощь" }
        else { "#ActionButtonLabelSelect" }
    };
    vec![("a", select, 1, "Enter", Action::A), ("b", "#ActionButtonLabelBack", 1, "Esc", Action::B)]
}

fn n_items(s: &St) -> i32 {
    F_ROW0 + s.rows.len() as i32
}

// Stock spatial order: the tabs sit above the layout card, help and the gear sit at its right edge,
// and the view/edit/gear row is below the card. Quick-settings rows form a vertical list below that row.
fn move_focus(f: i32, count: i32, act: Action) -> i32 {
    let last = count - 1;
    let row = if count > F_ROW0 { F_ROW0 } else { f };
    match (f, act) {
        (F_DECK_TAB, Action::Right) => F_X360_TAB,
        (F_X360_TAB, Action::Left) => F_DECK_TAB,
        (F_DECK_TAB | F_X360_TAB, Action::Down) => F_CARD,
        (F_CARD, Action::Up) => F_X360_TAB,
        (F_CARD, Action::Right) => F_HELP,
        (F_CARD, Action::Down) => F_VIEW,
        (F_HELP, Action::Left | Action::Down) => F_CARD,
        (F_VIEW, Action::Up) | (F_EDIT, Action::Up) | (F_GEAR, Action::Up) => F_CARD,
        (F_VIEW, Action::Right) => F_EDIT,
        (F_VIEW, Action::Down) | (F_EDIT, Action::Down) | (F_GEAR, Action::Down) => row,
        (F_EDIT, Action::Left) => F_VIEW,
        (F_EDIT, Action::Right) => F_GEAR,
        (F_GEAR, Action::Left) => F_EDIT,
        (r, Action::Up) if r == F_ROW0 => F_VIEW,
        (r, Action::Up) if r > F_ROW0 => r - 1,
        (r, Action::Down) if r >= F_ROW0 => (r + 1).min(last),
        _ => f,
    }
}

fn available_focus(focus: i32, x360_connected: bool) -> i32 {
    // Without a second controller the tabs are not drawn: the card is the topmost focus.
    if !x360_connected && (focus == F_X360_TAB || focus == F_DECK_TAB) { F_CARD } else { focus }
}

/// Device-px height of one quick-settings row (stock: toggle 69, dropdown 96, slider 98).
fn row_height(r: &Row) -> f32 {
    match r {
        Row::Slider(_) | Row::Live(quick::QRow { kind: quick::Kind::Slider(_), .. }) => 98.0,
        Row::Mode { .. } => 96.0,
        Row::Live(r) if r.kind == quick::Kind::Dropdown => 96.0,
        _ => 69.0,
    }
}

fn scroll_to_row(scroll: f32, y: f32, height: f32, visible: f32, content: f32) -> f32 {
    let next = if y < scroll { y } else if y + height > scroll + visible { y + height - visible } else { scroll };
    next.clamp(0.0, (content - visible).max(0.0))
}

fn ensure_visible(a: &App) {
    let s = st();
    if s.foc < F_ROW0 { return; }
    let index = (s.foc - F_ROW0) as usize;
    let Some(row) = s.rows.get(index) else { return };
    let y: f32 = s.rows.iter().take(index).map(|r| row_height(r) + ROW_GAP).sum();
    let content: f32 = s.rows.iter().map(|r| row_height(r) + ROW_GAP).sum::<f32>() - ROW_GAP;
    let ui = a.ui();
    let g = ui.global::<ControllerState>();
    let scroll = -g.get_vy();
    g.set_vy(-scroll_to_row(scroll, y, row_height(row), root_geo(&s).list_h, content));
}

fn activate(a: &mut App) {
    if st().editor_busy { return; }
    let foc = { st().foc };
    match foc {
        F_DECK_TAB | F_X360_TAB => return select_controller_tab(a, foc as u8),
        F_HELP => st().help_page = 1,
        F_CARD => {
            // Tabs stay unknown until the list arrives; no Official tab is assumed.
            let mut s = st();
            s.browser_open = true;
            s.browser = Some(Browser::new(Availability::default()));
            drop(s);
            request_browser_list(a, true);
        },
        F_VIEW => open_summary(),
        F_EDIT => { start_editor(a); },
        F_GEAR => { let mut s = st(); s.menu_page = 1; s.menu_focus = 0; },
        f => {
            let row = st().rows.get((f - F_ROW0) as usize).cloned();
            match row {
                Some(Row::Live(r)) => return quick_activate(a, &r),
                _ => a.toast(&t_or("#DeckShell_ControllerReadOnly", "Только просмотр: изменение раскладки пока недоступно")),
            }
        }
    }
    sync(a);
}

// The captured root menu has a disabled revert row. Keep the cursor when a dialog closes.
fn menu_move(focus: usize, down: bool) -> usize {
    let enabled = [0, 1, 2, 4, 5];
    let p = enabled.iter().position(|&v| v == focus).unwrap_or(0);
    enabled[if down { (p + 1).min(enabled.len() - 1) } else { p.saturating_sub(1) }]
}
fn menu_activate(a: &mut App) {
    let mut s = st();
    if s.editor_busy { return; }
    match s.menu_focus {
        2 => s.menu_page = 2,
        5 => s.menu_page = 0,
        0 => { drop(s); return open_export(a); }
        1 => { s.menu_page = 3; s.menu_error = "Публикация не поддерживается. Раскладка не отправлена в сообщество.".into(); },
        4 => {
            let id = s.app_id;
            let source = s.root_active.as_ref().zip(s.root_controller_index).map(|(active, index)| (active.url.clone(), index));
            drop(s);
            let Some((url, index)) = source else { a.toast("Текущая раскладка не прочитана из Steam"); return; };
            if steam_bridge::controller_list::check_personal_url(&url).is_ok() { a.toast("Текущая раскладка уже личная"); return; }
            layout_write(a, id, Some(index), "Своя раскладка создана из текущей", LayoutWriteEffect::RefreshQuick, move |bridge, index| bridge.layout_create_from_current(id, index, Some(&url)));
            return;
        }
        _ => (),
    }
    let info = s.menu_page == 2;
    drop(s);
    if info { request_menu_info(a); }
    sync(a);
}

/// Stock suggestion for the «Сохранить как…» title: the current name for the user's own layout, empty for official, foreign or `#token` ones.
fn export_default_name(active: Option<&ActiveLayout>) -> String {
    active.filter(|a| a.url.starts_with("autosave://") && !a.title.starts_with('#') && a.title != "$$$autosave").map(|a| a.title.chars().take(steam_bridge::controller_list::EXPORT_TITLE_MAX).collect()).unwrap_or_default()
}

fn export_message(name: &str) -> String { format!("Сохранить как… (Личная, локально)\nНазвание: {name}_\nEnter — сохранить, B — отмена") }

const EXPORT_SINK: super::osk::ExtSink = super::osk::ExtSink {
    id: 14, mode: super::osk::InputMode::Normal, typed: type_export_name, enter: commit_export, cancel: cancel_export,
};

fn open_export(a: &mut App) {
    if fixture_catalog() || crate::actions::dryrun() { a.toast("Пробный запуск: запись раскладки не выполняется"); return; }
    let name = { let s = st(); if s.root_active.is_none() || s.root_controller_index.is_none() { drop(s); a.toast("Текущая раскладка не прочитана из Steam"); return; } export_default_name(s.root_active.as_ref()) };
    { let mut s = st(); s.menu_page = 3; s.menu_error = export_message(&name); s.export_prompt = Some(name); }
    super::osk::attach(a, EXPORT_SINK);
    sync(a);
}

fn type_export_name(a: &mut App, character: Option<char>) {
    let mut s = st();
    let Some(name) = s.export_prompt.as_mut() else { return };
    match character {
        Some(c) if !c.is_control() && name.chars().count() < steam_bridge::controller_list::EXPORT_TITLE_MAX => name.push(c),
        Some(_) => (),
        None => { name.pop(); }
    }
    s.menu_error = export_message(name);
    drop(s);
    sync(a);
}

fn cancel_export(a: &mut App) {
    { let mut s = st(); s.export_prompt = None; s.menu_page = 1; }
    super::osk::detach(a, EXPORT_SINK);
    sync(a);
}

fn commit_export(a: &mut App) {
    let (name, id, index) = { let s = st(); (s.export_prompt.clone().unwrap_or_default(), s.app_id, s.root_controller_index) };
    if steam_bridge::controller_list::check_export_text(&name, "").is_err() { a.toast("Введите название раскладки"); return; }
    { let mut s = st(); s.export_prompt = None; s.menu_page = 0; }
    super::osk::detach(a, EXPORT_SINK);
    let title = name.trim().to_owned();
    layout_write(a, id, index, format!("Раскладка сохранена как «{title}»"), LayoutWriteEffect::RefreshList, move |bridge, idx| bridge.layout_export_local(id, idx, &title, ""));
}

fn layout_info_link(app: u32, info: &ActiveLayout) -> Option<String> {
    if app == 0 || !info.url.starts_with("workshop://") { return None; }
    let id = info.published_file_id.filter(|&id| id != 0)?;
    Some(format!("steam://controllerconfig/{app}/{id}"))
}

fn request_menu_info(a: &mut App) {
    let (id, generation, tab) = {
        let mut s = st();
        s.menu_info = None;
        s.menu_description.clear();
        s.menu_author.clear();
        s.menu_link_copied = false;
        s.menu_generation = s.menu_generation.wrapping_add(1);
        (s.app_id, s.menu_generation, s.tab)
    };
    let Some(bridge) = crate::actions::bridge() else { st().menu_description = "Steam недоступен".into(); return; };
    let spawn = std::thread::Builder::new().name("controller-layout-info".into()).spawn(move || {
        let result = selected_controller_index(&bridge, tab)
            .map_err(|e| e.to_string())
            .and_then(|idx| {
                let active = bridge.layout_active_with_author(id, idx).map_err(|e| e.to_string())?.ok_or_else(|| "Активная раскладка не найдена".to_string())?;
                let desc = bridge.layout_list(id, idx, false).map_err(|e| eprintln!("[controller] layout_list: {e}")).ok().and_then(|items| items.into_iter().find(|entry| entry.url == active.url)).map(|entry| entry.description).unwrap_or_default();
                let details = active.published_file_id.filter(|id| *id > 1000)
                    .and_then(|id| bridge.layout_details(&[id]).map_err(|e| eprintln!("[controller] layout_details: {e}")).ok())
                    .and_then(|items| items.into_iter().find(|d| d.result == 1));
                let author = if active.url.starts_with("template://") {
                    t_or("#AppControllerConfiguration_TemplateConfig", "Шаблон")
                } else {
                    active.author_name.clone()
                };
                let desc = if desc.is_empty() { details.map(|d| d.description).unwrap_or_default() } else { desc };
                Ok((active, desc, author))
            });
        super::defer(move |a| {
            let mut s = st();
            if !s.open || s.app_id != id || s.menu_page != 2 || s.menu_generation != generation { return; }
            let link = match result {
                Ok((info, desc, author)) => {
                    let link = layout_info_link(id, &info);
                    s.menu_info = Some(info);
                    s.menu_description = desc;
                    s.menu_author = author;
                    link
                }
                Err(e) => { s.menu_description = e; None }
            };
            drop(s);
            sync(a);
            if let Some(link) = link.filter(|_| !crate::actions::dryrun()) {
                copy_layout_link(bridge, link, generation);
            }
        });
    });
    if spawn.is_err() { st().menu_description = "Не удалось запросить данные Steam".into(); }
    sync(a);
}

fn copy_layout_link(bridge: steam_bridge::Bridge, link: String, generation: u64) {
    std::thread::spawn(move || {
        let link = serde_json::to_string(&link).expect("serialize controller link");
        let expr = format!(r#"(() => {{
            const field = document.createElement("textarea");
            field.value = {link};
            field.style.position = "fixed";
            field.style.opacity = "0";
            document.body.appendChild(field);
            try {{
                field.select();
                return document.execCommand("copy");
            }} finally {{ field.remove(); }}
        }})()"#);
        let copied = bridge.eval_json_once(&expr).is_ok_and(|value| value == true);
        super::defer(move |a| {
            let mut s = st();
            if s.menu_page == 2 && s.menu_generation == generation {
                s.menu_link_copied = copied;
                drop(s);
                sync(a);
            }
        });
    });
}

/// `first`: the chooser was just opened (initial-tab rule applies); otherwise a filter toggle keeps the user's tab.
fn request_browser_list(a: &mut App, first: bool) {
    let (id, tab, generation, show_all) = {
        let mut s = st();
        s.browser_generation = s.browser_generation.wrapping_add(1);
        s.browser_entries = None;
        s.browser_active = None;
        s.browser_connected = false;
        s.browser_details.clear();
        s.browser_votes.clear();
        s.browser_delete_pending = None;
        s.browser_controller_index = None;
        (s.app_id, s.tab, s.browser_generation, s.browser.as_ref().is_some_and(|b| b.show_all))
    };
    if fixture_catalog() {
        let mut s = st();
        let result = steam_bridge::controller_list::offline_catalog().map_err(|err| err.to_string());
        if let (Ok(entries), Some(browser)) = (&result, s.browser.as_mut()) {
            update_browser_tabs(browser, entries, id, first);
        }
        s.browser_controller_type = 4;
        s.browser_connected = true; // Demo catalog follows the online view, with writes disabled.
        s.browser_entries = Some(result);
        drop(s);
        sync(a);
        return;
    }
    let Some(bridge) = crate::actions::bridge() else {
        st().browser_entries = Some(Err("Steam недоступен".into()));
        sync(a);
        return;
    };
    let spawned = std::thread::Builder::new().name("controller-layout-list".into()).spawn(move || {
        // The Steam store filter is global and must not overlap another chooser request.
        static LIST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = LIST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Use the controller selected in DeckShell even if the other pad moved focus.
        let result = selected_controller_index(&bridge, tab).map_err(|e| e.to_string()).and_then(|index| {
            let controller_type = bridge.eval_json(&format!("window.ControllerStore.GetController({index})?.eControllerType ?? null"))
                .ok().and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let catalog = bridge.layout_catalog(id, index, show_all).map_err(|e| e.to_string())?;
            let active = bridge.layout_active(id, index).ok().flatten();
            Ok((catalog, active, controller_type, index))
        });
        let mut seen = std::collections::HashSet::new();
        let vote_ids = result.as_ref().ok().filter(|(catalog, ..)| catalog.connected).map(|(catalog, ..)| {
            catalog.entries.iter()
                .filter(|entry| entry.sections & (4 | 16) != 0 && entry.url.starts_with("workshop://"))
                .filter_map(|entry| entry.published_file_id)
                .filter(|id| seen.insert(*id))
                .collect::<Vec<_>>()
        }).unwrap_or_default();
        super::defer(move |a| {
            let mut s = st();
            if !s.browser_open || s.app_id != id || s.browser_generation != generation { return; }
            s.browser_entries = Some(match result {
                Ok((catalog, active, controller_type, index)) => {
                    if let Some(browser) = s.browser.as_mut() {
                        update_browser_tabs(browser, &catalog.entries, id, first);
                    }
                    s.browser_connected = catalog.connected;
                    s.browser_controller_type = controller_type;
                    s.browser_controller_index = Some(index);
                    s.browser_active = active;
                    Ok(catalog.entries)
                }
                Err(error) => Err(error),
            });
            clamp_browser_focus(&mut s);
            let loaded = matches!(s.browser_entries, Some(Ok(_)));
            drop(s);
            sync(a);
            if loaded { watch_browser_connection(generation); }
        });
        for ids in vote_ids.chunks(50) {
            let details = bridge.layout_details(ids).unwrap_or_default();
            let votes = bridge.layout_user_votes(ids).ok().map(|summaries| {
                // An omitted summary in a successful reply has no positive vote.
                let mut votes: HashMap<_, _> = ids.iter().map(|id| (*id, false)).collect();
                votes.extend(summaries.into_iter().map(|vote| (vote.published_file_id, vote.vote == Some(true))));
                votes
            });
            super::defer(move |a| {
                let mut s = st();
                if !s.browser_open || s.app_id != id || s.browser_generation != generation { return; }
                s.browser_details.extend(details.into_iter().filter(|d| d.result == 1).map(|d| (d.published_file_id, d)));
                if let Some(votes) = votes { s.browser_votes.extend(votes); }
                drop(s);
                sync(a);
            });
        }
    });
    if spawned.is_err() { st().browser_entries = Some(Err("Не удалось запустить загрузку".into())); }
    sync(a);
}

fn watch_browser_connection(generation: u64) {
    slint::Timer::single_shot(Duration::from_secs(3), move || {
        let s = st();
        if !s.browser_open || !s.open || s.browser_generation != generation { return; }
        if s.editor_busy || s.browser_preview.is_some() {
            drop(s);
            watch_browser_connection(generation);
            return;
        }
        drop(s);
        let Some(bridge) = crate::actions::bridge() else {
            watch_browser_connection(generation);
            return;
        };
        std::thread::spawn(move || {
            let connected = bridge.eval_json("window.workshopStore.BConnectedToSteam()")
                .ok().and_then(|value| value.as_bool());
            super::defer(move |a| {
                let mut s = st();
                if !s.browser_open || !s.open || s.browser_generation != generation { return; }
                if let Some(connected) = connected.filter(|_| !s.editor_busy && s.browser_preview.is_none()) {
                    if connected && !s.browser_connected {
                        drop(s);
                        request_browser_list(a, false);
                        return;
                    }
                    if !connected && s.browser_connected {
                        s.browser_connected = false;
                        clamp_browser_focus(&mut s);
                        drop(s);
                        sync(a);
                    }
                }
                watch_browser_connection(generation);
            });
        });
    });
}

fn fixture_catalog() -> bool { std::env::var("DECKSHELL_CONTROLLER_FIXTURES").as_deref() == Ok("1") }

fn update_browser_tabs(browser: &mut Browser, entries: &[LayoutEntry], app_id: u32, first: bool) {
    let count = |section| entries.iter().filter(|entry| entry.sections & section != 0).count();
    let availability = Availability {
        official: count(1),
        recommended: count(32),
        personal: count(8),
        community_allowed: community_allowed(app_id),
    };
    if first { browser.finish_load(availability); } else { browser.refresh_availability(availability); }
}

/// A / Y / Menu on a focused chooser card (task 505): the decision is `controller_layout_actions::decide`; writes go through the
/// bridge on a worker thread and never run under DECKSHELL_DRYRUN. The list is reloaded afterwards, so the card marks follow Steam.
fn layout_card_action(a: &mut App, button: LButton) {
    if fixture_catalog() && !matches!(button, LButton::A) { a.toast("Демонстрационный каталог: изменения отключены"); return; }
    if st().editor_busy { return; }
    if button == LButton::A && !fixture_catalog() {
        let pending = focused_browser_entry(&st()).filter(|entry| entry.published_file_id.is_some() && !entry.downloaded).cloned();
        if let Some(entry) = pending {
            if downloadable(&st(), &entry) { download_focused_layout(a, true); }
            return;
        }
    }
    let plan = {
        let mut s = st();
        let Some(browser) = s.browser.as_ref() else { return };
        let Focus::Card(i) = browser.focus else { return };
        let tab = browser.active;
        let entry = if button == LButton::X { s.browser_preview.take() } else { visible_layouts(&s, browser).get(i).map(|e| (*e).clone()) };
        let Some(e) = entry else { return; };
        let active = s.browser_active.clone();
        let card = lact::Card {
            url: e.url.clone(), personal: e.sections & 8 != 0 && steam_bridge::controller_list::check_personal_url(&e.url).is_ok(), template: e.url.starts_with("template://"), community: e.sections & 4 != 0,
            active: steam_bridge::controller_list::is_active(&e, active.as_ref()),
            selection_type: active.as_ref().map(|l| l.selection_type).unwrap_or(0),
        };
        let plan = lact::decide(tab, &card, button, s.browser_delete_pending.as_deref());
        s.browser_delete_pending = if let Plan::ConfirmDelete { url } = &plan { Some(url.clone()) } else { None };
        if plan == Plan::Preview { s.browser_preview = Some(e); }
        plan
    };
    if plan == Plan::Preview { sync(a); return; }
    let (id, Some(index)) = ({ let s = st(); (s.app_id, s.browser_controller_index) }) else { return; };
    match plan {
        Plan::Preview => unreachable!(),
        Plan::Nothing => {}
        Plan::Info(m) => a.toast(m),
        Plan::ConfirmDelete { .. } => a.toast("Удалить свою раскладку? Нажмите кнопку меню ещё раз"),
        Plan::Apply { url, selection_type } => layout_write(a, id, Some(index), "Раскладка применена", LayoutWriteEffect::RefreshQuick, move |b, idx| b.layout_select(id, idx, &url, selection_type)),
        Plan::CreateFromTemplate { url } => layout_write(a, id, Some(index), "Своя раскладка создана из шаблона", LayoutWriteEffect::RefreshQuick, move |b, idx| b.layout_create_from_template(id, idx, &url)),
        Plan::Delete { url, selected_type } => layout_write(a, id, Some(index), "Раскладка удалена", LayoutWriteEffect::RefreshQuick, move |b, idx| b.layout_delete_personal(id, idx, &url, selected_type)),
    }
    sync(a);
}

fn focused_browser_entry(s: &St) -> Option<&LayoutEntry> {
    let browser = s.browser.as_ref()?;
    let Focus::Card(index) = browser.focus else { return None };
    visible_layouts(s, browser).get(index).copied()
}

fn clamp_browser_focus(s: &mut St) {
    let Some(browser) = &s.browser else { return; };
    let Focus::Card(index) = browser.focus else { return; };
    let count = visible_layouts(s, browser).len();
    let focus = match count {
        0 if browser.active == Tab::Search => Focus::SearchField,
        0 => Focus::Tab,
        _ => Focus::Card(index.min(count - 1)),
    };
    s.browser.as_mut().unwrap().focus = focus;
}

fn downloadable(s: &St, entry: &LayoutEntry) -> bool {
    s.browser_connected && entry.published_file_id.is_some() && !entry.downloaded && !entry.downloading
}

fn selectable(s: &St, entry: &LayoutEntry) -> bool {
    entry.published_file_id.is_none() || entry.downloaded || downloadable(s, entry)
}

fn browser_download_entry(s: &St) -> Option<&LayoutEntry> {
    focused_browser_entry(s).filter(|entry| downloadable(s, entry))
}

fn browser_vote(s: &St) -> Option<(u64, bool)> {
    let browser = s.browser.as_ref()?;
    if !s.browser_connected || !matches!(browser.active, Tab::Community | Tab::Search) { return None; }
    let entry = focused_browser_entry(s)?;
    if entry.owned || !entry.url.starts_with("workshop://") { return None; }
    let id = entry.published_file_id?;
    if !s.browser_details.contains_key(&id) { return None; }
    s.browser_votes.get(&id).map(|&up| (id, up))
}

fn vote_browser_layout(a: &mut App, published_id: u64, voted_up: bool) {
    let (app, index) = { let s = st(); (s.app_id, s.browser_controller_index) };
    let done = if voted_up { "Голос убран" } else { "Голос учтён" };
    layout_write(a, app, index, done, LayoutWriteEffect::RefreshList, move |bridge, _| bridge.layout_vote(published_id, !voted_up));
}

fn download_focused_layout(a: &mut App, preview: bool) {
    let request = {
        let s = st();
        browser_download_entry(&s).map(|entry| (s.app_id, s.browser_controller_index, entry.clone()))
    };
    let Some((app, index, entry)) = request else { return; };
    let published_id = entry.published_file_id.unwrap();
    let effect = if preview { LayoutWriteEffect::Preview(entry) } else { LayoutWriteEffect::RefreshList };
    layout_write(a, app, index, "Раскладка загружена", effect, move |bridge, _| bridge.layout_download(published_id));
}

fn download_browser_layout(a: &mut App) {
    if st().editor_busy || st().browser_preview.is_some() { return; }
    if browser_download_entry(&st()).is_none() {
        let mut s = st();
        let Some(browser) = s.browser.as_ref() else { return; };
        if matches!(browser.focus, Focus::Card(_)) { return; }
        let index = visible_layouts(&s, browser).iter().position(|entry| downloadable(&s, entry));
        let Some(index) = index else { return; };
        s.browser.as_mut().unwrap().focus = Focus::Card(index);
    }
    download_focused_layout(a, false);
}

enum LayoutWriteEffect {
    RefreshQuick,
    RefreshList,
    Preview(LayoutEntry),
}

fn layout_write(a: &mut App, id: u32, index: Option<u32>, done: impl Into<String>, effect: LayoutWriteEffect, call: impl FnOnce(&steam_bridge::Bridge, u32) -> steam_bridge::Result<()> + Send + 'static) {
    let done: String = done.into();
    if st().editor_busy { return; }
    if fixture_catalog() { a.toast("Демонстрационный каталог: изменения отключены"); return; }
    if crate::actions::dryrun() { eprintln!("[deckshell] DRYRUN controller layout write app {id}: {done}"); a.toast("Пробный запуск: запись раскладки не выполняется"); return; }
    let Some(bridge) = crate::actions::bridge() else { a.toast("Steam недоступен"); return };
    let tab = {
        let mut s = st();
        s.editor_busy = true;
        s.tab
    };
    a.toast("Steam обрабатывает раскладку…");
    let generation = st().browser_generation;
    let spawned = std::thread::Builder::new().name("controller-layout-write".into()).spawn(move || {
        let result = selected_controller_index(&bridge, tab).map_err(|e| e.to_string())
            .and_then(|idx| if index.is_none_or(|expected| expected == idx) { call(&bridge, idx).map_err(|e| e.to_string()) } else { Err("Активный контроллер изменился".into()) });
        super::defer(move |a| {
            if st().app_id != id { return; }
            st().editor_busy = false;
            let layout_changed = result.is_ok() && matches!(effect, LayoutWriteEffect::RefreshQuick);
            if let LayoutWriteEffect::Preview(mut entry) = effect {
                if result.is_ok() {
                    let mut s = st();
                    if s.browser_open && s.browser_generation == generation {
                        entry.downloaded = true;
                        entry.downloading = false;
                        if let Some(Ok(entries)) = s.browser_entries.as_mut() {
                            for listed in entries.iter_mut().filter(|listed| listed.published_file_id == entry.published_file_id) {
                                listed.downloaded = true;
                                listed.downloading = false;
                            }
                        }
                        s.browser_preview = Some(entry);
                        drop(s);
                        sync(a);
                        return;
                    }
                }
            }
            a.toast(&match result { Ok(()) => done, Err(e) => format!("Не удалось: {e}") });
            if layout_changed {
                let mut s = st();
                s.quick = None;
                s.rows = rows(s.layout.as_ref());
                drop(s);
                request_quick_after_stop(None);
            }
            if !layout_changed { request_root_active(id); }
            if st().browser_open { request_browser_list(a, false); } else { sync(a); }
        });
    });
    if spawned.is_err() { st().editor_busy = false; a.toast("Не удалось запустить запись"); }
    sync(a);
}

// Steam uses Motiva for Latin and a Cyrillic fallback. Slint's generic fallback
// differs, so choose the existing locale face for strings containing Cyrillic.
fn chooser_font(text: &str) -> slint::SharedString {
    if text.chars().any(|c| ('\u{0400}'..='\u{052f}').contains(&c)) { "DejaVu Sans" } else { "Motiva Sans" }.into()
}

pub(super) fn controller_icon_name(controller_type: u32) -> &'static str {
    match controller_type {
        2 => "ControllerType.Steam",
        4 => "ControllerType.Deck",
        10 => "ControllerType.Triton",
        21 => "ControllerType.Frame",
        31 | 32 | 47 => "ControllerType.Xbox",
        34 | 46 => "ControllerType.PS4",
        38..=41 | 44 | 51 => "ControllerType.Switch",
        43 => "ControllerType.Mobile",
        45 | 48 => "ControllerType.PS5",
        49 => "ControllerType.Hori",
        _ => "ControllerType.Generic",
    }
}

fn browser_icon(name: &str, focused: bool) -> slint::Image {
    crate::assets::tint_image(crate::assets::controller_icon(name).unwrap_or_default(), if focused { slint::Color::from_rgb_u8(35,38,46) } else { slint::Color::from_rgb_u8(139,146,154) })
}

fn layout_title(title: &str) -> String {
    if title == "$$$autosave" { t_or("#SettingsController_AutosaveName", "Личное автосохранение") } else { title.to_owned() }
}

fn visible_layouts<'a>(s: &'a St, browser: &Browser) -> Vec<&'a LayoutEntry> {
    let Some(Ok(entries)) = &s.browser_entries else { return Vec::new() };
    let mut result: Vec<_> = entries.iter().filter(|entry| match browser.active {
        Tab::Recommended => entry.sections & (if browser.availability.official > 0 { 1 } else { 32 }) != 0,
        Tab::Templates => entry.sections & 2 != 0,
        Tab::Community => entry.sections & 4 != 0 && (s.browser_connected || entry.downloaded),
        Tab::User => entry.sections & 8 != 0,
        Tab::Search => entry.sections & 16 != 0 && super::controller_browser::matches_search(&super::controller_browser::SearchEntry {
            title: entry.title.clone(), description: entry.description.clone(), game_name: entry.game_name.clone(), app_id: entry.app_id, uses_siapi: entry.uses_siapi,
        }, s.app_id, &s.browser_query),
    }).collect();
    if browser.active == Tab::Community { super::controller_browser::sort_community_by_playtime(&mut result, |entry| entry.playtime_seconds); }
    if browser.active == Tab::User { result.sort_by_key(|entry| entry.personal_order); }
    if browser.active == Tab::Search { result.sort_by_key(|entry| entry.search_order); }
    result
}

pub fn browser_keyboard_target() -> bool {
    let s = st();
    s.open && s.browser_open && s.browser.as_ref().is_some_and(|b| b.active == Tab::Search && b.focus == Focus::SearchField)
}
pub fn browser_type(a: &mut App, c: Option<char>) {
    { let mut s = st(); if let Some(c) = c { if !c.is_control() { s.browser_query.push(c); } } else { s.browser_query.pop(); } }
    sync(a);
}
fn open_browser_keyboard(a: &mut App) {
    if let Some(b) = st().browser.as_mut() { b.keyboard_open = true; }
    crate::screens::osk::attach(a, BROWSER_SINK);
    sync(a);
}
/// Поиск раскладок: единый приёмник клавиатуры (Поиск/Enter и закрытие оставляют фокус в поле).
const BROWSER_SINK: crate::screens::osk::ExtSink = crate::screens::osk::ExtSink { id: 3, mode: crate::screens::osk::InputMode::Search, typed: browser_type, enter: browser_keyboard_closed, cancel: browser_keyboard_closed };
pub fn browser_keyboard_closed(a: &mut App) {
    if let Some(b) = st().browser.as_mut() { b.keyboard_open = false; }
    sync(a);
}
fn leave_browser(s: &mut St) { s.browser_open = false; }

// ---- Editor pages (stock `editor-*` captures): rows come from controller_pages, the focus walks only focusable rows. ----
fn page_items_of(s: &St, section: usize) -> Vec<pages::Item> {
    s.editing.as_ref().map(|e| pages::items_for_set(&e.1, &e.2, section, s.editor_set_index)).unwrap_or_default()
}
fn focusable_of(s: &St, section: usize) -> Vec<usize> {
    page_items_of(s, section).iter().enumerate().filter(|(_, i)| i.kind != pages::Kind::Header).map(|(n, _)| n).collect()
}
fn controls_of(it: &pages::Item) -> RowControls {
    match it.kind { pages::Kind::SetRow => RowControls::GearOnly, _ if it.gear => RowControls::ValueAndGear, _ => RowControls::Value }
}
/// Slint rows of an editor page, the (y, h) of every row and the content height.
pub fn page_model(raw: &[u8], bindings: &[live::BindingRow], section: usize) -> (Vec<ControllerEditorRow>, Vec<(f32, f32)>, f32) {
    model_items(pages::items(raw, bindings, section), bindings)
}

fn model_items(items: Vec<pages::Item>, bindings: &[live::BindingRow]) -> (Vec<ControllerEditorRow>, Vec<(f32, f32)>, f32) {
    let (placed, content) = pages::place(&items);
    let glyph = |name: &str| crate::assets::controller_glyph(name, "knockout.md").unwrap_or_default();
    let rows = items.iter().zip(&placed).map(|(it, p)| {
        let header = it.kind == pages::Kind::Header;
        ControllerEditorRow {
            label: p.label.clone().into(), value: p.value.clone().into(), y: p.y, h: p.h, header, bare: it.kind == pages::Kind::Button && (it.label.is_empty() || it.x.is_some()),
            dropdown: matches!(it.kind, pages::Kind::Function | pages::Kind::Param), dim: it.dim, gear: it.gear, gear_only: it.kind == pages::Kind::SetRow,
            glyph: glyph(it.glyph), value_width: it.width, value_x: it.x.map_or(0., |x| x.max(0.1)),
            command_glyph: it.binding.and_then(|b| bindings.get(b)).map(|r| editor::command_glyph(&r.binding, false)).unwrap_or_default(),
            ..Default::default()
        }
    }).collect();
    (rows, placed.iter().map(|p| (p.y, p.h)).collect(), content)
}
/// Page rows straight from a raw native layout (gallery / tests).
pub fn page_model_raw(raw: &[u8], section: usize) -> (Vec<ControllerEditorRow>, Vec<(f32, f32)>, f32) {
    let bindings = live::rows(raw).unwrap_or_default();
    page_model(raw, &bindings, section)
}
pub fn section_title(section: usize) -> &'static str { pages::SECTION_TITLES[section.min(7)] }
fn editor_model(s: &St, section: usize) -> (Vec<ControllerEditorRow>, Vec<(f32, f32)>, f32) {
    match &s.editing { Some(e) => model_items(page_items_of(s, section), &e.2), None => (Vec::new(), Vec::new(), 0.) }
}

fn editor_activate(a: &mut App) {
    let mut s = st();
    if s.editor_busy || controller_gyro::active(Owner::Editor) { return; }
    let Some((section, row, column)) = s.editor.as_ref().map(|f| (f.section, f.row, f.column)) else { return };
    let items = page_items_of(&s, section);
    let item = focusable_of(&s, section).get(row).and_then(|&i| items.get(i)).cloned();
    if column == Column::Rail {
        if let (Some(it), Some(f)) = (item.as_ref(), s.editor.as_mut()) { f.column = if controls_of(it) == RowControls::GearOnly { Column::Gear } else { Column::Value }; }
        else { drop(s); a.toast("В этом разделе пока нет доступных строк"); return; }
        drop(s); sync(a); return;
    }
    let Some(it) = item else { return };
    if it.id == pages::Id::Calibrate {
        drop(s);
        return start_gyro_calibration(a);
    }
    if it.id == pages::Id::AddVirtualMenu {
        drop(s);
        return open_virtual_menu_name(a);
    }
    if matches!(it.id, pages::Id::AddActionSet | pages::Id::ActionSet) {
        let rename = (it.id == pages::Id::ActionSet).then_some(row);
        drop(s);
        open_action_set_name(a, rename);
        return;
    }
    if it.kind == pages::Kind::Function && column == Column::Gear {
        let stick_uses_legacy_settings = it.stick.zip(s.editing.as_ref()).is_some_and(|(i, e)| {
            sticks::sticks(&e.1).is_ok_and(|sticks| sticks.get(i).is_some_and(|stick| stick.mode == 6))
        });
        if let Some(source) = it.source.filter(|_| !stick_uses_legacy_settings) {
            let set = s.editor_set_index;
            drop(s);
            return pad_open(a, source, Some(set), false);
        }
    }
    if it.kind == pages::Kind::Function && column == Column::Value {
        if let Some(source) = it.source.filter(|&source| !quick::mode_choices(source).is_empty()) {
            if s.editing.is_none() { drop(s); a.toast("Раскладка ещё читается"); return; }
            let current = s.editing.as_ref().map_or(0, |e| quick::mode_in_set(&e.1, s.editor_set_index, source));
            let cursor = quick::list_modes(source, false, current).iter().position(|m| *m == current).unwrap_or(0);
            s.quick_dialog = Some((source, cursor));
            s.mode_list_editor = true;
            drop(s);
            sync(a);
            return;
        }
    }
    let message: Option<&str> = match (it.kind, it.stick) {
        (pages::Kind::Input, _) if column == Column::Gear && it.binding.is_some() => {
            s.bind_name = it.name.clone(); s.menu_page = 4; s.menu_focus = 0;
            s.bind_target = it.binding.and_then(|b| s.editing.as_ref().and_then(|e| e.2.get(b)).map(|r| r.target.clone()));
            None
        }
        (pages::Kind::Function, Some(index)) => {
            if s.editing.is_none() { Some("Раскладка ещё читается") }
            else {
                let mode = s.editing.as_ref().and_then(|e| sticks::sticks(&e.1).ok()).and_then(|v| v.get(index).map(|x| x.mode));
                if mode == Some(6) { s.stick_index = index; let mut focus = editor::StickFocus::default(); focus.content = true; s.stick_settings = Some(focus); None }
                else { s.editor_status = "Параметры этого режима стика пока недоступны".into(); None }
            }
        }
        (pages::Kind::Input, _) if column == Column::Value && it.binding.is_some() => { if let Some(f) = s.editor.as_mut() { f.picker_open = true; } s.picker_tab = 0; None }
        (pages::Kind::Input, _) if column == Column::Value && it.binding.is_none() => {
            // «Добавить команду»: the picker writes a new command (append) after the same readback/rollback as a replacement.
            match empty_target_of(&s, &it) {
                Ok(_) => { if let Some(f) = s.editor.as_mut() { f.picker_open = true; } s.picker_tab = 0; None }
                Err(_) => Some("Для этого входа нет режима источника в раскладке"),
            }
        }
        (pages::Kind::Function, _) => Some("Для этого источника список режимов не найден в раскладке"),
        (pages::Kind::Param, _) => Some("Эта настройка пока недоступна: запись параметров активатора не подтверждена"),
        (pages::Kind::Button, _) | (pages::Kind::SetRow, _) => Some("Это действие пока недоступно: вызов Steam не подтверждён"),
        _ => Some("Настройка пока недоступна"),
    };
    drop(s);
    if let Some(m) = message { a.toast(m); }
    sync(a);
}

fn close_binding_menu_layer(s: &mut St) {
    (s.menu_page, s.menu_focus) = match s.menu_page {
        7 if s.bind_activation_parent == 4 => (4, 0),
        7 => (5, 4),
        6 => (4, 6),
        5 => (4, 1),
        _ => (0, 0),
    };
}

/// Command options, activator settings, interval and activation type.
fn binding_menu_handle(a: &mut App, act: Action) {
    let mut s = st();
    let page = s.menu_page;
    let last = match page { 4 => 7, 5 => 12, 7 => s.bind_activations.len(), _ => 0 };
    let mut toast = false;
    let mut write: Option<(steam_bridge::controller_edit::BindingTarget, i64)> = None;
    match act {
        Action::B => close_binding_menu_layer(&mut s),
        Action::Up => s.menu_focus = s.menu_focus.saturating_sub(1),
        Action::Down => s.menu_focus = (s.menu_focus + 1).min(last),
        Action::A => match (page, s.menu_focus) {
            (4, 1) => { s.menu_page = 5; s.menu_focus = 0; }
            (4, 6) => { s.menu_page = 6; s.menu_focus = 0; }
            (4, 7) | (5, 12) => close_binding_menu_layer(&mut s),
            (4, 0) | (5, 4) => match s.bind_target.clone() {
                Some(target) => {
                    let analog = bind::pressure_activator(&target, s.tab == 0);
                    s.bind_activations = bind::activation_options(analog, false).into_iter().map(|o| o.0).collect();
                    let current = s.editing.as_ref().and_then(|e| live::activation_at(&e.1, &target));
                    s.menu_focus = current.and_then(|c| s.bind_activations.iter().position(|v| *v == c)).unwrap_or(0);
                    s.bind_activation_parent = page;
                    s.menu_page = 7;
                }
                None => toast = true,
            },
            (7, i) => {
                if let (Some(target), Some(&value)) = (s.bind_target.clone(), s.bind_activations.get(i)) { write = Some((target, value)); }
                close_binding_menu_layer(&mut s);
            }
            _ => toast = true,
        },
        _ => (),
    }
    drop(s);
    if toast { a.toast("Только просмотр: запись настроек привязки не подтверждена"); }
    if let Some((target, value)) = write { write_activation(a, target, value, false); }
    sync(a);
}


pub fn handle(a: &mut App, act: Action) {
    if controller_gyro::active(Owner::Editor) {
        return controller_gyro::act(a, act);
    }
    if st().help_page > 0 {
        let mut s = st();
        s.help_page = help_step(s.help_page, help_pages(s.tab), act);
        drop(s);
        sync(a);
        return;
    }
    if { let s = st(); s.editor_busy && (s.menu_page != 0 || s.browser_open) } { return; }
    if st().menu_page >= 4 { return binding_menu_handle(a, act); }
    if st().menu_page > 0 {
        let page = st().menu_page;
        if page == 1 {
            match act {
                Action::B => st().menu_page = 0,
                Action::Up | Action::Down => { let mut s = st(); s.menu_focus = menu_move(s.menu_focus, act == Action::Down); },
                Action::A => return menu_activate(a), _ => (),
            }
        } else if st().export_prompt.is_some() {
            if act == Action::B { cancel_export(a); } else { a.osk_handle(act); }
            return;
        } else if matches!(act, Action::B | Action::A) { st().menu_page = 1; }
        sync(a); return;
    }
    if st().quick_dialog.is_some() { return quick_dialog_handle(a, act); }
    if st().pad.is_some() { return pad_handle(a, act); }
    if st().stick_settings.is_some() {
        if st().editor_busy {return;}
        match act {
            Action::X => {undo_editor(a);return;},
            Action::B => { let mut s=st(); if !s.stick_settings.as_mut().unwrap().back() {s.stick_settings=None;} },
            Action::A => {if st().stick_settings.as_ref().unwrap().content {step_stick(a,1);return;} else {st().stick_settings.as_mut().unwrap().content=true;}},
            Action::Left|Action::Right if st().stick_settings.as_ref().unwrap().content => {step_stick(a,if act==Action::Left{-1}else{1});return;},
            Action::Up|Action::Down|Action::Left|Action::Right => {
                let counts={let s=st();std::array::from_fn(|section|stick_parameters(&s,section).len())};
                let mut s=st();let f=s.stick_settings.as_mut().unwrap();
                f.step_with_counts(match act {Action::Up=>Direction::Up,Action::Down=>Direction::Down,Action::Left=>Direction::Left,_=>Direction::Right},counts);
            },_=>()
        }
        sync(a);return;
    }
    if st().summary_open {
        match act {
            Action::B => st().summary_open = false,
            Action::Up | Action::Down | Action::Left | Action::Right => {
                let mut s = st();
                s.summary_focus = summary_move(&live_summary(&s), s.summary_focus, act);
            }
            _ => {}
        }
        sync(a);
        return;
    }
    if st().editor.is_some() {
        if st().editor_busy { return; }
        if st().editor_set_prompt.is_some() {
            if act == Action::B { cancel_action_set_name(a); } else { a.osk_handle(act); }
            return;
        }
        if st().command_dialog.is_some() { command_dialog_handle(a,act); return; }
        if act == Action::X { undo_editor(a); return; }
        let picker = st().editor.as_ref().is_some_and(|f| f.picker_open);
        if picker {
            let mut s = st(); let tab = s.picker_tab;
            match act {
                Action::B => { if let Some(f) = s.editor.as_mut() { f.back_from_picker(); } },
                Action::LB|Action::RB => {
                    let tabs=picker_tabs(&s);let i=tabs.iter().position(|t|*t==tab).unwrap_or(0);
                    let next=if act==Action::LB{i.checked_sub(1)}else{Some(i+1).filter(|n|*n<tabs.len())};
                    match next {Some(n)=>s.picker_tab=tabs[n],None=>say(crate::sound::Snd::FailedNav)}
                },
                Action::Up | Action::Down | Action::Left | Action::Right => {
                    let d = match act { Action::Up => Direction::Up, Action::Down => Direction::Down, Action::Left => Direction::Left, _ => Direction::Right };
                    s.picker_cursors[tab] = editor::picker_move(&picker_keys(&s), s.picker_cursors[tab], d);
                },
                Action::A => { drop(s); apply_picker(a); return; }, _ => {}
            }
        } else {
            match act {
                Action::B => { let mut s = st(); if s.editor.as_ref().is_some_and(|f| f.column != Column::Rail) { s.editor.as_mut().unwrap().column = Column::Rail; } else { drop(s); stop_editor(a); } },
                Action::A => return editor_activate(a),
                Action::View => { open_summary(); }
                Action::Menu => return save_personal_editor(a),
                Action::LB | Action::RB => {
                    let mut s = st();
                    let count = s.editing.as_ref().and_then(|e| live::msg(&e.1, 8).ok()).map_or(0, |sets| sets.len());
                    // Stock `GoToRelativeEditingConfigurationActionSetInFlattenedList(±1)` does not wrap: ChangeTabs on success, FailedNav at either end (sp.js:360458).
                    let next = if act == Action::LB { s.editor_set_index.checked_sub(1) } else { Some(s.editor_set_index + 1).filter(|&n| n < count) };
                    match next {
                        Some(next) if count > 1 => {
                            s.editor_set_index = next;
                            if let Some(focus) = &mut s.editor { focus.row = 0; focus.column = Column::Rail; }
                            say(crate::sound::Snd::ChangeTabs);
                        }
                        _ => say(crate::sound::Snd::FailedNav),
                    }
                },
                Action::Y => {
                    let pair = { let s = st(); focused_swap_pair(&s).map(|(left, right)| (s.editor_set_index, left, right)) };
                    if let Some((set, left, right)) = pair { return swap_sources(a, set, left, right, false); }
                }
                Action::Up | Action::Down | Action::Left | Action::Right => {
                    let d = match act { Action::Up => Direction::Up, Action::Down => Direction::Down, Action::Left => Direction::Left, _ => Direction::Right };
                    let gears: Vec<Vec<RowControls>> = { let s = st(); (0..8).map(|n| page_items_of(&s, n).iter().filter(|i| i.kind != pages::Kind::Header).map(controls_of).collect()).collect() };
                    if let Some(f) = st().editor.as_mut() {
                        let rows = gears;
                        f.move_in(d, &rows);
                    }
                }, _ => {}
            }
        }
        sync(a); return;
    }
    let browser_open = { st().browser_open };
    if browser_open {
        if st().browser_preview.is_some() {
            match act {
                Action::B => st().browser_preview = None,
                Action::X => return layout_card_action(a, LButton::X),
                _ => (),
            }
            sync(a);
            return;
        }
        // Stock shows only a spinner while the list loads: nothing but B responds.
        if st().browser_entries.is_none() && act != Action::B { return; }
        if act != Action::Menu { st().browser_delete_pending = None; }
        if act == Action::Backspace && browser_keyboard_target() {
            browser_type(a, None);
            return;
        }
        if a.osk_is_open() && browser_keyboard_target() {
            if act == Action::B {
                a.osk_set_open(false);
                browser_keyboard_closed(a);
            } else { a.osk_handle(act); }
            return;
        }
        if act == Action::A && st().browser.as_ref().is_some_and(|b| b.focus == Focus::SearchField) {
            open_browser_keyboard(a);
            return;
        }
        if act == Action::Y {
            if st().browser.as_ref().is_some_and(|b| b.focus == Focus::SearchField) {
                st().browser_query.clear();
                sync(a);
                return;
            }
            let vote = browser_vote(&st());
            if let Some((id, voted_up)) = vote {
                return vote_browser_layout(a, id, voted_up);
            }
        }
        if matches!(act, Action::A | Action::Y | Action::Menu) && focused_browser_entry(&st()).is_some() {
            layout_card_action(a, match act { Action::A => LButton::A, Action::Y => LButton::Y, _ => LButton::Menu });
            return;
        }
        if act == Action::X {
            if browser_download_entry(&st()).is_some() && !fixture_catalog() {
                return download_focused_layout(a, false);
            }
            if let Some(browser) = st().browser.as_mut() { browser.toggle_show_all(); }
            request_browser_list(a, false);
            return;
        }
        if let Action::Char(c) = act {
            if browser_keyboard_target() { browser_type(a, Some(c)); }
            return;
        }
        if matches!(act, Action::LB | Action::RB) {
            // Стоковый TabBar: L1/R1 листают вкладки с любого элемента (sp.js:347512); без второй вкладки — FailedNav.
            let moved = st().browser.as_mut().is_some_and(|b| b.shift_tab(if act == Action::LB { -1 } else { 1 }));
            if !moved { say(crate::sound::Snd::FailedNav); }
            sync(a);
            return;
        }
        let input = match act {
            Action::Left => Some(Input::Left), Action::Right => Some(Input::Right),
            Action::Up => Some(Input::Up), Action::Down => Some(Input::Down), Action::B => Some(Input::Back), _ => None,
        };
        if let Some(input) = input {
            let mut s = st();
            let count = s.browser.as_ref().map(|b| visible_layouts(&s, b).len()).unwrap_or(0);
            if s.browser.as_mut().is_some_and(|b| b.input(input, count) == Outcome::CloseChooser) { leave_browser(&mut s); }
        }
        sync(a);
        return;
    }
    if st().quick_slider.is_some() { return handle_quick_slider(a, act); }
    match act {
        Action::B => return close(a),
        Action::LB | Action::RB => {
            let other = if st().tab == 0 { 1 } else { 0 };
            return select_controller_tab(a, other);
        }
        Action::Up | Action::Down | Action::Left | Action::Right => {
            let mut s = st();
            if matches!(act, Action::Left | Action::Right) && s.foc >= F_ROW0 {
                // Rows with a gear (right trackpad): Right goes to the gear, Left back to the drop-down.
                let gear = s.rows.get((s.foc - F_ROW0) as usize).is_some_and(|r| matches!(r, Row::Live(q) if q.gear));
                if gear { s.quick_gear = act == Action::Right; }
                drop(s);
                sync(a);
                return;
            }
            s.quick_gear = false;
            s.foc = available_focus(move_focus(s.foc, n_items(&s), act), s.x360_connected);
        }
        Action::A => return activate(a),
        _ => {}
    }
    sync(a);
    ensure_visible(a);
}

fn stick_parameters(s:&St,section:usize)->Vec<sticks::Setting> {
    s.editing.as_ref().and_then(|e|sticks::sticks(&e.1).ok().and_then(|v|v.get(s.stick_index).cloned()).and_then(|source|sticks::settings(&e.1,&source.target,section).ok())).unwrap_or_default()
}
fn stick_rows(s:&St,section:usize)->Vec<ControllerSettingRow> {
    let mut y=0.;stick_parameters(s,section).into_iter().map(|p|{
        let kind=if p.toggle {1}else if p.choices.is_empty(){0}else{2};let h=if kind==0{138.}else{96.};
        let fraction=if p.toggle{p.value as f32}else if p.max>p.min{(p.value-p.min) as f32/(p.max-p.min) as f32}else{0.};
        let row=ControllerSettingRow{kind,label:p.label.into(),value:p.text().into(),fraction,y,h,..Default::default()};y+=h+9.;row
    }).collect()
}

fn sync(a: &mut App) {
    let ui = a.ui();
    let g = ui.global::<ControllerState>();
    let s = st();
    OPEN.set(s.open);
    SIG.set(s.foc as i64 + if s.open { 64 } else { 0 } + if s.browser_open { 128 } else { 0 } + if s.browser_entries.is_some() { 256 } else { 0 } + if s.browser.as_ref().is_some_and(|b| b.show_all) { 512 } else { 0 } + if s.pad.is_some() { 1024 } else { 0 });
    g.set_open(s.open);
    g.set_menu_page(s.menu_page as i32);
    g.set_menu_focus(s.menu_focus as i32);
    g.set_menu_error(s.menu_error.clone().into());
    g.set_menu_choices(ModelRc::new(VecModel::from(s.bind_activations.iter().map(|v| slint::SharedString::from(bind::activation_label(*v)))
        .chain(std::iter::once(t_or("#ActionButtonLabelCancel", "Отмена").into())).collect::<Vec<_>>())));
    g.set_menu_activation(s.bind_target.as_ref().and_then(|t| s.editing.as_ref().and_then(|e| live::activation_at(&e.1, t))).map(bind::activation_label).unwrap_or_default().into());
    g.set_menu_info_name(if s.menu_page >= 4 { s.bind_name.clone().into() } else { s.menu_info.as_ref().map(|v| v.title.as_str()).filter(|v| !v.is_empty()).unwrap_or("Данные недоступны").into() });
    g.set_menu_info_author(if s.menu_author.is_empty() { "Данные недоступны".into() } else { s.menu_author.clone().into() });
    g.set_menu_info_url(s.menu_info.as_ref().map(|v| v.url.as_str()).filter(|v| !v.is_empty()).unwrap_or("Данные недоступны").into());
    let link = s.menu_info.as_ref().and_then(|info| layout_info_link(s.app_id, info));
    g.set_menu_info_id(link.as_ref().and_then(|_| s.menu_info.as_ref()?.published_file_id)
        .map(|id| id.to_string()).unwrap_or_default().into());
    let link_status = if s.menu_info.is_none() { String::new() }
        else if s.menu_link_copied { t_or("#AppControllerConfiguration_Configuration_LinkInfo", "Ссылка на раскладку скопирована в буфер обмена") }
        else if link.is_none() { t_or("#AppControllerConfiguration_Configuration_NoLinkInfo", "Раскладка недоступна в облаке") }
        else { String::new() };
    g.set_menu_info_link_status(link_status.into());
    g.set_menu_info_link(link.unwrap_or_default().into());
    g.set_menu_info_description(if s.menu_description.is_empty() { "Данные недоступны" } else { &s.menu_description }.into());
    g.set_command_dialog_open(s.command_dialog.is_some());
    sync_pad(&g, &s);
    if let Some(d)=&s.command_dialog {
        g.set_command_dialog_title(editor::command_label(&live::command(7,9,d.action)).into());
        g.set_command_dialog_focus(d.cursor as i32);
        g.set_command_dialog_rows(ModelRc::new(VecModel::from(d.parameters.iter().map(|p|format!("{}:  ‹ {} ›",p.label,p.text()).into()).chain(std::iter::once("Применить команду".into())).collect::<Vec<slint::SharedString>>())));
    }
    g.set_editor_fixture_open(s.editor.is_some());
    g.set_picker_fixture_open(s.editor.as_ref().is_some_and(|f| f.picker_open));
    if let Some(f) = &s.editor {
        let set_name = s.editing.as_ref().and_then(|e| live::msg(&e.1, 8).ok()).filter(|sets| sets.len() > 1)
            .and_then(|sets| live::string(sets.get(s.editor_set_index)?, 2).ok()).unwrap_or_default();
        let section = section_title(f.section);
        g.set_editor_help(f.section == 7);
        g.set_editor_title(if set_name.is_empty() { section.to_owned() } else { format!("{section} · {set_name}") }.into());
        g.set_editor_group_title("".into());
        g.set_editor_active_section(f.section as i32); g.set_editor_focus_row(if f.column == Column::Rail { -1 } else { focusable_of(&s, f.section).get(f.row).map_or(-1, |&i| i as i32) });
        g.set_editor_focus_gear(f.column == Column::Gear);
        // Names correspond to the controller source/input pairs in Steam's configurator.
        let sections = [("Кнопки", "shared_button_a"), ("Крестовина", "shared_dpad"), ("Триггеры", "sd_l2"), ("Джойстики", "shared_lstick"), ("Трекпады", "sd_ltrackpad_swipe"), ("Гироскопы", "shared_gyro"), ("Виртуальное меню", ""), ("Наборы действий", "")];
        let glyph = |name: &str| crate::assets::controller_glyph(name, "knockout.md").unwrap_or_default();
        g.set_editor_sections(ModelRc::new(VecModel::from(sections.into_iter().map(|(label, name)| ControllerEditorSection { label: label.into(), glyph: glyph(name) }).collect::<Vec<_>>())));
        let (model_rows, spans, content) = editor_model(&s, f.section);
        let focus_item = focusable_of(&s, f.section).get(f.row).copied();
        g.set_editor_rows(ModelRc::new(VecModel::from(model_rows)));
        g.set_editor_content_height(content);
        let rows_y = if s.editor_status == "Раскладка прочитана · изменения в редактируемой копии" { if f.section == 7 { 96. } else { 93. } } else { 132. };
        g.set_editor_rows_y(rows_y);
        g.set_editor_status(s.editor_status.clone().into());
        g.set_editor_can_undo(!s.editor_undo.is_empty() && !s.editor_busy);
        g.set_editor_scroll(match (f.column == Column::Rail, focus_item.and_then(|i| spans.get(i))) { (false, Some(&(y, h))) => (y + h - (600. - rows_y)).max(0.), _ => 0. });
        let tab=s.picker_tab; let tabs=picker_tabs(&s);
        g.set_picker_tabs(ModelRc::new(VecModel::from(tabs.iter().map(|t|["ГЕЙМПАД","МЫШЬ","КЛАВИАТУРА","ЦИФРОВАЯ КЛАВИАТУРА","НАБОРЫ ДЕЙСТВИЙ","СИСТЕМА","КАМЕРА","ДЕЙСТВИЯ ИГРЫ"][*t].into()).collect::<Vec<slint::SharedString>>())));
        g.set_picker_fixture_tab(tabs.iter().position(|t|*t==tab).unwrap_or(0) as i32); g.set_picker_focus(s.picker_cursors[tab] as i32);
        let names=["геймпада", "мыши", "клавиатуры", "клавиатуры", "набора действий", "системы", "камеры", "игры"];
        let button=editor_binding(&s).map(|r|r.label.as_str()).unwrap_or("");
        g.set_picker_fixture_title(format!("Выберите команду {}: {} → {}",names[tab],section_title(f.section),button).into());
        g.set_picker_keys(ModelRc::new(VecModel::from(picker_keys(&s).into_iter().enumerate().map(|(i,k)| { let binding=picker_binding(&s,i).unwrap_or_default(); ControllerPickerKey { label: k.label.into(), x: k.x, y: k.y, w: k.w, h: k.h, center: k.center, bottom: k.bottom, glyph: editor::command_glyph(&binding,false), focused_glyph: editor::command_glyph(&binding,true), glyph_only: tab==0 && i>=6, ..Default::default() } }).collect::<Vec<_>>())));
    }
    g.set_settings_fixture_open(s.stick_settings.is_some());
    if let Some(f) = &s.stick_settings {
        let names = ["Чувствительность", "Вывод", "Мёртвые зоны", "Внешнее кольцо", "Тактильная отдача"];
        g.set_setting_section(f.section as i32); g.set_setting_title(names[f.section].into());
        g.set_setting_focus(if f.content { f.row as i32 } else { -1 });
        let rows = stick_rows(&s,f.section);
        let source=s.editing.as_ref().and_then(|e|sticks::sticks(&e.1).ok()).and_then(|v|v.get(s.stick_index).cloned()).map(|v|v.label).unwrap_or_default();
        g.set_setting_source(source.into());
        let scroll = if f.content { rows.get(f.row).map(|r| (r.y + r.h - 450.).max(0.)).unwrap_or(0.) } else { 0. };
        g.set_setting_scroll(scroll);
        g.set_setting_content_height(rows.last().map(|r|r.y+r.h).unwrap_or(0.));
        g.set_setting_rows(ModelRc::new(VecModel::from(rows)));
    }
    sync_browser(&g, &s, fixture_catalog());
    sync_root(&g, &s);
    drop(s);
    a.update_hint();
}

fn sync_browser(g: &ControllerState, s: &St, fixture: bool) {
    g.set_browser_open(s.browser_open);
    g.set_browser_download_visible(s.browser.as_ref().is_some_and(|b| matches!(b.active, Tab::Community | Tab::Search)));
    g.set_browser_download_ready(!s.editor_busy && s.browser.as_ref().is_some_and(|b| {
        if matches!(b.focus, Focus::Card(_)) { browser_download_entry(s).is_some() }
        else { visible_layouts(s, b).iter().any(|entry| downloadable(s, entry)) }
    }));
    g.set_browser_preview_open(s.browser_preview.is_some());
    if let Some(entry) = &s.browser_preview {
        g.set_browser_preview_title(layout_title(&entry.title).into());
        g.set_browser_preview_description(entry.description.clone().into());
        g.set_browser_preview_apply(!fixture && !s.editor_busy);
        g.set_browser_preview_install(entry.url.starts_with("workshop://") && !entry.downloaded);
    }
    if let Some(browser) = &s.browser {
        // Stock replaces the whole chooser with a spinner while the list loads, so no tab is shown yet.
        let tabs = if s.browser_entries.is_none() { Vec::new() } else { browser.availability.tabs() };
        let tab_geometry = |tab| match (tab, tabs.len() == 5) {
            (Tab::Recommended, _) => ("РЕКОМЕНДАЦИИ", 282.0),
            (Tab::User, true) => ("МОИ", 145.0),
            (Tab::User, false) => ("МОИ РАСКЛАДКИ", 250.0),
            (Tab::Templates, true) => ("ШАБЛОНЫ", 206.0),
            (Tab::Templates, false) => ("ШАБЛОНЫ", 215.109375),
            (Tab::Community, true) => ("СООБЩЕСТВО", 275.0),
            (Tab::Community, false) => ("РАСКЛАДКИ СООБЩЕСТВА", 392.421875),
            (Tab::Search, true) => ("ПОИСК", 160.0),
            (Tab::Search, false) => ("ПОИСК", 173.109375),
        };
        let width = tabs.iter().map(|&tab| tab_geometry(tab).1).sum::<f32>() + tabs.len().saturating_sub(1) as f32 * 3.0;
        let mut x = (1280.0 - width) / 2.0;
        let items: Vec<ControllerBrowserTab> = tabs.iter().map(|tab| {
            let (label, w) = tab_geometry(*tab);
            let name = match tab { Tab::Recommended => "Star", Tab::User => "User", Tab::Templates => controller_icon_name(s.browser_controller_type), Tab::Community => "SteamLogo", Tab::Search => "Search" };
            let item = ControllerBrowserTab { label: label.into(), x, w, glyph: crate::assets::tint_image(browser_icon(name, false), slint::Color::from_rgb_u8(220,222,223)), focused_glyph: browser_icon(name, true) }; x += w + 3.0; item
        }).collect();
        g.set_browser_tabs(ModelRc::new(VecModel::from(items)));
        g.set_browser_active_tab(tabs.iter().position(|t| *t == browser.active).unwrap_or(0) as i32);
        g.set_browser_tab_focused(browser.focus == Focus::Tab);
        g.set_browser_search_visible(browser.active == Tab::Search);
        g.set_browser_query(s.browser_query.clone().into());
        g.set_browser_search_focused(browser.focus == Focus::SearchField);
        let entries = visible_layouts(s, browser);
        let mut y = match browser.active { Tab::Search => 101.0, Tab::Templates | Tab::Recommended => 90.0, _ => 51.0 };
        let cards: Vec<ControllerConfigCard> = entries.iter().map(|entry| {
            // Text width and line height follow the stock card; Slint handles wrapping.
            let lines = entry.description_lines;
            let description_height = lines as f32 * 24.0;
            let workshop = matches!(browser.active, Tab::Community | Tab::Search);
            let revision = match entry.revision_mismatch {
                2 => t_or("#AppControllerConfiguration_OutdatedConfig", "Устаревшая раскладка"),
                3 => t_or("#AppControllerConfiguration_RequiresSteamInputConfig", "Требуется раскладка Steam Input"),
                _ => String::new(),
            };
            let playtime = workshop && revision.is_empty() && entry.has_playtime;
            let votes = entry.published_file_id.filter(|_| playtime)
                .and_then(|id| s.browser_details.get(&id)).and_then(|detail| detail.votes);
            let voted_up = entry.published_file_id.and_then(|id| s.browser_votes.get(&id)).copied().unwrap_or(false);
            let vote_icon = |focused| {
                if votes.is_none() { return slint::Image::default(); }
                if voted_up {
                    crate::assets::tint_image(crate::assets::controller_icon("ThumbsUp").unwrap_or_default(), slint::Color::from_rgb_u8(97, 182, 232))
                } else { browser_icon("ThumbsUp", focused) }
            };
            let (status, status_icon) = if entry.published_file_id.is_some() {
                let (key, label, icon) = if entry.downloading {
                    ("#AppControllerConfiguration_Downloading", "Загружается", "Download")
                } else if entry.downloaded {
                    ("#AppControllerConfiguration_Downloaded", "Загружено", "Checkmark")
                } else {
                    ("#AppControllerConfiguration_NotDownloaded", "Не загружено", "Download")
                };
                (t_or(key, label).to_uppercase(), icon)
            } else { (String::new(), "") };
            let updated = if browser.active == Tab::User && !entry.updated_label.is_empty() {
                t_or("#AppControllerConfiguration_ConfigUpdated", "Обновлено %1$s").replace("%1$s", &entry.updated_label).to_uppercase()
            } else { String::new() };
            let selected = steam_bridge::controller_list::is_active(entry, s.browser_active.as_ref());
            let prefix = if browser.active == Tab::Search { format!("{}: ", if entry.game_name.is_empty() { if entry.app_id == 0 { "Шаблон".to_string() } else { entry.app_id.to_string() } } else { entry.game_name.clone() }) } else { String::new() };
            let title = layout_title(&entry.title);
            let card = ControllerConfigCard { y, title: title.clone().into(), title_prefix_font: chooser_font(&prefix), title_prefix: prefix.into(), title_prefix_width: if browser.active == Tab::Search { entry.title_prefix_width } else { 0.0 }, description: entry.description.clone().into(),
                author: (if browser.active == Tab::Templates || (browser.active == Tab::Recommended && entry.sections & 32 != 0) { if entry.recommended { t_or("#AppControllerConfiguration_RecommendedTemplateConfig", "Рекомендуемый шаблон") } else { t_or("#AppControllerConfiguration_TemplateConfig", "Шаблон") } } else { entry.author_name.clone() }).into(),
                recommended: entry.recommended, glyph: crate::assets::controller_icon(controller_icon_name(entry.controller_type)).unwrap_or_default(), focused_glyph: browser_icon(controller_icon_name(entry.controller_type), true),
                vote_glyph: vote_icon(false), vote_glyph_focused: vote_icon(true),
                time_glyph: if playtime { browser_icon("Clock", false) } else { Default::default() }, time_glyph_focused: browser_icon("Clock", true),
                selected, status: status.into(),
                status_glyph: browser_icon(status_icon, false), status_glyph_focused: browser_icon(status_icon, true),
                revision: revision.into(), revision_glyph: browser_icon("Caution", false), revision_glyph_focused: browser_icon("Caution", true),
                updated: updated.into(), updated_glyph: browser_icon("Edit", false), updated_glyph_focused: browser_icon("Edit", true),
                votes: votes.map(|v| v.up.to_string()).unwrap_or_default().into(),
                playtime_number: if playtime { entry.playtime_seconds.to_string() } else { String::new() }.into(), playtime_number_width: if playtime { entry.playtime_number_width } else { 0.0 },
                playtime: if playtime { format!("{} Ч. СЫГРАНО", entry.playtime_seconds) } else { String::new() }.into(),
                title_font: chooser_font(&title), author_font: chooser_font(if browser.active == Tab::Templates { "Шаблон" } else { &entry.author_name }), description_font: chooser_font(&entry.description),
                description_height, ..Default::default() };
            y += 12.0 + if !card.status.is_empty() || !card.votes.is_empty() || !card.playtime.is_empty() || !card.revision.is_empty() || !card.updated.is_empty() { 30.0 } else if !card.author.is_empty() { 24.0 } else { 0.0 } + 34.5 + description_height + 15.0 + 9.0;
            card
        }).collect();
        let scroll = -g.get_browser_scroll_y();
        let next_scroll = if let Focus::Card(i) = browser.focus {
            cards.get(i).map(|card| {
                let bottom = cards.get(i + 1).map(|next| next.y - 9.0).unwrap_or(y - 9.0);
                scroll_to_row(scroll, card.y, bottom - card.y, 580.0, y)
            }).unwrap_or(scroll)
        } else if browser.focus == Focus::SearchField { 0.0 } else { scroll };
        g.set_browser_scroll_y(-next_scroll);
        g.set_browser_content_height(y.max(600.0));
        let has_cards = !cards.is_empty();
        g.set_browser_cards(ModelRc::new(VecModel::from(cards)));
        g.set_browser_focused_card(if let Focus::Card(i) = browser.focus { i as i32 } else { -1 });
        // Section titles and descriptions are the sp.js keys of each chooser section (empty fallback = not shown).
        let (title_key, title_fallback, description_key) = match browser.active {
            Tab::Recommended if browser.availability.official > 0 => ("#ControllerConfigurator_ChooseConfiguration_Official", "Официальные раскладки", "#ControllerConfigurator_ChooseConfiguration_Official_Description"),
            Tab::Recommended => ("#ControllerConfigurator_ChooseConfiguration_Recommended", "Рекомендуемые раскладки", "#ControllerConfigurator_ChooseConfiguration_Recommended_Description"),
            Tab::User => ("#ControllerConfigurator_ChooseConfiguration_User", "Мои раскладки", ""),
            Tab::Templates => ("#ControllerConfigurator_ChooseConfiguration_Template", "Шаблоны", "#ControllerConfigurator_ChooseConfiguration_Template_Description"),
            Tab::Community => ("#ControllerConfigurator_ChooseConfiguration_Community", "Раскладки сообщества", ""),
            Tab::Search => ("#ControllerConfigurator_ChooseConfiguration_SearchResults", "Поиск", ""),
        };
        let title = t_or(title_key, title_fallback);
        g.set_browser_page_title(if fixture { format!("{title} · Демоданные") } else { title }.into());
        // Stock prints the description only over a non-empty list; an empty list prints its own text instead.
        let subtitle = if has_cards && !description_key.is_empty() { t_or(description_key, if browser.active == Tab::Templates { "Раскладки от Valve" } else if browser.availability.official > 0 { "Рекомендовано разработчиками игры" } else { "" }) } else { String::new() };
        g.set_browser_page_subtitle(subtitle.into());
        let (load, message) = match &s.browser_entries {
            None => (0, "Загрузка раскладок…".to_string()),
            Some(Err(_)) => (2, "Список раскладок недоступен. Для просмотра примеров запустите с DECKSHELL_CONTROLLER_FIXTURES=1.".to_string()),
            Some(Ok(_)) => (1, t_or("#ControllerConfigurator_ChooseConfiguration_EmptyDescription", "Раскладки не найдены.")),
        };
        g.set_browser_load_state(load);
        g.set_browser_message(message.into());
    }
}

fn help_pages(tab: u8) -> u8 { if tab == 0 { 3 } else { 2 } }

fn help_step(page: u8, last: u8, action: Action) -> u8 {
    match action {
        Action::A if page < last => page + 1,
        Action::A => 0,
        Action::B => page.saturating_sub(1),
        _ => page,
    }
}

fn help_text(page: u8, tab: u8) -> String {
    let localized = |key, fallback| {
        t_or(key, fallback).replace("<1></1>", "Xbox 360")
            .replace("<1>", "").replace("</1>", "").replace("<2>", "").replace("</2>", "")
    };
    let paragraphs = match page {
        1 => [
            if tab == 0 {
                localized("#ControllerConfigurator_Interstitial_Intro_DescA1", "Steam Input преобразует ввод с Deck в команды игры.")
            } else {
                localized("#ControllerConfigurator_Interstitial_Intro_DescA1_NonDeck", "Steam Input преобразует ввод с геймпада в команды игры.")
            },
            localized("#ControllerConfigurator_Interstitial_Intro_DescA2", "Командой может быть кнопка геймпада, клавиша или действие мыши."),
        ],
        2 => [
            localized("#ControllerConfigurator_Interstitial_Intro_DescB1", "Раскладка связывает элементы управления с командами."),
            localized("#ControllerConfigurator_Interstitial_Intro_DescB2", "Выберите элемент ввода и назначьте команду в редакторе раскладки."),
        ],
        _ => [
            localized("#ControllerConfigurator_Interstitial_Intro_DescC1", "Совместимые игры уже имеют готовую раскладку."),
            localized("#ControllerConfigurator_Interstitial_Intro_DescC2", "В быстрых настройках можно назначить кнопки хвата и гироскоп."),
        ],
    };
    paragraphs.join("\n\n")
}

fn sync_root(g: &ControllerState, s: &St) {
    g.set_help_page(i32::from(s.help_page));
    if s.help_page > 0 {
        g.set_help_title(t_or("#ControllerConfigurator_Interstitial_Intro_Title", "Система ввода Steam").into());
        g.set_help_text(help_text(s.help_page, s.tab).into());
        if s.help_page == 1 {
            let file = if s.tab == 0 { "config1.png" } else { "interstitial_controller_config.png" };
            let path = steam_assets::steam_root().join("steamui/images").join(file);
            g.set_help_image(slint::Image::load_from_path(&path).unwrap_or_default());
        }
    }
    g.set_summary_open(s.summary_open);
    g.set_summary_focus(s.summary_focus as i32);
    if s.summary_open {
        let rows = live_summary(s);
        let notice = if !rows.is_empty() { "" }
            else if s.editor.is_some() { &s.editor_status }
            else if s.quick.is_some() { "Steam не вернул набор действий для просмотра" }
            else { s.quick_error.as_deref().unwrap_or("Чтение раскладки Steam…") };
        g.set_summary_notice(notice.into());
        g.set_summary_rows(ModelRc::new(VecModel::from(rows)));
        g.set_summary_device(summary_device_art());
    }
    g.set_title(format!("{} {}", t_or("#ControllerConfigurator_Title", "Настройки контроллера для"), s.name).into());
    let (author, lname) = root_texts(s);
    let geo = root_geo(s);
    g.set_geo_title_y(geo.title_y.into());
    g.set_geo_card_y(geo.card_y.into());
    g.set_geo_card_h(geo.card_h.into());
    g.set_geo_banner(has_banner(s));
    g.set_geo_banner_y(geo.banner_y.into());
    g.set_geo_btn_y(geo.btn_y.into());
    g.set_geo_band_end(geo.band_end.into());
    g.set_geo_head_y(geo.head_y.into());
    g.set_geo_rows_y(geo.rows_y.into());
    g.set_geo_list_h(geo.list_h.into());
    g.set_foc_gear(s.quick_gear);
    g.set_quick_slider(s.quick_slider.is_some());
    g.set_author(author.into());
    g.set_layout_name(lname.into());
    g.set_foc(s.foc);
    g.set_tab(s.tab as i32);
    g.set_x360_connected(s.x360_connected);
    let mut y = 0.0f32;
    let rows: Vec<CRow> = s
        .rows
        .iter()
        .enumerate()
        .map(|(index, r)| {
            let (kind, label, value, on, frac) = match r {
                Row::Live(q) => match &q.kind {
                    quick::Kind::Toggle => (0, q.label.clone(), q.value.clone(), q.on, 0.0),
                    quick::Kind::Dropdown => (1, q.label.clone(), q.value.clone(), false, 0.0),
                    quick::Kind::Slider(row) => {
                        let pending = if s.foc == F_ROW0 + index as i32 { s.quick_slider } else { None };
                        let mut display = row.as_ref().clone();
                        display.value = pending.unwrap_or(row.value);
                        (2, q.label.clone(), row.slider_text_at(display.value), false, display.fraction())
                    }
                },
                Row::Grips(on) => (0, "Включить кнопки хвата на обратной стороне".to_string(), String::new(), *on, 0.0),
                Row::Mode { label, value, .. } => (1, label.clone(), value.clone(), false, 0.0),
                Row::Slider(v) => (2, label_of(v), fmt_val(v), false, if v.known && v.max > v.min { ((v.v - v.min) / (v.max - v.min)).clamp(0.0, 1.0) } else { -1.0 }),
                Row::Toggle { label, on, .. } => (0, label.clone(), String::new(), *on, 0.0),
            };
            let h = row_height(r);
            let gear = matches!(r, Row::Live(q) if q.gear);
            let c = CRow { kind, label: label.into(), value: value.into(), on, frac, gear, y: y.into(), h: h.into() };
            y += h + ROW_GAP;
            c
        })
        .collect();
    g.set_content_h(y.into());
    g.set_rows(ModelRc::new(VecModel::from(rows)));
}


// Stock "Посмотреть раскладку" geometry, device px (CSS x 1.5). Fixture values are the Balatro stock capture,
// not the active layout. See controller_summary.slint for the drawing rules.
/// `glyph` is the Steam glyph file name (`sd_l4`, `shared_dpad_up`, …); empty = no icon.
fn sum_row(label: &str, glyph: &str, label_x: f32, label_w: f32, right: bool, icon: (f32, f32), cy: f32, focus: Option<(f32, f32)>) -> SummaryRow {
    let (fx, fw) = focus.unwrap_or((0., 0.));
    let icon_image = summary_glyph(glyph);
    SummaryRow { label: label.into(), glyph: glyph.into(), has_icon: icon_image.is_some(), icon: icon_image.unwrap_or_default(), label_x, label_w, align_right: right, icon_x: icon.0, icon_w: icon.1, cy, heading: false, focusable: true, fx, fw }
}

fn summary_glyph(glyph: &str) -> Option<slint::Image> {
    macro_rules! bundled {
        ($($name:literal),+ $(,)?) => {
            match glyph {
                $($name => include_bytes!(concat!("../../assets/hotkeys/", $name, ".svg")).as_slice(),)+
                _ => return None,
            }
        };
    }
    let svg = bundled!(
        "sd_l1", "sd_r1", "sd_l2", "sd_r2", "sd_l4", "sd_l5", "sd_r4", "sd_r5",
        "sd_button_view", "sd_button_menu", "sd_rtrackpad_click",
        "sd_ltrackpad_up", "sd_ltrackpad_down", "sd_ltrackpad_left", "sd_ltrackpad_right",
        "shared_dpad_up", "shared_dpad_down", "shared_dpad_left", "shared_dpad_right",
        "shared_button_a", "shared_button_b", "shared_button_x", "shared_button_y",
        "shared_l3", "shared_r3",
    );
    slint::Image::load_from_svg_data(svg).ok()
}

fn sum_head(label: &str, label_x: f32, label_w: f32, right: bool, cy: f32) -> SummaryRow {
    SummaryRow { label: label.into(), glyph: "".into(), icon: Default::default(), has_icon: false, label_x, label_w, align_right: right, icon_x: 0., icon_w: 0., cy, heading: true, focusable: false, fx: 0., fw: 0. }
}

pub(crate) fn summary_rows() -> Vec<SummaryRow> {
    thread_local! {
        static ROWS: Vec<SummaryRow> = build_deck_summary_rows();
    }
    ROWS.with(Clone::clone)
}

fn build_deck_summary_rows() -> Vec<SummaryRow> {
    const LEFT_PLATE: (f32, f32) = (8., 347.);
    const RIGHT_PLATE: (f32, f32) = (925., 347.);
    let mut rows = Vec::new();
    // Left column: labels end at x 301, glyphs sit to the right of them.
    rows.push(sum_row("Левый бампер", "sd_l1", 1., 300., true, (314., 22.), 153.5, Some(LEFT_PLATE)));
    rows.push(sum_row("", "sd_l2", 1., 300., true, (314., 22.), 191., Some(LEFT_PLATE)));
    rows.push(sum_row("", "sd_l4", 1., 300., true, (314., 22.), 237.5, Some(LEFT_PLATE)));
    rows.push(sum_row("", "sd_l5", 1., 300., true, (314., 22.), 276.5, Some(LEFT_PLATE)));
    rows.push(sum_row("Выбрать", "sd_button_view", 1., 300., true, (313., 25.), 314.5, Some(LEFT_PLATE)));
    rows.push(sum_head("ЛЕВЫЙ ТРЕКПАД", 8., 300., true, 354.));
    for (label, glyph, cy) in [("Крестовина вверх", "sd_ltrackpad_up", 382.), ("Крестовина вниз", "sd_ltrackpad_down", 408.5), ("Крестовина влево", "sd_ltrackpad_left", 435.), ("Крестовина вправо", "sd_ltrackpad_right", 461.)] {
        rows.push(sum_row(label, glyph, 1., 300., true, (316., 22.), cy, Some(LEFT_PLATE)));
    }
    // Right column: labels start at x 979 (x 932 for the trackpad group), glyphs at x 944.
    rows.push(sum_row("Правый бампер", "sd_r1", 979., 220., false, (944., 22.), 180.5, Some(RIGHT_PLATE)));
    rows.push(sum_row("", "sd_r2", 979., 220., false, (944., 22.), 218., Some(RIGHT_PLATE)));
    rows.push(sum_row("", "sd_r4", 979., 220., false, (944., 22.), 265., Some(RIGHT_PLATE)));
    rows.push(sum_row("", "sd_r5", 979., 220., false, (944., 22.), 302., Some(RIGHT_PLATE)));
    rows.push(sum_row("Начать", "sd_button_menu", 979., 220., false, (943., 24.), 341., Some(RIGHT_PLATE)));
    rows.push(sum_head("ПРАВЫЙ ТРЕКПАД", 932., 220., false, 381.));
    rows.push(sum_row("Джойстик", "", 932., 220., false, (0., 0.), 409., Some(RIGHT_PLATE)));
    rows.push(sum_row("Нажатие на правый стик", "sd_rtrackpad_click", 979., 220., false, (944., 22.), 435., Some(RIGHT_PLATE)));
    // Bottom blocks: heading at cy 593, rows from cy 624 with a 29 px step (glyph at column x).
    let bottom_plate = |x: f32| (x - 6., 230.);
    rows.push(sum_head("КРЕСТОВИНА", 60., 220., false, 593.));
    for (i, (label, glyph)) in [("Крестовина вверх", "shared_dpad_up"), ("Крестовина вниз", "shared_dpad_down"), ("Крестовина влево", "shared_dpad_left"), ("Крестовина вправо", "shared_dpad_right")].into_iter().enumerate() {
        rows.push(sum_row(label, glyph, 90., 200., false, (59., 24.), 624. + i as f32 * 29., Some(bottom_plate(59.))));
    }
    rows.push(sum_head("ЛЕВЫЙ ДЖОЙСТИК", 306., 220., false, 593.));
    rows.push(sum_row("Джойстик", "", 306., 220., false, (0., 0.), 620., Some(bottom_plate(306.))));
    rows.push(sum_row("Нажатие на левый стик", "shared_l3", 336., 240., false, (305., 23.), 646., Some(bottom_plate(305.))));
    rows.push(sum_head("ГИРОСКОП", 589., 140., false, 593.));
    rows.push(sum_head("ПРАВЫЙ ДЖОЙСТИК", 751., 220., false, 593.));
    rows.push(sum_row("Джойстик", "", 751., 220., false, (0., 0.), 620., Some(bottom_plate(751.))));
    rows.push(sum_row("Нажатие на правый стик", "shared_r3", 781., 240., false, (750., 23.), 646., Some(bottom_plate(750.))));
    rows.push(sum_head("ЛИЦЕВЫЕ КНОПКИ", 1043., 200., false, 593.));
    for (i, (label, glyph)) in [("Кнопка A", "shared_button_a"), ("Кнопка B", "shared_button_b"), ("Кнопка X", "shared_button_x"), ("Кнопка Y", "shared_button_y")].into_iter().enumerate() {
        rows.push(sum_row(label, glyph, 1074., 180., false, (1043., 22.), 624. + i as f32 * 29., Some(bottom_plate(1043.))));
    }
    rows
}

/// Steam's own Deck artwork for the layout preview (`steamui/images/controller`).
pub(crate) fn summary_device_art() -> slint::Image {
    thread_local! {
        static DEVICE: slint::Image = slint::Image::load_from_svg_data(
            include_bytes!("../../ui/assets/controller/controller_config_controller_steam_deck.svg"),
        ).unwrap_or_default();
    }
    DEVICE.with(Clone::clone)
}

fn open_summary() {
    let mut s = st();
    s.summary_open = true;
    s.summary_focus = 0;
}

fn live_summary(s: &St) -> Vec<SummaryRow> {
    if s.editor.is_some() {
        return s.editing.as_ref().map_or_else(Vec::new, |(_, raw, _)| summary_from_layout(raw, s.editor_set_index));
    }
    s.quick.as_ref().map_or_else(Vec::new, |(_, raw)| summary_from_layout(raw, 0))
}

fn summary_from_layout(raw: &[u8], set_index: usize) -> Vec<SummaryRow> {
    use std::collections::BTreeMap;
    let Ok(sets) = live::msg(raw, 8) else { return vec![] };
    let Some(set) = sets.get(set_index) else { return vec![] };
    let mut bindings: BTreeMap<(i64, i64), Vec<String>> = BTreeMap::new();
    let mut modes = BTreeMap::new();
    for source in live::msg(set, 6).unwrap_or_default() {
        let Ok(source_key) = live::number(source, 1) else { continue };
        for group in live::msg(source, 2).unwrap_or_default() {
            if let Ok(mode) = live::number(group, 1) { modes.insert(source_key, mode); }
            for input in live::msg(group, 3).unwrap_or_default() {
                let Ok(key) = live::number(input, 1) else { continue };
                let physical = if matches!(key, 9..=12 | 51..=58) { 0 } else { source_key };
                for activator in live::msg(input, 2).unwrap_or_default() {
                    for binding in live::msg(activator, 2).unwrap_or_default() {
                        let label = editor::command_label(binding);
                        let labels = bindings.entry((physical, key)).or_default();
                        if !labels.contains(&label) { labels.push(label); }
                    }
                }
            }
        }
    }
    let command = |source, key| bindings.get(&(source, key)).map(|labels| labels.join(" · ")).unwrap_or_default();
    let mode = |source| modes.get(&source).filter(|&&mode| mode != 0).map(|&mode| quick::mode_name(source, mode)).unwrap_or_default();
    let mut rows = summary_rows();
    let values = [
        (0, command(0, 53)), (1, command(6, 13)), (2, command(0, 55)), (3, command(0, 57)),
        (4, command(0, 51)), (6, command(1, 1)), (7, command(1, 5)), (8, command(1, 7)), (9, command(1, 3)),
        (10, command(0, 54)), (11, command(7, 13)), (12, command(0, 56)), (13, command(0, 58)),
        (14, command(0, 52)), (16, mode(2)), (17, command(2, 13)),
        (19, command(13, 1)), (20, command(13, 5)), (21, command(13, 7)), (22, command(13, 3)),
        (24, mode(3)), (25, command(3, 13)), (28, mode(12)), (29, command(12, 13)),
        (31, command(0, 12)), (32, command(0, 10)), (33, command(0, 11)), (34, command(0, 9)),
    ];
    for (i, label) in values { rows[i].label = label.into(); }
    let left_pad_mode = mode(1);
    if !left_pad_mode.is_empty() && modes.get(&1) != Some(&1) && rows[6].label.is_empty() {
        rows[6].label = left_pad_mode.into();
        rows[6].glyph = "".into();
        rows[6].has_icon = false;
    }
    let gyro = mode(10);
    if !gyro.is_empty() || !command(10, 13).is_empty() {
        rows.push(sum_row(&[gyro, command(10, 13)].into_iter().filter(|value| !value.is_empty()).collect::<Vec<_>>().join(" · "),
            "", 589., 155., false, (0., 0.), 620., Some((583., 160.))));
    }
    rows
}

fn summary_move(rows: &[SummaryRow], current: usize, action: Action) -> usize {
    let Some(from) = rows.get(current) else { return 0 };
    let cx = from.fx + from.fw / 2.;
    let cy = from.cy;
    rows.iter().enumerate().filter(|(_, r)| !r.heading).filter_map(|(i, r)| {
        let dx = r.fx + r.fw / 2. - cx;
        let dy = r.cy - cy;
        let (primary, secondary) = match action {
            Action::Up if dy < -1. => (-dy, dx.abs()),
            Action::Down if dy > 1. => (dy, dx.abs()),
            Action::Left if dx < -1. => (-dx, dy.abs()),
            Action::Right if dx > 1. => (dx, dy.abs()),
            _ => return None,
        };
        Some((i, primary + secondary * 2.))
    }).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(i, _)| i).unwrap_or(current)
}

#[cfg(test)]
mod tests {
    #[test]
    fn controller_help_opens_on_request_and_returns_to_the_same_focus() {
        use super::*;
        let (ui, mut app) = super::super::osk::field_tests::app();
        *st() = St { open: true, foc: F_HELP, ..Default::default() };
        handle(&mut app, Action::A);
        assert_eq!(ui.global::<ControllerState>().get_help_page(), 1);
        handle(&mut app, Action::A);
        handle(&mut app, Action::A);
        assert_eq!(st().help_page, 3);
        handle(&mut app, Action::B);
        assert_eq!(st().help_page, 2);
        handle(&mut app, Action::A);
        handle(&mut app, Action::A);
        assert_eq!(ui.global::<ControllerState>().get_help_page(), 0);
        assert_eq!(st().foc, F_HELP);
        assert_eq!(help_step(2, help_pages(1), Action::A), 0);
        *st() = St::default();
        sync(&mut app);
    }

    #[test]
    fn controller_tab_uses_native_device_type_even_after_other_pad_input() {
        let devices: super::ControllerDevices = serde_json::from_value(serde_json::json!({
            "active": 22,
            "devices": [
                {"index": -1, "kind": 4},
                {"index": 11, "kind": 4},
                {"index": 21, "kind": 31},
                {"index": 22, "kind": 31},
            ]
        })).unwrap();
        assert_eq!(devices.for_tab(0), Some(11));
        assert_eq!(devices.for_tab(1), Some(22));
        let only_x360 = super::ControllerDevices {
            active: devices.active,
            devices: devices.devices.into_iter().filter(|device| device.kind == 31).collect(),
        };
        assert_eq!(only_x360.for_tab(0), None);
    }

    #[test]
    fn controller_tab_switch_keeps_focus_on_shared_controls() {
        assert_eq!(super::controller_tab_focus(super::F_GEAR, 1, 1), super::F_GEAR);
        assert_eq!(super::controller_tab_focus(super::F_DECK_TAB, 1, 1), super::F_X360_TAB);
        assert_eq!(super::controller_tab_focus(super::F_ROW0 + 1, 1, 1), super::F_VIEW);
        assert_eq!(super::controller_tab_focus(super::F_ROW0, 1, 1), super::F_ROW0);
    }

    #[test]
    fn editor_preview_uses_the_selected_sets_current_commands() {
        use steam_bridge::controller_edit::{gamepad_binding, Writer};
        let mut raw = Writer::new();
        for (key, button) in [("Default", 3), ("Alternate", 4)] {
            let mut activator = Writer::new();
            activator.message(2, &gamepad_binding(button));
            let mut input = Writer::new();
            input.int(1, 12);
            input.message(2, &activator.finish());
            let mut group = Writer::new();
            group.int(1, 3);
            group.message(3, &input.finish());
            let mut source = Writer::new();
            source.int(1, 4);
            source.message(2, &group.finish());
            let mut set = Writer::new();
            set.string(1, key);
            set.message(6, &source.finish());
            raw.message(8, &set.finish());
        }
        let raw = raw.finish();
        let (ui, mut app) = super::super::osk::field_tests::app();
        *st() = St { open: true, editor: Some(EditorFocus::new(8)), editing: Some((0, raw.clone(), vec![])),
            editor_set_index: 1, ..Default::default() };
        handle(&mut app, Action::View);
        let global = ui.global::<ControllerState>();
        use slint::Model;
        assert!(global.get_summary_open());
        let a_face = global.get_summary_rows().row_data(31).unwrap();
        assert_eq!(a_face.label.as_str(), editor::command_label(&gamepad_binding(4)));
        assert_ne!(summary_from_layout(&raw, 0)[31].label, a_face.label);
        handle(&mut app, Action::B);
        assert!(!global.get_summary_open());
        assert!(st().editor.is_some());
        *st() = St::default();
        sync(&mut app);
    }

    #[test]
    fn numeric_slider_uses_osk_and_validates_steam_bounds() {
        let (ui, mut app) = super::super::osk::field_tests::app();
        let (raw, _) = pad::tests::fixture(1, 0);
        let mut s = St { open: true, quick: Some((0, raw)), pad: Some(Pad { source: quick::RPAD, ..Default::default() }), ..Default::default() };
        let focus = pad_rows(&s).iter().position(|r| r.key == 45).unwrap();
        s.pad.as_mut().unwrap().focus = focus;
        *st() = s;
        let global = ui.global::<ControllerState>();
        handle(&mut app, Action::Menu);
        assert!(app.osk_is_open() && global.get_pad_number_open());
        assert_eq!(global.get_pad_number(), "0");
        for c in "999".chars() { app.handle(Action::Char(c)); }
        app.handle(Action::R2);
        assert!(app.osk_is_open(), "rejected input keeps the field open");
        for _ in 0..3 { app.handle(Action::Backspace); }
        for c in "-42".chars() { app.handle(Action::Char(c)); }
        assert_eq!(global.get_pad_number(), "-42");
        app.handle(Action::R2);
        assert!(!app.osk_is_open() && !global.get_pad_number_open());
        assert_eq!(pad_rows(&st())[focus].value, 0, "dry-run issues no Steam write");
        assert_eq!(st().pad.as_ref().unwrap().focus, focus);
        *st() = St::default();
        sync(&mut app);
    }

    #[test]
    fn summary_focus_skips_headings_and_stays_in_bounds() {
        use super::*;
        let mut focus = 0;
        for action in [Action::Down, Action::Down, Action::Down, Action::Down, Action::Down, Action::Right, Action::Down, Action::Left, Action::Up] {
            focus = summary_move(&summary_rows(), focus, action);
            assert!(!summary_rows()[focus].heading);
        }
        assert_eq!(summary_move(&summary_rows(), 0, Action::Up), 0);
    }
    use super::*;

    #[test]
    fn controller_catalog_gallery() {
        use slint::Model;
        let (ui, mut app) = super::super::osk::field_tests::app();
        app.set_pad(true);
        ui.show().unwrap();
        let global = ui.global::<ControllerState>();
        global.set_open(true);
        let footer = ui.global::<crate::FooterState>();
        footer.set_left_hint(app.hint("steam", "#ActionButtonLabelMenu", 3, ""));
        footer.set_hints(ModelRc::new(VecModel::from(vec![
            app.hint("x", "#ControllerConfigurator_ChooseConfiguration_ShowAllConfigs", 1, ""),
            app.hint("a", "#ActionButtonLabelSelect", 1, ""),
            app.hint("b", "#ActionButtonLabelBack", 1, ""),
        ])));
        let mut entries = steam_bridge::controller_list::offline_catalog().unwrap();
        let mut official = entries.iter().find(|entry| entry.sections & 2 != 0).unwrap().clone();
        official.source = steam_bridge::controller_list::LayoutSource::Official;
        official.sections = 1;
        official.url = "workshop://1".into();
        official.title = "Ведьмак 3: Дикая Охота — Официальная раскладка".into();
        official.description = "Official Configuration for The Witcher 3: Wild Hunt.".into();
        official.description_lines = 1;
        official.author_name = "CD PROJEKT RED".into();
        official.downloaded = true;
        official.published_file_id = Some(1);
        let active_official = ActiveLayout { url: official.url.clone(), title: official.title.clone(), author_name: official.author_name.clone(),
            published_file_id: official.published_file_id, selection_type: 1, enabled: true };
        entries.push(official);
        let mut browser = Browser::new(Availability::default());
        update_browser_tabs(&mut browser, &entries, 2379780, true);
        let mut state = St { app_id: 2379780, browser_open: true, browser_connected: true, browser_controller_type: 4, browser: Some(browser), browser_entries: Some(Ok(entries)), ..Default::default() };
        let snapshot = |name: &str| {
            let image = ui.window().take_snapshot().unwrap();
            assert_eq!((image.width(), image.height()), (1280, 800));
            if let Some(path) = std::env::var_os("DECKSHELL_GALLERY_DIR") {
                let dir = std::path::PathBuf::from(path);
                std::fs::create_dir_all(&dir).unwrap();
                image::save_buffer(dir.join(name), image.as_bytes(), 1280, 800, image::ColorType::Rgba8).unwrap();
            }
        };
        state.browser_active = Some(active_official);
        assert!(state.browser.as_mut().unwrap().select_tab(Tab::Recommended));
        sync_browser(&global, &state, false);
        snapshot("recommended.png");
        state.browser_active = None;
        for (tab, bit, name) in [(Tab::Community, 4, "community.png"), (Tab::Templates, 2, "templates.png"), (Tab::User, 8, "mine.png")] {
            let browser = state.browser.as_mut().unwrap();
            assert!(browser.select_tab(tab));
            browser.focus = if tab == Tab::Community { Focus::Tab } else { Focus::Card(0) };
            let entries = visible_layouts(&state, state.browser.as_ref().unwrap());
            assert!(!entries.is_empty() && entries.iter().all(|entry| entry.sections & bit != 0));
            sync_browser(&global, &state, true);
            assert_eq!(global.get_browser_cards().row_count(), entries.len());
            assert!(global.get_browser_page_title().ends_with("Демоданные"));
            let tabs = global.get_browser_tabs();
            let first_tab = tabs.row_data(0).unwrap();
            let last_tab = tabs.row_data(tabs.row_count() - 1).unwrap();
            assert!(first_tab.x >= 85. && last_tab.x + last_tab.w <= 1196.);
            snapshot(name);
        }
        let browser = state.browser.as_mut().unwrap();
        assert!(browser.select_tab(Tab::Community));
        browser.focus = Focus::Card(0);
        sync_browser(&global, &state, false);
        assert!(global.get_browser_download_visible() && global.get_browser_download_ready());
        let id = focused_browser_entry(&state).unwrap().published_file_id.unwrap();
        state.browser_details.insert(id, LayoutDetails {
            published_file_id: id, result: 1, title: String::new(), description: String::new(),
            creator_steamid: String::new(), time_created: 0, time_updated: 0, file_size: 0,
            consumer_appid: 2379780, file_type: 0,
            votes: Some(steam_bridge::controller_list::Votes { up: 42, down: 0, score: 0.0 }),
        });
        state.browser_votes.insert(id, true);
        assert_eq!(browser_vote(&state), Some((id, true)));
        sync_browser(&global, &state, false);
        assert_eq!(global.get_browser_cards().row_data(0).unwrap().votes.as_str(), "42");
        state.browser_connected = false;
        assert_eq!(browser_vote(&state), None);
        assert!(visible_layouts(&state, state.browser.as_ref().unwrap()).iter().all(|entry| entry.downloaded));
        state.browser.as_mut().unwrap().select_tab(Tab::Search);
        let index = visible_layouts(&state, state.browser.as_ref().unwrap()).iter()
            .position(|entry| entry.published_file_id == Some(id)).unwrap();
        state.browser.as_mut().unwrap().focus = Focus::Card(index);
        assert!(!selectable(&state, focused_browser_entry(&state).unwrap()));
        sync_browser(&global, &state, false);
        assert!(!global.get_browser_download_ready());
        state.browser_connected = true;
        state.browser.as_mut().unwrap().select_tab(Tab::Search);
        sync_browser(&global, &state, false);
        assert!(global.get_browser_download_visible());
        snapshot("search-list.png");
        let browser = state.browser.as_mut().unwrap();
        browser.select_tab(Tab::Community);
        browser.focus = Focus::Card(0);
        state.browser_preview = visible_layouts(&state, state.browser.as_ref().unwrap()).first().map(|entry| (*entry).clone());
        sync_browser(&global, &state, true);
        assert!(global.get_browser_preview_open() && !global.get_browser_preview_apply());
        snapshot("preview-offline.png");
        sync_browser(&global, &state, false);
        assert!(global.get_browser_preview_apply() && global.get_browser_preview_install());
        snapshot("preview-install.png");
        global.set_browser_open(false);
        global.set_browser_preview_open(false);
        global.set_title("Настройки контроллера для Balatro".into());
        global.set_editor_fixture_open(true);
        global.set_editor_title("Наборы действий · Инвентарь".into());
        global.set_editor_status("Набор изменён в редакторе · Menu — сохранить".into());
        global.set_editor_active_section(7);
        global.set_editor_sections(ModelRc::new(VecModel::from(pages::SECTION_TITLES.iter().map(|title| ControllerEditorSection {
            label: (*title).into(), ..Default::default()
        }).collect::<Vec<_>>())));
        let mut layout = steam_bridge::controller_edit::Writer::new();
        for (key, title) in [("Default", "По умолчанию"), ("Inventory", "Инвентарь")] {
            let mut set = steam_bridge::controller_edit::Writer::new();
            set.string(1, key);
            set.string(2, title);
            layout.message(8, &set.finish());
        }
        let (rows, _, height) = page_model_raw(&layout.finish(), 7);
        assert_eq!(rows.len(), 4);
        global.set_editor_rows(ModelRc::new(VecModel::from(rows)));
        global.set_editor_rows_y(132.);
        global.set_editor_content_height(height);
        global.set_editor_focus_row(1);
        global.set_editor_focus_gear(true);
        snapshot("action-sets.png");
    }

    #[test]
    fn search_results_keep_focus_when_keyboard_is_closed() {
        let mut browser = Browser::new(Availability::default());
        assert!(browser.select_tab(Tab::Search));
        browser.input(Input::Down, 2);
        browser.keyboard_open = true;
        assert_eq!(browser.input(Input::Back, 2), Outcome::CloseKeyboard);
        assert_eq!(browser.focus, Focus::SearchField);
        browser.input(Input::Down, 2);
        browser.input(Input::Down, 2);
        browser.input(Input::Back, 2);
        browser.input(Input::Down, 2);
        assert_eq!(browser.focus, Focus::Card(1));
    }
    fn open_root() -> St { St { open: true, foc: F_CARD, rows: vec![Row::Grips(false)], ..Default::default() } }
    fn sound_after(act: Action, change: impl FnOnce(&mut St)) -> Option<crate::sound::Snd> {
        let before = nav_snap();
        change(&mut st());
        nav_sound(act, &before)
    }
    #[test]
    fn nav_sounds_follow_stock_rules() {
        use crate::sound::Snd;
        *st() = open_root();
        // фокус сдвинулся — BasicNav; не сдвинулся — FailedNav; A без перемены — DefaultOk; B/прочее без перемены — тишина
        assert_eq!(sound_after(Action::Down, |s| s.foc = F_VIEW), Some(Snd::BasicNav));
        assert_eq!(sound_after(Action::Up, |_| {}), Some(Snd::FailedNav));
        assert_eq!(sound_after(Action::A, |_| {}), Some(Snd::DefaultOk));
        assert_eq!(sound_after(Action::X, |_| {}), None);
        // диалог открылся/закрылся
        assert_eq!(sound_after(Action::A, |s| s.quick_dialog = Some((quick::GYRO, 0))), Some(Snd::ShowModal));
        assert_eq!(sound_after(Action::Down, |s| s.quick_dialog = Some((quick::GYRO, 1))), Some(Snd::BasicNav));
        assert_eq!(sound_after(Action::B, |s| s.quick_dialog = None), Some(Snd::HideModal));
        // Отмена диалога звучит, возврат по маршруту — тихий.
        assert_eq!(sound_after(Action::A, |s| s.summary_open = true), Some(Snd::DefaultOk));
        assert_eq!(sound_after(Action::Down, |s| s.summary_focus = 3), Some(Snd::BasicNav));
        assert_eq!(sound_after(Action::B, |s| s.summary_open = false), None);
        assert_eq!(sound_after(Action::B, |s| s.open = false), None);
    }
    #[test]
    fn command_settings_open_with_a_and_back_restores_each_focus() {
        let (_ui, mut app) = super::super::osk::field_tests::app();
        let raw = quick::tests::layout(false, None, Some((12, None)), None);
        let target = quick::target(&raw, quick::RPAD).unwrap();
        *st() = St { menu_page: 4, bind_target: Some(target), ..open_root() };

        handle(&mut app, Action::A);
        assert_eq!(st().menu_page, 7);
        handle(&mut app, Action::B);
        { let s = st(); assert_eq!((s.menu_page, s.menu_focus), (4, 0)); }
        handle(&mut app, Action::Down);
        let before = nav_snap();
        handle(&mut app, Action::A);
        assert_eq!(nav_sound(Action::A, &before), Some(crate::sound::Snd::ShowModal));
        assert_eq!(st().menu_page, 5);
        for _ in 0..4 { handle(&mut app, Action::Down); }
        handle(&mut app, Action::A);
        assert_eq!(st().menu_page, 7);
        let count = st().bind_activations.len();
        for _ in 0..count { handle(&mut app, Action::Down); }
        handle(&mut app, Action::A);
        { let s = st(); assert_eq!((s.menu_page, s.menu_focus), (5, 4)); }
        handle(&mut app, Action::B);
        { let s = st(); assert_eq!((s.menu_page, s.menu_focus), (4, 1)); }
        handle(&mut app, Action::B);
        assert_eq!(st().menu_page, 0);
        *st() = St::default();
        sync(&mut app);
    }

    #[test]
    fn root_mode_rows_and_cancel_share_the_keyboard_order() {
        use slint::Model;
        let (ui, mut app) = super::super::osk::field_tests::app();
        let raw = quick::tests::layout(false, None, Some((12, None)), None);
        *st() = St { quick: Some((0, raw)), quick_dialog: Some((quick::RPAD, 2)), ..open_root() };
        sync(&mut app);
        let g = ui.global::<ControllerState>();
        let labels = || g.get_choice_items().iter().map(|item| item.label.to_string()).collect::<Vec<_>>();
        assert_eq!(labels(), ["Не выбрано", "Как мышь", "Как джойстик", "Создать сменный режим", "Отмена"]);
        handle(&mut app, Action::Down);
        handle(&mut app, Action::Down);
        assert_eq!(g.get_choice_focus(), 4);
        handle(&mut app, Action::A);
        assert!(!g.get_choice_open());
        assert!(st().open);
        *st() = St::default();
        sync(&mut app);
    }

    #[test]
    fn quick_gyro_slider_shows_live_values_and_stays_in_bounds() {
        use crate::sound::Snd;
        use slint::Model;
        let (ui, mut app) = super::super::osk::field_tests::app();
        let raw = pad::tests::other_mode_fixture(quick::GYRO, 22, &[
            (140, 50, 0, 100), (137, 6545, 200, 20000), (42, 1, 0, 1),
        ]);
        *st() = St { quick: Some((0, raw)), ..open_root() };
        rebuild_rows(&mut st());
        let index = st().rows.iter().position(|r| matches!(r,
            Row::Live(quick::QRow { kind: quick::Kind::Slider(row), .. }) if row.key == 140
        )).unwrap();
        st().foc = F_ROW0 + index as i32;
        sync(&mut app);

        let g = ui.global::<ControllerState>();
        let displayed = || g.get_rows().row_data(index).unwrap();
        assert_eq!(displayed().kind, 2);
        assert_eq!(displayed().frac, 0.5);
        handle(&mut app, Action::A);
        assert!(g.get_quick_slider());
        let before = nav_snap();
        handle(&mut app, Action::Right);
        assert_eq!(nav_sound(Action::Right, &before), Some(Snd::SliderUp));
        assert_eq!(st().quick_slider, Some(52));
        assert_eq!(displayed().frac, 0.52);
        for _ in 0..26 { handle(&mut app, Action::Right); }
        let before = nav_snap();
        handle(&mut app, Action::Right);
        assert_eq!(nav_sound(Action::Right, &before), Some(Snd::FailedNav));
        assert_eq!(displayed().frac, 1.0);
        for _ in 0..25 { handle(&mut app, Action::Left); }
        handle(&mut app, Action::B);
        assert!(!g.get_quick_slider());
        assert_eq!(st().foc, F_ROW0 + index as i32);
        assert_eq!(displayed().frac, 0.5);
        *st() = St::default();
        sync(&mut app);
    }

    #[test]
    fn swap_hint_follows_the_focused_source_and_ignores_the_rail() {
        let raw = quick::tests::layout(false, None, Some((12, None)), None);
        let mut s = St { editor: Some(EditorFocus::new(8)), editing: Some((0, raw, vec![])), ..open_root() };
        let focus = s.editor.as_mut().unwrap();
        focus.section = 4;
        assert_eq!(focused_swap_pair(&s), None);
        s.editor.as_mut().unwrap().column = Column::Value;
        assert_eq!(focused_swap_pair(&s), Some((quick::LPAD, quick::RPAD)));
        s.editor.as_mut().unwrap().row = 1;
        assert_eq!(focused_swap_pair(&s), None);
    }

    #[test]
    fn layout_info_links_only_to_published_layouts() {
        let mut info = ActiveLayout {
            url: "template://fixture".into(), title: "fixture".into(), author_name: String::new(),
            published_file_id: Some(123456789), selection_type: 3, enabled: true,
        };
        assert_eq!(layout_info_link(42, &info), None);
        info.url = "workshop://123456789".into();
        assert_eq!(layout_info_link(42, &info).as_deref(), Some("steam://controllerconfig/42/123456789"));
        assert_eq!(layout_info_link(0, &info), None);
        info.published_file_id = None;
        assert_eq!(layout_info_link(42, &info), None);
    }

    #[test]
    fn editor_and_picker_moves_are_audible() {
        use crate::sound::Snd;
        *st() = St { editor: Some(EditorFocus::new(8)), ..open_root() };
        // общая сигнатура экрана раньше не видела фокус редактора: ход по строкам молчал или давал FailedNav
        assert_eq!(sound_after(Action::Down, |s| s.editor.as_mut().unwrap().row = 2), Some(Snd::BasicNav));
        assert_eq!(sound_after(Action::Right, |s| s.editor.as_mut().unwrap().column = Column::Value), Some(Snd::BasicNav));
        assert_eq!(sound_after(Action::Left, |_| {}), Some(Snd::FailedNav));
        assert_eq!(sound_after(Action::RB, |s| s.editor_set_index = 1), Some(Snd::ChangeTabs));
        assert_eq!(sound_after(Action::A, |s| s.editor.as_mut().unwrap().picker_open = true), Some(Snd::DefaultOk));
        assert_eq!(sound_after(Action::RB, |s| s.picker_tab = 1), Some(Snd::ChangeTabs));
        assert_eq!(sound_after(Action::Down, |s| s.picker_cursors[1] = 4), Some(Snd::BasicNav));
    }
    #[test]
    fn explicit_sounds_beat_the_focus_diff() {
        use crate::sound::Snd;
        *st() = open_root();
        say(Snd::ToggleOn);
        say(Snd::ToggleOff);
        assert_eq!(nav_sound(Action::A, &nav_snap_with_say_cleared()), Some(Snd::ToggleOn));
        // снимок обнуляет хвост неиспользованного звука: следующее нажатие не наследует чужой
        say(Snd::SliderUp);
        let _ = nav_snap();
        assert_eq!(nav_sound(Action::A, &nav_snap()), Some(Snd::DefaultOk));
        // идёт запись: нажатия молчат
        let mut busy = nav_snap();
        busy.busy = true;
        assert_eq!(nav_sound(Action::A, &busy), None);
    }
    fn nav_snap_with_say_cleared() -> NavSnap { let s = SAY.with(|c| c.get()); let snap = nav_snap(); SAY.with(|c| c.set(s)); snap }
    #[test]
    fn browser_shoulder_buttons_switch_tabs_from_any_element() {
        let mut b = Browser::new(Availability { personal: 1, official: 1, community_allowed: true, ..Default::default() });
        let first = b.active;
        b.focus = Focus::Card(2);
        assert!(b.shift_tab(1));
        assert_ne!(b.active, first);
        assert_eq!(b.focus, Focus::Tab);
        assert!(b.shift_tab(-1));
        assert_eq!(b.active, first);
        // вкладки циклические (TabBar wrapAround): с последней RB возвращает на первую
        b.active = Tab::Search;
        assert!(b.shift_tab(1));
        assert_eq!(b.active, first);
    }
    #[test]
    fn closing_browser_preserves_root_focus_and_rows() {
        let mut state = St { browser_open: true, foc: F_CARD, rows: vec![Row::Grips(false)], ..Default::default() };
        leave_browser(&mut state);
        assert!(!state.browser_open);
        assert_eq!(state.foc, F_CARD);
        assert_eq!(state.rows, vec![Row::Grips(false)]);
    }
    #[test]
    fn root_focus_follows_stock_spatial_order() {
        // count = F_ROW0 + 3 quick-settings rows
        let n = F_ROW0 + 3;
        assert_eq!(move_focus(F_X360_TAB, n, Action::Down), F_CARD);
        assert_eq!(move_focus(F_CARD, n, Action::Up), F_X360_TAB);
        assert_eq!(move_focus(F_CARD, n, Action::Right), F_HELP);
        assert_eq!(move_focus(F_HELP, n, Action::Left), F_CARD);
        assert_eq!(move_focus(F_CARD, n, Action::Down), F_VIEW);
        assert_eq!(move_focus(F_VIEW, n, Action::Right), F_EDIT);
        assert_eq!(move_focus(F_EDIT, n, Action::Right), F_GEAR);
        assert_eq!(move_focus(F_GEAR, n, Action::Left), F_EDIT);
        assert_eq!(move_focus(F_GEAR, n, Action::Down), F_ROW0);
        assert_eq!(move_focus(F_ROW0, n, Action::Up), F_VIEW);
        assert_eq!(move_focus(F_ROW0 + 2, n, Action::Down), F_ROW0 + 2);
        assert_eq!(move_focus(F_DECK_TAB, n, Action::Up), F_DECK_TAB);
    }
    #[test]
    fn root_without_rows_never_leaves_the_button_row_downwards() {
        assert_eq!(move_focus(F_VIEW, F_ROW0, Action::Down), F_VIEW);
        assert_eq!(move_focus(F_GEAR, F_ROW0, Action::Down), F_GEAR);
    }
    #[test]
    fn geometry_matches_the_owner_frames_of_20_11() {
        // Zelda (template: hint plate, one-line name): card 135..215, plate 216..276, buttons 291, header, rows 464.
        let z = geometry(false, false, 1, true);
        assert_eq!((z.card_y, z.card_h, z.banner_y, z.btn_y, z.rows_y), (135.0, 80.0, 216.0, 292.0, 465.0));
        assert!((z.band_end - 396.0).abs() < 2.0 && (z.head_y - 420.0).abs() < 2.0);
        // КЛЕТЬ (author, two-line name, no plate): card 135..249, buttons 264, rows 437.
        let c = geometry(false, true, 2, false);
        assert_eq!((c.card_h, c.btn_y, c.rows_y), (114.0, 264.0, 437.0));
        // Tabs (second controller) push everything down by the old tab band.
        assert_eq!(geometry(true, false, 1, false).card_y, 223.0);
        assert!(z.list_h > 200.0 && c.list_h > z.list_h);
    }
    #[test]
    fn root_without_second_controller_has_no_tab_focus_and_no_tour() {
        assert_eq!(available_focus(F_DECK_TAB, false), F_CARD);
        assert_eq!(move_focus(F_CARD, F_ROW0, Action::Up), F_X360_TAB);
        assert_eq!(available_focus(move_focus(F_CARD, F_ROW0, Action::Up), false), F_CARD);
        // The help button is an element only: activating it changes nothing and no overlay state exists.
        let s = St { open: true, foc: F_HELP, ..Default::default() };
        assert_eq!(s.foc, F_HELP);
        assert_eq!(hints_for(F_HELP).0, "Помощь");
        assert_eq!(hints_for(F_VIEW).0, "#ActionButtonLabelSelect");
    }
    fn hints_for(foc: i32) -> (&'static str, i32) {
        // same rule as `hints()` for the root page
        let select = if foc == F_CARD { "Загрузить раскладку" } else if foc == F_HELP { "Помощь" } else { "#ActionButtonLabelSelect" };
        (select, foc)
    }
    #[test]
    fn live_rows_replace_vdf_rows_and_keep_focus_in_range() {
        let raw = quick::tests::layout(false, None, Some((12, Some(0))), Some(1));
        let mut s = St { open: true, foc: F_ROW0 + 9, rows: vec![Row::Grips(true)], quick: Some((0, raw)), ..Default::default() };
        rebuild_rows(&mut s);
        assert_eq!(s.rows.len(), 5);
        assert!(matches!(&s.rows[2], Row::Live(q) if q.gear));
        assert_eq!(s.foc, F_ROW0 + 4);
        assert_eq!(row_height(&s.rows[0]), 69.0);
        assert_eq!(row_height(&s.rows[1]), 96.0);
    }
    #[test]
    fn controller_root_gallery() {
        let (ui, mut app) = super::super::osk::field_tests::app();
        app.set_pad(true);
        ui.global::<crate::Icons>().on_tinted(crate::assets::tint_image);
        ui.window().set_size(slint::PhysicalSize::new(1280, 800));
        ui.show().unwrap();
        let g = ui.global::<ControllerState>();
        g.set_open(true);
        let footer = ui.global::<crate::FooterState>();
        footer.set_left_hint(app.hint("steam", "#ActionButtonLabelMenu", 3, ""));
        footer.set_hints(ModelRc::new(VecModel::from(vec![
            app.hint("a", "#ActionButtonLabelSelect", 1, ""),
            app.hint("b", "#ActionButtonLabelBack", 1, ""),
        ])));
        let snapshot = |name: &str| {
            let image = ui.window().take_snapshot().unwrap();
            if let Some(path) = std::env::var_os("DECKSHELL_GALLERY_DIR") {
                let dir = std::path::PathBuf::from(path);
                std::fs::create_dir_all(&dir).unwrap();
                image::save_buffer(dir.join(name), image.as_bytes(), 1280, 800, image::ColorType::Rgba8).unwrap();
            }
        };
        let active = |url: &str, title: &str| ActiveLayout { url: url.into(), title: title.into(), author_name: String::new(), published_file_id: None, selection_type: 3, enabled: true };
        let raw = quick::tests::layout(false, None, Some((12, Some(0))), Some(0));
        let mut s = St { open: true, app_id: 2, name: "The Legend of Zelda: Twilight Princess".into(), foc: F_HELP, quick: Some((0, raw.clone())), root_active: Some(active("template://controller_neptune_gamepad_camera.vdf", "Геймпад с управлением камерой")), ..Default::default() };
        rebuild_rows(&mut s);
        sync_root(&g, &s);
        snapshot("root-template.png");
        s.x360_connected = true;
        s.foc = F_VIEW;
        sync_root(&g, &s);
        snapshot("root-template-two-controllers.png");
        s.x360_connected = false;
        s.foc = F_ROW0 + 4;
        g.set_vy((-(s.rows.iter().map(|r| row_height(r) + ROW_GAP).sum::<f32>() - ROW_GAP - root_geo(&s).list_h).max(0.)).into());
        sync_root(&g, &s);
        snapshot("root-template-scrolled.png");
        g.set_vy(0.0);
        let mut s = St { open: true, app_id: 3, name: "КЛЕТЬ".into(), foc: F_VIEW, quick: Some((0, raw)), root_active: Some(ActiveLayout { author_name: "death_nick_".into(), ..active("workshop://1", "Официальная раскладка КЛЕТЬ - Геймпад с управлением камерой") }), ..Default::default() };
        rebuild_rows(&mut s);
        sync_root(&g, &s);
        assert!(!g.get_geo_banner());
        snapshot("root-official.png");
        s.help_page = 1;
        sync_root(&g, &s);
        footer.set_hints(ModelRc::new(VecModel::from(vec![
            app.hint("a", "#ControllerConfigurator_Interstitial_NextPage", 1, ""),
            app.hint("b", "#ActionButtonLabelBack", 1, ""),
        ])));
        snapshot("root-help.png");
        footer.set_hints(ModelRc::new(VecModel::from(vec![
            app.hint("a", "#ActionButtonLabelSelect", 1, ""),
            app.hint("b", "#ActionButtonLabelBack", 1, ""),
        ])));
        s.help_page = 0;
        s.foc = F_ROW0 + 1;
        s.quick_dialog = Some((quick::GYRO, 0));
        sync_root(&g, &s);
        sync_pad(&g, &s);
        snapshot("root-gyro-list.png");
        s.quick_dialog = Some((quick::RPAD, 2));
        sync_pad(&g, &s);
        snapshot("root-rpad-list.png");
    }
    #[test]
    fn controller_pad_gallery() {
        let (ui, mut app) = super::super::osk::field_tests::app();
        app.set_pad(true);
        ui.show().unwrap();
        let g = ui.global::<ControllerState>();
        g.set_open(true);
        let snapshot = |name: &str| {
            let image = ui.window().take_snapshot().unwrap();
            if let Some(path) = std::env::var_os("DECKSHELL_GALLERY_DIR") {
                let dir = std::path::PathBuf::from(path);
                std::fs::create_dir_all(&dir).unwrap();
                image::save_buffer(dir.join(name), image.as_bytes(), 1280, 800, image::ColorType::Rgba8).unwrap();
            }
        };
        let (raw, _) = pad::tests::fixture(1, 0);
        let mut s = St { open: true, app_id: 2, name: "The Legend of Zelda: Twilight Princess".into(), quick: Some((0, raw)), ..Default::default() };
        s.pad = Some(Pad { source: quick::RPAD, ..Default::default() });
        let rows = pad_rows(&s);
        assert!(!rows.is_empty());
        let at = |key: i64| rows.iter().position(|r| r.key == key && !r.heading).unwrap();
        let y = |i: usize| rows[i].y;
        // (имя, строка в фокусе, прокрутка) — кадры 05…11
        let frames = [
            ("pad-05-basic.png", at(31), 0.0),
            ("pad-06-output.png", at(42), y(at(52)) - 96.0),
            ("pad-07-deadzone.png", at(50), y(at(44)) - 156.0),
            ("pad-08-trackpad.png", at(35), y(at(51)) + 30.0),
            ("pad-09-trackpad2.png", at(35), y(at(35))),
            ("pad-10-trigger-haptic.png", at(3), y(at(78) - 1) - 249.0),
            ("pad-11-gyro-haptic.png", at(158), y(at(78) - 1) + 6.0),
        ];
        for (name, focus, scroll) in frames {
            s.pad.as_mut().unwrap().focus = focus;
            s.pad.as_mut().unwrap().scroll = scroll.max(0.0);
            *st() = St { open: true, app_id: s.app_id, name: s.name.clone(), quick: s.quick.clone(), pad: s.pad.clone(), ..Default::default() };
            sync(&mut app);
            assert_eq!(g.get_pad_two_line(), matches!(name, "pad-05-basic.png" | "pad-07-deadzone.png" | "pad-08-trackpad.png" | "pad-09-trackpad2.png"));
            snapshot(name);
        }
        s.pad.as_mut().unwrap().choice = Some(1);
        sync_pad(&g, &s);
        snapshot("pad-12-choice.png");
        let focus = at(45);
        if let Some(p) = st().pad.as_mut() {
            p.focus = focus;
            p.scroll = y(focus) - 240.;
        }
        handle(&mut app, Action::Menu);
        assert!(g.get_pad_number_open());
        for _ in 0..4 {
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(300));
            ui.window().take_snapshot().unwrap();
        }
        snapshot("pad-numeric.png");
        app.handle(Action::B);
        for (source, mode, key, filename) in [
            (2, 4, 31, "settings-mouse.png"),
            (2, 9, 53, "settings-wheel.png"),
            (2, 14, 77, "settings-radial.png"),
            (13, 1, 24, "settings-dpad.png"),
            (10, 22, 140, "settings-gyro-mouse.png"),
            (10, 23, 140, "settings-gyro-camera.png"),
            (10, 24, 154, "settings-gyro-deflection.png"),
        ] {
            st().quick = Some((0, pad::tests::other_mode_fixture(source, mode, &[(key, 1, 0, 100), (3, 1, 0, 3)])));
            pad_open(&mut app, source, None, false);
            assert!(g.get_pad_open());
            snapshot(filename);
        }
        if let Some(raw) = std::env::var_os("HOME").and_then(|home| {
            std::fs::read(std::path::Path::new(&home).join(".cache/deckshell/task-264/20-start-raw.bin")).ok()
        }) {
            *st() = St { open: true, name: "Balatro".into(), editor: Some(EditorFocus::new(8)),
                editing: Some((0, raw.clone(), live::rows(&raw).unwrap())), ..Default::default() };
            for (section, name) in [(0, "editor-536-buttons.png"), (5, "editor-536-gyro.png"), (7, "editor-536-action-sets.png")] {
                st().editor.as_mut().unwrap().section = section;
                sync(&mut app);
                snapshot(name);
            }
            handle(&mut app, Action::View);
            assert!(g.get_summary_open() && g.get_summary_notice().is_empty());
            snapshot("editor-536-preview.png");
        }
        *st() = St::default();
        sync(&mut app);
    }
    #[test]
    fn choice_list_geometry_matches_the_stock_popups() {
        let rows = |source: i64| {
            // Кадры 03/04 сняты со списками из 3–4 режимов; новые списки сверяются отдельным тестом ниже.
            let modes: &[i64] = if source == quick::RPAD { &[0, 4, 12] } else { quick::mode_choices(source) };
            let mut r: Vec<(String, i32)> = modes.iter().map(|&m| (quick::mode_name(source, m), quick::mode_icon(source, m))).collect();
            r.push(("Создать сменный режим".into(), 0));
            r
        };
        // кадр 03 (гироскоп): окно 359…922 × 136…626 → 563 × 490; кадр 04 (трекпад): 431…851 × 181…580 → 420 × 399
        let (items, w, h) = choice_model(rows(quick::GYRO), "Отмена".into());
        assert_eq!((h, items.len()), (489.0, 6));
        assert!((w - 563.0).abs() < 3.0, "{w}");
        let (_, w, h) = choice_model(rows(quick::RPAD), "Отмена".into());
        assert_eq!((w, h), (420.0, 399.0));
    }
    #[test]
    fn pad_hints_and_navigation_follow_the_stock_footer() {
        let (raw, _) = pad::tests::fixture(1, 0);
        {
            let mut s = st();
            *s = St { open: true, quick: Some((0, raw)), pad: Some(Pad { source: quick::RPAD, ..Default::default() }), ..Default::default() };
        }
        fn keys(h: Vec<(&'static str, &'static str, i32, &'static str, Action)>) -> Vec<&'static str> { h.into_iter().map(|x| x.0).collect() }
        // ползунок в фокусе: Y плавный сдвиг, ≡ изменить значение, A положение ползунка, B назад
        let rows = pad_rows(&st());
        st().pad.as_mut().unwrap().focus = rows.iter().position(|r| r.key == 31).unwrap();
        assert_eq!(keys(hints()), ["y", "menu", "a", "b"]);
        st().pad.as_mut().unwrap().focus = rows.iter().position(|r| r.key == 8).unwrap();
        assert_eq!(keys(hints()), ["a", "b"]);
        *st() = St::default();
    }
    #[test]
    fn stock_row_heights_match_capture() {
        assert_eq!(row_height(&Row::Grips(false)) + ROW_GAP, 78.0);
        assert_eq!(row_height(&Row::Mode { src: "x", label: String::new(), value: String::new() }), 96.0);
    }
    #[test]
    fn focused_row_scrolls_into_view_without_overscrolling() {
        assert_eq!(scroll_to_row(0.0, 200.0, 73.0, 150.0, 500.0), 123.0);
        assert_eq!(scroll_to_row(123.0, 20.0, 73.0, 150.0, 500.0), 20.0);
        assert_eq!(scroll_to_row(20.0, 30.0, 73.0, 150.0, 500.0), 20.0);
        assert_eq!(scroll_to_row(0.0, 0.0, 73.0, 150.0, 73.0), 0.0);
    }
    fn val(key: &'static str, v: f32, unit: Unit) -> Val {
        Val { key, v, min: 0.0, max: 30.0, step: 0.1, unit, known: true }
    }
    #[test]
    fn formats() {
        assert_eq!(fmt_val(&val("gyro_natural", 0.6, Unit::Mult)), "0.6x");
        assert_eq!(fmt_val(&val("gyro_pixels", 6545.0, Unit::Pixels)), "6545пикс.");
    }
    #[test]
    fn vdf_fixture() {
        let t = r##""controller_mappings" { "title" "#Title" "creator" "7656" "localization" { "english" { "title" "Eng" } "russian" { "title" "Рус" } }
 "group" { "id" "18" "mode" "gyro_to_mouse" "settings" { "gyro_natural_sensitivity" "20" } }
 "group" { "id" "12" "mode" "mouse_joystick" }
 "preset" { "id" "0" "group_source_bindings" { "18" "gyro active" "12" "right_trackpad active" } } }"##;
        let l = layout_from_vdf(t, "russian");
        assert_eq!(l.name, "Рус");
        assert_eq!(fmt_val(&val_of(&l)), "0.2x");
        assert_eq!(mode_label("mouse_joystick"), "Как джойстик");
        assert_eq!(mode_label("gyro_to_mouse"), "Гироскоп как мышь");
        assert!(l.author.is_empty());
        let r = rows(Some(&l));
        // gyro mode, natural (known), pixels (dash), invert, rpad mode; no calibration row in quick settings
        assert_eq!(r.len(), 5);
        assert!(matches!(&r[2], Row::Slider(v) if v.key == "gyro_pixels" && fmt_val(v) == "—"));
        assert!(matches!(&r[1], Row::Slider(v) if (v.v - 0.2).abs() < 1e-6));
    }
    fn val_of(l: &Layout) -> Val {
        l.gyro.as_ref().unwrap().natural.clone().unwrap()
    }
    #[test]
    fn zelda_values() {
        let t = r##""controller_mappings" { "title" "x" "group" { "id" "18" "mode" "gyro_to_mouse" "settings" { "gyro_natural_sensitivity" "60" "haptic_intensity" "1" } }
 "group" { "id" "12" "mode" "mouse_joystick" }
 "preset" { "id" "0" "group_source_bindings" { "18" "gyro active" "12" "right_trackpad active" } } }"##;
        let l = layout_from_vdf(t, "russian");
        assert_eq!(fmt_val(&val_of(&l)), "0.6x");
        assert_eq!(l.gyro.as_ref().unwrap().mode, "Гироскоп как мышь");
        assert_eq!(l.rpad.as_ref().unwrap().mode, "Как джойстик");
        // points per 360 is not stored in this file: shown as a dash, never a default
        assert_eq!(fmt_val(l.gyro.as_ref().unwrap().pixels.as_ref().unwrap()), "—");
        assert_eq!(l.gyro.as_ref().unwrap().invert, Some(false));
    }
    #[test]
    fn unreadable_layout_has_no_quick_settings_rows() {
        assert!(rows(None).is_empty());
    }
    #[test]
    fn source_without_active_group_shows_not_selected() {
        // Stock root (quick-settings/09): gyro and right-trackpad rows stay, with «Не выбрано» when unset.
        let t = r##""controller_mappings" { "title" "x" "group" { "id" "12" "mode" "mouse_joystick" } "preset" { "id" "0" "group_source_bindings" { } } }"##;
        let l = layout_from_vdf(t, "russian");
        assert_eq!(mode_label(""), "Не выбрано");
        let r = rows(Some(&l));
        assert_eq!(r.len(), 2);
        assert!(matches!(&r[0], Row::Mode { src: "gyro", value, .. } if value == "Не выбрано"));
        assert!(matches!(&r[1], Row::Mode { src: "rpad", value, .. } if value == "Не выбрано"));
    }
    #[test]
    fn rows_follow_supported_keys() {
        let l = Layout {
            grips: Some(false),
            gyro: Some(Src { mode: "Гироскоп как мышь".into(), natural: Some(val("gyro_natural", 0.6, Unit::Mult)), invert: Some(false), ..Default::default() }),
            rpad: Some(Src { mode: "Как джойстик".into(), invert: Some(false), ..Default::default() }),
            ..Default::default()
        };
        let r = rows(Some(&l));
        // grips, gyro mode, natural sens, gyro invert, rpad mode, rpad invert: no pixels row, no rstick rows
        assert_eq!(r.len(), 6);
        assert!(matches!(r[0], Row::Grips(false)));
        assert!(matches!(r[5], Row::Toggle { src: "rpad", .. }));
        assert!(!r.iter().any(|x| matches!(x, Row::Slider(v) if v.key == "gyro_pixels")));
    }
}

#[cfg(test)]
mod x360_connection_tests {
    use super::*;
    #[test]
    fn export_suggests_names_like_stock() {
        let own = |url: &str, title: &str| ActiveLayout { url: url.into(), title: title.into(), author_name: String::new(), published_file_id: None, selection_type: 1, enabled: true };
        assert_eq!(export_default_name(Some(&own("autosave://u/1/a.vdf", "Мой вариант"))), "Мой вариант");
        assert_eq!(export_default_name(Some(&own("autosave://u/1/a.vdf", "$$$autosave"))), "");
        assert_eq!(export_default_name(Some(&own("autosave://u/1/a.vdf", "#Title_Token"))), "");
        assert_eq!(export_default_name(Some(&own("template://a.vdf", "Шаблон"))), "");
        assert_eq!(export_default_name(None), "");
    }

    #[test]
    fn ignores_steam_virtual_pad_and_detects_physical_x360() {
        assert!(!is_physical_x360(std::path::Path::new("/sys/devices/virtual/input/input32"), "Microsoft X-Box 360 pad 1"));
        assert!(is_physical_x360(std::path::Path::new("/sys/devices/pci/usb/input/input9"), "Microsoft X-Box 360 pad"));
        assert!(is_physical_x360(std::path::Path::new("/sys/devices/virtual/input/input9"), "Microsoft X-Box 360 pad (vpad)"));
        assert!(!is_physical_x360(std::path::Path::new("/sys/devices/pci/usb/input/input7"), "Valve Software Steam Deck Controller"));
        assert_eq!(available_focus(F_X360_TAB, false), F_CARD);
        assert_eq!(available_focus(F_X360_TAB, true), F_X360_TAB);
    }
}

#[cfg(test)]
mod layout_menu_focus_tests {
    use super::*;
    #[test]
    fn skips_disabled_revert_and_stays_within_menu() {
        assert_eq!(menu_move(0, false), 0);
        assert_eq!(menu_move(2, true), 4);
        assert_eq!(menu_move(4, false), 2);
        assert_eq!(menu_move(5, true), 5);
    }
}

// ---- Quick settings of the root: live rows, real writes -------------------------------------------------------

/// Rows come from the live editing copy once it has been read; until then (or without Steam) the local VDF rows stay.
fn rebuild_rows(s: &mut St) {
    if let Some((_, raw)) = &s.quick {
        s.rows = quick::rows(raw, s.tab == 0).into_iter().map(Row::Live).collect();
        let last = n_items(s) - 1;
        s.foc = s.foc.min(last);
    }
}

/// Read the editing copy of the current layout (the same `StartEditing…` the stock page performs on entering).
fn request_quick(_a: &mut App) {
    request_quick_after_stop(None);
}

fn request_quick_after_stop(old_index: Option<u32>) {
    let (id, tab) = {
        let mut s = st();
        if !s.open || s.editor.is_some() || s.editor_busy { return; }
        s.quick_error = None;
        (s.app_id, s.tab)
    };
    let Some(bridge) = crate::actions::bridge() else {
        st().quick_error = Some("Steam недоступен".into());
        return;
    };
    st().editor_busy = true;
    // The root and editor share the native editing slot for this controller.
    std::thread::spawn(move || {
        let result = (|| {
            let stopped = old_index.map(|index| bridge.eval_json_once(&format!(
                "SteamClient.Input.StopEditingControllerConfiguration({index})"
            )));
            if let Some(Err(error)) = stopped {
                // An unplugged device can disappear before its editing session is closed.
                eprintln!("[controller] Stop: {error}");
            }
            let index = selected_controller_index(&bridge, tab)?;
            bridge.editor_start(id, index).map(|(raw, _)| (index, raw))
        })();
        if let Err(e) = &result { eprintln!("[controller] quick settings: {e}"); }
        super::defer(move |a| {
            let mut s = st();
            s.editor_busy = false;
            s.root_controller_index = result.as_ref().ok().map(|(index, _)| *index);
            match result {
                Ok(q) => { s.quick = Some(q); s.quick_error = None; rebuild_rows(&mut s); }
                Err(error) => s.quick_error = Some(format!("Не удалось прочитать раскладку: {error}")),
            }
            drop(s);
            request_root_active(id);
            sync(a);
        });
    });
}

#[derive(Clone, Copy)]
enum QuickWrite { Setting { source: i64, key: i64, value: i64 }, Mode { source: i64, mode: i64, shift: bool } }

fn focused_quick_slider(s: &St) -> Option<(i64, &pad::PRow)> {
    match s.rows.get(usize::try_from(s.foc - F_ROW0).ok()?)? {
        Row::Live(quick::QRow { kind: quick::Kind::Slider(row), action: quick::Action::Adjust(source), .. }) =>
            Some((*source, row)),
        _ => None,
    }
}

fn handle_quick_slider(a: &mut App, act: Action) {
    let mut s = st();
    let Some(value) = s.quick_slider else { return };
    let Some((source, row)) = focused_quick_slider(&s) else {
        s.quick_slider = None;
        drop(s);
        sync(a);
        return;
    };
    match act {
        Action::A | Action::B => {
            let changed = row.value != value;
            let key = row.key;
            s.quick_slider = None;
            drop(s);
            if changed { quick_write(a, QuickWrite::Setting { source, key, value }); }
        }
        Action::Left | Action::Right => {
            let mut row = row.clone();
            row.value = value;
            let next = row.step(if act == Action::Left { -1 } else { 1 }, false);
            s.quick_slider = Some(next);
            say(match next.cmp(&value) {
                std::cmp::Ordering::Less => crate::sound::Snd::SliderDown,
                std::cmp::Ordering::Greater => crate::sound::Snd::SliderUp,
                std::cmp::Ordering::Equal => crate::sound::Snd::FailedNav,
            });
            drop(s);
        }
        _ => return,
    }
    sync(a);
}

fn quick_activate(a: &mut App, row: &quick::QRow) {
    match row.action {
        quick::Action::Shift(source) => return pad_open(a, source, None, true),
        quick::Action::Mode(source) if st().quick_gear => return pad_open(a, source, None, false),
        _ => {}
    }
    let raw = st().quick.as_ref().map(|q| q.1.clone());
    let Some(raw) = raw else { a.toast("Раскладка Steam ещё не прочитана"); return };
    match quick::activate(&raw, row) {
        quick::Plan::Adjust => {
            if let quick::Kind::Slider(setting) = &row.kind {
                st().quick_slider = Some(setting.value);
            }
        }
        quick::Plan::Refuse(message) => { say(crate::sound::Snd::FailedNav); a.toast(message) }
        quick::Plan::OpenList(source) => {
            let current = quick::source_group(&raw, source).map(|g| g.0).unwrap_or(0);
            let cursor = quick::list_modes(source, true, current).iter().position(|m| *m == current).unwrap_or(0);
            st().quick_dialog = Some((source, cursor));
        }
        quick::Plan::Setting { source, key, value } => {
            if row.kind == quick::Kind::Toggle { say(if value != 0 { crate::sound::Snd::ToggleOn } else { crate::sound::Snd::ToggleOff }); }
            return quick_write(a, QuickWrite::Setting { source, key, value });
        }
    }
    sync(a);
}

fn mode_dialog_current(s: &St, source: i64) -> i64 {
    if s.mode_list_editor {
        s.editing.as_ref().map_or(0, |(_, raw, _)| quick::mode_in_set(raw, s.editor_set_index, source))
    } else {
        s.quick.as_ref().map_or(0, |(_, raw)| quick::mode_in_set(raw, 0, source))
    }
}

fn root_shift_available(s: &St, source: i64) -> bool {
    !s.mode_list_editor && s.quick.as_ref().is_some_and(|(_, raw)| quick::can_create_shift(raw, source))
}

fn quick_dialog_handle(a: &mut App, act: Action) {
    let Some((source, cursor)) = st().quick_dialog else { return };
    let (editor, current_mode, set_index, can_create) = {
        let s = st();
        (s.mode_list_editor, mode_dialog_current(&s, source), s.editor_set_index, root_shift_available(&s, source))
    };
    let choices = quick::list_modes(source, !editor, current_mode);
    let mode_count = choices.len();
    let create_count = if editor { quick::create_entries(source).len() } else { 0 };
    let cancel = mode_count + create_count + usize::from(can_create);
    match act {
        Action::B => { let mut s = st(); s.quick_dialog = None; s.mode_list_editor = false; s.vmenu_name = None; }
        Action::Up => st().quick_dialog = Some((source, cursor.saturating_sub(1))),
        Action::Down => st().quick_dialog = Some((source, (cursor + 1).min(cancel))),
        Action::A => {
            { let mut s = st(); s.quick_dialog = None; s.mode_list_editor = false; }
            sync(a);
            if source == quick::VMENU {
                let name = st().vmenu_name.take();
                if let (Some(&mode), Some(name)) = (choices.get(cursor), name) { create_virtual_menu(a, set_index, mode, name); }
                return;
            }
            if let Some(&mode) = choices.get(cursor) {
                if mode == current_mode { return; }
                return if editor { write_editor_mode(a, set_index, source, mode, false) } else { quick_write(a, QuickWrite::Mode { source, mode, shift: false }) };
            }
            if cursor < mode_count + create_count {
                a.toast("Создание этого режима пока недоступно: вызов Steam не подтверждён на Деке");
                return;
            }
            if can_create && cursor == mode_count + create_count {
                return quick_write(a, QuickWrite::Mode { source, mode: quick::initial_shift_mode(current_mode), shift: true });
            }
            return;
        }
        _ => {}
    }
    sync(a);
}

/// Stock «Добавить виртуальное меню»: one `SetControllerSourceMode` with `new_virtual_menu` into the editing copy.
/// The reply must be a different layout that lists the new menu; otherwise the editor is marked uncertain.
fn create_virtual_menu(a: &mut App, set_index: usize, mode: i64, name: String) {
    if fixture_catalog() || !live::writes_allowed(crate::actions::dryrun(), std::env::var("DECKSHELL_TEST_CONTROLLER_WRITE").as_deref() == Ok("1")) {
        st().editor_status = "Режим теста: создание виртуального меню отключено".into();
        sync(a);
        return;
    }
    let Some(bridge) = crate::actions::bridge() else { st().editor_status = "Steam недоступен".into(); sync(a); return };
    let (idx, raw) = {
        let mut s = st();
        if s.editor_busy { return; }
        let Some((idx, raw, _)) = s.editing.clone() else { return };
        s.editor_busy = true;
        s.editor_status = "Создание виртуального меню…".into();
        (idx, raw)
    };
    sync(a);
    std::thread::spawn(move || {
        let result = (|| {
            let target = quick::mode_target_at(&raw, set_index, steam_bridge::controller_edit::VIRTUAL_MENU_SOURCE)?;
            let (reply, _) = bridge.editor_create_virtual_menu(idx, &target.action_set_key, target.action_set_layer_key.as_deref(), mode, &name)?;
            if reply == raw { return Err(steam_bridge::Error::Protocol("Steam не изменил раскладку".into())); }
            let rows = live::rows(&reply)?;
            Ok::<_, steam_bridge::Error>((reply, rows))
        })();
        super::defer(move |a| {
            let mut s = st();
            s.editor_busy = false;
            match result {
                Ok((reply, rows)) => {
                    s.editing = Some((idx, reply, rows));
                    s.editor_dirty = true;
                    s.editor_undo.clear();
                    s.editor_status = "Виртуальное меню создано · Menu — сохранить".into();
                }
                Err(err) => { s.editor_uncertain = true; s.editor_status = format!("Ошибка создания виртуального меню: {err}"); }
            }
            drop(s);
            sync(a);
        });
    });
}

/// Mode of a source in the full editor: write into the editing copy, read the reply back, and restore the previous
/// mode when Steam returned something else. The layout is saved later by the editor's own Menu → Save.
fn write_editor_mode(a: &mut App, set_index: usize, source: i64, mode: i64, undo: bool) {
    if fixture_catalog() || !live::writes_allowed(crate::actions::dryrun(), std::env::var("DECKSHELL_TEST_CONTROLLER_WRITE").as_deref() == Ok("1")) {
        st().editor_status = "Режим теста: запись режима источника отключена".into();
        sync(a);
        return;
    }
    let Some(bridge) = crate::actions::bridge() else { st().editor_status = "Steam недоступен".into(); sync(a); return };
    let (idx, raw) = {
        let mut s = st();
        if s.editor_busy { return; }
        let Some((idx, raw, _)) = s.editing.clone() else { return };
        if !undo { s.editor_undo.push(sticks::Undo::SourceMode(set_index, source, quick::mode_in_set(&raw, set_index, source))); }
        s.editor_busy = true;
        s.editor_status = "Запись режима источника…".into();
        (idx, raw)
    };
    sync(a);
    std::thread::spawn(move || {
        let outcome: steam_bridge::Result<ModeWrite> = Ok(match bind::write_mode(&bind::Live(&bridge, idx), &raw, set_index, source, mode) {
            bind::Outcome::Done(r) => live::rows(&r).map(|rows| ModeWrite::Done(r, rows)).unwrap_or_else(|e| ModeWrite::Unsure(e.to_string())),
            bind::Outcome::RolledBack(why, r) => live::rows(&r).map(|rows| ModeWrite::RolledBack(why.clone(), r, rows)).unwrap_or_else(|_| ModeWrite::Unsure(why)),
            bind::Outcome::Unsure(why) => ModeWrite::Unsure(why),
        });
        super::defer(move |a| {
            let mut s = st();
            s.editor_busy = false;
            match outcome {
                Ok(ModeWrite::Done(raw, rows)) => {
                    s.editing = Some((idx, raw, rows));
                    s.editor_dirty = true;
                    if undo { s.editor_undo.pop(); }
                    s.editor_status = if undo { "Режим источника восстановлен и прочитан из Steam".into() } else { "Режим источника записан и прочитан из Steam · X — отменить".into() };
                }
                Ok(ModeWrite::RolledBack(why, raw, rows)) => {
                    s.editing = Some((idx, raw, rows));
                    if !undo { s.editor_undo.pop(); }
                    s.editor_status = format!("Режим источника не изменён: {why}; прежний режим восстановлен и подтверждён");
                }
                Ok(ModeWrite::Unsure(why)) => {
                    s.editor_uncertain = true;
                    s.editor_status = format!("Ошибка режима источника: {why}; прежний режим восстановить не удалось · откройте раскладку заново");
                }
                Err(e) => {
                    s.editor_uncertain = true;
                    s.editor_status = format!("Ошибка режима источника: {e} · откройте раскладку заново");
                }
            }
            drop(s);
            sync(a);
        });
    });
}

enum ModeWrite { Done(Vec<u8>, Vec<live::BindingRow>), RolledBack(String, Vec<u8>, Vec<live::BindingRow>), Unsure(String) }

// ---- Страница настроек источника (кадры controls/05–11) ----------------------------------------------------------

fn pad_slider_focused(p: &Pad, rows: &[pad::PRow]) -> bool {
    !p.uncertain && !p.rail && p.choice.is_none() && rows.get(p.focus).is_some_and(|r| r.kind == pad::Kind::Slider)
}

fn pad_view_height(p: &Pad, rows: &[pad::PRow]) -> f32 {
    if pad_slider_focused(p, rows) { 626. } else { 678. }
}

fn pad_data<'a>(s: &'a St, p: &Pad) -> Option<(u32, &'a [u8], steam_bridge::controller_edit::BindingTarget)> {
    let (index, raw) = if p.editor_set.is_some() {
        s.editing.as_ref().map(|e| (e.0, e.1.as_slice()))?
    } else {
        s.quick.as_ref().map(|e| (e.0, e.1.as_slice()))?
    };
    let target = quick::target_at(raw, p.editor_set.unwrap_or(0), p.source, p.shift).ok()?;
    Some((index, raw, target))
}

fn pad_rows(s: &St) -> Vec<pad::PRow> {
    s.pad.as_ref().and_then(|p| pad_data(s, p)).map(|(_, raw, t)| pad::rows(raw, &t)).unwrap_or_default()
}

fn pad_content(rows: &[pad::PRow]) -> f32 { rows.last().map(|r| r.y + r.h + 20.).unwrap_or(0.) }

fn pad_open(a: &mut App, source: i64, editor_set: Option<usize>, shift: bool) {
    let mut s = st();
    if s.editor_busy { return; }
    let mut p = Pad { source, editor_set, shift, ..Default::default() };
    let rows = pad_data(&s, &p).map(|(_, raw, target)| pad::rows(raw, &target)).unwrap_or_default();
    let Some(first) = rows.iter().position(pad::PRow::focusable) else {
        drop(s);
        a.toast("Steam не вернул параметры для этого режима");
        return;
    };
    p.focus = first;
    s.pad = Some(p);
    drop(s);
    sync(a);
}

/// The root saves its own editing copy. In the full editor the existing Save/Undo workflow applies.
fn pad_close(a: &mut App) {
    let mut s = st();
    let Some(p) = s.pad.as_ref() else { return };
    if !p.dirty || p.editor_set.is_some() || p.uncertain {
        let refresh = p.uncertain && p.editor_set.is_none();
        s.pad = None;
        if refresh { s.quick = None; }
        drop(s);
        if refresh { request_quick_after_stop(None); }
        sync(a);
        return;
    }
    let Some(idx) = s.quick.as_ref().map(|(idx, _)| *idx) else { return };
    let app = s.app_id;
    let url = s.root_active.as_ref().map(|v| v.url.clone()).unwrap_or_default();
    drop(s);
    let Some(bridge) = crate::actions::bridge() else { a.toast("Steam недоступен"); return };
    st().editor_busy = true;
    sync(a);
    std::thread::spawn(move || {
        let result = bridge.editor_save_personal(app, idx, &url);
        super::defer(move |a| {
            let mut s = st();
            s.editor_busy = false;
            if result.is_ok() { s.pad = None; }
            drop(s);
            match result {
                Ok(()) => request_root_active(app),
                Err(e) => a.toast(&format!("Не удалось сохранить раскладку: {e}")),
            }
            sync(a);
        });
    });
}

fn pad_write(a: &mut App, row: &pad::PRow, value: i64) {
    if value == row.value { return; }
    if fixture_catalog() || !live::writes_allowed(crate::actions::dryrun(), std::env::var("DECKSHELL_TEST_CONTROLLER_WRITE").as_deref() == Ok("1")) {
        a.toast("Пробный запуск: запись настроек отключена");
        return;
    }
    let request = {
        let s = st();
        if s.editor_busy || s.editor_uncertain { return; }
        s.pad.as_ref().and_then(|p| pad_data(&s, p).map(|(idx, raw, t)| (idx, raw.to_vec(), t, p.editor_set.is_some(),
            s.root_active.as_ref().map(|active| active.url.clone()).unwrap_or_default())))
    };
    let Some((idx, raw, target, editor, url)) = request else { return };
    if !editor && steam_bridge::controller_list::check_personal_url(&url).is_err() {
        a.toast("Для сохранения настроек выберите личную раскладку");
        return;
    }
    let Some(bridge) = crate::actions::bridge() else { a.toast("Steam недоступен"); return };
    let (key, mask, old_value) = (row.key, row.uses_mask(), row.value);
    st().editor_busy = true;
    sync(a);
    std::thread::spawn(move || {
        let result = (|| {
            let reply = if key == pad::SHIFT_BUTTONS {
                let mut buttons = pad::read_shift_buttons(&raw, &target)?;
                if let Some(i) = buttons.iter().position(|&key| key == value) { buttons.remove(i); } else { buttons.push(value); }
                buttons.sort_unstable();
                let (reply, _) = bridge.editor_set_mode_shift_buttons(idx, &target, &buttons)?;
                let mut confirmed = match quick::target_at(&reply, 0, target.source_binding_key, true) {
                    Ok(reply_target) => pad::read_shift_buttons(&reply, &reply_target)?,
                    Err(_) => Vec::new(),
                };
                confirmed.sort_unstable();
                if confirmed != buttons {
                    return Err(steam_bridge::Error::Protocol("Steam не подтвердил кнопки сменного режима".into()));
                }
                reply
            } else {
                write_source_value(&bridge, idx, &raw, &target, key, value, mask)?
            };
            let bindings = if editor { live::rows(&reply)? } else { Vec::new() };
            Ok((reply, bindings))
        })();
        super::defer(move |a| {
            let mut s = st();
            s.editor_busy = false;
            match result {
                Ok((reply, bindings)) => {
                    if editor {
                        s.editing = Some((idx, reply, bindings));
                        s.editor_dirty = true;
                        s.editor_status = "Настройка изменена · Menu — сохранить · X — отменить".into();
                        s.editor_undo.push(sticks::Undo::SourceSetting(target, key, old_value, mask));
                    } else {
                        s.quick = Some((idx, reply));
                        rebuild_rows(&mut s);
                    }
                    let rows = pad_rows(&s);
                    if let Some(p) = s.pad.as_mut() {
                        p.dirty = true;
                        if let Some(focus) = rows.iter().position(|r| !r.heading && r.key == key) {
                            p.focus = focus;
                            p.scroll = pad::scroll_for(&rows, focus, p.scroll, pad_view_height(p, &rows), pad_content(&rows));
                        }
                    }
                }
                Err(e) => {
                    if editor { s.editor_uncertain = true; }
                    if let Some(p) = s.pad.as_mut() { p.uncertain = true; }
                    drop(s);
                    a.toast(&format!("Steam не подтвердил настройку: {e}. Выйдите со страницы и откройте её снова."));
                    sync(a);
                    return;
                }
            }
            drop(s);
            sync(a);
        });
    });
}

const PAD_NUMBER_SINK: super::osk::ExtSink = super::osk::ExtSink {
    id: 14, mode: super::osk::InputMode::Numeric,
    typed: pad_number_type, enter: pad_number_enter, cancel: pad_number_cancel,
};

fn pad_number_type(a: &mut App, c: Option<char>) {
    if let Some((value, replace)) = st().pad_number.as_mut() {
        if *replace { value.clear(); *replace = false; }
        match c {
            Some(c) if value.len() < 24 => value.push(c),
            None => { value.pop(); }
            _ => {}
        }
    }
    sync(a);
}

fn pad_number_cancel(a: &mut App) {
    st().pad_number = None;
    sync(a);
}

pub fn pad_number_active() -> bool { st().pad_number.is_some() }

fn pad_number_enter(a: &mut App) {
    let request = {
        let s = st();
        s.pad_number.as_ref().zip(s.pad.as_ref()).and_then(|((text, _), p)| {
            let row = pad_rows(&s).get(p.focus)?.clone();
            row.parse_numeric(text).map(|value| (row, value))
        })
    };
    let Some((row, value)) = request else {
        a.toast("Введите число в пределах ползунка");
        super::osk::attach(a, PAD_NUMBER_SINK);
        return;
    };
    super::osk::detach(a, PAD_NUMBER_SINK);
    st().pad_number = None;
    sync(a);
    pad_write(a, &row, value);
}

fn pad_handle(a: &mut App, act: Action) {
    if st().pad_number.is_some() { a.osk_handle(act); return; }
    if st().editor_busy { return; }
    let rows = pad_rows(&st());
    let focusable = pad::focusable(&rows);
    let Some(mut p) = st().pad.clone() else { return };
    if p.uncertain {
        if act == Action::B { pad_close(a); }
        return;
    }
    let content = pad_content(&rows);
    // Открытый список значений: ↑/↓, A выбирает, B закрывает.
    if let Some(cursor) = p.choice {
        let row = rows.get(p.focus);
        let options = row.map(|r| r.options()).unwrap_or_default();
        let mut write = None;
        match act {
            Action::B => p.choice = None,
            Action::Up => p.choice = Some(cursor.saturating_sub(1)),
            Action::Down => p.choice = Some((cursor + 1).min(options.len())),
            Action::A => { p.choice = None; if let (Some(o), Some(r)) = (options.get(cursor), row) { write = Some((r.clone(), o.0)); } }
            _ => {}
        }
        st().pad = Some(p);
        sync(a);
        if let Some((row, value)) = write { pad_write(a, &row, value); }
        return;
    }
    if st().editor_busy { return; }
    let pos = focusable.iter().position(|&i| i == p.focus).unwrap_or(0);
    let row = rows.get(p.focus).cloned();
    let sections = pad::present_sections(&rows);
    let section_pos = sections.iter().position(|&x| x == pad::active_section(&rows, p.focus)).unwrap_or(0);
    let go = |p: &mut Pad, to: usize| { p.focus = to; };
    let mut write = None;
    match act {
        Action::B => { if p.slider { p.slider = false; } else { return pad_close(a); } }
        Action::Up | Action::Down => {
            p.slider = false;
            let down = act == Action::Down;
            if p.rail {
                let next = if down { (section_pos + 1).min(sections.len().saturating_sub(1)) } else { section_pos.saturating_sub(1) };
                if let Some(to) = sections.get(next).and_then(|&sec| pad::first_row_of(&rows, sec)) { go(&mut p, to); }
            } else {
                let next = if down { (pos + 1).min(focusable.len().saturating_sub(1)) } else { pos.saturating_sub(1) };
                if let Some(&to) = focusable.get(next) { go(&mut p, to); }
            }
        }
        Action::Left | Action::Right => {
            let dir = if act == Action::Left { -1 } else { 1 };
            match &row {
                Some(r) if p.slider && r.kind == pad::Kind::Slider => {
                    let next = r.step(dir, p.fine);
                    // Стоковый Slider: SliderUp/SliderDown при изменении, FailedNav на пределе (sp.js:277843).
                    say(if next == r.value { crate::sound::Snd::FailedNav } else if dir > 0 { crate::sound::Snd::SliderUp } else { crate::sound::Snd::SliderDown });
                    write = Some((r.clone(), next));
                }
                _ if act == Action::Left => p.rail = true,
                _ => p.rail = false,
            }
        }
        Action::A => {
            if p.rail { p.rail = false; }
            else if let Some(r) = &row {
                match r.kind {
                    pad::Kind::Slider => p.slider = !p.slider,
                    pad::Kind::Toggle => {
                        say(if r.value == 0 { crate::sound::Snd::ToggleOn } else { crate::sound::Snd::ToggleOff });
                        write = Some((r.clone(), 1 - r.value));
                    }
                    pad::Kind::Choice => p.choice = Some(r.options().iter().position(|o| o.0 == r.value).unwrap_or(0)),
                }
            }
        }
        Action::Y if pad_slider_focused(&p, &rows) => p.fine = !p.fine,
        Action::Menu if pad_slider_focused(&p, &rows) => {
            if let Some(row) = row {
                st().pad_number = Some((row.numeric_text(), true));
                super::osk::attach(a, PAD_NUMBER_SINK);
                return sync(a);
            }
        }
        _ => {}
    }
    p.scroll = pad::scroll_for(&rows, p.focus, p.scroll, pad_view_height(&p, &rows), content);
    st().pad = Some(p);
    sync(a);
    if let Some((row, value)) = write { pad_write(a, &row, value); }
}

/// Список значений в стоковом виде: строки + «Отмена» под чертой; (модель, ширина, высота).
fn choice_model(rows: Vec<(String, i32)>, cancel: String) -> (Vec<ChoiceItem>, f32, f32) {
    let mut y = 0.0f32;
    let mut width = 420.0f32;
    let mut items: Vec<ChoiceItem> = Vec::new();
    let total = rows.len();
    for (i, (label, icon)) in rows.into_iter().chain(std::iter::once((cancel, 0))).enumerate() {
        if i == total { y += 3.0; }
        let h = if icon > 0 { 90.0 } else { 72.0 };
        width = width.max((if icon > 0 { 78.0 } else { 21.0 }) + label.chars().count() as f32 * 14.06 + 21.0);
        let image = if icon > 0 {
            crate::assets::CONTROLLER_MODE_ICONS.get(icon as usize - 1).and_then(|name| crate::assets::controller_icon(name)).unwrap_or_default()
        } else { Default::default() };
        items.push(ChoiceItem { label: label.into(), icon, image, y: y.into(), h: h.into() });
        y += h;
    }
    (items, width.min(1000.0), y)
}

fn sync_choice(g: &ControllerState, list: Option<(Vec<(String, i32)>, usize)>) {
    g.set_choice_open(list.is_some());
    if let Some((rows, focus)) = list {
        let (items, w, h) = choice_model(rows, "Отмена".into());
        g.set_choice_items(ModelRc::new(VecModel::from(items)));
        g.set_choice_focus(focus as i32);
        g.set_choice_width(w.into());
        g.set_choice_height(h.into());
    }
}

fn sync_pad(g: &ControllerState, s: &St) {
    // Список режимов источника с корня (кадры 03, 04): режимы, «Создать сменный режим», «Отмена».
    let mut choice = None;
    if let Some((source, cursor)) = s.quick_dialog {
        let mut rows = quick::list_rows(source, !s.mode_list_editor, mode_dialog_current(s, source));
        if root_shift_available(s, source) {
            rows.push((t_or("#ControllerBinding_ModeShift_Create", "Создать сменный режим"), 0));
        }
        choice = Some((rows, cursor));
    }
    g.set_pad_open(s.pad.is_some());
    g.set_pad_two_line(s.pad.as_ref().is_some_and(|p| s.pad_number.is_none() && pad_slider_focused(p, &pad_rows(s))));
    g.set_pad_number_open(s.pad_number.is_some());
    if let Some((value, _)) = &s.pad_number { g.set_pad_number(value.clone().into()); }
    if let Some(p) = &s.pad {
        let rows = pad_rows(s);
        let content = pad_content(&rows);
        let sections = pad::present_sections(&rows);
        let active = pad::active_section(&rows, p.focus);
        g.set_pad_sections(ModelRc::new(VecModel::from(sections.iter().filter_map(|i| rows.iter().find(|r| r.heading && r.section == *i).map(|r| r.label.clone().into())).collect::<Vec<slint::SharedString>>())));
        g.set_pad_section(sections.iter().position(|&x| x == active).unwrap_or(0) as i32);
        g.set_pad_focus(if p.rail { -1 } else { p.focus as i32 });
        g.set_pad_rail(p.rail);
        g.set_pad_slider(p.slider);
        g.set_pad_scroll(p.scroll.into());
        g.set_pad_content_height(content.into());
        let mode = pad_data(s, p).map_or(0, |(_, _, target)| target.source_mode);
        g.set_pad_source(pages::source_title(p.source).to_uppercase().into());
        g.set_pad_subtitle(t_or("#ControllerConfigurator_SourceMode_Group_Settings", "Настройки %1$s").replace("%1$s", &quick::mode_name(p.source, mode)).into());
        g.set_pad_rows(ModelRc::new(VecModel::from(rows.iter().map(|r| PadRow {
            kind: if r.heading { 3 } else { match r.kind { pad::Kind::Slider => 0, pad::Kind::Toggle => 1, pad::Kind::Choice => 2 } },
            label_lines: ModelRc::new(VecModel::from(if r.heading { vec![r.label.clone().into()] } else { r.label_text.iter().map(|l| l.clone().into()).collect::<Vec<slint::SharedString>>() })),
            desc_lines: ModelRc::new(VecModel::from(r.desc_text.iter().map(|l| l.clone().into()).collect::<Vec<slint::SharedString>>())),
            value: r.text.clone().into(), fraction: r.fraction(), on: r.on(), y: r.y.into(), h: r.h.into(),
            label_cy: pad::label_center(r).into(), desc_top: pad::desc_top(r).into(), box_w: pad::dropdown_width(&r.text).into(),
        }).collect::<Vec<_>>())));
        if let (Some(cursor), Some(r)) = (p.choice, rows.get(p.focus)) {
            choice = Some((r.options().iter().map(|o| (o.1.clone(), 0)).collect(), cursor));
        }
    }
    sync_choice(g, choice);
}

/// One native write into the editing copy, read-back check, then save of the personal layout.
/// Only personal layouts are written: a template or workshop layout is never saved over.
fn quick_write(a: &mut App, write: QuickWrite) {
    if fixture_catalog() || !live::writes_allowed(crate::actions::dryrun(), std::env::var("DECKSHELL_TEST_CONTROLLER_WRITE").as_deref() == Ok("1")) {
        a.toast("Пробный запуск: запись быстрых настроек отключена");
        return;
    }
    let (app, idx, raw, url) = {
        let s = st();
        if s.editor_busy { return; }
        let Some((idx, raw)) = s.quick.clone() else { return };
        (s.app_id, idx, raw, s.root_active.as_ref().map(|v| v.url.clone()).unwrap_or_default())
    };
    if steam_bridge::controller_list::check_personal_url(&url).is_err() {
        a.toast("Раскладка не личная. Откройте ⚙ → «Переключиться на уникальную конфигурацию…» или выберите свою, затем меняйте быстрые настройки");
        return;
    }
    let Some(bridge) = crate::actions::bridge() else { a.toast("Steam недоступен"); return };
    st().editor_busy = true;
    sync(a);
    std::thread::spawn(move || {
        let result = (|| -> steam_bridge::Result<Vec<u8>> {
            let (new_raw, ok) = match write {
                QuickWrite::Setting { source, key, value, .. } => {
                    let target = quick::target(&raw, source)?;
                    let (r, _) = bridge.editor_set_source_setting(idx, &raw, &target, key, value)?;
                    let ok = quick::target(&r, source).and_then(|t| sticks::value(&r, &t, key)).map(|v| v == value).unwrap_or(false);
                    (r, ok)
                }
                QuickWrite::Mode { source, mode, shift } => {
                    let mut target = quick::mode_target(&raw, source)?;
                    target.mode_shift = shift;
                    let (r, _) = bridge.editor_set_source_mode(idx, &target, mode)?;
                    let ok = if shift {
                        quick::target_at(&r, 0, source, true).is_ok_and(|t| t.source_mode == mode)
                            && quick::source_group(&r, source) == quick::source_group(&raw, source)
                    } else { quick::source_group(&r, source).map_or(0, |g| g.0) == mode };
                    (r, ok)
                }
            };
            if !ok { return Err(steam_bridge::Error::Protocol("Steam вернул другое значение".into())); }
            // Ползунки страницы настроек сохраняются при выходе со страницы, а не на каждый шаг.
            bridge.editor_save_personal(app, idx, &url)?;
            Ok(new_raw)
        })();
        super::defer(move |a| {
            let mut s = st();
            s.editor_busy = false;
            let failed = match result {
                Ok(raw) => { if s.open && s.app_id == app { s.quick = Some((idx, raw)); rebuild_rows(&mut s); } None }
                Err(e) => Some(e.to_string()),
            };
            drop(s);
            if let Some(e) = failed {
                a.toast(&format!("Не удалось изменить быстрые настройки: {e}"));
                request_quick_after_stop(None);
            } else {
                request_root_active(app);
            }
            sync(a);
        });
    });
}

fn start_editor(_a: &mut App) {
    let (id, tab) = {
        let mut s = st();
        if s.editor_busy { return; }
        s.editor = Some(EditorFocus::new(8));
        s.editor_busy = true;
        s.editor_status = "Чтение раскладки Steam…".into();
        s.editing = None;
        s.editor_gyro_target = None;
        s.editor_undo.clear();
        s.editor_dirty = false;
        s.editor_uncertain = false;
        s.editor_set_index = 0;
        (s.app_id, s.tab)
    };
    let Some(b) = crate::actions::bridge() else {
        st().editor_busy = false;
        st().editor_status = "Steam недоступен".into();
        return;
    };
    std::thread::spawn(move || {
        let result = (|| {
            let idx = selected_controller_index(&b, tab)?;
            let target = if crate::actions::dryrun() { None } else {
                controller_devices::request(&b, serde_json::json!({"op": "gyro-target", "index": idx}), false)
                    .ok().and_then(|value| serde_json::from_value::<Identity>(value).ok())
            };
            let (raw, _) = b.editor_start(id, idx)?;
            let rows = live::rows(&raw)?;
            Ok::<_, steam_bridge::Error>((idx, raw, rows, target))
        })();
        super::defer(move |a| {
            let mut s = st();
            if s.app_id != id || s.editor.is_none() { return; }
            s.editor_busy = false;
            match result {
                Ok((idx, raw, rows, target)) => {
                    s.editor_status = "Раскладка прочитана · изменения в редактируемой копии".into();
                    s.editor_gyro_target = target;
                    s.editing = Some((idx, raw, rows));
                }
                Err(e) => s.editor_status = format!("Ошибка чтения: {e}"),
            };
            drop(s);
            sync(a);
        });
    });
}

fn start_gyro_calibration(a: &mut App) {
    if fixture_catalog() || crate::actions::dryrun() {
        a.toast("Пробный запуск · калибровка отключена");
        return;
    }
    let target = st().editor_gyro_target.clone();
    let Some(target) = target else {
        a.toast("Гироскоп выбранного контроллера недоступен. Откройте редактор заново.");
        return;
    };
    controller_gyro::open(a, Owner::Editor, target);
}

fn save_personal_editor(a: &mut App) {
    if fixture_catalog() || crate::actions::dryrun() { a.toast("Пробный запуск: сохранение отключено"); return; }
    let request = {
        let s = st();
        if s.editor_busy { return; }
        if s.editor_uncertain {
            drop(s);
            a.toast("Steam не подтвердил состояние редактора. Откройте раскладку заново.");
            return;
        }
        if !s.editor_dirty { return; }
        s.editing.as_ref().and_then(|(index, raw, _)| steam_bridge::controller_layout::parse_layout(raw).ok().map(|layout| (s.app_id, *index, layout.url)))
    };
    let Some((app, index, url)) = request else { return; };
    if steam_bridge::controller_list::check_personal_url(&url).is_err() {
        a.toast("Сохранять можно только свою раскладку. Сначала создайте личную копию.");
        return;
    }
    let Some(bridge) = crate::actions::bridge() else { a.toast("Steam недоступен"); return; };
    st().editor_busy = true;
    st().editor_status = "Сохранение личной раскладки…".into();
    sync(a);
    std::thread::spawn(move || {
        let result = bridge.editor_save_personal(app, index, &url);
        super::defer(move |a| {
            let mut s = st();
            s.editor_busy = false;
            s.editor_status = match result {
                Ok(()) => { s.editor_dirty = false; s.editor_undo.clear(); "Steam принял сохранение личной раскладки".into() }
                Err(err) => { s.editor_uncertain = true; format!("Сохранение не подтверждено: {err}") },
            };
            drop(s);
            request_root_active(app);
            sync(a);
        });
    });
}

const ACTION_SET_SINK: super::osk::ExtSink = super::osk::ExtSink {
    id: 9, mode: super::osk::InputMode::Normal, typed: type_action_set_name,
    enter: commit_action_set_name, cancel: cancel_action_set_name,
};

fn open_action_set_name(a: &mut App, rename: Option<usize>) {
    if fixture_catalog() || crate::actions::dryrun() { a.toast("Пробный запуск: запись наборов отключена"); return; }
    let prompt = {
        let s = st();
        s.editing.as_ref().and_then(|(_, raw, _)| {
            let sets = live::msg(raw, 8).ok()?;
            let set = sets.get(rename.unwrap_or(s.editor_set_index))?;
            let key = live::string(set, 1).ok()?;
            if key.is_empty() { return None; }
            if rename.is_some() { Some((key, None, String::new())) } else { Some((String::new(), Some(key), String::new())) }
        })
    };
    if prompt.is_none() { a.toast("Набор действий недоступен"); return; }
    st().editor_set_prompt = prompt;
    super::osk::attach(a, ACTION_SET_SINK);
    type_action_set_name(a, None);
}

/// Marker key of `editor_set_prompt` for the virtual menu name dialog (stock `CreateVirtualMenuDialog`).
const VIRTUAL_MENU_PROMPT: &str = "\u{1}virtual-menu";

fn open_virtual_menu_name(a: &mut App) {
    if fixture_catalog() || crate::actions::dryrun() { a.toast("Пробный запуск: создание виртуального меню отключено"); return; }
    if st().editing.is_none() { a.toast("Раскладка ещё читается"); return; }
    st().editor_set_prompt = Some((VIRTUAL_MENU_PROMPT.into(), None, String::new()));
    super::osk::attach(a, ACTION_SET_SINK);
    type_action_set_name(a, None);
}

fn type_action_set_name(a: &mut App, character: Option<char>) {
    let mut s = st();
    let Some((key, source, name)) = s.editor_set_prompt.as_mut() else { return; };
    let virtual_menu = key == VIRTUAL_MENU_PROMPT;
    match character {
        Some(value) if !value.is_control() && name.chars().count() < 80 => name.push(value),
        Some(_) => (),
        None => { name.pop(); }
    }
    s.editor_status = if virtual_menu { format!("{}: {name}", t_or("#ControllerConfigurator_CreateVirtualMenuDialog_Title_1", "Имя нового виртуального меню")) } else { format!("{}: {name}", if source.is_some() { "Имя нового набора (копия текущего)" } else { "Новое имя набора" }) };
    drop(s);
    sync(a);
}

fn cancel_action_set_name(a: &mut App) {
    st().editor_set_prompt = None;
    st().editor_status = "Имя набора не изменено".into();
    super::osk::detach(a, ACTION_SET_SINK);
    sync(a);
}

fn commit_action_set_name(a: &mut App) {
    let request = {
        let mut s = st();
        s.editor_set_prompt.take().zip(s.editing.as_ref().map(|(index, raw, _)| (*index, raw.clone())))
    };
    super::osk::detach(a, ACTION_SET_SINK);
    let Some(((key, source, name), (index, raw))) = request else { return; };
    let name = name.trim().to_owned();
    if name.is_empty() { cancel_action_set_name(a); return; }
    if key == VIRTUAL_MENU_PROMPT {
        // Stock: after the name, a «Создать» menu with the three styles; nothing is written before the choice.
        let mut s = st();
        s.vmenu_name = Some(name);
        s.quick_dialog = Some((quick::VMENU, 0));
        s.mode_list_editor = true;
        s.editor_status.clear();
        drop(s);
        sync(a);
        return;
    }
    let Some(bridge) = crate::actions::bridge() else { a.toast("Steam недоступен"); return; };
    st().editor_busy = true;
    st().editor_status = "Изменение набора действий…".into();
    sync(a);
    std::thread::spawn(move || {
        let result = (|| {
            let previous = steam_bridge::controller_layout::parse_layout(&raw)?;
            let (raw, layout) = bridge.editor_set_action_set(index, &key, &name, source.as_deref())?;
            let selected = layout.action_sets.iter().position(|set| set.display_name == name && if source.is_some() {
                !set.key.is_empty() && !previous.action_sets.iter().any(|old| old.key == set.key)
            } else { set.key == key }).ok_or_else(|| steam_bridge::Error::Protocol("Steam не подтвердил изменение набора".into()))?;
            let rows = live::rows(&raw)?;
            Ok::<_, steam_bridge::Error>((raw, rows, selected))
        })();
        super::defer(move |a| {
            let mut s = st();
            s.editor_busy = false;
            match result {
                Ok((raw, rows, selected)) => {
                    s.editing = Some((index, raw, rows));
                    s.editor_set_index = selected;
                    s.editor_dirty = true;
                    s.editor_undo.clear();
                    s.editor_status = "Набор изменён в редакторе · Menu — сохранить".into();
                }
                Err(err) => { s.editor_uncertain = true; s.editor_status = format!("Ошибка изменения набора: {err}"); }
            }
            drop(s);
            sync(a);
        });
    });
}

fn editor_binding(s: &St) -> Option<&live::BindingRow> {
    let focus = s.editor.as_ref()?;
    let (_, _, rows) = s.editing.as_ref()?;
    page_items_of(s, focus.section).into_iter().filter(|item| item.kind != pages::Kind::Header)
        .nth(focus.row)?.binding.and_then(|index| rows.get(index))
}
/// Target of a command on a row without one (append to the next free activator).
fn empty_target_of(s:&St,it:&pages::Item)->steam_bridge::Result<steam_bridge::controller_edit::BindingTarget> {
    let (_,raw,_)=s.editing.as_ref().ok_or_else(||steam_bridge::Error::Protocol("Раскладка ещё читается".into()))?;
    let input=it.input.ok_or_else(||steam_bridge::Error::Protocol("У строки нет входа".into()))?;
    quick::empty_input_target(raw,s.editor_set_index,input,it.input_source)
}
/// Target of the focused row: the existing command's, or the append target of an empty row.
fn editor_target(s:&St)->Option<steam_bridge::controller_edit::BindingTarget> {
    let focus=s.editor.as_ref()?;
    let item=page_items_of(s,focus.section).into_iter().filter(|item|item.kind!=pages::Kind::Header).nth(focus.row)?;
    match item.binding {
        Some(index)=>s.editing.as_ref()?.2.get(index).map(|r|r.target.clone()),
        None=>empty_target_of(s,&item).ok(),
    }
}
fn apply_picker(a:&mut App) {
    let request={let s=st();if s.editor_busy{return} let Some(target)=editor_target(&s) else{return};
        picker_binding(&s,s.picker_cursors[s.picker_tab]).map(|binding|(target,binding)) };
    if let Some((target,binding))=request {
        if let Some((7,action))=live::identity(&binding) {
            let result={let s=st();live::CommandDialog::new(&s.editing.as_ref().unwrap().1,target.clone(),action)};
            match result {Ok(Some(dialog))=>{st().command_dialog=Some(dialog);sync(a);return},Err(e)=>{st().editor_status=e.to_string();sync(a);return},_=>()}
        }
        write_editor(a,target,binding,false)
    }
}
fn focused_swap_pair(s: &St) -> Option<(i64, i64)> {
    let focus = s.editor.as_ref().filter(|f| f.column != Column::Rail)?;
    let (_, raw, _) = s.editing.as_ref()?;
    let source = page_items_of(s, focus.section).into_iter().filter(|item| item.kind != pages::Kind::Header)
        .nth(focus.row)?.source?;
    let pair = match source {
        quick::LTRIGGER | quick::RTRIGGER => (quick::LTRIGGER, quick::RTRIGGER),
        quick::LSTICK | quick::RSTICK => (quick::LSTICK, quick::RSTICK),
        quick::LPAD | quick::RPAD => (quick::LPAD, quick::RPAD),
        _ => return None,
    };
    let left = quick::group_in_set(raw, s.editor_set_index, pair.0);
    let right = quick::group_in_set(raw, s.editor_set_index, pair.1);
    let supported = |source, group: Option<(i64, i64)>| group.is_none_or(|(mode, _)| quick::mode_choices(source).contains(&mode));
    (left != right && supported(pair.0, right) && supported(pair.1, left)).then_some(pair)
}

fn swap_sources(a: &mut App, set: usize, left: i64, right: i64, undo: bool) {
    if fixture_catalog() || crate::actions::dryrun() {
        st().editor_status = "Пробный запуск: обмен источников отключён".into();
        sync(a);
        return;
    }
    let Some(bridge) = crate::actions::bridge() else { a.toast("Steam недоступен"); return; };
    let (index, raw, action_set) = {
        let mut s = st();
        if s.editor_busy || s.editor_uncertain { return; }
        let Some((index, raw, _)) = s.editing.as_ref() else { return; };
        let Some(action_set) = live::msg(raw, 8).ok().and_then(|sets| live::string(sets.get(set)?, 1).ok()) else { return; };
        let payload = (*index, raw.clone(), action_set);
        s.editor_busy = true;
        s.editor_status = "Обмен источников…".into();
        payload
    };
    sync(a);
    std::thread::spawn(move || {
        let result = bridge.editor_swap_sources(index, &action_set, left, right).and_then(|reply| {
            let expected_left = quick::group_in_set(&raw, set, right);
            let expected_right = quick::group_in_set(&raw, set, left);
            if quick::group_in_set(&reply, set, left) != expected_left || quick::group_in_set(&reply, set, right) != expected_right {
                return Err(steam_bridge::Error::Protocol("Steam не подтвердил обмен обеих групп".into()));
            }
            let rows = live::rows(&reply)?;
            Ok((reply, rows))
        });
        super::defer(move |a| {
            let mut s = st();
            s.editor_busy = false;
            match result {
                Ok((raw, rows)) => {
                    s.editing = Some((index, raw, rows));
                    s.editor_dirty = true;
                    if undo { s.editor_undo.pop(); }
                    else { s.editor_undo.push(sticks::Undo::SwapSources { set, left, right }); }
                    s.editor_status = if undo { "Обмен отменён" } else { "Источники поменялись местами · X — отменить" }.into();
                }
                Err(error) => {
                    s.editor_uncertain = true;
                    s.editor_status = format!("Обмен не подтверждён: {error} · откройте раскладку заново");
                }
            }
            drop(s);
            sync(a);
        });
    });
}

fn undo_editor(a:&mut App) {
    let item={let s=st();if s.editor_busy{return}s.editor_undo.last().cloned()};
    match item {Some(sticks::Undo::Binding(target,binding))=>write_editor(a,target,binding,true),Some(sticks::Undo::Setting(target,key,value))=>write_stick(a,target,key,value,true), Some(sticks::Undo::SourceSetting(target,key,value,mask))=>undo_source_setting(a,target,key,value,mask), Some(sticks::Undo::SourceMode(set,source,mode))=>write_editor_mode(a,set,source,mode,true), Some(sticks::Undo::SwapSources { set, left, right }) => swap_sources(a, set, left, right, true), Some(sticks::Undo::Remove(target))=>remove_editor_binding(a,target), Some(sticks::Undo::Activation(target,old))=>write_activation(a,target,old,true), None=>() }
}
fn write_source_value(bridge: &steam_bridge::Bridge, index: u32, raw: &[u8], target: &steam_bridge::controller_edit::BindingTarget,
    key: i64, value: i64, mask: bool) -> steam_bridge::Result<Vec<u8>> {
    let (reply, _) = if mask { bridge.editor_set_source_mask(index, raw, target, key, value)? }
        else { bridge.editor_set_source_setting(index, raw, target, key, value)? };
    if pad::read_value(&reply, target, key, mask) != Some(value) {
        return Err(steam_bridge::Error::Protocol("Steam вернул другое значение".into()));
    }
    Ok(reply)
}

fn undo_source_setting(a: &mut App, target: steam_bridge::controller_edit::BindingTarget, key: i64, value: i64, mask: bool) {
    if fixture_catalog() || crate::actions::dryrun() { a.toast("Пробный запуск: запись настроек отключена"); return; }
    let Some(bridge) = crate::actions::bridge() else { a.toast("Steam недоступен"); return };
    let Some((index, raw, _)) = st().editing.clone() else { return };
    st().editor_busy = true;
    sync(a);
    std::thread::spawn(move || {
        let result = (|| -> steam_bridge::Result<_> {
            let reply = write_source_value(&bridge, index, &raw, &target, key, value, mask)?;
            let rows = live::rows(&reply)?;
            Ok((reply, rows))
        })();
        super::defer(move |a| {
            let mut s = st();
            s.editor_busy = false;
            match result {
                Ok((raw, rows)) => {
                    s.editing = Some((index, raw, rows));
                    s.editor_undo.pop();
                    s.editor_status = "Настройка восстановлена".into();
                }
                Err(e) => {
                    s.editor_uncertain = true;
                    s.editor_status = e.to_string();
                }
            }
            drop(s);
            sync(a);
        });
    });
}
fn write_editor(a:&mut App,target:steam_bridge::controller_edit::BindingTarget,binding:Vec<u8>,undo:bool) {
    if fixture_catalog() || !live::writes_allowed(crate::actions::dryrun(), std::env::var("DECKSHELL_TEST_CONTROLLER_WRITE").as_deref()==Ok("1")) {st().editor_status="Режим теста: запись привязок отключена".into();sync(a);return;}
    let Some(b)=crate::actions::bridge() else{a.toast("Steam недоступен");return};
    let append=target.binding_index<0;
    let (idx,raw)={let mut s=st();if s.editor_busy{return}let Some((idx,raw,_))=&s.editing else{return};let v=(*idx,raw.clone());
        if !undo {
            if append {let mut first=target.clone();first.binding_index=0;s.editor_undo.push(sticks::Undo::Remove(first));}
            else {match live::binding_at(&v.1,&target) {Ok(old)=>s.editor_undo.push(sticks::Undo::Binding(target.clone(),old)),Err(e)=>{s.editor_status=e.to_string();return}}}
        }
        s.editor_busy=true;s.editor_status=if undo{"Отмена…"}else{"Применение…"}.into();v};
    sync(a);
    std::thread::spawn(move || {
        let outcome=bind::write_binding(&bind::Live(&b,idx),&raw,&target,&binding);
        let rows=match &outcome {bind::Outcome::Done(r)|bind::Outcome::RolledBack(_,r)=>live::rows(r).ok(),_=>None};
        super::defer(move|a| {let mut s=st();s.editor_busy=false;match (outcome,rows) {
            (bind::Outcome::Done(raw),Some(rows))=>{s.editing=Some((idx,raw,rows));s.editor_dirty=true;if undo{s.editor_undo.pop();}s.editor_status=if undo{"Изменение отменено · исходная команда прочитана из Steam"}else{"Команда изменена в редакторе · Menu — сохранить · X — отменить"}.into();if let Some(f)=s.editor.as_mut(){f.picker_open=false;}}
            (bind::Outcome::RolledBack(why,raw),Some(rows))=>{s.editing=Some((idx,raw,rows));if !undo{s.editor_undo.pop();}s.editor_status=format!("Команда не изменена: {why}; прежнее состояние восстановлено и подтверждено");if let Some(f)=s.editor.as_mut(){f.picker_open=false;}}
            (bind::Outcome::Unsure(why),_)=>{s.editor_uncertain=true;s.editor_status=format!("Ошибка применения: {why}; прежнее состояние восстановить не удалось · откройте раскладку заново");}
            _=>{s.editor_uncertain=true;s.editor_status="Ошибка применения: ответ Steam не удалось прочитать · откройте раскладку заново".into();}
        };drop(s);sync(a)});
    });
}
/// Undo of a command added in this session: remove it and the activator it created, confirmed by readback.
fn remove_editor_binding(a:&mut App,target:steam_bridge::controller_edit::BindingTarget) {
    if fixture_catalog() || !live::writes_allowed(crate::actions::dryrun(), std::env::var("DECKSHELL_TEST_CONTROLLER_WRITE").as_deref()==Ok("1")) {st().editor_status="Режим теста: запись привязок отключена".into();sync(a);return;}
    let Some(b)=crate::actions::bridge() else{a.toast("Steam недоступен");return};
    let (idx,raw)={let mut s=st();if s.editor_busy{return}let Some((idx,raw,_))=&s.editing else{return};let v=(*idx,raw.clone());s.editor_busy=true;s.editor_status="Отмена…".into();v};
    sync(a);
    std::thread::spawn(move || {
        let result=bind::remove_new_binding(&bind::Live(&b,idx),&raw,&target).map_err(|e|e.to_string()).and_then(|raw|live::rows(&raw).map(|rows|(raw,rows)).map_err(|e|e.to_string()));
        super::defer(move|a|{let mut s=st();s.editor_busy=false;match result {
            Ok((raw,rows))=>{s.editing=Some((idx,raw,rows));s.editor_dirty=true;s.editor_undo.pop();s.editor_status="Добавленная команда удалена · пустой вход прочитан из Steam".into();}
            Err(e)=>{s.editor_uncertain=true;s.editor_status=format!("Ошибка отмены: {e} · откройте раскладку заново");}
        };drop(s);sync(a)});
    });
}
/// «Способ включения» of the open command's activator; same request path as everything else: write, readback, rollback.
fn write_activation(a:&mut App,target:steam_bridge::controller_edit::BindingTarget,activation:i64,undo:bool) {
    if fixture_catalog() || !live::writes_allowed(crate::actions::dryrun(), std::env::var("DECKSHELL_TEST_CONTROLLER_WRITE").as_deref()==Ok("1")) {st().editor_status="Режим теста: запись способа включения отключена".into();sync(a);return;}
    let Some(b)=crate::actions::bridge() else{a.toast("Steam недоступен");return};
    let (idx,raw)={let mut s=st();if s.editor_busy{return}let Some((idx,raw,_))=&s.editing else{return};let v=(*idx,raw.clone());
        if !undo {let Some(old)=live::activation_at(&v.1,&target) else{s.editor_status="Активатор не найден в копии раскладки".into();drop(s);sync(a);return};if old==activation{return}s.editor_undo.push(sticks::Undo::Activation(target.clone(),old));}
        s.editor_busy=true;s.editor_status="Запись способа включения…".into();v};
    sync(a);
    std::thread::spawn(move || {
        let outcome=bind::write_activation(&bind::Live(&b,idx),&raw,&target,activation);
        let rows=match &outcome {bind::Outcome::Done(r)|bind::Outcome::RolledBack(_,r)=>live::rows(r).ok(),_=>None};
        super::defer(move|a|{let mut s=st();s.editor_busy=false;match (outcome,rows) {
            (bind::Outcome::Done(raw),Some(rows))=>{s.editing=Some((idx,raw,rows));s.editor_dirty=true;if undo{s.editor_undo.pop();}s.editor_status=if undo{"Способ включения восстановлен и прочитан из Steam"}else{"Способ включения записан и прочитан из Steam · X — отменить"}.into();}
            (bind::Outcome::RolledBack(why,raw),Some(rows))=>{s.editing=Some((idx,raw,rows));if !undo{s.editor_undo.pop();}s.editor_status=format!("Способ включения не изменён: {why}; прежний восстановлен и подтверждён");}
            (bind::Outcome::Unsure(why),_)=>{s.editor_uncertain=true;s.editor_status=format!("Ошибка способа включения: {why}; прежний восстановить не удалось · откройте раскладку заново");}
            _=>{s.editor_uncertain=true;s.editor_status="Ошибка способа включения: ответ Steam не удалось прочитать · откройте раскладку заново".into();}
        };drop(s);sync(a);});
    });
}
fn stop_editor(a:&mut App) {
    let (idx, dirty)={let mut s=st();if s.editor_busy{return}s.editor=None;s.command_dialog=None;s.editor_set_prompt=None;s.vmenu_name=None;s.editor_undo.clear();let idx=s.editing.take().map(|e|e.0);s.editor_busy=idx.is_some();(idx,s.editor_dirty && !s.editor_uncertain)};
    super::osk::detach(a, ACTION_SET_SINK);
    if dirty { a.toast("Изменения не сохранены"); }
    if let (Some(idx),Some(b))=(idx,crate::actions::bridge()) {std::thread::spawn(move||{if let Err(e)=b.eval_json_once(&format!("SteamClient.Input.StopEditingControllerConfiguration({idx})")){eprintln!("[controller-editor] Stop: {e}");}super::defer(|a|{st().editor_busy=false;request_quick_after_stop(None);sync(a)});});}else{st().editor_busy=false;}
    sync(a);
}
fn command_dialog_handle(a:&mut App,act:Action) {
    let mut s=st();let Some(d)=s.command_dialog.as_mut() else{return};
    match act {
        Action::B=>s.command_dialog=None,
        Action::Up=>d.cursor=d.cursor.saturating_sub(1),
        Action::Down=>d.cursor=(d.cursor+1).min(d.parameters.len()),
        Action::Left|Action::Right=>{if let Some(p)=d.parameters.get_mut(d.cursor){p.step(if act==Action::Left{-1}else{1});}},
        Action::A if d.cursor==d.parameters.len()=>{let d=s.command_dialog.take().unwrap();drop(s);write_editor(a,d.target.clone(),d.binding(),false);return;},
        Action::A=>{if let Some(p)=d.parameters.get_mut(d.cursor){p.step(if p.value==p.max{-p.max}else{1});}},
        _=>(),
    };drop(s);sync(a);
}
fn picker_game_actions(s:&St)->Vec<(String,Vec<u8>)>{
    s.editing.as_ref().zip(editor_target(s)).and_then(|(editing,target)|live::game_actions(&editing.1,&target).ok()).unwrap_or_default()
}
fn picker_tabs(s:&St)->Vec<usize>{let mut tabs=Vec::new();if !picker_game_actions(s).is_empty(){tabs.push(7)}tabs.extend(0..7);tabs}
fn picker_keys(s:&St)->Vec<editor::Key>{if s.picker_tab==7 {picker_game_actions(s).into_iter().enumerate().map(|(i,(label,_))|editor::key(&label,60.+(i%4) as f32*300.,150.+(i/4) as f32*85.,280.,75.)).collect()}else{editor::picker_keys(s.picker_tab)}}
fn picker_binding(s:&St,i:usize)->Option<Vec<u8>>{if s.picker_tab==7{picker_game_actions(s).get(i).map(|r|r.1.clone())}else{editor::picker_command(s.picker_tab,i).map(|(kind,field,value)|live::command(kind,field,value))}}

fn step_stick(a:&mut App,dir:i64) {
    let request={let s=st();if s.editor_busy{return;}let Some(f)=&s.stick_settings else{return;};
        let p=stick_parameters(&s,f.section).get(f.row).cloned();
        let source=s.editing.as_ref().and_then(|e|sticks::sticks(&e.1).ok()).and_then(|v|v.get(s.stick_index).cloned());
        source.zip(p).map(|(source,p)|(source.target,p.key,p.next(dir),p.value))};
    if let Some((target,key,value,old))=request {
        say(if value==old{crate::sound::Snd::FailedNav}else if dir>0{crate::sound::Snd::SliderUp}else{crate::sound::Snd::SliderDown});
        if value!=old{write_stick(a,target,key,value,false);}
    }
}
fn write_stick(a:&mut App,target:steam_bridge::controller_edit::BindingTarget,key:i64,value:i64,undo:bool) {
    if fixture_catalog() || !live::writes_allowed(crate::actions::dryrun(),std::env::var("DECKSHELL_TEST_CONTROLLER_WRITE").as_deref()==Ok("1")) {st().editor_status="Режим теста: запись параметров отключена".into();sync(a);return;}
    let Some(b)=crate::actions::bridge() else {st().editor_status="Steam недоступен".into();sync(a);return;};
    let (idx,raw)={let mut s=st();if s.editor_busy{return;}let Some((idx,raw,_))=&s.editing else{return;};let v=(*idx,raw.clone());
        if !undo {match sticks::value(&v.1,&target,key){Ok(old)=>s.editor_undo.push(sticks::Undo::Setting(target.clone(),key,old)),Err(e)=>{s.editor_status=e.to_string();drop(s);sync(a);return;}}}
        s.editor_busy=true;s.editor_status="Запись параметра…".into();v};sync(a);
    std::thread::spawn(move||{
        let result=(||{let (raw,_)=b.editor_set_source_setting(idx,&raw,&target,key,value)?;
            if sticks::value(&raw,&target,key)?!=value {return Err(steam_bridge::Error::Protocol("Steam вернул другое значение параметра".into()));}
            let rows=live::rows(&raw)?;Ok((raw,rows))})();
        super::defer(move|a|{let mut s=st();s.editor_busy=false;match result {
            Ok((raw,rows))=>{s.editing=Some((idx,raw,rows));s.editor_dirty=true;if undo{s.editor_undo.pop();}s.editor_status=if undo{"Параметр восстановлен и прочитан из Steam"}else{"Параметр записан и прочитан из Steam · X — отменить"}.into();
                if let Some((section,row))=s.stick_settings.as_ref().map(|f|(f.section,f.row)) {let count=stick_parameters(&s,section).len();s.stick_settings.as_mut().unwrap().row=row.min(count.saturating_sub(1));}},
            Err(e)=>{s.editor_uncertain=true;s.editor_status=format!("Ошибка параметра: {e} · X — восстановить исходное значение");},
        };drop(s);sync(a);});
    });
}
