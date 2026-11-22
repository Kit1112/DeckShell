//! Смена аккаунта через фоновый перезапуск Steam (задача 430).
//!
//! Стоковый Steam меняет пользователя вызовом `RestartAsUser`: клиент закрывается, файлы входа переписываются,
//! клиент стартует и входит по сохранённым данным. Здесь то же самое, но Steam перезапускается в фоне, а
//! DeckShell всё время остаётся на экране (поверх — обложка с котиком, её рисует приложение).
//!
//! Порядок: проверка → hold-файл (сессия не поднимает Steam сама) → `steam -shutdown` → ждём выхода →
//! правка `loginusers.vdf` и `registry.vdf` (только пока Steam закрыт: на выходе он их переписывает) → снятие
//! hold → сессия запускает Steam → ждём готовности (состояние 5) именно целевого аккаунта. Любая ошибка,
//! тайм-аут или отмена после закрытия Steam — откат: файлы из бэкапа, ещё один перезапуск, ожидание
//! прежнего аккаунта. Откат делается один раз; ничего не зависает: у каждого ожидания есть предел.
//!
//! Автомат ([`run`]) не знает ни о файлах, ни о процессах: всё это даёт [`Host`]. Поэтому его гоняют
//! на моке Steam с виртуальными часами.
use steam_assets::i18n::{tr, tr_args, tr_ref};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Сколько ждём выхода Steam после `-shutdown`.
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(45);
/// Сколько ждём готовности Steam (состояние 5) после запуска.
pub const START_TIMEOUT: Duration = Duration::from_secs(150);
/// Если Steam поднялся, но стоит на экране входа (0/1) дольше этого — один раз просим вход из кэша.
pub const LOGIN_SCREEN_NUDGE: Duration = Duration::from_secs(12);
/// Период опроса.
pub const POLL: Duration = Duration::from_millis(500);
/// Если после снятия hold процесса Steam нет столько секунд, Steam запускаем сами (сессия могла не поднять его:
/// старый `deckshell-session.sh` без `supervise_steam`, упавший супервизор).
pub const START_KICK: Duration = Duration::from_secs(8);
/// Steam готов (5), но имя аккаунта пока пустое: ждём не дольше этого и только потом считаем аккаунт неизвестным.
pub const NAME_GRACE: Duration = Duration::from_secs(20);
/// Предел всей смены (закрытие + вход + откат). Ничто не ждёт дольше: экран с котиком не может висеть вечно.
pub const TOTAL_LIMIT: Duration = Duration::from_secs(420);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Проверка файлов, бэкап.
    Preparing,
    /// `steam -shutdown`, ждём выхода.
    ShuttingDown,
    /// Правка файлов входа (Steam закрыт).
    Rewriting,
    /// Steam запускается сессией.
    Starting,
    /// Steam поднялся и входит.
    SigningIn,
    /// Ошибка/отмена: возвращаем прежний аккаунт.
    RollingBack,
}

impl Phase {
    /// Подпись под котиком.
    pub fn message(self) -> &'static str {
        match self {
            Phase::Preparing => tr_ref("account_restart.preparing"),
            Phase::ShuttingDown => tr_ref("account_restart.shutting_down"),
            Phase::Rewriting => tr_ref("account_restart.selecting"),
            Phase::Starting => tr_ref("account_restart.starting"),
            Phase::SigningIn => tr_ref("account_restart.signing_in"),
            Phase::RollingBack => tr_ref("account_restart.rolling_back"),
        }
    }
}

/// Что видно снаружи Steam.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Probe {
    /// Порта отладки нет: Steam закрыт или ещё не поднялся.
    Down,
    /// Steam отвечает. `state` — состояние входа (0 нет, 1 экран входа, 2–4 загрузка, 5 готов, 6 выход).
    Up { account: Option<String>, state: Option<u32> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Вошли в целевой аккаунт, Steam готов.
    Done,
    /// Отмена: прежний аккаунт возвращён (или Steam не трогали).
    Cancelled,
    /// Не вышло. `back_on_previous` — Steam снова готов под прежним аккаунтом.
    Failed { message: String, back_on_previous: bool },
}

/// Всё, что автомат делает с внешним миром.
pub trait Host {
    /// Проверить, что цель есть в сохранённых и помнит пароль, сделать бэкап файлов входа.
    fn prepare(&mut self, target: &str) -> Result<(), String>;
    fn hold(&mut self, on: bool);
    fn request_shutdown(&mut self);
    fn steam_running(&mut self) -> bool;
    /// Выбрать `target` для автовхода (Steam закрыт).
    fn rewrite(&mut self, target: &str) -> Result<(), String>;
    /// Вернуть файлы из бэкапа.
    fn restore(&mut self) -> Result<(), String>;
    fn probe(&mut self) -> Probe;
    /// Попросить вход из кэша на экране входа (один раз).
    fn nudge_login(&mut self, target: &str);
    /// Было нажатие B с прошлого вызова (флаг снимается).
    fn cancelled(&mut self) -> bool;
    fn phase(&mut self, phase: Phase);
    /// Монотонное время с начала работы.
    fn elapsed(&mut self) -> Duration;
    fn sleep(&mut self, d: Duration);
    /// Запустить Steam самим, если его процесса нет (подстраховка сессии). `true` — запуск выполнен.
    fn start_steam(&mut self) -> bool { false }
    /// Строка журнала (с меткой времени ставит хост).
    fn note(&mut self, _msg: &str) {}
    /// Смена началась: запомнить цель и прежний аккаунт (для файла состояния потока).
    fn begin(&mut self, _target: &str, _previous: &str) {}
    /// Смена закончилась (любым итогом): файл состояния потока больше не нужен.
    fn finished(&mut self, _outcome: &Outcome) {}
}

fn same(a: &str, b: &str) -> bool { a.trim().eq_ignore_ascii_case(b.trim()) }

/// SteamID64 → accountid (младшие 32 бита); число до 10 цифр — уже accountid.
fn account_id(s: &str) -> Option<u64> {
    let t = s.trim();
    if t.is_empty() || !t.bytes().all(|b| b.is_ascii_digit()) { return None; }
    let n: u64 = t.parse().ok()?;
    if t.len() == 17 { Some(n & 0xFFFF_FFFF) } else if t.len() <= 10 { Some(n) } else { None }
}

/// Тот же аккаунт: имя без учёта регистра и пробелов по краям либо одинаковый accountid (SteamID64 ↔ accountid).
pub fn same_account(a: &str, b: &str) -> bool {
    if same(a, b) { return true; }
    matches!((account_id(a), account_id(b)), (Some(x), Some(y)) if x == y)
}

/// Ждёт закрытия Steam. `true` — закрылся.
fn wait_down(h: &mut impl Host, end: Duration) -> bool {
    let start = h.elapsed();
    while h.steam_running() {
        let now = h.elapsed();
        if now.saturating_sub(start) >= SHUTDOWN_TIMEOUT || now >= end { h.note(tr_ref("account_restart.shutdown_timeout_log")); return false; }
        h.sleep(POLL);
    }
    let took = h.elapsed().saturating_sub(start).as_secs_f32();
    h.note(&tr_args("account_restart.shutdown_elapsed", &[("time", &format!("{:.1}", took))]));
    true
}

enum Wait { Ready, Other(Option<String>), Timeout, Cancelled }

