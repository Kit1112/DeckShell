//! Settings → «Питание» (task 466). Rebuilds the stock Steam Deck page from its React source (sp.js modules «Settings_Power_*»:
//! `jn` battery section, `hn` idle sections, `ln.dU` HDMI CEC) instead of the captured geometry, which lacked the idle
//! (sleep / screensaver / dim) sections and drew the charge limit as a button.
//!
//! Sections (stock order): «Батарея» (percentage, charge limit + slider), «При работе от батареи и бездействии» /
//! «При работе от сети и бездействии» («При бездействии» without a battery): sleep, screensaver, dim drop-downs,
//! «Завершить загрузки перед переходом в спящий режим», «HDMI CEC». Strings are the pinned Russian Steam bundle
//! (`power_ru.json`). Values come from the live client settings snapshot (`c.*`); nothing here suspends or powers off.
//! Writes use the existing DRYRUN-gated `setters`; the table is in docs/power-page-2026-11-18.md.
use super::*;
use super::text_wrap;
use serde_json::json;

pub const PAGE: &str = "power";
/// Stock geometry in device px (1280x800 captures, `~/.cache/deckshell/next-wave2/power`); `PRow` coordinates are divided by `SC`
/// because the settings renderer multiplies them back (the power page skips the legacy 1.2x reflow).
const CARD_X: f32 = 396.;
const CARD_W: f32 = 849.;
const FIRST_Y: f32 = 167.;
const CARD_GAP: f32 = 9.;
/// text widths: label / description wrap width (card minus the toggle) and the drop-down label wrap width, in px
/// card metrics measured on the stock captures: padding 18, label line pitch 30, description line pitch 23.5 after a 6.5 gap, min height 68
/// (toggle 33 + 2x17.5); the Slint rows (`recording_settings.slint`, kinds 29/30) use the same numbers
pub const CARD_PAD: f32 = 18.;
pub const CARD_MIN_H: f32 = 68.;
pub const LABEL_LINE: f32 = 30.;
pub const DESC_LINE: f32 = 23.5;
pub const DESC_GAP: f32 = 6.5;
const LABEL_W: f32 = 710.;
const DD_LABEL_W: f32 = 420.;
const LABEL_PX: f32 = 24.; // label font size (stock `Settings` field label), DejaVu Sans on the Deck
const DESC_PX: f32 = 18.; // description font size
/// charge limit slider: stock `S()` (sp.js:420123) min = devmode ? charge_limit_min : max(50, charge_limit_min), max = charge_limit_max
pub const LIMIT_FLOOR: i64 = 50;
pub const LIMIT_DEFAULT_MAX: i64 = 100;
pub const LIMIT_DEFAULT_MARK: i64 = 80;
/// idle choices of the stock drop-downs (`B` in sp.js): off, 1, 5, 15, 30 min, 1 h, plus the current value
const IDLE_CHOICES: [i64; 5] = [60, 300, 900, 1800, 3600];

fn ru(key: &str) -> String {
    static TABLE: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    let t = TABLE.get_or_init(|| serde_json::from_str(include_str!("power_ru.json")).unwrap());
    t[key].as_str().unwrap_or("").to_string()
}

/// Stock `Hq(sec, {eSuffix: None, bHighGranularity: true})` (library.js:22848) with the pinned Russian `TimeInterval_*` strings:
/// 1 мин. / 20 мин. / 1 час / 2 ч. / 1 мин. 30 сек. / 1 час и 5 мин.
pub fn idle_label(sec: i64) -> String {
    if sec == 0 { return ru("Settings_System_IdleDisabled"); }
    let (h, m, r) = (sec / 3600, sec % 3600 / 60, sec % 60);
    match sec {
        s if s >= 2 * 86400 => format!("{} дн.", s / 86400),
        s if s >= 86400 => if (s - 86400) >= 7200 { format!("1 день и {} ч.", (s - 86400) / 3600) } else { "1 день".into() },
        s if s >= 7200 => format!("{h} ч."),
        s if s >= 3600 => if s - 3600 >= 120 { format!("1 час и {} мин.", (s - 3600) / 60) } else { "1 час".into() },
        s if s >= 120 => match r { 0 => format!("{m} мин."), 1 => format!("{m} мин. 1 сек."), r => format!("{m} мин. {r} сек.") },
        s if s >= 60 => match r { 0 => "1 мин.".into(), 1 => "1 мин. 1 сек.".into(), r => format!("1 мин. {r} сек.") },
        1 => "1 сек.".into(),
        s => format!("{s} сек."),
    }
}

