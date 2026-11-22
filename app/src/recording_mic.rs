//! Режимы микрофона записи и независимый от UI сторож PipeWire/PulseAudio.
use std::{collections::BTreeMap, fs, io, path::PathBuf, process::{Command, Stdio}, sync::{Mutex, OnceLock}, time::{Duration, Instant, SystemTime}};
use serde_json::{json, Value};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config { pub mode: String, pub button: String }
impl Default for Config { fn default() -> Self { Self { mode: "always".into(), button: "Deck:L4".into() } } }
impl Config {
    fn decode(v: &Value) -> Self {
        let mut c = Self::default();
        if let Some(m @ ("always" | "hold" | "toggle")) = v["mode"].as_str() { c.mode = m.into(); }
        if let Some(b) = v["button"].as_str().filter(|b| valid_button(b)) { c.button = b.into(); }
        c
    }
    fn value(&self) -> Value { json!({"mode":self.mode,"button":self.button}) }
}
fn valid_button(b: &str) -> bool {
    b.strip_prefix("Deck:").is_some_and(|s| crate::input::DECK_BTNS.iter().any(|(b,_,_)| format!("{b:?}") == s))
        || b.strip_prefix("Pad:").is_some_and(|s| !s.is_empty() && s.len() < 80 && s.chars().all(|c| c.is_ascii_alphanumeric() || "():, ".contains(c)))
}
fn config_path() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")).join("deckshell/recording-mic.json")
}
fn atomic_json(path: &std::path::Path, value: &Value) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::io::Write;
    if let Some(p) = path.parent() { fs::create_dir_all(p)?; }
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut f = fs::OpenOptions::new().create(true).truncate(true).write(true).mode(0o600).open(&tmp)?;
    f.write_all(value.to_string().as_bytes())?; f.sync_all()?; fs::rename(tmp, path)
}
fn settings() -> &'static Mutex<Config> {
    static S: OnceLock<Mutex<Config>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(if crate::actions::dryrun() { Config::default() } else { fs::read(config_path()).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).map(|v| Config::decode(&v)).unwrap_or_default() }))
}
pub fn config() -> Config { settings().lock().unwrap().clone() }
pub fn save(c: Config) -> io::Result<()> {
    if !matches!(c.mode.as_str(), "always" | "hold" | "toggle") || !valid_button(&c.button) { return Err(io::Error::other("Некорректная привязка")); }
    if !crate::actions::dryrun() { atomic_json(&config_path(), &c.value())?; }
    *settings().lock().unwrap() = c; Ok(())
}
pub fn mode_label(mode: &str) -> &'static str { match mode { "hold" => "По удержанию (push-to-talk)", "toggle" => "Переключатель (нажал — вкл/выкл)", _ => "Всегда активен" } }
pub fn button_label(b: &str) -> String {
    let name = b.strip_prefix("Deck:").or_else(|| b.strip_prefix("Pad:")).unwrap_or(b);
    match name {
        "Up" | "DPadUp" => "Крестовина ↑", "Down" | "DPadDown" => "Крестовина ↓",
        "Left" | "DPadLeft" => "Крестовина ←", "Right" | "DPadRight" => "Крестовина →",
        "LeftPad" => "Левый трекпад", "RightPad" => "Правый трекпад",
        "Menu" | "Start" => "Меню (≡)", "View" | "Select" => "Просмотр (⧉)",
        "Steam" | "Mode" => "STEAM / системная кнопка", "Qam" => "Быстрое меню (…)",
        "South" => "Нижняя кнопка (A / ✕)", "East" => "Правая кнопка (B / ○)",
        "West" => "Левая кнопка (X / □)", "North" => "Верхняя кнопка (Y / △)",
        "LeftTrigger" => "L1", "RightTrigger" => "R1", "LeftTrigger2" => "L2", "RightTrigger2" => "R2",
        "LeftThumb" => "L3", "RightThumb" => "R3",
        _ => return name.strip_prefix("Code").map(|code| format!("Кнопка контроллера ({code})")).unwrap_or_else(|| name.to_owned()),
    }.to_owned()
}