/// Ждёт, пока Steam станет готов (5) под аккаунтом `want`. `Other` — готов, но под другим аккаунтом.
/// Предел — `START_TIMEOUT` и общий `end`; отмена (B) прерывает ожидание всегда, в том числе при откате.
fn wait_ready(h: &mut impl Host, want: &str, nudge: bool, end: Duration) -> Wait {
    let start = h.elapsed();
    let mut login_screen_since: Option<Duration> = None;
    let mut no_name_since: Option<Duration> = None;
    let mut down_since: Option<Duration> = None;
    let mut kicked = false;
    let mut nudged = !nudge;
    let mut last: Option<Probe> = None;
    h.note(&tr_args("account_restart.waiting_log", &[("want", want), ("limit", &START_TIMEOUT.as_secs().to_string()), ("remaining", &end.saturating_sub(start).as_secs().to_string())]));
    loop {
        if h.cancelled() { h.note(tr_ref("account_restart.cancelled_wait_log")); return Wait::Cancelled; }
        let now = h.elapsed();
        if now.saturating_sub(start) >= START_TIMEOUT || now >= end {
            h.note(&tr_args("account_restart.wait_timeout_log", &[("elapsed", &format!("{:.1}", now.saturating_sub(start).as_secs_f32())), ("limit", &(now >= end).to_string())]));
            return Wait::Timeout;
        }
        let probe = h.probe();
        if last.as_ref() != Some(&probe) {
            h.note(&tr_args("account_restart.probe_log", &[("probe", &format!("{:?}", probe)), ("elapsed", &format!("{:.1}", now.saturating_sub(start).as_secs_f32()))]));
            last = Some(probe.clone());
        }
        match probe {
            Probe::Up { account, state: Some(5) } => {
                down_since = None;
                let name = account.filter(|a| !a.trim().is_empty());
                match name {
                    Some(a) if same_account(&a, want) => { h.note(&tr_args("account_restart.target_ready", &[("a", &a)])); return Wait::Ready; }
                    Some(a) => { h.note(&tr_args("account_restart.wrong_account_ready", &[("a", &a), ("want", want)])); return Wait::Other(Some(a)); }
                    None => {
                        // состояние 5, но имя ещё не отдано: подождать, а не объявлять «другой аккаунт»
                        let since = *no_name_since.get_or_insert(now);
                        if now.saturating_sub(since) >= NAME_GRACE {
                            h.note(tr_ref("account_restart.no_ready_name"));
                            return Wait::Other(None);
                        }
                    }
                }
            }
            Probe::Up { state: Some(0 | 1), .. } => {
                down_since = None; no_name_since = None;
                let since = *login_screen_since.get_or_insert(now);
                if !nudged && now.saturating_sub(since) >= LOGIN_SCREEN_NUDGE {
                    nudged = true;
                    h.note(&tr_args("account_restart.cached_login", &[("seconds", &LOGIN_SCREEN_NUDGE.as_secs().to_string()), ("want", want)]));
                    h.nudge_login(want);
                }
            }
            Probe::Down => {
                login_screen_since = None; no_name_since = None;
                let since = *down_since.get_or_insert(now);
                if !kicked && now.saturating_sub(since) >= START_KICK && !h.steam_running() {
                    kicked = true;
                    h.note(&tr_args("account_restart.launch_fallback", &[("seconds", &START_KICK.as_secs().to_string())]));
                    let ok = h.start_steam();
                    let status = if ok { tr_ref("account_restart.launch_ok") } else { tr_ref("account_restart.launch_failed") };
                    h.note(&tr_args("account_restart.launch_result", &[("status", status)]));
                }
            }
            _ => { login_screen_since = None; no_name_since = None; down_since = None; }
        }
        h.sleep(POLL);
    }
}

/// Закрывает Steam под hold. `false` — не закрылся (hold снят, файлы не тронуты).
fn shutdown(h: &mut impl Host, end: Duration) -> bool {
    h.note(tr_ref("account_restart.shutdown_log"));
    h.hold(true);
    h.request_shutdown();
    if wait_down(h, end) { return true; }
    h.note(tr_ref("account_restart.shutdown_failed_log"));
    h.hold(false);
    false
}

/// Откат после закрытия Steam: файлы из бэкапа, перезапуск, ожидание прежнего аккаунта.
fn roll_back(h: &mut impl Host, previous: &str, reason: String, end: Duration) -> Outcome {
    h.note(&tr_args("account_restart.rollback_log", &[("reason", &reason)]));
    h.phase(Phase::RollingBack);
    let files = h.restore();
    h.note(&tr_args("account_restart.backup_restored_log", &[("files", &format!("{:?}", files))]));
    h.hold(false);
    let w = wait_ready(h, previous, true, end);
    let back = matches!(w, Wait::Ready);
    let mut message = reason;
    if let Err(e) = files { let _ = write!(message, "{}", tr_args("account_restart.restore_failed", &[("e", &e.to_string())])); }
    if matches!(w, Wait::Cancelled) { message.push_str(tr_ref("account_restart.wait_cancelled")); }
    if !back { message.push_str(tr_ref("account_restart.previous_unconfirmed")); }
    Outcome::Failed { message, back_on_previous: back }
}

/// Вся смена. `previous` — аккаунт, под которым Steam готов сейчас.
pub fn run(h: &mut impl Host, target: &str, previous: &str) -> Outcome {
    h.begin(target, previous);
    let outcome = run_inner(h, target, previous);
    h.note(&tr_args("account_restart.outcome_log", &[("outcome", &format!("{:?}", outcome))]));
    h.finished(&outcome);
    outcome
}

fn run_inner(h: &mut impl Host, target: &str, previous: &str) -> Outcome {
    let end = h.elapsed() + TOTAL_LIMIT;
    h.note(&tr_args("account_restart.start_log", &[("target", target), ("previous", previous)]));
    if same_account(target, previous) {
        // Выбран аккаунт, под которым Steam уже вошёл: перезапуск ничего не даст и только вылогинит.
        h.note(tr_ref("account_restart.same_account_log"));
        return Outcome::Done;
    }
    h.phase(Phase::Preparing);
    if let Err(e) = h.prepare(target) {
        h.note(&tr_args("account_restart.prepare_failed_log", &[("e", &e.to_string())]));
        return Outcome::Failed { message: e, back_on_previous: true };
    }
    h.note(tr_ref("account_restart.prepared_log"));
    if h.cancelled() { h.note(tr_ref("account_restart.early_cancel_log")); return Outcome::Cancelled; }

    h.phase(Phase::ShuttingDown);
    if !shutdown(h, end) {
        return Outcome::Failed { message: tr_ref("account_restart.shutdown_timeout").into(), back_on_previous: true };
    }
    // Отмену, пришедшую во время закрытия, исполняем без правки файлов: Steam просто поднимется как был.
    if h.cancelled() {
        h.note(tr_ref("account_restart.shutdown_cancel_log"));
        h.phase(Phase::RollingBack);
        h.hold(false);
        let back = matches!(wait_ready(h, previous, true, end), Wait::Ready);
        return if back { Outcome::Cancelled }
            else { Outcome::Failed { message: tr_ref("account_restart.cancelled_unconfirmed").into(), back_on_previous: false } };
    }

    h.phase(Phase::Rewriting);
    let rewritten = h.rewrite(target);
    h.note(&tr_args("account_restart.login_files_changed", &[("rewritten", &format!("{:?}", rewritten))]));
    if let Err(e) = rewritten {
        return roll_back(h, previous, tr_args("account_restart.selection_failed", &[("e", &e.to_string())]), end);
    }
    h.phase(Phase::Starting);
    h.note(tr_ref("account_restart.release_hold_log"));
    h.hold(false);

    h.phase(Phase::SigningIn);
    match wait_ready(h, target, true, end) {
        Wait::Ready => Outcome::Done,
        Wait::Cancelled => {
            // Steam уже поднимается под целью: возвращаем прежнего ещё одним перезапуском.
            if !shutdown(h, end) {
                return Outcome::Failed { message: tr_ref("account_restart.cancel_shutdown_failed").into(), back_on_previous: false };
            }
            match roll_back(h, previous, String::new(), end) {
                Outcome::Failed { back_on_previous: true, .. } => Outcome::Cancelled,
                other => other,
            }
        }
        Wait::Timeout => {
            if !shutdown(h, end) {
                return Outcome::Failed { message: tr_ref("account_restart.rollback_shutdown_failed").into(), back_on_previous: false };
            }
            roll_back(h, previous, tr_ref("account_restart.login_timeout").into(), end)
        }
        Wait::Other(account) => {
            // Steam готов, но не под целью (автовход проигнорирован). Перезапуск не нужен: вернуть файлы.
            let files = h.restore();
            h.note(&tr_args("account_restart.wrong_account_restore_log", &[("account", &format!("{:?}", account)), ("files", &format!("{:?}", files))]));
            let on_previous = account.as_deref().is_some_and(|a| same_account(a, previous));
            let mut message = tr("account_restart.wrong_account");
            if let Err(e) = files { let _ = write!(message, "{}", tr_args("account_restart.restore_failed", &[("e", &e.to_string())])); }
            Outcome::Failed { message, back_on_previous: on_previous }
        }
    }
}