/// Drop-down key → (client setting key, Steam store write). Dim is not a client setting (`SetIdleBacklightDimSeconds`).
pub const DROPDOWNS: [(&str, &str); 6] = [
    ("sleep_battery", "system_idle_suspend_battery_sec"), ("sleep_ac", "system_idle_suspend_ac_sec"),
    ("saver_battery", "system_idle_screensaver_battery_sec"), ("saver_ac", "system_idle_screensaver_ac_sec"),
    ("dim_battery", "dim_battery"), ("dim_ac", "dim_ac"),
];
/// Toggle token → client setting key.
pub const TOGGLES: [(&str, &str); 8] = [
    ("limit", "steamos_charge_limit_enabled"), ("lowpower", "system_enable_low_power_downloads"),
    ("lowpower_battery", "system_allow_battery_low_power_downloads"), ("cec", "steamos_cec_control"),
    ("cec_wake_device", "steamos_cec_wake_device"), ("cec_suspend_device", "steamos_cec_suspend_device"),
    ("cec_wake_tv", "steamos_cec_wake_tv"), ("cec_suspend_tv", "steamos_cec_suspend_tv"),
];

pub fn dropdown_key(tok: &str) -> Option<&'static str> {
    let k = tok.strip_prefix("power-dd:")?;
    DROPDOWNS.iter().find(|d| d.0 == k).map(|d| d.1)
}
pub fn toggle_key(tok: &str) -> Option<&'static str> {
    let k = tok.strip_prefix("power-tg:")?;
    TOGGLES.iter().find(|d| d.0 == k).map(|d| d.1)
}

/// Where a value lives in the snapshot: client settings `c.<key>`, dim seconds under `power.<key>`.
fn value<'a>(snapshot: &'a Value, key: &str) -> &'a Value {
    if key.starts_with("dim_") { &snapshot["power"][key] } else { &snapshot["c"][key] }
}

/// Number of lines `text` takes in `avail` px at font size `px` (DejaVu advance widths, see `text_wrap`).
pub fn wrap_lines(text: &str, px: f32, avail: f32) -> usize { text_wrap::line_count(text, px, avail) }

/// Texts of a card row with the line breaks decided here (the renderer does not wrap them again): (label, description).
/// Drop-down labels wrap at the label column, toggle cards at the text column; the drop-down value is not wrapped.
pub fn card_texts(r: &PRow) -> (String, String) {
    if r.k == 30 { (text_wrap::wrapped(&r.lab, LABEL_PX, DD_LABEL_W), r.desc.clone()) }
    else { (text_wrap::wrapped(&r.lab, LABEL_PX, LABEL_W), text_wrap::wrapped(&r.desc, DESC_PX, LABEL_W)) }
}

struct Layout { rows: Vec<PRow>, y: f32 }
impl Layout {
    fn push(&mut self, mut r: PRow) {
        // the renderer multiplies by SC again (the power page skips the legacy reflow)
        r.x /= SC; r.y /= SC; r.w /= SC; r.h /= SC;
        self.rows.push(r);
    }
    fn header(&mut self, title: String) {
        self.push(PRow { k: 31, lab: title, x: CARD_X, y: self.y, w: CARD_W, h: 28., ..Default::default() });
        self.y += 40.;
    }
    /// Stock toggle card: label (+ description) on the left, switch on the right; focusable as a whole.
    fn toggle(&mut self, tok: &str, title: String, desc: String) {
        let (l, d) = (wrap_lines(&title, LABEL_PX, LABEL_W), wrap_lines(&desc, DESC_PX, LABEL_W));
        let h = CARD_MIN_H.max(2. * CARD_PAD + LABEL_LINE * l as f32 + if d > 0 { DESC_GAP + DESC_LINE * d as f32 } else { 0. });
        self.push(PRow { k: 29, tok: format!("power-tg:{tok}"), lab: title, desc, full: l.to_string(), x: CARD_X, y: self.y, w: CARD_W, h, f: true,
            role: "switch".into(), pad: "A,FOCUS,touch,mouse".into(), ..Default::default() });
        self.y += h + CARD_GAP;
    }
    /// Stock drop-down card: label on the left, 375x60 button with the current value on the right.
    fn dropdown(&mut self, tok: &str, title: String, current: String) {
        let l = wrap_lines(&title, LABEL_PX, DD_LABEL_W);
        self.push(PRow { k: 30, tok: format!("power-dd:{tok}"), lab: title, desc: current, full: l.to_string(), x: CARD_X, y: self.y, w: CARD_W, h: 96., f: true,
            role: "combobox".into(), pad: "A,FOCUS,touch,mouse".into(), ..Default::default() });
        self.y += 96. + CARD_GAP;
    }
    fn gap(&mut self) { self.y += 40.; }
}

