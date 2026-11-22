//! End the UI loop before process teardown, so graphics are not torn down while rendering.
use std::sync::atomic::{AtomicI32, Ordering};

/// Exit codes the session wrapper (session/deckshell-session.sh) understands.
/// Only the owner's explicit menu choice may use `STOCK_STEAM`.
pub const STOCK_STEAM: i32 = 42;
pub const DESKTOP: i32 = 43;
/// Internal failure that must not bring up the stock UI: the wrapper restarts DeckShell.
pub const RESTART_UI: i32 = 44;

static EXIT_CODE: AtomicI32 = AtomicI32::new(0);

fn record(slot: &AtomicI32, code: i32) {
    // Preserve the first handoff if two independent shutdown paths race.
    let _ = slot.compare_exchange(0, code, Ordering::SeqCst, Ordering::SeqCst);
}

fn schedule_handoff<E>(slot: &AtomicI32, code: i32, schedule: impl FnOnce() -> Result<(), E>) -> Result<(), E> {
    record(slot, code);
    schedule()
}

pub fn request(code: i32) {
    if let Err(e) = schedule_handoff(&EXIT_CODE, code, || slint::invoke_from_event_loop(|| { let _ = slint::quit_event_loop(); })) {
        eprintln!("[deckshell] could not request UI shutdown: {e}");
    }
}

pub fn take() -> Option<i32> {
    match EXIT_CODE.swap(0, Ordering::SeqCst) { 0 => None, code => Some(code) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn power_handoff_preserves_first_exit_code() {
        let slot = AtomicI32::new(0);
        let mut queued = 0;
        schedule_handoff(&slot, 42, || { queued += 1; Ok::<_, ()>(()) }).unwrap();
        schedule_handoff(&slot, 43, || { queued += 1; Ok::<_, ()>(()) }).unwrap();
        assert_eq!(queued, 2);
        assert_eq!(slot.swap(0, Ordering::SeqCst), 42);
    }
}