#[derive(Default)]
struct Inputs { sources: BTreeMap<String, (Instant, Vec<String>)>, capture: Option<Instant> }
static INPUT_EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
fn inputs() -> &'static Mutex<Inputs> { static I: OnceLock<Mutex<Inputs>> = OnceLock::new(); I.get_or_init(|| Mutex::new(Inputs::default())) }
pub fn capture() { inputs().lock().unwrap().capture = Some(Instant::now()); }
pub fn cancel_capture() { inputs().lock().unwrap().capture = None; }
/// Вызывается до фильтра фокуса: работает и поверх игры. Возвращает true при выборе кнопки.
pub fn report(source: &str, down: Vec<String>, connected: bool) -> bool {
    let mut s = inputs().lock().unwrap();
    let previous = s.sources.get(source);
    if (!connected && previous.is_some()) || (connected && previous.is_none_or(|(t,_)| t.elapsed() >= Duration::from_millis(750))) {
        INPUT_EPOCH.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    let fresh = previous.filter(|(t,_)| t.elapsed() < Duration::from_millis(750)).and_then(|(_,p)| down.iter().find(|b| !p.contains(b))).cloned();
    let capture = s.capture.is_some_and(|t| t.elapsed() < Duration::from_secs(15));
    if connected { s.sources.insert(source.into(), (Instant::now(), down)); } else { s.sources.remove(source); }
    if capture {
        if let Some(button) = fresh {
            s.capture = None;
            let _ = slint::invoke_from_event_loop(move || { crate::with_app(|a| {
                let mut c = config(); c.button = button;
                if save(c).is_ok() { a.toast("Кнопка микрофона сохранена"); a.push_settings(); }
                else { a.toast("Не удалось сохранить кнопку микрофона"); }
            }); });
        }
    }
    capture
}

/// Начальное удержание, повторные отчёты, смена режима и потеря ввода не создают нажатие.
#[derive(Default)]
struct Gate { armed: bool, was_down: bool, open: bool }
impl Gate {
    fn step(&mut self, mode: &str, down: Option<bool>) -> bool {
        let Some(down) = down else { *self = Self::default(); return false; };
        if !down { self.armed = true; }
        if self.armed && down && !self.was_down {
            if mode == "toggle" { self.open = !self.open; }
        }
        self.was_down = down;
        if mode == "hold" { self.open = self.armed && down; }
        self.open
    }
}
fn input_down(button: &str) -> Option<bool> {
    let s = inputs().lock().unwrap();
    let prefix = if button.starts_with("Deck:") { "deck" } else { "pad:" };
    let sources: Vec<_> = s.sources.iter().filter(|(k,(t,_))| k.starts_with(prefix) && t.elapsed() < Duration::from_millis(750)).collect();
    if sources.is_empty() { None } else { Some(sources.iter().any(|(_,(_,d))| d.iter().any(|b| b == button))) }
}
fn runtime_dir() -> PathBuf { PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap_or_else(|| format!("/run/user/{}", unsafe { libc::getuid() }).into())).join("deckshell-recording-mic") }
fn process_identity(pid: u32) -> Option<String> {
    fs::read_to_string(format!("/proc/{pid}/stat")).ok()?.rsplit_once(") ")?.1.split_whitespace().nth(19).map(str::to_owned)
}
fn status() -> &'static Mutex<String> { static S: OnceLock<Mutex<String>> = OnceLock::new(); S.get_or_init(Default::default) }
pub fn status_text() -> String { status().lock().unwrap().clone() }

fn lease_request(bytes: &[u8]) -> io::Result<Option<bool>> {
    let v: Value = serde_json::from_slice(bytes)?;
    match v.get("mute") {
        Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        _ => Err(io::Error::other("Повреждена команда микрофона")),
    }
}

