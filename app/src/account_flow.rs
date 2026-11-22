//! Смена аккаунта с перезапуском Steam в фоне (задача 430): мост между автоматом `steam_bridge::account_restart`
//! и интерфейсом. Пока идёт перезапуск, поверх DeckShell стоит обложка с котиком (`launch_splash::begin_hold`),
//! подпись под котиком отражает фазу. Всё вызывается из рабочего потока окна входа.
use steam_assets::i18n::{tr_args, tr_ref};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use steam_bridge::account_restart::{self, Files, Host, Outcome, Phase, SystemHost};

use crate::launch_splash;
use crate::screens::defer;

/// Подпись под котиком: фаза и подсказка отмены (в откате отменять уже нечего).
pub fn caption(p: Phase) -> String {
    match p {
        Phase::RollingBack => p.message().to_string(),
        _ => tr_args("account_flow.cancel_hint", &[("progress", p.message())]),
    }
}

/// Итог автомата для окна входа. `Ok` — Steam готов под выбранным аккаунтом (библиотека перечитана отдельно).
pub fn settle(outcome: Outcome) -> Result<(), String> {
    match outcome {
        Outcome::Done => Ok(()),
        Outcome::Cancelled => Err(tr_ref("account_flow.cancelled").into()),
        Outcome::Failed { message, back_on_previous } => Err(if back_on_previous {
            tr_args("account_flow.unchanged", &[("message", &message)])
        } else {
            message
        }),
    }
}

fn state_dir() -> std::path::PathBuf {
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    std::env::var_os("XDG_STATE_HOME").map(std::path::PathBuf::from).unwrap_or_else(|| home.join(".local/state")).join("deckshell")
}

/// Steam пересоздал соединение: перечитать данные аккаунта, пока не получится (библиотека грузится не мгновенно).
fn reload_after_restart(host: &mut SystemHost<'_>, target: &str) -> bool {
    let mut profile_queued = false;
    let mut library_reloaded = false;
    for _ in 0..20 {
        if let Ok(b) = steam_bridge::Bridge::connect() {
            if !profile_queued {
                profile_queued = crate::backend::refresh_profile(&b, Some(target));
                if profile_queued {
                    host.note(tr_ref("account_flow.header_refreshed"));
                }
            }
            library_reloaded = crate::backend::reload_account(&b);
            if library_reloaded { break; }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    if !profile_queued {
        host.note(tr_ref("account_flow.header_pending"));
    }
    library_reloaded
}

/// Жёсткий предел жизни экрана с котиком: общий предел автомата + перечитывание библиотеки + запас.
/// Даже если рабочий поток завис (мост, процесс), по истечении котик снимается и интерфейс возвращается.
pub const SPLASH_HARD_LIMIT: Duration = Duration::from_secs(account_restart::TOTAL_LIMIT.as_secs() + 60);

/// Сторож: по истечении `limit` снимает котика, если поток ещё не закончил (`done`). Возвращает его поток.
fn spawn_watchdog(done: Arc<AtomicBool>, cancel_flag: bool, limit: Duration) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let t0 = std::time::Instant::now();
        while t0.elapsed() < limit {
            if done.load(Ordering::SeqCst) { return; }
            std::thread::sleep(Duration::from_millis(200));
        }
        if !done.swap(true, Ordering::SeqCst) {
            eprintln!("{}", tr_args("account_flow.watchdog", &[("seconds", &limit.as_secs().to_string()), ("cancel_flag", &cancel_flag.to_string())]));
            defer(|_| launch_splash::end_hold("account switch watchdog"));
        }
    })
}

/// Запуск DeckShell: незавершённая смена аккаунта (`flow.json`) отменяется/доводится в фоне, без котика.
/// Снимает устаревший hold, запускает Steam, если сессия этого не сделала, возвращает файлы входа.
pub fn recover_on_start() {
    if std::env::var_os("DECKSHELL_DRYRUN").is_some() || deck_system::guard::active() { return; }
    let dir = state_dir();
    if !account_restart::flow_path(&dir).exists() { return; }
    std::thread::spawn(move || {
        match account_restart::recover_interrupted(&steam_assets::steam_root(), &dir) {
            Some(r) => eprintln!("{}", tr_args("account_flow.startup_recovery", &[("result", &format!("{:?}", r))])),
            None => eprintln!("{}", tr_ref("account_flow.no_startup_recovery")),
        }
        // котик мог остаться от прежней обложки только в этом процессе; на старте его нет — но гарантируем
        defer(|_| launch_splash::end_hold("account switch recovered"));
    });
}

/// Что показать по завершении смены: клип выхода из сна — только при успехе (итог Done и библиотека перечитана).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Finish { ResumeClip, Plain }

pub fn finish_kind(outcome_done: bool, reloaded: bool) -> Finish {
    if outcome_done && reloaded { Finish::ResumeClip } else { Finish::Plain }
}

