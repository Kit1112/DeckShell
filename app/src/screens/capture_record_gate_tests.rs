use crate::screens::expected_text;
use super::*;

const SEC: fn(u64) -> Duration = Duration::from_secs;

#[test]
fn start_within_the_cooldown_after_a_stop_is_refused() {
    // Task 182 failure: Stop at 23:58:43.19, Start at 23:58:53.93 (10.7 s later), GPU ring timeout 18 s after it.
    let ended = Instant::now();
    let gap = Duration::from_millis(10_740);
    let wait = start_wait(ended + gap, Some(ended), false).expect("refused");
    assert!(matches!(wait, StartWait::Cooldown(left) if left > SEC(49) && left < SEC(50)));
}

#[test]
fn start_after_a_long_gap_is_allowed_like_the_good_run() {
    // Good run: Stop 23:52:55.58, Start 23:54:39.58 (104 s later).
    let ended = Instant::now();
    assert_eq!(start_wait(ended + Duration::from_millis(104_000), Some(ended), false), None);
    assert_eq!(start_wait(ended + START_COOLDOWN, Some(ended), false), None);
    assert!(start_wait(ended + START_COOLDOWN - Duration::from_millis(1), Some(ended), false).is_some());
}

#[test]
fn no_session_end_means_no_wait_and_an_open_save_always_waits() {
    let now = Instant::now();
    assert_eq!(start_wait(now, None, false), None);
    assert_eq!(start_wait(now, None, true), Some(StartWait::SaveOpen));
    assert_eq!(start_wait(now + SEC(600), Some(now), true), Some(StartWait::SaveOpen));
}

#[test]
fn cooldown_message_rounds_up_and_names_the_reason() {
    let text = wait_message(StartWait::Cooldown(Duration::from_millis(49_260)));
    let expected = expected_text("50 с", "50 sec");
    assert!(text.contains(expected), "{text}");
    assert!(wait_message(StartWait::SaveOpen).contains(expected_text("сохраняет", "saving")));
}

#[test]
fn repeated_presses_never_send_a_second_start_while_one_is_unconfirmed() {
    // Start sent, reply lost or session not yet observed: the next presses wait, then expire. No replay.
    let now = Instant::now();
    assert_eq!(press_outcome(SEC(0), true, false, true, now, None, false), PressOutcome::Queued);
    assert_eq!(press_outcome(Duration::from_millis(9_900), true, false, true, now, None, false), PressOutcome::Queued);
    assert_eq!(press_outcome(SEC(10), true, false, true, now, None, false), PressOutcome::Expired);
    assert_eq!(press_outcome(SEC(30), true, true, false, now, None, false), PressOutcome::Expired);
}

#[test]
fn a_lost_stop_reply_is_never_replayed_as_stop_or_start() {
    // Stop was sent (pending) and the session still shows active: presses must not stop again.
    let now = Instant::now();
    assert_eq!(press_outcome(SEC(1), true, true, false, now, Some(now), true), PressOutcome::Queued);
    assert_eq!(press_outcome(SEC(12), true, true, false, now, Some(now), true), PressOutcome::Expired);
}

#[test]
fn stop_is_the_only_action_while_a_session_is_observed() {
    let now = Instant::now();
    assert_eq!(press_outcome(SEC(0), false, true, false, now, None, false), PressOutcome::Stop);
    // A Start cannot be chosen while a session is active, even if a stale read says it could.
    assert_eq!(press_outcome(SEC(0), false, true, true, now, None, false), PressOutcome::Stop);
}

#[test]
fn press_after_stop_waits_then_starts_once_the_gate_opens() {
    let stop = Instant::now();
    // Press 10.7 s after the Stop: refused with the remaining cooldown.
    let at = stop + Duration::from_millis(10_740);
    assert!(matches!(press_outcome(SEC(0), false, false, true, at, Some(stop), false), PressOutcome::Wait(StartWait::Cooldown(_))));
    // Save still open after the cooldown: still refused.
    assert_eq!(press_outcome(SEC(0), false, false, true, stop + START_COOLDOWN, Some(stop), true), PressOutcome::Wait(StartWait::SaveOpen));
    // Gate open: exactly one Start.
    assert_eq!(press_outcome(SEC(0), false, false, true, stop + START_COOLDOWN, Some(stop), false), PressOutcome::Start);
    assert_eq!(press_outcome(SEC(0), false, false, false, stop + START_COOLDOWN, Some(stop), false), PressOutcome::Unavailable);
}
