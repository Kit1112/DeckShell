//! «HTTP-прокси → Настроить» (task 599): the stock Steam Deck proxy dialog — a toggle, host and port fields, «Отмена» /
//! «Применить» and «Сбросить». Applying goes through `SteamClient.System.Network.SetProxyInfo` (fire-and-forget setter,
//! only logged under `DECKSHELL_DRYRUN`). Reference: `docs/stock-screens-capture-2026-11-21.md` (`settings-internet-proxy`,
//! initial state: the toggle off, no fields).
use super::*;

/// Applied proxy settings (what the dialog opens with).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProxyCfg { pub enabled: bool, pub host: String, pub port: String }

/// Focus items, top to bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum P { Toggle, Host, Port, Reset, Apply, Cancel }

#[derive(Clone, Debug)]
pub struct ProxyForm {
    pub cfg: ProxyCfg,
    /// what was applied before: «Сбросить» is offered only when something is configured
    pub saved: ProxyCfg,
    pub focus: P,
    /// a rejected value: red line above the title
    pub error: Option<String>,
}

impl ProxyForm {
    pub fn new(saved: ProxyCfg) -> ProxyForm {
        ProxyForm { cfg: saved.clone(), saved, focus: P::Toggle, error: None }
    }
    pub fn items(&self) -> Vec<P> {
        let mut v = vec![P::Toggle];
        if self.cfg.enabled { v.extend([P::Host, P::Port]); }
        if self.saved.enabled || !self.saved.host.is_empty() { v.push(P::Reset); }
        v.extend([P::Apply, P::Cancel]);
        v
    }
    /// The settings to apply, or the field to focus and what is wrong with it.
    pub fn validate(&self) -> Result<ProxyCfg, (P, String)> {
        if !self.cfg.enabled { return Ok(ProxyCfg { enabled: false, ..self.saved.clone() }); }
        let host = self.cfg.host.trim().trim_start_matches("http://").trim_end_matches('/').to_string();
        if host.is_empty() { return Err((P::Host, "Укажите адрес прокси-сервера".into())); }
        if host.chars().any(|c| c.is_whitespace() || c == '/' || c == '@') { return Err((P::Host, "Неверный адрес прокси-сервера".into())); }
        match self.cfg.port.parse::<u32>() {
            Ok(p) if (1..=65535).contains(&p) => Ok(ProxyCfg { enabled: true, host, port: p.to_string() }),
            _ => Err((P::Port, "Порт должен быть числом от 1 до 65535".into())),
        }
    }
}

/// The JS the setter runs: an empty host disables the proxy.
pub fn apply_js(c: &ProxyCfg) -> String {
    if c.enabled {
        format!("SteamClient.System.Network.SetProxyInfo({{ enabled: true, host: {}, port: {} }})", crate::setters::js_str(&c.host), c.port.parse::<u32>().unwrap_or(0))
    } else {
        "SteamClient.System.Network.SetProxyInfo({ enabled: false, host: \"\", port: 0 })".into()
    }
}

pub(super) fn sig(p: &ProxyForm) -> u64 { 7 + (p.focus as u64) * 16 + (p.cfg.enabled as u64) * 4096 + (p.error.is_some() as u64) * 8192 }

pub(super) fn view(p: &ProxyForm, osk: bool, scroll: f32) -> DialogView {
    let mut b = B::new();
    title_block(&mut b, &lt("#Settings_Internet_HTTPProxy", "HTTP-прокси"), p.error.as_deref());
    let items = p.items();
    let mut focus_row = None;
    for (i, it) in items.iter().enumerate() {
        let foc = p.focus == *it;
        let row0 = b.rows.len();
        match it {
            P::Toggle => b.push(DRow { kind: dk::TOGGLE, h: 68.0, label: lt("#Settings_Internet_UseProxy", "Использовать HTTP-прокси"), on: i32::from(p.cfg.enabled), focus: i32::from(foc), item: i as i32, ..Default::default() }, 78.0),
            P::Host => field_row(&mut b, &lt("#Settings_Internet_ProxyHost", "Адрес прокси-сервера"), p.cfg.host.clone(), "", foc, i, None, false),
            P::Port => field_row(&mut b, &lt("#Settings_Internet_ProxyPort", "Порт"), p.cfg.port.clone(), "", foc, i, None, true),
            P::Reset => b.push(DRow { kind: dk::BUTTON_ROW, h: DLG_ROW_H, label: lt("#Settings_Internet_ProxyReset", "Сбросить настройки прокси"), value: lt("#Settings_Internet_ProxyResetButton", "Сбросить"), w2: 240.0, focus: i32::from(foc), item: i as i32, ..Default::default() }, DLG_PITCH),
            P::Apply | P::Cancel => {
                if *it == P::Apply {
                    let pf = match p.focus { P::Apply => 1, P::Cancel => 2, _ => 0 };
                    b.y = b.last_bottom + 24.0;
                    b.push(DRow { kind: dk::PAIR, h: DLG_BTN_H, label: lt("#Button_Cancel", "Отмена"), value: lt("#Button_Apply", "Применить"), on: 1, focus: pf, item: i as i32, ..Default::default() }, DLG_BTN_H);
                }
                if foc { focus_row = Some(b.rows.len() - 1); }
                continue;
            }
        }
        if foc { focus_row = Some(row0); }
    }
    let focus = items.iter().position(|i| *i == p.focus).unwrap_or(0);
    finish(b, items.len(), focus, focus_row, osk, 37.0, scroll)
}