// ---------------------------------------------------------------- состояние потока и подхват после перезапуска UI

/// Состояние незавершённой смены (`flow.json`): по нему новый запуск DeckShell довёдет или отменит поток.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FlowState {
    pub phase: String,
    pub target: String,
    pub previous: String,
    pub pid: u32,
    pub started_unix: u64,
    /// Каталог бэкапа этой смены (пусто — бэкап ещё не делался).
    #[serde(default)]
    pub backup_dir: String,
}

impl Phase {
    pub fn key(self) -> &'static str {
        match self {
            Phase::Preparing => "preparing", Phase::ShuttingDown => "shutting_down", Phase::Rewriting => "rewriting",
            Phase::Starting => "starting", Phase::SigningIn => "signing_in", Phase::RollingBack => "rolling_back",
        }
    }
    /// Файлы входа уже могли быть изменены.
    fn files_touched(key: &str) -> bool { matches!(key, "rewriting" | "starting" | "signing_in" | "rolling_back") }
}

pub fn flow_path(state_dir: &Path) -> PathBuf { state_dir.join("account-switch").join("flow.json") }

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Recovery {
    /// Steam готов под целью: смена фактически завершилась.
    Completed,
    /// Steam готов под прежним аккаунтом (файлы возвращены, если их меняли).
    BackOnPrevious,
    /// Steam так и не подтвердил аккаунт за предел; файлы возвращены, hold снят.
    GaveUp,
}

/// Новый запуск DeckShell нашёл незавершённый поток: снять hold, при необходимости запустить Steam, дождаться
/// ответа (≤ `START_TIMEOUT`), вернуть файлы входа. Ничего не показывает на экране и не ждёт дольше предела.
pub fn recover(h: &mut impl Host, flow: &FlowState) -> Recovery {
    let end = h.elapsed() + START_TIMEOUT;
    h.note(&tr_args("account_restart.recovery_log", &[("phase", &(flow.phase).to_string()), ("target", &flow.target), ("previous", &flow.previous), ("pid", &flow.pid.to_string())]));
    h.hold(false);
    let touched = Phase::files_touched(&flow.phase);
    let result = match wait_ready(h, &flow.target, false, end) {
        Wait::Ready => Recovery::Completed,
        Wait::Other(Some(a)) if same_account(&a, &flow.previous) => Recovery::BackOnPrevious,
        _ => Recovery::GaveUp,
    };
    if result != Recovery::Completed && touched {
        let r = h.restore();
        h.note(&tr_args("account_restart.recovery_files_log", &[("result", &format!("{:?}", r))]));
    }
    h.note(&tr_args("account_restart.recovery_done_log", &[("result", &format!("{:?}", result))]));
    result
}

// ---------------------------------------------------------------- правка файлов (чистые функции)

/// Значение строки вида `"Key"\t\t"value"`.
fn kv(line: &str) -> Option<(&str, &str)> {
    let t = line.trim();
    let rest = t.strip_prefix('"')?;
    let (k, rest) = rest.split_once('"')?;
    let rest = rest.trim_start().strip_prefix('"')?;
    let v = rest.strip_suffix('"')?;
    Some((k, v))
}

fn with_value(line: &str, value: &str) -> String {
    let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
    let (k, _) = kv(line).unwrap_or(("", ""));
    format!("{indent}\"{k}\"\t\t\"{}\"", vdf_escape(value))
}

fn vdf_escape(s: &str) -> String { s.replace('\\', "\\\\").replace('"', "\\\"") }

/// Аккаунты из `loginusers.vdf`: (имя входа, помнит пароль).
pub fn saved_users(loginusers: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let (mut name, mut remember) = (None, false);
    for line in loginusers.lines() {
        if let Some((k, v)) = kv(line) {
            match k {
                "AccountName" => name = Some(v.to_string()),
                "RememberPassword" => remember = v == "1",
                _ => {}
            }
        } else if line.trim() == "}" {
            if let Some(n) = name.take() { out.push((n, remember)); }
            remember = false;
        }
    }
    out
}

/// `loginusers.vdf` с выбранным для автовхода `target`: у него `MostRecent`/`AutoLogin` = 1, у остальных = 0.
/// Если у цели нет ни одного из ключей — добавляется `MostRecent`. Остальной текст не меняется.
pub fn select_in_loginusers(text: &str, target: &str) -> Result<String, String> {
    // блок пользователя: от `"<id>"` до `}`; имя читаем заранее
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len() + 2);
    let mut i = 0;
    let mut found = false;
    while i < lines.len() {
        // начало блока пользователя: строка из одного ключа, следующая — `{`, ключ — 17 цифр
        let t = lines[i].trim();
        let is_id = t.len() == 19 && t.starts_with('"') && t.ends_with('"') && t[1..18].chars().all(|c| c.is_ascii_digit());
        if is_id && lines.get(i + 1).is_some_and(|l| l.trim() == "{") {
            let end = (i + 2..lines.len()).find(|&j| lines[j].trim() == "}").ok_or(tr_ref("account_restart.loginusers_invalid"))?;
            let body = &lines[i + 2..end];
            let name = body.iter().find_map(|l| kv(l).filter(|(k, _)| *k == "AccountName").map(|(_, v)| v)).unwrap_or("");
            let is_target = same(name, target);
            found |= is_target;
            let want = if is_target { "1" } else { "0" };
            out.push(lines[i].to_string());
            out.push(lines[i + 1].to_string());
            let mut seen = false;
            for l in body {
                match kv(l) {
                    Some(("MostRecent" | "AutoLogin", _)) => { seen = true; out.push(with_value(l, want)); }
                    _ => out.push((*l).to_string()),
                }
            }
            if is_target && !seen { out.push(format!("\t\t\"MostRecent\"\t\t\"{want}\"")); }
            out.push(lines[end].to_string());
            i = end + 1;
        } else {
            out.push(lines[i].to_string());
            i += 1;
        }
    }
    if !found { return Err(tr_args("account_restart.loginusers_account_missing", &[("target", target)])); }
    let mut s = out.join("\n");
    if text.ends_with('\n') { s.push('\n'); }
    Ok(s)
}