/// UI thread: убрать лёгкий экран; при успехе — под ним запускается клип выхода из сна. `play` — запуск клипа
/// (true = запущен). Экран снимается всегда, в том же вызове: зависнуть в нём нечему.
pub fn finish_on_ui(kind: Finish, play: impl FnOnce() -> bool, end: impl FnOnce(&str)) {
    let started = kind == Finish::ResumeClip && play();
    let status = if started { tr_ref("account_flow.clip_started") } else { tr_ref("account_flow.clip_unavailable") };
    eprintln!("{}", tr_args("account_flow.wake_clip", &[("status", status)]));
    end(if started { "account switch finished: resume clip" } else { "account switch finished" });
}

/// Рабочий поток. Показывает котика, гоняет автомат, убирает котика. Возвращает итог для окна входа.
pub fn run(cancel: &AtomicBool, id: &str, target: &str, previous: Option<&str>) -> Result<(), String> {
    let Some(previous) = previous.filter(|p| !p.is_empty()) else {
        return Err(tr_ref("account_flow.current_unknown").into());
    };
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let files = Files::default_paths(&steam_assets::steam_root(), &home, &state_dir(), crate::app::now_secs());
    defer(|_| launch_splash::begin_hold(&caption(Phase::Preparing)));
    let mut host = SystemHost::new(files, cancel, id.to_string(), Box::new(|p| defer(move |_| launch_splash::set_caption(&caption(p)))));
    let done = Arc::new(AtomicBool::new(false));
    spawn_watchdog(done.clone(), false, SPLASH_HARD_LIMIT);
    let outcome = account_restart::run(&mut host, target, previous);
    let was_done = matches!(outcome, Outcome::Done);
    let reloaded = !was_done || reload_after_restart(&mut host, target);
    let result = if reloaded { settle(outcome) } else {
        Err(tr_ref("account_flow.library_pending").into())
    };
    done.store(true, Ordering::SeqCst);
    let kind = finish_kind(was_done, reloaded);
    defer(move |_| finish_on_ui(kind, crate::anim::play_resume_clip, launch_splash::end_hold));
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::expected_text;

    #[test]
    fn every_outcome_has_a_localized_message_and_no_hang() {
        assert!(settle(Outcome::Done).is_ok());
        assert!(settle(Outcome::Cancelled).unwrap_err().contains(expected_text("отменена", "canceled")));
        let m = settle(Outcome::Failed { message: "Steam не закрылся.".into(), back_on_previous: true }).unwrap_err();
        assert!(m.contains(expected_text("не изменён", "unchanged")));
        let m = settle(Outcome::Failed { message: "Нет связи.".into(), back_on_previous: false }).unwrap_err();
        assert_eq!(m, "Нет связи.");
    }
    #[test]
    fn caption_offers_cancel_except_while_rolling_back() {
        assert!(caption(Phase::ShuttingDown).contains(expected_text("B — отмена", "B — Cancel")));
        assert!(!caption(Phase::RollingBack).contains(expected_text("отмена", "Cancel")));
    }
    #[test]
    fn watchdog_removes_the_cover_state_when_the_flow_hangs_and_stays_quiet_when_it_finishes() {
        let done = Arc::new(AtomicBool::new(false));
        let h = spawn_watchdog(done.clone(), false, Duration::from_millis(300));
        h.join().unwrap();
        assert!(done.load(Ordering::SeqCst), "сторож сработал по пределу");
        let done = Arc::new(AtomicBool::new(false));
        let h = spawn_watchdog(done.clone(), false, Duration::from_secs(30));
        done.store(true, Ordering::SeqCst);
        let t = std::time::Instant::now();
        h.join().unwrap();
        assert!(t.elapsed() < Duration::from_secs(2), "завершённый поток отпускает сторожа сразу");
    }
    #[test]
    fn resume_clip_only_after_done_with_reloaded_library() {
        assert_eq!(finish_kind(true, true), Finish::ResumeClip);
        assert_eq!(finish_kind(true, false), Finish::Plain);
        assert_eq!(finish_kind(false, true), Finish::Plain);
    }
    #[test]
    fn finish_plays_clip_then_always_drops_the_screen() {
        for (kind, clip_ok, plays, reason) in [
            (Finish::ResumeClip, true, 1, "account switch finished: resume clip"),
            (Finish::ResumeClip, false, 1, "account switch finished"),
            (Finish::Plain, true, 0, "account switch finished"),
        ] {
            let mut played = 0;
            let mut ended = None;
            finish_on_ui(kind, || { played += 1; clip_ok }, |r| ended = Some(r.to_string()));
            assert_eq!(played, plays);
            assert_eq!(ended.as_deref(), Some(reason));
        }
    }
    #[test]
    fn hard_limit_exceeds_the_automaton_limit() {
        assert!(SPLASH_HARD_LIMIT > account_restart::TOTAL_LIMIT);
    }
    #[test]
    fn recovery_is_a_no_op_under_dryrun() {
        recover_on_start();
    }
    #[test]
    fn unknown_current_account_refuses_before_touching_steam() {
        let cancel = AtomicBool::new(false);
        assert!(run(&cancel, "x", "t", None).unwrap_err().contains(expected_text("не перезапускался", "Steam was left running")));
        assert!(run(&cancel, "x", "t", Some("")).is_err());
    }
}