/// What the running device offers, read from the snapshot. A missing snapshot (no bridge) keeps the Deck defaults.
/// A boolean client setting. Steam's settings object omits a field that holds the proto default, so on a live snapshot (`c` is an object)
/// a missing flag is «off»; an absent snapshot (bridge down / not yet read) stays unknown. Anything but a bool/missing is unknown.
fn flag(s: &Value, key: &str) -> Option<bool> {
    match &s["c"][key] { Value::Bool(b) => Some(*b), Value::Null if s["c"].is_object() => Some(false), _ => None }
}
fn has_battery(s: &Value) -> bool { s["power"]["battery"].as_bool().unwrap_or(true) }
/// Stock `TP()`: `is_charge_limit_available` of the SteamOSManager state; an unread flag (bridge down / old service) does not hide the rows.
fn limit_supported(s: &Value) -> bool {
    let available = s["power"]["charge_limit_available"].as_bool();
    available != Some(false) && (s["c"].is_null() || s["c"]["steamos_charge_limit_enabled"].is_boolean() || available == Some(true))
}
fn low_power_supported(s: &Value) -> bool { s["c"].is_null() || s["c"]["system_enable_low_power_downloads"].is_boolean() || s["c"]["system_idle_suspend_ac_sec"].is_number() /* live snapshot that omits a default-off flag */ }
/// Stock `S()`: the slider exists only while «Ограничить предельный заряд» is on.
pub fn slider_visible(s: &Value) -> bool { limit_supported(s) && flag(s, "steamos_charge_limit_enabled") == Some(true) }

/// (min, max, default mark) of the charge limit slider. `power.charge_limit_*` come from the SteamOSManager state (`js::POWER_LIMITS`);
/// with the developer-mode switch (`steamos_charge_limit_devmode`) the full range is allowed. Unread values fall back to 50…100 / 80.
pub fn limit_range(s: &Value) -> (i64, i64, i64) {
    let cmin = s["power"]["charge_limit_min"].as_i64().unwrap_or(LIMIT_FLOOR);
    let min = if s["c"]["steamos_charge_limit_devmode"].as_bool() == Some(true) { cmin } else { cmin.max(LIMIT_FLOOR) };
    let max = s["power"]["charge_limit_max"].as_i64().unwrap_or(LIMIT_DEFAULT_MAX).max(min + 1);
    (min, max, s["power"]["charge_limit_default"].as_i64().unwrap_or(LIMIT_DEFAULT_MARK).clamp(min, max))
}