/// `registry.vdf` с `AutoLoginUser = target`. Ключ нет — добавляется в `HKCU/Software/Valve/Steam`.
pub fn select_in_registry(text: &str, target: &str) -> Result<String, String> {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    if let Some(l) = lines.iter_mut().find(|l| kv(l).is_some_and(|(k, _)| k == "AutoLoginUser")) {
        *l = with_value(l, target);
    } else {
        let hk = lines.iter().position(|l| l.trim() == "\"HKCU\"").ok_or(tr_ref("account_restart.registry_hkcu_missing"))?;
        let steam = (hk..lines.len()).find(|&j| lines[j].trim() == "\"Steam\"").ok_or(tr_ref("account_restart.registry_steam_missing"))?;
        if lines.get(steam + 1).map(|l| l.trim()) != Some("{") { return Err(tr_ref("account_restart.registry_invalid").into()); }
        let indent: String = lines[steam + 1].chars().take_while(|c| c.is_whitespace()).collect();
        lines.insert(steam + 2, format!("{indent}\t\"AutoLoginUser\"\t\t\"{}\"", vdf_escape(target)));
    }
    let mut s = lines.join("\n");
    if text.ends_with('\n') { s.push('\n'); }
    Ok(s)
}

/// Записать файл целиком: во временный рядом, затем переименовать; права сохраняются.
fn write_atomic(path: &Path, data: &str) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("deckshell-{}.tmp", std::process::id()));
    std::fs::write(&tmp, data)?;
    if let Ok(meta) = std::fs::metadata(path) { let _ = std::fs::set_permissions(&tmp, meta.permissions()); }
    std::fs::rename(&tmp, path).inspect_err(|_| { let _ = std::fs::remove_file(&tmp); })
}

// ---------------------------------------------------------------- файлы входа Steam

/// Где лежат файлы и куда класть бэкап.
#[derive(Clone, Debug)]
pub struct Files {
    pub loginusers: PathBuf,
    pub registry: PathBuf,
    pub backup_dir: PathBuf,
}

impl Files {
    /// `steam_root/config/loginusers.vdf`, `~/.steam/registry.vdf` (`DECKSHELL_STEAM_REGISTRY` переопределяет).
    pub fn default_paths(steam_root: &Path, home: &Path, state_dir: &Path, stamp: u64) -> Files {
        Files {
            loginusers: steam_root.join("config/loginusers.vdf"),
            registry: std::env::var_os("DECKSHELL_STEAM_REGISTRY").map(PathBuf::from).unwrap_or_else(|| home.join(".steam/registry.vdf")),
            backup_dir: state_dir.join("account-switch").join(format!("backup-{stamp}")),
        }
    }
    fn backup_of(&self, p: &Path) -> PathBuf { self.backup_dir.join(p.file_name().unwrap_or_default()) }

