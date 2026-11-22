//! gamescope root-window control atoms behind the QAM Performance rows (task 242).
//!
//! Steam's SystemPerfStore is absent on the Deck (task 240), so these atoms are the source instead. They are
//! read and written through DeckShell's own X connection to gamescope's Xwayland (DISPLAY), the same connection
//! type `gamescope.rs` uses. Values are 32-bit CARDINALs on the root window (`xprop -root -f NAME 32c`).
//!
//! What a readback proves depends on the atom (checked on the Deck, 2026-11-13, with no game running):
//! - `GAMESCOPE_FPS_LIMIT` is stored as written; gamescope exposes no separate applied-limit atom, so the echo is
//!   all there is. The limit is not shown as applied on the Deck until a game is measured.
//! - `GAMESCOPE_DYNAMIC_REFRESH` is stored as written, but `GAMESCOPE_DISPLAY_REFRESH_RATE_FEEDBACK` stayed at 90
//!   after a write of 60 in the shell. The refresh row therefore stays unavailable.
//! - `GAMESCOPE_VRR_CAPABLE` is 0 on the Deck, so VRR stays hidden (stock hides it when not supported).
use std::time::Duration;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, PropMode};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

pub const FPS_LIMIT: &str = "GAMESCOPE_FPS_LIMIT";
pub const SCALING_FILTER: &str = "GAMESCOPE_NEW_SCALING_FILTER";
pub const SCALING_SCALER: &str = "GAMESCOPE_NEW_SCALING_SCALER";
pub const DYNAMIC_REFRESH: &str = "GAMESCOPE_DYNAMIC_REFRESH";
const REFRESH_GAME_POLICY: &str = "GAMESCOPE_DISPLAY_DYNAMIC_REFRESH_BASED_ON_GAME_PRESENCE";
const REFRESH_FEEDBACK: &str = "GAMESCOPE_DISPLAY_REFRESH_RATE_FEEDBACK";
const VRR_CAPABLE: &str = "GAMESCOPE_VRR_CAPABLE";
const VRR_ENABLED: &str = "GAMESCOPE_VRR_ENABLED";
const COLOR_NIGHT_MODE: &str = "GAMESCOPE_COLOR_NIGHT_MODE";

fn night_mode_atom(c: &RustConnection) -> Option<u32> {
    let atom = c.intern_atom(true, COLOR_NIGHT_MODE.as_bytes()).ok()?.reply().ok()?.atom;
    (atom != 0).then_some(atom)
}

fn night_mode_values(c: &RustConnection, root: u32, atom: u32) -> Option<[u32; 3]> {
    let reply = c.get_property(false, root, atom, AtomEnum::CARDINAL, 0, 3).ok()?.reply().ok()?;
    let mut values = reply.value32()?;
    (reply.bytes_after == 0).then_some([values.next()?, values.next()?, values.next()?])
}

/// The compositor owns this atom. A missing property on gamescope is the identity transform.
pub fn night_mode_active() -> Option<bool> {
    let (c, root) = connect()?;
    let atom = night_mode_atom(&c)?;
    Some(night_mode_values(&c, root, atom).is_some_and(|v| f32::from_bits(v[0]) > 0.0))
}

fn night_mode_payload(amount: f32, hue_degrees: f32, saturation: f32) -> [u32; 3] {
    [amount, hue_degrees / 360.0, saturation].map(f32::to_bits)
}

pub fn write_night_mode(amount: f32, hue_degrees: f32, saturation: f32) -> Result<(), String> {
    if cfg!(test) || crate::actions::dryrun() { return Ok(()); }
    let (c, root) = connect().ok_or("no X connection (DISPLAY)")?;
    let atom = night_mode_atom(&c).ok_or("gamescope night mode unavailable")?;
    // gamescope decodes three float bit patterns, with hue expressed as a fraction of a turn.
    let values = night_mode_payload(amount, hue_degrees, saturation);
    c.change_property32(PropMode::REPLACE, root, atom, AtomEnum::CARDINAL, &values)
        .map_err(|e| e.to_string())?.check().map_err(|e| e.to_string())?;
    if night_mode_values(&c, root, atom) == Some(values) {
        Ok(())
    } else {
        Err("gamescope night mode property did not retain the request".into())
    }
}