pub fn rows_for(s: &Value) -> Vec<PRow> {
    let mut l = Layout { rows: vec![], y: FIRST_Y };
    let battery = has_battery(s);
    if battery {
        l.header(ru("Settings_Power_Battery_Title"));
        l.toggle("pct", ru("Settings_Battery_Percentage"), ru("Settings_Battery_Percentage_Desc"));
        if limit_supported(s) {
            l.toggle("limit", ru("Settings_Battery_ChargeLimitEnable"), String::new());
            if slider_visible(s) {
                let limit = s["c"]["steamos_charge_limit"].as_i64();
                l.y += 2.; // stock: the slider card sits 11 px below the switch
                l.push(PRow { k: 27, tok: "power-slider".into(), lab: ru("Settings_Battery_ChargeLimitValue"), desc: limit.map(|v| format!("{v}%")).unwrap_or_else(|| "—".into()),
                    x: CARD_X, y: l.y, w: CARD_W, h: 78., f: true, role: "slider".into(), pad: "A,FOCUS,DPAD,touch,mouse".into(), ..Default::default() });
                l.y += 78. + CARD_GAP;
            }
        }
        l.gap();
    }
    for (ac, section) in [(false, "Settings_System_Idle_OnBattery_Section"), (true, if battery { "Settings_System_Idle_PluggedIn_Section" } else { "Settings_System_Idle_NoBattery_Section" })] {
        if !ac && !battery { continue; }
        l.header(ru(section));
        let sfx = if ac { "ac" } else { "battery" };
        // stock order (screens): dim, screensaver, sleep
        for (what, title) in [("dim", "Settings_System_Idle_Dim_Label"), ("saver", "Settings_System_Idle_Screensaver_Label"), ("sleep", "Settings_System_Idle_Sleep_Label")] {
            let tok = format!("{what}_{sfx}");
            let key = DROPDOWNS.iter().find(|d| d.0 == tok).unwrap().1;
            let shown = value(s, key).as_f64().map(|v| idle_label(v.round() as i64)).unwrap_or_else(|| "Недоступно".into());
            l.dropdown(&tok, ru(title), shown);
        }
        l.gap();
    }
    if low_power_supported(s) {
        l.header(ru("Settings_LowPowerDownloads_Section"));
        l.toggle("lowpower", ru("Settings_LowPowerDownloads_Enable"), ru("Settings_LowPowerDownloads_Details").trim().to_string());
        if battery { l.toggle("lowpower_battery", ru("Settings_LowPowerDownloads_AllowBattery"), ru("Settings_LowPowerDownloads_AllowBatteryDetails")); }
        l.gap();
    }
    l.header(ru("Settings_CEC_Header_Long"));
    l.toggle("cec", ru("Settings_CEC_Control"), String::new());
    let wake_supported = s["power"]["cec_wake_supported"].as_bool().unwrap_or(false);
    l.toggle("cec_wake_device", ru("Settings_CEC_WakeDevice"), if wake_supported { String::new() } else { ru("Settings_CEC_WakeDeviceSupportedDesc") });
    l.toggle("cec_suspend_device", ru("Settings_CEC_SuspendDevice"), String::new());
    l.toggle("cec_wake_tv", ru("Settings_CEC_WakeTV"), String::new());
    l.toggle("cec_suspend_tv", ru("Settings_CEC_SuspendTV"), String::new());
    l.rows
}

/// Control state for the renderer: toggles 0/1, -1 unavailable; combo boxes / slider >= 0 usable, -1 unavailable;
/// the slider returns the fill (0..100) through `slider_fill`.
pub fn available_on(r: &PRow, s: &Value) -> i32 {
    if let Some(key) = toggle_key(&r.tok) {
        let cec_off = key.starts_with("steamos_cec_") && key != "steamos_cec_control" && flag(s, "steamos_cec_control") != Some(true);
        if cec_off { return -1; } // stock: displayOffIfDisabled — shown off, not operable
        if key == "steamos_cec_wake_device" && !s["power"]["cec_wake_supported"].as_bool().unwrap_or(false) { return -1; }
        return match flag(s, key) { Some(true) => 1, Some(false) => 0, None => -1 };
    }
    if r.tok == "power-tg:pct" { return s["power"]["percent"].as_bool().map(i32::from).unwrap_or(-1); }
    if r.tok == "power-slider" { return if flag(s, "steamos_charge_limit_enabled") == Some(true) && s["c"]["steamos_charge_limit"].as_i64().is_some() { 1 } else { -1 }; }
    if let Some(key) = dropdown_key(&r.tok) { return if value(s, key).as_f64().is_some() { 1 } else { -1 }; }
    0
}

pub fn slider_fill(s: &Value) -> String {
    let (min, max, _) = limit_range(s);
    let v = s["c"]["steamos_charge_limit"].as_i64().unwrap_or(min).clamp(min, max);
    ((v - min) as f32 * 100. / (max - min) as f32).to_string()
}

/// Position (0..100) of the stock default-value triangle above the track.
pub fn slider_tick(s: &Value) -> String {
    let (min, max, mark) = limit_range(s);
    ((mark - min) as f32 * 100. / (max - min) as f32).to_string()
}

/// Options of a drop-down: stock list plus the current custom value, sorted.
pub fn options(key: &str, s: &Value) -> Option<Vec<(String, Value)>> {
    let current = value(s, key).as_f64()?.round() as i64;
    let mut v: Vec<i64> = std::iter::once(0).chain(IDLE_CHOICES).chain([current]).collect();
    v.sort_unstable();
    v.dedup();
    Some(v.into_iter().map(|x| (idle_label(x), json!(x))).collect())
}