    /// Проверка и бэкап. Нужны оба файла и сохранённые данные цели.
    pub fn prepare(&self, target: &str) -> Result<(), String> {
        let users = std::fs::read_to_string(&self.loginusers).map_err(|e| tr_args("account_restart.loginusers_read_failed", &[("e", &e.to_string())]))?;
        std::fs::read_to_string(&self.registry).map_err(|e| tr_args("account_restart.registry_read_failed", &[("e", &e.to_string())]))?;
        match saved_users(&users).into_iter().find(|(n, _)| same(n, target)) {
            None => return Err(tr_args("account_restart.saved_account_missing", &[("target", target)])),
            Some((_, false)) => return Err(tr_ref("account_restart.password_not_saved").into()),
            Some(_) => {}
        }
        std::fs::create_dir_all(&self.backup_dir).map_err(|e| tr_args("account_restart.backup_no_space", &[("e", &e.to_string())]))?;
        for p in [&self.loginusers, &self.registry] {
            std::fs::copy(p, self.backup_of(p)).map_err(|e| tr_args("account_restart.backup_failed", &[("e", &e.to_string())]))?;
        }
        Ok(())
    }
    /// Steam закрыт: выбрать `target`. При ошибке второго файла первый возвращается.
    pub fn rewrite(&self, target: &str) -> Result<(), String> {
        let users = std::fs::read_to_string(&self.loginusers).map_err(|e| e.to_string())?;
        let reg = std::fs::read_to_string(&self.registry).map_err(|e| e.to_string())?;
        let (users2, reg2) = (select_in_loginusers(&users, target)?, select_in_registry(&reg, target)?);
        write_atomic(&self.loginusers, &users2).map_err(|e| e.to_string())?;
        if let Err(e) = write_atomic(&self.registry, &reg2) {
            let _ = write_atomic(&self.loginusers, &users);
            return Err(e.to_string());
        }
        Ok(())
    }
    pub fn restore(&self) -> Result<(), String> {
        let mut errors = Vec::new();
        for p in [&self.loginusers, &self.registry] {
            let saved = self.backup_of(p);
            let res = std::fs::read_to_string(&saved).and_then(|d| write_atomic(p, &d));
            if let Err(e) = res { errors.push(format!("{}: {e}", p.display())); }
        }
        if errors.is_empty() { Ok(()) } else { Err(errors.join("; ")) }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn steam_started_by_deckshell_is_always_headless() {
        assert_eq!(super::headless_args(None), ["-steamos3", "-silent"]);
        assert_eq!(super::headless_args(Some("-steamos3")), ["-steamos3", "-silent"]);
        assert_eq!(super::headless_args(Some("-steamos3 -steampal -gamepadui")), ["-steamos3", "-silent"]);
    }
    use super::*;
    use std::collections::VecDeque;

    // ------------------------------------------------------------ мок Steam с виртуальными часами
    #[derive(Default)]
    struct Mock {
        t: Duration,
        log: Vec<String>,
        running: bool,
        /// сколько ещё Steam закрывается после `-shutdown` (None — не закроется)
        close_after: Option<Duration>,
        closing_since: Option<Duration>,
        /// когда Steam снимет hold и запущен: задержки до появления порта и до готовности
        hold: bool,
        started_at: Option<Duration>,
        boot: Boot,
        account_files: String,   // кого выберет автовход ("target" / "previous")
        previous: String,
        cancel_at: Option<Duration>,
        rewrite_fails: bool,
        prepare_fails: bool,
        nudged: bool,
        phases: Vec<Phase>,
        scripted: VecDeque<Probe>,
        /// сессия не следит за Steam (старый deckshell-session.sh): сам он не поднимется
        no_session: bool,
        /// имя аккаунта в состоянии 5 пустое столько секунд после готовности
        name_empty_for: Duration,
        notes: Vec<String>,
        cancel_taken: bool,
    }
    #[derive(Clone, Copy, Default)]
    enum Boot {
        /// порт через 5 с, готов через `ready` с, под тем аккаунтом, что выбран в файлах
        #[default] Normal,
        /// поднимается, но остаётся на экране входа
        StuckAtLogin,
        /// никогда не поднимается
        Never,
        /// на экране входа до nudge, после готов
        NeedsNudge,
    }
    impl Mock {
        fn new(previous: &str) -> Mock {
            Mock { running: true, close_after: Some(Duration::from_secs(3)), account_files: previous.into(), previous: previous.into(), ..Default::default() }
        }
        fn ensure_started(&mut self) {
            // сессия поднимает Steam, как только hold снят и Steam не работает
            if !self.no_session && !self.hold && !self.running && self.closing_since.is_none() && self.started_at.is_none() {
                self.running = true; self.started_at = Some(self.t); self.log.push("start".into());
            }
        }
        fn tick(&mut self) {
            if let (Some(since), Some(after)) = (self.closing_since, self.close_after) {
                if self.t >= since + after { self.running = false; self.closing_since = None; self.started_at = None; self.log.push("closed".into()); }
            }
            self.ensure_started();
        }
    }
    impl Host for Mock {
        fn prepare(&mut self, _: &str) -> Result<(), String> {
            self.log.push("prepare".into());
            if self.prepare_fails { Err("нет аккаунта".into()) } else { Ok(()) }
        }
        fn hold(&mut self, on: bool) { self.hold = on; self.log.push(format!("hold={on}")); self.tick(); }
        fn request_shutdown(&mut self) {
            self.log.push("shutdown".into());
            self.closing_since = Some(self.t);
            if self.close_after.is_none() { self.closing_since = None; }
            self.started_at = None;
            if self.closing_since.is_none() { /* зависший Steam: остаётся running */ }
        }
        fn steam_running(&mut self) -> bool { self.tick(); self.running }
        fn rewrite(&mut self, target: &str) -> Result<(), String> {
            assert!(!self.running, "файлы нельзя править при работающем Steam");
            self.log.push("rewrite".into());
            if self.rewrite_fails { return Err("диск".into()); }
            self.account_files = target.into(); Ok(())
        }
        fn restore(&mut self) -> Result<(), String> { self.log.push("restore".into()); self.account_files = self.previous.clone(); Ok(()) }
        fn probe(&mut self) -> Probe {
            self.tick();
            if let Some(p) = self.scripted.pop_front() { return p; }
            let Some(at) = self.started_at else { return Probe::Down };
            if !self.running { return Probe::Down; }
            let up = self.t.saturating_sub(at);
            match self.boot {
                Boot::Never => Probe::Down,
                _ if up < Duration::from_secs(5) => Probe::Down,
                Boot::StuckAtLogin => Probe::Up { account: None, state: Some(1) },
                Boot::NeedsNudge if !self.nudged => Probe::Up { account: None, state: Some(1) },
                _ if up < Duration::from_secs(15) => Probe::Up { account: None, state: Some(4) },
                _ if up < Duration::from_secs(15) + self.name_empty_for => Probe::Up { account: None, state: Some(5) },
                _ => Probe::Up { account: Some(self.account_files.clone()), state: Some(5) },
            }
        }
        fn nudge_login(&mut self, _: &str) { self.nudged = true; self.account_files = "target".into(); self.log.push("nudge".into()); }
        fn cancelled(&mut self) -> bool {
            // как в SystemHost: нажатие B исполняется один раз; следующее нажатие — новая отмена
            if !self.cancel_taken && self.cancel_at.is_some_and(|c| self.t >= c) { self.cancel_taken = true; true } else { false }
        }
        fn phase(&mut self, p: Phase) { self.phases.push(p); }
        fn elapsed(&mut self) -> Duration { self.t }
        fn sleep(&mut self, d: Duration) { self.t += d; self.tick(); }
        fn start_steam(&mut self) -> bool {
            if self.running { return false; }
            self.running = true; self.started_at = Some(self.t); self.log.push("kick".into()); true
        }
        fn note(&mut self, msg: &str) { self.notes.push(msg.to_string()); }
    }
    fn count(m: &Mock, what: &str) -> usize { m.log.iter().filter(|l| *l == what).count() }

    #[test]
    fn happy_path_runs_every_state_in_order() {
        let mut m = Mock::new("previous");
        assert_eq!(run(&mut m, "target", "previous"), Outcome::Done);
        assert_eq!(m.phases, vec![Phase::Preparing, Phase::ShuttingDown, Phase::Rewriting, Phase::Starting, Phase::SigningIn]);
        let order: Vec<&str> = m.log.iter().map(String::as_str).collect();
        let pos = |s: &str| order.iter().position(|l| *l == s).unwrap();
        assert!(pos("prepare") < pos("hold=true") && pos("hold=true") < pos("shutdown"));
        assert!(pos("shutdown") < pos("closed") && pos("closed") < pos("rewrite") && pos("rewrite") < pos("hold=false"));
        assert!(pos("hold=false") <= pos("start"), "Steam поднимается только после снятия hold");
        assert_eq!(count(&m, "start"), 1);
        assert_eq!(count(&m, "restore"), 0);
    }
    #[test]
    fn prepare_failure_touches_nothing() {
        let mut m = Mock::new("previous"); m.prepare_fails = true;
        assert!(matches!(run(&mut m, "target", "previous"), Outcome::Failed { back_on_previous: true, .. }));
        assert!(m.running && !m.log.contains(&"shutdown".to_string()) && !m.hold);
    }
    #[test]
    fn steam_that_will_not_close_fails_without_changes_and_releases_hold() {
        let mut m = Mock::new("previous"); m.close_after = None;
        let o = run(&mut m, "target", "previous");
        assert!(matches!(o, Outcome::Failed { back_on_previous: true, .. }), "{o:?}");
        assert!(!m.hold && m.running && count(&m, "rewrite") == 0);
        assert!(m.t < SHUTDOWN_TIMEOUT + Duration::from_secs(2), "ожидание ограничено");
    }
    #[test]
    fn cancel_during_shutdown_skips_the_rewrite_and_brings_steam_back_unchanged() {
        let mut m = Mock::new("previous"); m.cancel_at = Some(Duration::from_secs(1));
        assert_eq!(run(&mut m, "target", "previous"), Outcome::Cancelled);
        assert_eq!(count(&m, "rewrite"), 0);
        assert_eq!(m.account_files, "previous");
        assert!(!m.hold && m.running);
    }
    #[test]
    fn cancel_while_signing_in_rolls_back_to_the_previous_account() {
        let mut m = Mock::new("previous"); m.cancel_at = Some(Duration::from_secs(12));
        assert_eq!(run(&mut m, "target", "previous"), Outcome::Cancelled);
        assert_eq!(count(&m, "rewrite"), 1);
        assert_eq!(count(&m, "restore"), 1);
        assert_eq!(m.account_files, "previous");
        assert!(!m.hold && m.running);
        assert!(m.phases.contains(&Phase::RollingBack));
    }
    #[test]
    fn steam_that_never_comes_back_times_out_then_rolls_back_once() {
        let mut m = Mock::new("previous"); m.boot = Boot::Never;
        let o = run(&mut m, "target", "previous");
        // откат тоже не дождался (Never): ошибка, но без бесконечного ожидания
        assert!(matches!(&o, Outcome::Failed { back_on_previous: false, .. }), "{o:?}");
        assert_eq!(count(&m, "restore"), 1, "откат ровно один");
        assert!(m.t < START_TIMEOUT * 2 + SHUTDOWN_TIMEOUT * 2, "общий предел: {:?}", m.t);
        assert!(!m.hold);
    }
    #[test]
    fn stuck_login_screen_gets_one_cached_sign_in_request_then_times_out_and_rolls_back() {
        let mut m = Mock::new("previous"); m.boot = Boot::StuckAtLogin;
        let o = run(&mut m, "target", "previous");
        assert!(matches!(o, Outcome::Failed { .. }));
        assert_eq!(count(&m, "nudge"), 2, "по одному разу в ожидании цели и в откате");
        assert_eq!(count(&m, "restore"), 1);
    }
    #[test]
    fn login_screen_is_recovered_by_one_cached_sign_in() {
        let mut m = Mock::new("previous"); m.boot = Boot::NeedsNudge;
        assert_eq!(run(&mut m, "target", "previous"), Outcome::Done);
        assert_eq!(count(&m, "nudge"), 1);
        assert_eq!(count(&m, "restore"), 0);
    }
    #[test]
    fn rewrite_failure_restores_files_and_returns_to_the_previous_account() {
        let mut m = Mock::new("previous"); m.rewrite_fails = true;
        let o = run(&mut m, "target", "previous");
        assert!(matches!(&o, Outcome::Failed { back_on_previous: true, message } if message.contains("диск")), "{o:?}");
        assert_eq!(count(&m, "restore"), 1);
        assert!(!m.hold && m.running);
    }
    #[test]
    fn steam_ready_under_another_account_restores_files_without_a_second_restart() {
        let mut m = Mock::new("previous"); m.account_files = "previous".into();
        // Steam проигнорирует выбор: всегда поднимается под прежним
        m.scripted = VecDeque::new();
        struct Ignoring(Mock);
        impl Host for Ignoring {
            fn prepare(&mut self, t: &str) -> Result<(), String> { self.0.prepare(t) }
            fn hold(&mut self, on: bool) { self.0.hold(on) }
            fn request_shutdown(&mut self) { self.0.request_shutdown() }
            fn steam_running(&mut self) -> bool { self.0.steam_running() }
            fn rewrite(&mut self, _: &str) -> Result<(), String> { self.0.log.push("rewrite".into()); Ok(()) }
            fn restore(&mut self) -> Result<(), String> { self.0.restore() }
            fn probe(&mut self) -> Probe { self.0.probe() }
            fn nudge_login(&mut self, t: &str) { self.0.nudge_login(t) }
            fn cancelled(&mut self) -> bool { false }
            fn phase(&mut self, p: Phase) { self.0.phase(p) }
            fn elapsed(&mut self) -> Duration { self.0.elapsed() }
            fn sleep(&mut self, d: Duration) { self.0.sleep(d) }
        }
        let mut h = Ignoring(m);
        let o = run(&mut h, "target", "previous");
        assert!(matches!(o, Outcome::Failed { back_on_previous: true, .. }), "{o:?}");
        assert_eq!(count(&h.0, "start"), 1, "Steam не перезапускали повторно");
        assert_eq!(count(&h.0, "restore"), 1);
    }

    #[test]
    fn session_that_never_restarts_steam_is_covered_by_our_own_start() {
        // Баг 18.11: на Деке стоял старый deckshell-session.sh без supervise_steam; Steam не поднимался, экран висел.
        let mut m = Mock::new("previous"); m.no_session = true;
        assert_eq!(run(&mut m, "target", "previous"), Outcome::Done);
        assert_eq!(count(&m, "kick"), 1);
        assert!(m.t < Duration::from_secs(60), "{:?}", m.t);
        assert!(m.notes.iter().any(|n| n.contains(crate::expected_text("запускаем сами", "Launching Steam"))));
    }
    #[test]
    fn target_equal_to_current_account_does_nothing() {
        let mut m = Mock::new("Previous");
        assert_eq!(run(&mut m, "previous ", "Previous"), Outcome::Done);
        assert!(m.log.is_empty(), "{:?}", m.log);
        assert!(m.phases.is_empty());
    }
    #[test]
    fn account_ids_compare_across_formats() {
        assert!(same_account("Death_Nick_", " death_nick_"));
        assert!(same_account("76561198960265729", "1000000001"));
        assert!(same_account("1000000001", "76561198960265729"));
        assert!(!same_account("76561198960265729", "1000000002"));
        assert!(!same_account("alpha", "beta"));
        assert!(!same_account("", "1000000001"));
    }
    #[test]
    fn ready_with_a_late_account_name_is_not_taken_for_another_account() {
        let mut m = Mock::new("previous"); m.name_empty_for = Duration::from_secs(8);
        assert_eq!(run(&mut m, "target", "previous"), Outcome::Done);
        assert_eq!(count(&m, "restore"), 0);
    }
    #[test]
    fn ready_that_never_names_the_account_gives_up_after_the_grace() {
        let mut m = Mock::new("previous"); m.name_empty_for = Duration::from_secs(10_000);
        let o = run(&mut m, "target", "previous");
        assert!(matches!(o, Outcome::Failed { back_on_previous: false, .. }), "{o:?}");
        assert!(m.t < START_TIMEOUT, "{:?}", m.t);
        assert_eq!(count(&m, "restore"), 1);
    }
    #[test]
    fn cancel_works_during_rollback_and_the_whole_flow_is_bounded() {
        // цель не поднимается (тайм-аут → откат), откат тоже висит; B в откате обрывает ожидание
        let mut m = Mock::new("previous"); m.boot = Boot::Never; m.cancel_at = Some(START_TIMEOUT + Duration::from_secs(60));
        // сначала отмена не видна (после тайм-аута cancel_at уже в будущем); дальше она обрывает откат
        let o = run(&mut m, "target", "previous");
        assert!(matches!(&o, Outcome::Failed { .. } | Outcome::Cancelled), "{o:?}");
        assert!(m.t <= TOTAL_LIMIT + Duration::from_secs(2), "{:?}", m.t);
        assert!(!m.hold);
    }
    #[test]
    fn total_limit_holds_even_when_every_wait_would_take_longest() {
        let mut m = Mock::new("previous"); m.boot = Boot::StuckAtLogin;
        let _ = run(&mut m, "target", "previous");
        assert!(m.t <= TOTAL_LIMIT + Duration::from_secs(2), "{:?}", m.t);
        assert!(!m.hold);
    }
    #[test]
    fn journal_names_every_decision() {
        let mut m = Mock::new("previous"); m.boot = Boot::NeedsNudge;
        run(&mut m, "target", "previous");
        for (russian, english) in [
            ("старт", "Starting"),
            ("подготовка", "Preparation"),
            ("hold=вкл", "hold enabled"),
            ("Steam закрыт", "Steam exited"),
            ("правка файлов", "Login files updated"),
            ("ожидание готовности", "Waiting for Steam"),
            ("вход из кэша", "cached login"),
            ("готов под целью", "Ready under target"),
            ("итог", "Outcome"),
        ] {
            let want = crate::expected_text(russian, english);
            assert!(m.notes.iter().any(|n| n.contains(want)), "нет «{want}» в {:#?}", m.notes);
        }
    }

    // ------------------------------------------------------------ подхват после перезапуска UI
    fn flow(phase: &str) -> FlowState {
        FlowState { phase: phase.into(), target: "target".into(), previous: "previous".into(), pid: 1, started_unix: 0, backup_dir: String::new() }
    }
    #[test]
    fn interrupted_flow_with_steam_ready_on_target_is_completed_without_restore() {
        let mut m = Mock::new("previous"); m.account_files = "target".into(); m.hold = true;
        m.started_at = Some(Duration::ZERO); m.t = Duration::from_secs(60);
        assert_eq!(recover(&mut m, &flow("signing_in")), Recovery::Completed);
        assert_eq!(count(&m, "restore"), 0);
        assert!(!m.hold);
    }
    #[test]
    fn interrupted_flow_with_steam_down_and_no_session_starts_steam_and_restores_files() {
        // UI умер между «Steam закрыт» и запуском; hold остался файлом; сессия не поднимает Steam
        let mut m = Mock::new("previous"); m.running = false; m.hold = true; m.no_session = true; m.account_files = "previous".into();
        let r = recover(&mut m, &flow("rewriting"));
        assert_eq!(r, Recovery::BackOnPrevious, "{:?}", m.notes);
        assert_eq!(count(&m, "kick"), 1);
        assert_eq!(count(&m, "restore"), 1, "файлы могли быть изменены");
        assert!(!m.hold);
    }
    #[test]
    fn interrupted_flow_before_any_file_change_does_not_restore() {
        let mut m = Mock::new("previous"); m.running = false; m.hold = true;
        assert_eq!(recover(&mut m, &flow("shutting_down")), Recovery::BackOnPrevious);
        assert_eq!(count(&m, "restore"), 0);
        assert!(!m.hold);
    }
    #[test]
    fn interrupted_flow_with_hung_steam_gives_up_in_bounded_time_and_restores() {
        let mut m = Mock::new("previous"); m.running = false; m.boot = Boot::Never;
        assert_eq!(recover(&mut m, &flow("signing_in")), Recovery::GaveUp);
        assert!(m.t <= START_TIMEOUT + Duration::from_secs(2), "{:?}", m.t);
        assert_eq!(count(&m, "restore"), 1);
    }
    #[test]
    fn flow_state_round_trips_and_old_files_without_backup_dir_load() {
        let f = flow("starting");
        let json = serde_json::to_string(&f).unwrap();
        assert_eq!(serde_json::from_str::<FlowState>(&json).unwrap(), f);
        let old = r#"{"phase":"starting","target":"t","previous":"p","pid":3,"started_unix":9}"#;
        assert_eq!(serde_json::from_str::<FlowState>(old).unwrap().backup_dir, "");
    }

    // ------------------------------------------------------------ файлы
    const USERS: &str = "\"users\"\n{\n\t\"76561198000000001\"\n\t{\n\t\t\"AccountName\"\t\t\"alpha\"\n\t\t\"PersonaName\"\t\t\"A\"\n\t\t\"RememberPassword\"\t\t\"1\"\n\t\t\"AutoLogin\"\t\t\"1\"\n\t\t\"timestamp\"\t\t\"10\"\n\t}\n\t\"76561198000000002\"\n\t{\n\t\t\"AccountName\"\t\t\"Beta\"\n\t\t\"PersonaName\"\t\t\"B\"\n\t\t\"RememberPassword\"\t\t\"1\"\n\t\t\"AutoLogin\"\t\t\"0\"\n\t\t\"timestamp\"\t\t\"5\"\n\t}\n\t\"76561198000000003\"\n\t{\n\t\t\"AccountName\"\t\t\"gamma\"\n\t\t\"RememberPassword\"\t\t\"0\"\n\t}\n}\n";
    const REG: &str = "\"Registry\"\n{\n\t\"HKCU\"\n\t{\n\t\t\"Software\"\n\t\t{\n\t\t\t\"Valve\"\n\t\t\t{\n\t\t\t\t\"Steam\"\n\t\t\t\t{\n\t\t\t\t\t\"language\"\t\t\"russian\"\n\t\t\t\t\t\"AutoLoginUser\"\t\t\"alpha\"\n\t\t\t\t}\n\t\t\t}\n\t\t}\n\t}\n}\n";

    #[test]
    fn saved_users_list_names_and_remember_flags() {
        assert_eq!(saved_users(USERS), vec![("alpha".into(), true), ("Beta".into(), true), ("gamma".into(), false)]);
    }
    #[test]
    fn loginusers_selection_flips_only_the_flags() {
        let out = select_in_loginusers(USERS, "beta").unwrap();
        let rows: Vec<&str> = out.lines().filter(|l| l.contains("AutoLogin")).collect();
        assert_eq!(rows[0].trim(), "\"AutoLogin\"\t\t\"0\"");
        assert_eq!(rows[1].trim(), "\"AutoLogin\"\t\t\"1\"");
        // всё остальное байт в байт
        assert_eq!(out.replace("\"AutoLogin\"\t\t\"0\"", "X").replace("\"AutoLogin\"\t\t\"1\"", "X"), USERS.replace("\"AutoLogin\"\t\t\"0\"", "X").replace("\"AutoLogin\"\t\t\"1\"", "X"));
        assert!(select_in_loginusers(USERS, "nobody").is_err());
    }
    #[test]
    fn target_without_flags_gets_most_recent_and_others_keep_theirs() {
        let out = select_in_loginusers(USERS, "gamma").unwrap();
        assert!(out.contains("\"MostRecent\"\t\t\"1\""));
        assert_eq!(out.matches("\"AutoLogin\"\t\t\"0\"").count(), 2);
    }
    #[test]
    fn registry_autologin_is_replaced_or_inserted() {
        let out = select_in_registry(REG, "Beta").unwrap();
        assert!(out.contains("\"AutoLoginUser\"\t\t\"Beta\"") && !out.contains("alpha"));
        let bare = REG.replace("\t\t\t\t\t\"AutoLoginUser\"\t\t\"alpha\"\n", "");
        let out = select_in_registry(&bare, "Beta").unwrap();
        assert!(out.contains("\"AutoLoginUser\"\t\t\"Beta\""));
        assert_eq!(out.matches('{').count(), REG.matches('{').count());
        assert!(select_in_registry("\"Registry\"\n{\n}\n", "x").is_err());
    }
    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ds-acc-restart-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("config")).unwrap();
        d
    }
    #[test]
    fn files_are_backed_up_rewritten_and_restored_exactly() {
        let d = tmp("files");
        let f = Files { loginusers: d.join("config/loginusers.vdf"), registry: d.join("registry.vdf"), backup_dir: d.join("bk") };
        std::fs::write(&f.loginusers, USERS).unwrap();
        std::fs::write(&f.registry, REG).unwrap();
        assert!(f.prepare("gamma").unwrap_err().contains(crate::expected_text("пароль", "password")));
        assert!(f.prepare("nobody").is_err());
        assert!(!f.backup_dir.exists(), "при отказе бэкап не создаётся");
        f.prepare("beta").unwrap();
        f.rewrite("beta").unwrap();
        assert!(std::fs::read_to_string(&f.registry).unwrap().contains("\"AutoLoginUser\"\t\t\"beta\""));
        assert_ne!(std::fs::read_to_string(&f.loginusers).unwrap(), USERS);
        f.restore().unwrap();
        assert_eq!(std::fs::read_to_string(&f.loginusers).unwrap(), USERS);
        assert_eq!(std::fs::read_to_string(&f.registry).unwrap(), REG);
        let _ = std::fs::remove_dir_all(&d);
    }
    #[test]
    fn a_failed_second_write_leaves_the_first_file_as_it_was() {
        let d = tmp("half");
        let f = Files { loginusers: d.join("config/loginusers.vdf"), registry: d.join("nodir/registry.vdf"), backup_dir: d.join("bk") };
        std::fs::write(&f.loginusers, USERS).unwrap();
        assert!(f.rewrite("beta").is_err());
        assert_eq!(std::fs::read_to_string(&f.loginusers).unwrap(), USERS);
        let _ = std::fs::remove_dir_all(&d);
    }
}

