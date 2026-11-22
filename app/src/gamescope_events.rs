//! Wake layer reconciliation on X11 writes, including before Steam's CDP is ready.
//!
//! PropertyNotify is delivered AFTER a write. This reduces repair latency; it is
//! not an authorization barrier and cannot promise that Gamescope saw no Steam frame.
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::thread::Thread;
use x11rb::{connection::Connection, protocol::{Event, xproto::{ChangeWindowAttributesAux, ConnectionExt, EventMask, Window}}};
use x11rb::rust_connection::RustConnection;

pub(super) struct LayerEvents {
    dirty: AtomicBool,
    disconnected: AtomicBool,
}

impl LayerEvents {
    fn new() -> Self {
        // Reconcile once even if the competing write preceded our subscription.
        Self { dirty: AtomicBool::new(true), disconnected: AtomicBool::new(false) }
    }

    fn notify(&self, event: &Event, root: Window, layer: u32) -> bool {
        let relevant = matches!(event, Event::PropertyNotify(e) if e.window == root && e.atom == layer);
        if relevant { self.dirty.store(true, Ordering::Release); }
        relevant
    }

    pub(super) fn take(&self) -> bool {
        // On connection failure retain the old worker's periodic recovery.
        self.dirty.swap(false, Ordering::AcqRel) || self.disconnected.load(Ordering::Acquire)
    }
}

/// Subscribe synchronously before the first claim. A separate connection blocks
/// in wait_for_event, so UI intents can still unpark the reconciliation worker.
/// The reader only wakes that worker: it never writes an old intent itself.
pub(super) fn watch(worker: Thread) -> Result<Arc<LayerEvents>, Box<dyn std::error::Error>> {
    let (conn, screen) = RustConnection::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let layer = conn.intern_atom(false, b"GAMESCOPECTRL_BASELAYER_APPID")?.reply()?.atom;
    conn.change_window_attributes(root, &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE))?.check()?;
    conn.flush()?;
    let events = Arc::new(LayerEvents::new());
    let reader = events.clone();
    std::thread::Builder::new().name("gamescope-events".into()).spawn(move || {
        loop {
            match conn.wait_for_event() {
                Ok(event) => if reader.notify(&event, root, layer) { worker.unpark(); },
                Err(error) => {
                    eprintln!("[deckshell] gamescope: layer event connection lost: {error}; falling back to polling");
                    reader.disconnected.store(true, Ordering::Release);
                    worker.unpark();
                    return;
                }
            }
        }
    })?;
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use x11rb::protocol::xproto::{PropertyNotifyEvent, Property, PROPERTY_NOTIFY_EVENT};

    fn event(root: Window, atom: u32, state: Property) -> Event {
        Event::PropertyNotify(PropertyNotifyEvent { response_type: PROPERTY_NOTIFY_EVENT,
            sequence: 0, window: root, atom, time: 0, state })
    }

    #[test]
    fn subscribes_with_an_initial_reconciliation_and_coalesces_writes() {
        let events = LayerEvents::new();
        assert!(events.take());
        assert!(!events.take());
        for _ in 0..10 { assert!(events.notify(&event(1, 2, Property::NEW_VALUE), 1, 2)); }
        assert!(events.take());
        assert!(!events.take());
        assert!(events.notify(&event(1, 2, Property::DELETE), 1, 2));
        assert!(events.take());
    }

    #[test]
    fn ignores_other_windows_atoms_and_events() {
        let events = LayerEvents::new();
        events.take();
        assert!(!events.notify(&event(3, 2, Property::NEW_VALUE), 1, 2));
        assert!(!events.notify(&event(1, 3, Property::NEW_VALUE), 1, 2));
        assert!(!events.take());
    }

    /// A mock X server: a competing writer publishes first, then PropertyNotify
    /// wakes the policy. CDP readiness never participates in the repair path.
    #[test]
    fn guide_qam_and_steam_restart_reconcile_without_cdp() {
        use super::super::{LayerPolicy, BP_APPID, STEAM_APPID, HOME};
        let now = std::time::Instant::now();
        let mut policy = LayerPolicy::default();
        let events = LayerEvents::new();
        events.take();
        policy.request(HOME, 0, &[BP_APPID], now);
        for bridge_ready in [false, true, false, true] {
            // Includes a replacement CEF window after Steam restarts. We watch
            // root, not an old Steam XID, so the subscription remains applicable.
            for _button in ["Steam", "QAM"] {
                let mut layer = vec![413091, STEAM_APPID];
                assert!(events.notify(&event(1, 2, Property::NEW_VALUE), 1, 2));
                assert!(events.take());
                layer = policy.repair_layer(&layer, &[BP_APPID, STEAM_APPID]).unwrap();
                assert_eq!(layer, vec![BP_APPID], "bridge_ready={bridge_ready}");
                // Our own write also notifies. Correct state produces no loop.
                events.notify(&event(1, 2, Property::NEW_VALUE), 1, 2);
                assert!(events.take());
                assert_eq!(policy.repair_layer(&layer, &[BP_APPID, STEAM_APPID]), None);
            }
        }
    }

    #[test]
    fn delayed_event_uses_new_delegation_and_selected_game() {
        use super::super::{LayerPolicy, BP_APPID, STEAM_APPID, HOME, STEAM_UI, GAME};
        let now = std::time::Instant::now();
        let apps = [BP_APPID, STEAM_APPID, 42];
        let mut policy = LayerPolicy::default();
        let events = LayerEvents::new();
        events.take();
        policy.request(HOME, 0, &apps, now);
        events.notify(&event(1, 2, Property::NEW_VALUE), 1, 2);
        // Explicit native UI (Store/account flow) wins over an older queued event.
        policy.request(STEAM_UI, 0, &apps, now);
        assert!(events.take());
        assert_eq!(policy.repair_layer(&[STEAM_APPID], &apps), None);
        policy.request(GAME, 42, &apps, now);
        events.notify(&event(1, 2, Property::NEW_VALUE), 1, 2);
        assert!(events.take());
        assert_eq!(policy.repair_layer(&[STEAM_APPID], &apps), Some(vec![42, BP_APPID]));
    }

    #[test]
    fn property_notification_cannot_prevent_an_intermediate_steam_layer() {
        use super::super::{LayerPolicy, BP_APPID, STEAM_APPID};
        let policy = LayerPolicy::default();
        let events = LayerEvents::new();
        events.take();
        let layer = vec![413091, STEAM_APPID]; // XChangeProperty has already succeeded.
        // Gamescope may consume the write before our notification is scheduled.
        assert!(layer.contains(&STEAM_APPID));
        assert!(!events.take());
        events.notify(&event(1, 2, Property::NEW_VALUE), 1, 2);
        assert!(events.take());
        assert_eq!(policy.repair_layer(&layer, &[BP_APPID, STEAM_APPID]), Some(vec![BP_APPID]));
    }

    #[test]
    fn disconnect_retains_fallback_reconciliation() {
        let events = LayerEvents::new();
        events.take();
        events.disconnected.store(true, Ordering::Release);
        assert!(events.take());
        assert!(events.take());
    }
}