pub fn start() {
    if crate::actions::dryrun() { return; }
    std::thread::spawn(|| {
        let pid = std::process::id();
        let Some(identity) = process_identity(pid) else { return; };
        let lease = runtime_dir().join(format!("lease-{pid}.json"));
        let mut last = config(); let mut gate = Gate::default();
        let mut child: Option<std::process::Child> = None;
        let mut retry = Instant::now() - Duration::from_secs(5);
        let steam_state = std::sync::Arc::new(Mutex::new((None, Instant::now())));
        let poll_state = steam_state.clone();
        std::thread::spawn(move || loop {
            let enabled = if config().mode == "always" { None } else {
                crate::actions::bridge().and_then(|b| b.eval_json("(()=>{const c=window.settingsStore?.clientSettings;return typeof c?.gamerecording_background_a_m==='boolean'&&typeof c?.gamerecording_background_mode==='number'?c.gamerecording_background_a_m&&c.gamerecording_background_mode!==0:null})()").ok()).and_then(|v| v.as_bool())
            };
            *poll_state.lock().unwrap() = (enabled, Instant::now());
            std::thread::sleep(Duration::from_secs(1));
        });
        let mut input_epoch = INPUT_EPOCH.load(std::sync::atomic::Ordering::Relaxed);
        let mut previous_tick = Instant::now();
        let mut written = Instant::now() - Duration::from_secs(1);
        let mut last_desired = None;
        loop {
            let epoch = INPUT_EPOCH.load(std::sync::atomic::Ordering::Relaxed);
            if epoch != input_epoch || previous_tick.elapsed() > Duration::from_millis(750) { gate = Gate::default(); }
            input_epoch = epoch;
            previous_tick = Instant::now();
            let c = config();
            if c != last { gate = Gate::default(); last = c.clone(); }
            let enabled = { let s = steam_state.lock().unwrap(); if s.1.elapsed() < Duration::from_secs(3) { s.0 } else { None } };
            // На неизвестном состоянии Steam оставляем источник закрытым.
            let desired = if c.mode == "always" || enabled == Some(false) { gate = Gate::default(); None }
                else if enabled.is_none() { gate = Gate::default(); Some(true) }
                else { Some(!gate.step(&c.mode, input_down(&c.button))) };
            if last_desired != desired || written.elapsed() >= Duration::from_millis(250) {
                if atomic_json(&lease, &json!({"mute":desired})).is_err() { std::thread::sleep(Duration::from_millis(100)); continue; }
                written = Instant::now(); last_desired = desired;
            }
            if let Some(ch) = child.as_mut() { if ch.try_wait().ok().flatten().is_some() { child = None; } }
            if child.is_none() && desired.is_some() && retry.elapsed() >= Duration::from_secs(3) {
                retry = Instant::now();
                if deck_system::guard::deny("systemd-run recording-mic guardian") { continue; }
                child = std::env::current_exe().ok().and_then(|exe| Command::new("systemd-run").args(["--user", "--quiet", "--collect", "--wait", &format!("--unit=deckshell-recording-mic-{pid}"), "--property=Restart=on-failure", "--property=RestartSec=1"])
                    .arg(exe).arg("--recording-mic-guardian").arg(pid.to_string()).arg(&identity).arg(&lease)
                    .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().ok());
            }
            let message = fs::read_to_string(runtime_dir().join("status")).unwrap_or_else(|_| if desired.is_some() { "Ожидание управления микрофоном…".into() } else { String::new() });
            let changed = { let mut s = status().lock().unwrap(); let changed = *s != message; *s = message; changed };
            if changed { let _ = slint::invoke_from_event_loop(|| { crate::with_app(|a| a.push_settings()); }); }
            std::thread::sleep(Duration::from_millis(25));
        }
    });
}
fn pactl(args: &[&str]) -> io::Result<String> {
    if args.first().is_some_and(|a| a.starts_with("set-")) && deck_system::guard::deny(&format!("pactl {}", args.join(" "))) { return Ok(String::new()); }
    // Ограничение времени также действует при недоступном сервере звука.
    let out = Command::new("timeout").args(["2", "pactl"]).args(args).output()?;
    if !out.status.success() { return Err(io::Error::other("PipeWire не подтвердил изменение микрофона")); }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}