// ---------------------------------------------------------------- настоящий хост (процессы, hold-файл, CDP)

/// Hold-файл, который читает `session/deckshell-session.sh` (`DECKSHELL_STEAM_HOLD` переопределяет).
pub fn hold_path() -> PathBuf {
    std::env::var_os("DECKSHELL_STEAM_HOLD").map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap_or_else(|| "/tmp".into())).join("deckshell-steam-hold"))
}

/// Работает ли клиент Steam этого пользователя (процесс `steam` или его обёртка).
/// Arguments for a Steam DeckShell starts itself. `-silent` is forced: without it the client started in the
/// gamescope session forces the stock Gamepad UI (`ComputeStartupMode: forcing gamepadui for steamdeck + gamescope`).
pub fn headless_args(configured: Option<&str>) -> Vec<String> {
    let mut v: Vec<String> = configured.unwrap_or("-steamos3 -silent").split_whitespace()
        .filter(|a| !matches!(*a, "-gamepadui" | "-bigpicture" | "-steampal" | "-tenfoot")).map(String::from).collect();
    if !v.iter().any(|a| a == "-silent") { v.push("-silent".into()); }
    v
}

fn steam_process_alive() -> bool {
    let uid = std::fs::metadata("/proc/self").map(|m| std::os::unix::fs::MetadataExt::uid(&m)).ok();
    let Ok(dir) = std::fs::read_dir("/proc") else { return false };
    dir.flatten().any(|e| {
        let name = e.file_name();
        let Some(pid) = name.to_str().filter(|n| n.bytes().all(|b| b.is_ascii_digit())) else { return false };
        let Ok(meta) = e.metadata() else { return false };
        if uid.is_some() && std::os::unix::fs::MetadataExt::uid(&meta) != uid.unwrap() { return false; }
        std::fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|c| c.trim() == "steam")
    })
}