/// Index of the row to focus after the layout was rebuilt: the same control; if it is gone, the focusable row just above its old spot
/// (the first one when nothing is above), so focus never drops to nothing while a control exists.
pub fn refocus(rows: &[PRow], old_tok: Option<&str>, old_y: f32) -> Option<usize> {
    if let Some(t) = old_tok { if let Some(i) = rows.iter().position(|r| r.f && r.tok == t) { return Some(i); } }
    let focusable = || rows.iter().enumerate().filter(|(_, r)| r.f);
    focusable().filter(|(_, r)| r.y <= old_y).last().or_else(|| focusable().next()).map(|(i, _)| i)
}

impl App {
    /// Applies a value: client setting through the DRYRUN-gated protobuf setter, dim through the display store; the snapshot keeps it.
    fn power_write(&mut self, key: &'static str, v: Value) {
        if key == "percent" {
            // stock page: SetBatteryPreferences({bShowBatteryPercentage}) (sp.js:550422); JSON built by the gated port.
            // The switch and the header follow at once; the stored value is read back and the last write settles the state.
            use std::sync::atomic::Ordering::Relaxed;
            let want = v.as_bool().unwrap_or(false);
            let before = self.se.snapshot["power"][key].clone();
            let gen = PCT_GEN.with(|g| { g.set(g.get() + 1); g.get() });
            crate::backend::PERCENT_WRITES.fetch_add(1, Relaxed);
            crate::backend::set_percent_pref(want);
            self.se.snapshot["power"][key] = v;
            if crate::actions::dryrun() {
                crate::backend::PERCENT_WRITES.fetch_sub(1, Relaxed);
            }
            crate::setters::write_then_read("battery-pref", format!("settingsStore.SetBatteryPreferences({})", battery_logic::pref_stored(want)),
                crate::backend::READ_PERCENT_PREF.into(), move |a, stored| {
                    crate::backend::PERCENT_WRITES.fetch_sub(1, Relaxed);
                    if PCT_GEN.with(|g| g.get()) != gen { return; } // a newer press owns the state
                    a.power_percent_settled(want, battery_logic::pref_show(&stored), before);
                });
        } else if let Some(rest) = key.strip_prefix("dim_") {
            let (ac, sec) = (rest == "ac", v.as_i64().unwrap_or(0));
            crate::setters::idle_dim(ac, sec);
            crate::idle::request_refresh();
            self.se.snapshot["power"][key] = v;
        } else {
            crate::setters::client_setting(key, &v);
            if key.starts_with("system_idle_") { crate::idle::request_refresh(); }
            self.se.snapshot["c"][key] = v;
        }
    }

    /// Read-back of the percentage write: the stored value wins (a failed write rolls the switch and the header back).
    fn power_percent_settled(&mut self, want: bool, stored: Option<bool>, before: Value) {
        let shown = match stored { Some(v) => json!(v), None => before };
        crate::backend::set_percent_pref(shown.as_bool().unwrap_or(false));
        let rolled_back = shown.as_bool() != Some(want);
        self.se.snapshot["power"]["percent"] = shown;
        if self.se.page_id() == PAGE && self.se.recording_menu.is_none() {
            if rolled_back { self.toast("Steam не сохранил настройку — значение возвращено"); }
            self.push_settings();
        }
    }

    /// The preference changed outside the page (Steam, another writer): an open page shows the new value without a reload.
    pub fn power_percent_external(&mut self, shown: bool) {
        use std::sync::atomic::Ordering::Relaxed;
        if self.se.page_id() != PAGE || crate::backend::PERCENT_WRITES.load(Relaxed) > 0 { return; }
        if self.se.snapshot["power"]["percent"].as_bool() == Some(shown) { return; }
        self.se.snapshot["power"]["percent"] = json!(shown);
        if self.se.recording_menu.is_none() { self.push_settings(); }
    }

    /// Debounced charge-limit write (stock slider debounces): flushed 500 ms after the last step, on B and on leaving the row.
    pub(super) fn power_flush(&mut self) {
        if let Some(v) = PENDING_LIMIT.with(|p| p.take()) { self.power_write("steamos_charge_limit", json!(v)); }
    }