/// Values read from the root window. `None` = the atom is absent (not gamescope, or not set yet).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub fps_limit: Option<u32>,
    pub scaling_filter: Option<u32>,
    pub scaling_scaler: Option<u32>,
    pub dynamic_refresh: Option<u32>,
    pub refresh_feedback: Option<u32>,
    pub refresh_game_policy: Option<bool>,
    pub vrr_capable: Option<bool>,
    pub vrr_enabled: Option<bool>,
}

impl Snapshot {
    fn is_empty(&self) -> bool {
        *self == Snapshot::default()
    }
}

fn connect() -> Option<(RustConnection, u32)> {
    let (c, screen) = RustConnection::connect(None).ok()?;
    let root = c.setup().roots.get(screen)?.root;
    Some((c, root))
}

/// First CARDINAL of a root property, or None when the atom does not exist or holds another type.
fn root_cardinal(c: &RustConnection, root: u32, name: &str) -> Option<u32> {
    let atom = c.intern_atom(true, name.as_bytes()).ok()?.reply().ok()?.atom;
    if atom == 0 {
        return None;
    }
    let r = c.get_property(false, root, atom, AtomEnum::CARDINAL, 0, 1).ok()?.reply().ok()?;
    if r.format != 32 {
        return None;
    }
    let first = r.value32()?.next();
    first
}

/// Snapshot of the gamescope atoms. None when DISPLAY is not a gamescope server (no atom answered).
pub fn read() -> Option<Snapshot> {
    let (c, root) = connect()?;
    let snap = Snapshot {
        fps_limit: root_cardinal(&c, root, FPS_LIMIT),
        scaling_filter: root_cardinal(&c, root, SCALING_FILTER),
        scaling_scaler: root_cardinal(&c, root, SCALING_SCALER),
        dynamic_refresh: root_cardinal(&c, root, DYNAMIC_REFRESH),
        refresh_feedback: root_cardinal(&c, root, REFRESH_FEEDBACK),
        refresh_game_policy: root_cardinal(&c, root, REFRESH_GAME_POLICY).map(|v| v != 0),
        vrr_capable: root_cardinal(&c, root, VRR_CAPABLE).map(|v| v != 0),
        vrr_enabled: root_cardinal(&c, root, VRR_ENABLED).map(|v| v != 0),
    };
    (!snap.is_empty()).then_some(snap)
}

/// Write one CARDINAL atom on the root window and read it back on the same connection. Ok(value) only when the
/// read value equals the request; the caller must not present anything else as applied.
pub fn write_cardinal(name: &str, value: u32) -> Result<u32, String> {
    let (c, root) = connect().ok_or("no X connection (DISPLAY)")?;
    let atom = c.intern_atom(false, name.as_bytes()).map_err(|e| e.to_string())?.reply().map_err(|e| e.to_string())?.atom;
    c.change_property32(PropMode::REPLACE, root, atom, AtomEnum::CARDINAL, &[value]).map_err(|e| e.to_string())?;
    c.flush().map_err(|e| e.to_string())?;
    for _ in 0..10 {
        if root_cardinal(&c, root, name) == Some(value) {
            return Ok(value);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(format!("{name}: readback {:?} != {value}", root_cardinal(&c, root, name)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_snapshot_means_no_gamescope_atoms() {
        assert!(Snapshot::default().is_empty());
        let s = Snapshot { fps_limit: Some(60), ..Snapshot::default() };
        assert!(!s.is_empty());
    }

    #[test]
    fn night_mode_uses_the_native_cardinal_layout() {
        // Deck :0, with Steam's 25-degree hue and .95 saturation while off.
        assert_eq!(night_mode_payload(0.0, 25.0, 0.95), [0, 1032730852, 1064514355]);
        assert_eq!(night_mode_payload(0.509, 25.0, 0.95)[0], 0.509f32.to_bits());
    }
}