fn unix_ms() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

/// Хост для живого Steam. Отмену отдаёт флаг, фазы — колбэк.
pub struct SystemHost<'a> {
    pub files: Files,
    pub steam_bin: PathBuf,
    pub cancel: &'a std::sync::atomic::AtomicBool,
    pub on_phase: Box<dyn FnMut(Phase) + 'a>,
    started: std::time::Instant,
    op_id: String,
    flow: Option<FlowState>,
    flow_file: Option<PathBuf>,
    journal: Option<PathBuf>,
}

impl<'a> SystemHost<'a> {
    pub fn new(files: Files, cancel: &'a std::sync::atomic::AtomicBool, op_id: String, on_phase: Box<dyn FnMut(Phase) + 'a>) -> Self {
        let steam_bin = std::env::var_os("DECKSHELL_STEAM_BIN").map(PathBuf::from).unwrap_or_else(|| "/usr/bin/steam".into());
        // каталог состояния: …/account-switch/backup-N → …/account-switch/
        let dir = files.backup_dir.parent().map(Path::to_path_buf);
        let flow_file = dir.as_ref().map(|d| d.join("flow.json"));
        let journal = dir.map(|d| d.join("flow.log"));
        SystemHost { files, steam_bin, cancel, on_phase, started: std::time::Instant::now(), op_id, flow: None, flow_file, journal }
    }
    fn save_flow(&self) {
        let (Some(f), Some(path)) = (&self.flow, &self.flow_file) else { return };
        if let Some(d) = path.parent() { let _ = std::fs::create_dir_all(d); }
        if let Ok(json) = serde_json::to_string(f) { let _ = write_atomic(path, &json); }
    }
}