    /// Left/Right on the focused slider. Returns true when the key was consumed.
    pub(super) fn power_slider_step(&mut self, a: Action) -> bool {
        let Some(r) = self.se.cf.and_then(|i| self.se.rows.get(i)) else { return false };
        if r.tok != "power-slider" || !matches!(a, Action::Left | Action::Right) { return false; }
        if available_on(r, &self.se.snapshot) < 0 {
            self.toast("Включите ограничение предельного заряда батареи");
            return true;
        }
        let (min, max, _) = limit_range(&self.se.snapshot);
        let cur = self.se.snapshot["c"]["steamos_charge_limit"].as_i64().unwrap_or(max);
        let next = (cur + if a == Action::Right { 1 } else { -1 }).clamp(min, max);
        if next != cur {
            self.se.snapshot["c"]["steamos_charge_limit"] = json!(next);
            PENDING_LIMIT.with(|p| p.set(Some(next)));
            let gen = LIMIT_GEN.with(|g| { g.set(g.get() + 1); g.get() });
            slint::Timer::single_shot(std::time::Duration::from_millis(500), move || {
                crate::with_app(|a| if LIMIT_GEN.with(|g| g.get()) == gen { a.power_flush() });
            });
            self.push_settings();
        }
        true
    }

    /// A / click on a power row. Returns true when handled here.
    pub(super) fn power_activate(&mut self, r: &PRow) -> bool {
        if let Some(index) = r.tok.strip_prefix("recording-option:").and_then(|s| s.parse::<usize>().ok()) {
            let Some((token, values)) = self.se.recording_menu.clone() else { return true };
            self.close_recording_menu();
            if let (Some(key), Some((_, v))) = (dropdown_key(&token), values.get(index)) {
                if value(&self.se.snapshot, key).as_f64().map(|c| c.round() as i64) != v.as_i64() {
                    self.power_write(key, v.clone());
                    self.push_settings();
                }
            }
            return true;
        }
        self.power_flush();
        if let Some(key) = dropdown_key(&r.tok) {
            let Some(values) = options(key, &self.se.snapshot) else {
                self.toast("Не удалось прочитать значение из Steam");
                return true;
            };
            self.header_clear();
            let current = value(&self.se.snapshot, key).as_f64().map(|c| c.round() as i64);
            let selected = values.iter().position(|(_, v)| v.as_i64() == current).unwrap_or(0);
            let start = self.se.rows.len();
            for (i, (label, _)) in values.iter().enumerate() {
                self.se.rows.push(PRow { k: 26, lab: label.clone(), tok: format!("recording-option:{i}"), f: true, pad: "A,touch,mouse".into(), ..Default::default() });
            }
            // stock list ends with «Отмена» after a separator; its index is past the values, so choosing it only closes the list
            self.se.rows.push(PRow { k: 26, lab: ru("Settings_Cancel"), tok: format!("recording-option:{}", values.len()), f: true, pad: "A,touch,mouse".into(), ..Default::default() });
            self.se.power_menu_y = (r.y + r.h / 2. - self.se.vy) * SC;
            self.se.recording_menu = Some((r.tok.clone(), values));
            self.se.cf = Some(start + selected);
            self.push_settings();
            return true;
        }
        if let Some(key) = toggle_key(&r.tok) {
            let state = available_on(r, &self.se.snapshot);
            if state < 0 {
                let msg = if key == "steamos_cec_wake_device" { ru("Settings_CEC_WakeDeviceSupportedDesc") } else if key.starts_with("steamos_cec_") && key != "steamos_cec_control" { "Сначала включите ввод с пульта управления".into() } else { "Настройка недоступна: Steam не вернул её значение".into() };
                self.toast(&msg);
                return true;
            }
            self.power_write(key, json!(state == 0));
            crate::sound::play(if state == 0 { crate::sound::Snd::ToggleOn } else { crate::sound::Snd::ToggleOff });
            self.push_settings();
            return true;
        }
        if r.tok == "power-tg:pct" {
            let state = available_on(r, &self.se.snapshot);
            if state < 0 {
                self.toast("Настройка недоступна: Steam не вернул её значение");
            } else {
                self.power_write("percent", json!(state == 0));
                crate::sound::play(if state == 0 { crate::sound::Snd::ToggleOn } else { crate::sound::Snd::ToggleOff });
                self.push_settings();
            }
            return true;
        }
        r.tok == "power-slider" // A on the slider does nothing: Left/Right change the value
    }
}

thread_local! {
    static PENDING_LIMIT: std::cell::Cell<Option<i64>> = const { std::cell::Cell::new(None) };
    static PCT_GEN: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static LIMIT_GEN: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
#[path = "power_page/tests.rs"]
mod tests;