fn set(p: ProxyForm) { with(|s| s.dialog = Some(Dialog::Proxy(p))); }

pub(super) fn open(app: &mut App) {
    let saved = with(|s| s.proxy.clone());
    set_dialog(app, Some(Dialog::Proxy(ProxyForm::new(saved))));
}

pub(super) fn attach(app: &mut App) {
    match with(|s| match &s.dialog { Some(Dialog::Proxy(p)) => Some(p.focus), _ => None }) {
        Some(P::Host) => osk::attach(app, HOST_SINK),
        Some(P::Port) => osk::attach(app, PORT_SINK),
        _ => {}
    }
}

pub(super) const HOST_SINK: ExtSink = ExtSink { id: 14, mode: InputMode::Normal, typed: typed_host, enter: enter_key, cancel: kb_closed };
pub(super) const PORT_SINK: ExtSink = ExtSink { id: 15, mode: InputMode::Numeric, typed: typed_port, enter: enter_key, cancel: kb_closed };

fn typed_host(a: &mut App, c: Option<char>) { edit(a, c, false); }
fn typed_port(a: &mut App, c: Option<char>) { edit(a, c.filter(|c| c.is_ascii_digit()).or(None).filter(|_| c.is_some()), true); }

fn edit(a: &mut App, c: Option<char>, port: bool) {
    with(|s| if let Some(Dialog::Proxy(p)) = &mut s.dialog {
        let (t, max) = match p.focus { P::Host => (&mut p.cfg.host, 253), P::Port => (&mut p.cfg.port, 5), _ => return };
        match c { Some(c) => if t.chars().count() < max && !(port && !c.is_ascii_digit()) { t.push(c) }, None => { t.pop(); } }
        p.error = None;
    });
    push(a);
}

fn kb_closed(a: &mut App) { push(a); }

/// Enter on the keyboard: host → port → apply.
fn enter_key(a: &mut App) {
    a.osk_set_open(false);
    let go = with(|s| match &mut s.dialog {
        Some(Dialog::Proxy(p)) if p.focus == P::Host => { p.focus = P::Port; false }
        _ => true,
    });
    if go { submit(a); } else { attach(a); push(a); }
}

pub(super) fn submit(a: &mut App) {
    let Some(p) = with(|s| match &s.dialog { Some(Dialog::Proxy(p)) => Some(p.clone()), _ => None }) else { return };
    match p.validate() {
        Ok(cfg) => apply(a, cfg),
        Err((field, msg)) => {
            with(|s| if let Some(Dialog::Proxy(p)) = &mut s.dialog { p.focus = field; p.error = Some(msg); });
            push(a);
        }
    }
}

fn apply(a: &mut App, cfg: ProxyCfg) {
    crate::setters::write("set-proxy", apply_js(&cfg));
    with(|s| s.proxy = cfg);
    close_and_refocus(a);
}

pub(super) fn input(app: &mut App, p: ProxyForm, a: Action) {
    let items = p.items();
    let i = items.iter().position(|x| *x == p.focus).unwrap_or(0);
    let mut p = p;
    match a {
        Action::Up | Action::Down => {
            let mut n = dir_step(items.len(), i, a);
            // the button pair is one row
            if a == Action::Up && p.focus == P::Cancel { n = i.saturating_sub(2); }
            if a == Action::Down && p.focus == P::Apply { n = i; }
            p.focus = items[n];
            set(p);
            if app.osk_is_open() { attach(app); }
            push(app);
        }
        Action::Left | Action::Right if matches!(p.focus, P::Apply | P::Cancel) => {
            p.focus = if a == Action::Left { P::Apply } else { P::Cancel };
            set(p);
            push(app);
        }
        Action::A => match p.focus {
            P::Toggle => {
                p.cfg.enabled = !p.cfg.enabled;
                p.error = None;
                crate::sound::play(if p.cfg.enabled { crate::sound::Snd::ToggleOn } else { crate::sound::Snd::ToggleOff });
                set(p);
                push(app);
            }
            P::Host | P::Port => attach(app),
            // reset: clear everything and apply at once
            P::Reset => apply(app, ProxyCfg::default()),
            P::Apply => submit(app),
            P::Cancel => close_and_refocus(app),
        },
        Action::B => close_and_refocus(app),
        _ => {}
    }
}

pub(super) fn click(p: &mut ProxyForm, item: usize, side: i32) {
    let it = p.items();
    if let Some(x) = it.get(item) { p.focus = if side == 1 && *x == P::Apply { P::Cancel } else { *x }; }
}