impl Host for SystemHost<'_> {
    fn prepare(&mut self, target: &str) -> Result<(), String> { self.files.prepare(target) }
    fn hold(&mut self, on: bool) {
        let p = hold_path();
        if on { let _ = std::fs::write(&p, std::process::id().to_string()); } else { let _ = std::fs::remove_file(&p); }
    }
    fn request_shutdown(&mut self) {
        // Over steam.pipe only: `steam -shutdown` STARTS a client when Steam has already gone, and that client is the
        // stock Gamepad UI (Deck 2026-11-18 14:03:34; docs/steam-ui-parallel-2026-11-20.md).
        if let Err(e) = steam_control::request_shutdown() { self.note(&tr_args("account_restart.shutdown_send_failed", &[("e", &e.to_string())])); }
    }
    fn steam_running(&mut self) -> bool { steam_process_alive() }
    fn rewrite(&mut self, target: &str) -> Result<(), String> { self.files.rewrite(target) }
    fn restore(&mut self) -> Result<(), String> { self.files.restore() }
    fn probe(&mut self) -> Probe {
        // hold-файл освежаем: пока идёт смена, сессия не должна считать его устаревшим
        if hold_path().exists() { let _ = std::fs::write(hold_path(), std::process::id().to_string()); }
        let Ok(b) = crate::Bridge::connect() else { return Probe::Down };
        match b.login_accounts() {
            Ok(s) => Probe::Up { account: s.account_name.filter(|a| !a.is_empty()), state: s.state },
            Err(_) => Probe::Up { account: None, state: None },
        }
    }
    fn nudge_login(&mut self, target: &str) {
        if let Ok(b) = crate::Bridge::connect() { let _ = b.login_cached_account(&self.op_id, target); }
    }
    /// Нажатие B исполняется один раз (флаг снимается); повторное нажатие в откате обрывает ожидание.
    fn cancelled(&mut self) -> bool { self.cancel.swap(false, std::sync::atomic::Ordering::SeqCst) }
    fn phase(&mut self, p: Phase) {
        self.note(&tr_args("account_restart.phase_log", &[("phase", &format!("{:?}", p))]));
        if let Some(f) = self.flow.as_mut() { f.phase = p.key().into(); }
        self.save_flow();
        (self.on_phase)(p)
    }
    fn elapsed(&mut self) -> Duration { self.started.elapsed() }
    fn sleep(&mut self, d: Duration) {
        // hold освежается и во время ожидания закрытия
        if hold_path().exists() { let _ = std::fs::write(hold_path(), std::process::id().to_string()); }
        std::thread::sleep(d)
    }
    fn start_steam(&mut self) -> bool {
        // те же аргументы, что у сессии (`STEAM_SHELL_ARGS`): тихий запуск без окна стокового UI
        let args = headless_args(std::env::var("STEAM_SHELL_ARGS").ok().as_deref());
        std::process::Command::new(&self.steam_bin).args(&args)
            .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn()
            .map(|mut c| { std::thread::spawn(move || { let _ = c.wait(); }); }).is_ok()
    }
    fn note(&mut self, msg: &str) {
        let line = format!("[account-switch] ts_ms={} t=+{:.1}s {msg}", unix_ms(), self.started.elapsed().as_secs_f32());
        eprintln!("{line}");
        if let Some(j) = &self.journal {
            use std::io::Write as _;
            if let Some(d) = j.parent() { let _ = std::fs::create_dir_all(d); }
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(j) { let _ = writeln!(f, "{line}"); }
        }
    }
    fn begin(&mut self, target: &str, previous: &str) {
        self.flow = Some(FlowState {
            phase: "preparing".into(), target: target.into(), previous: previous.into(), pid: std::process::id(),
            started_unix: (unix_ms() / 1000) as u64, backup_dir: self.files.backup_dir.display().to_string(),
        });
        self.save_flow();
    }
    fn finished(&mut self, _: &Outcome) {
        if let Some(p) = &self.flow_file { let _ = std::fs::remove_file(p); }
        self.flow = None;
    }
}

/// Хост подхвата: тот же `SystemHost`, но без UI-колбэков и отмены.
pub fn recover_interrupted(steam_root: &Path, state_dir: &Path) -> Option<Recovery> {
    let path = flow_path(state_dir);
    let text = std::fs::read_to_string(&path).ok()?;
    let flow: FlowState = match serde_json::from_str(&text) {
        Ok(f) => f,
        Err(e) => { eprintln!("{}", tr_args("account_restart.flow_removed_log", &[("e", &e.to_string())])); let _ = std::fs::remove_file(&path); return None; }
    };
    if flow.pid != std::process::id() && Path::new(&format!("/proc/{}", flow.pid)).exists() {
        eprintln!("{}", tr_args("account_restart.flow_running_log", &[("pid", &flow.pid.to_string())]));
        return None;
    }
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let mut files = Files::default_paths(steam_root, &home, state_dir, 0);
    if !flow.backup_dir.is_empty() { files.backup_dir = PathBuf::from(&flow.backup_dir); }
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let mut host = SystemHost::new(files, &cancel, format!("recover-{}", std::process::id()), Box::new(|_| {}));
    let r = recover(&mut host, &flow);
    let _ = std::fs::remove_file(&path);
    Some(r)
}