fn get_mute(source: &str) -> io::Result<bool> {
    let value = pactl(&["get-source-mute", source])?;
    if value.ends_with("yes") { Ok(true) } else if value.ends_with("no") { Ok(false) } else { Err(io::Error::other("Неизвестный ответ mute")) }
}
fn set_mute(source: &str, muted: bool) -> io::Result<()> {
    pactl(&["set-source-mute", source, if muted { "1" } else { "0" }])?;
    if get_mute(source)? != muted { return Err(io::Error::other("Mute не подтверждён")); } Ok(())
}
/// Отдельная user-служба переживает SIGKILL/UI restart; журнал восстанавливается после сбоя самого сторожа.
pub fn guardian(args: &[String]) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    if args.len() != 3 { return Err(io::Error::other("Некорректный запуск сторожа")); }
    let pid: u32 = args[0].parse().map_err(io::Error::other)?;
    let dir = runtime_dir(); fs::create_dir_all(&dir)?;
    let lock = fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join("lock"))?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 { return Err(io::Error::last_os_error()); }
    // LC_ALL нужен для однозначного разбора yes/no.
    std::env::set_var("LC_ALL", "C");
    let journal = dir.join("restore.json");
    let mut saved: Option<(String, bool)> = fs::read(&journal).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).and_then(|v| Some((v["source"].as_str()?.into(), v["mute"].as_bool()?)));
    let mut applied = None;
    let mut source_checked = Instant::now() - Duration::from_secs(2);
    let mut mute_checked = Instant::now() - Duration::from_secs(2);
    let mut last_message = String::new();
    loop {
        let live = process_identity(pid).as_deref() == Some(&args[1]);
        let lease = std::path::Path::new(&args[2]);
        let fresh = fs::metadata(lease).and_then(|m| m.modified()).ok().and_then(|t| SystemTime::now().duration_since(t).ok()).is_some_and(|age| age < Duration::from_secs(2));
        let request = if live && fresh { fs::read(lease).and_then(|b| lease_request(&b)).unwrap_or(Some(true)) } else if live { Some(true) } else { None };
        let result = (|| -> io::Result<()> {
            if request.is_none() {
                if let Some((source, mute)) = &saved { set_mute(source, *mute)?; fs::remove_file(&journal)?; saved = None; applied = None; }
                return Ok(());
            }
            let source = if source_checked.elapsed() >= Duration::from_secs(1) || saved.is_none() {
                source_checked = Instant::now(); pactl(&["get-default-source"])?
            } else { saved.as_ref().unwrap().0.clone() };
            if source.is_empty() || source.ends_with(".monitor") { return Err(io::Error::other("Системный микрофон не найден")); }
            if saved.as_ref().is_some_and(|(old,_)| old != &source) {
                let (old,mute) = saved.as_ref().unwrap(); set_mute(old,*mute)?; fs::remove_file(&journal)?; saved = None; applied = None;
            }
            if saved.is_none() {
                let mute = get_mute(&source)?;
                atomic_json(&journal, &json!({"source":source,"mute":mute}))?;
                saved = Some((source.clone(), mute)); applied = None;
            }
            let mute = request.unwrap();
            // Проверяем также внешнее изменение mute; выбранный режим остаётся единственным владельцем до возврата.
            if applied != Some(mute) || (mute_checked.elapsed() >= Duration::from_millis(500) && get_mute(&source)? != mute) { set_mute(&source,mute)?; applied = Some(mute); }
            if mute_checked.elapsed() >= Duration::from_millis(500) { mute_checked = Instant::now(); }
            Ok(())
        })();
        let message = match result {
            Ok(()) => match request { Some(true) => "Микрофон закрыт", Some(false) => "Микрофон активен", None => "Исходное состояние микрофона восстановлено" }.to_owned(),
            Err(e) => { applied = None; if let Some((source,_)) = &saved { let _ = set_mute(source,true); } format!("Ошибка управления микрофоном: {e}") },
        };
        if message != last_message { let _ = fs::write(dir.join("status"), &message); last_message = message; }
        if !live && saved.is_none() { let _ = fs::remove_file(lease); return Ok(()); }
        std::thread::sleep(Duration::from_millis(40));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn modes_edges_loss_reconnect() {
        let mut g = Gate::default();
        assert!(!g.step("hold",Some(true))); assert!(!g.step("hold",Some(false)));
        assert!(g.step("hold",Some(true))); assert!(g.step("hold",Some(true)));
        assert!(!g.step("hold",None)); assert!(!g.step("hold",Some(true)));
        assert!(!g.step("toggle",Some(false))); assert!(g.step("toggle",Some(true)));
        assert!(g.step("toggle",Some(true))); assert!(g.step("toggle",Some(false)));
        assert!(!g.step("toggle",Some(true))); assert!(!g.step("toggle",None));
    }
    #[test] fn broken_lease_cannot_release_the_gate() {
        assert_eq!(lease_request(br#"{"mute":true}"#).unwrap(),Some(true));
        assert_eq!(lease_request(br#"{"mute":false}"#).unwrap(),Some(false));
        assert_eq!(lease_request(br#"{"mute":null}"#).unwrap(),None);
        for bytes in [b"{".as_slice(),br#"{}"#,br#"{"mute":"false"}"#] { assert!(lease_request(bytes).is_err()); }
    }
    #[test] fn binding_is_restored_from_atomic_file() {
        let path = std::env::temp_dir().join(format!("deckshell-mic-test-{}.json",std::process::id()));
        let c = Config { mode:"toggle".into(), button:"Deck:R5".into() };
        atomic_json(&path,&c.value()).unwrap();
        let loaded = Config::decode(&serde_json::from_slice::<Value>(&fs::read(&path).unwrap()).unwrap());
        fs::remove_file(path).unwrap(); assert_eq!(loaded,c);
    }
    #[test] fn settings_roundtrip_all_deck_buttons_and_mode_changes_keep_binding() {
        for (button,_,_) in crate::input::DECK_BTNS {
            let mut c = Config { mode:"hold".into(), button:format!("Deck:{button:?}") };
            for mode in ["hold","always","toggle","hold"] { c.mode = mode.into(); assert_eq!(Config::decode(&c.value()), c); }
        }
        assert_eq!(Config::decode(&json!({"mode":"bad","button":"bad"})), Config::default());
    }
}
