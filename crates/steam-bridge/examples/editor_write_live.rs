//! Live write cycle of one binding on a Steam Deck (task 210). WRITES to Steam, Balatro only.
//!
//! Steps: read the editable layout (raw bytes kept as backup), write button A to gamepad B,
//! save, read back; write A back to gamepad A (the stock default, value 3), save, read back;
//! restore the original URL/selection type; delete only a proven new autosave copy.
//! Every step prints what it read; the restore is attempted even when a check fails.
//! Usage: STEAM_CDP_PORT=18080 cargo run -p steam-bridge --example editor_write_live -- <controller_index> <backup_dir> [--write]
//! Without --write only the read at the start runs.
//! Ordinary --write refuses an existing personal autosave or an incomplete baseline inventory.
//! Add --noop to --write for a single A -> A write, without saving or deleting a config.
//! Add --unsaved to --write for A -> B -> A with readback and no Save/select/delete calls.
#[path = "support/editor_write_cleanup.rs"]
mod cleanup;

use std::path::PathBuf;

use steam_bridge::controller_edit::{gamepad_binding, BindingTarget};
use steam_bridge::controller_layout::ControllerLayout;
use steam_bridge::Bridge;

const BALATRO: u32 = 2379780;
const DEFAULT_A: i64 = 3; // stock gamepad button value of A (case table in the bundle)
const TEST_B: i64 = 4; // stock gamepad button value of B

/// Button A's binding in the Buttons source mode (input 12, activator 0, binding 0).
fn button_a_target() -> BindingTarget {
    BindingTarget {
        action_set_key: "Default".into(),
        action_set_layer_key: None,
        source_binding_key: 4, // captured source_bindings key; resolved from raw below
        mode_shift: false,
        input_key: 12, // Button_A
        activator_index: 0,
        binding_index: 0,
        source_mode: 3,
        modeid: 0,
    }
}

/// The gamepad value of button A in a layout, or None when the structure differs.
fn button_a_value(layout: &ControllerLayout) -> Option<i64> {
    let mode = layout.modes.iter().find(|m| m.modeid == 0 && m.mode == 3)?;
    let input = mode.inputs.iter().find(|i| i.key == 12)?;
    let b = input.activators.first()?.bindings.first()?;
    b.gamepad_value
}

fn log<T: std::fmt::Debug>(name: &str, r: &T) {
    println!("{name}: {r:?}");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let idx: u32 = args.first().and_then(|s| s.parse().ok()).expect("controller index");
    let dir = PathBuf::from(args.get(1).expect("backup dir"));
    let write = args.iter().any(|a| a == "--write");
    let b = Bridge::connect().expect("connect");

    let active0 = b.layout_active(BALATRO, idx).expect("active before");
    log("active.before", &active0);
    let (raw0, l0) = b.editor_start(BALATRO, idx).expect("start editing");
    std::fs::write(dir.join("20-start-raw.bin"), &raw0).expect("backup raw");
    log("start.url", &l0.url);
    log("start.button_a", &button_a_value(&l0));
    if !write {
        return;
    }
    if button_a_value(&l0) != Some(DEFAULT_A) {
        println!("ABORT: button A is not at the default {DEFAULT_A}; nothing written");
        return;
    }

    let target = button_a_target().resolve_source_binding(&raw0).expect("resolve ABXY source path");
    log("resolved target", &target);
    if args.iter().any(|a| a == "--unsaved") {
        let Some(before) = active0.as_ref().filter(|a| a.url == l0.url) else {
            println!("ABORT: no matching original selection; nothing written");
            return;
        };
        // Keep the selection as well as the complete raw layout before touching the editor.
        std::fs::write(dir.join("20-start-selection.json"), serde_json::to_vec_pretty(&active0.as_ref().map(|a| serde_json::json!({"url":a.url,"selection_type":a.selection_type})))
            .expect("serialize selection")).expect("backup selection");
        let changed = b.editor_set_input_binding(idx, &raw0, &target, &gamepad_binding(TEST_B));
        let changed_ok = match &changed {
            Ok((raw, layout)) => {
                std::fs::write(dir.join("21-unsaved-b-reply.bin"), raw).ok();
                log("set B reply.button_a", &button_a_value(layout));
                button_a_value(layout) == Some(TEST_B)
            }
            Err(e) => { println!("set B error: {e}"); false }
        };
        // The native Set reply is the complete edited layout. StartEditing is NOT a dirty
        // state getter: on Deck it reloads the saved layout and discards the unsaved B.
        let read_b_value = changed.as_ref().ok().and_then(|(_, layout)| button_a_value(layout));
        log("native reply readback unsaved B", &read_b_value);
        // Always attempt restoration, including when the write/read failed. No Save anywhere.
        let restore_raw = changed.as_ref().map(|(raw, _)| raw.as_slice()).unwrap_or(&raw0);
        let restored = b.editor_set_input_binding(idx, restore_raw, &target, &gamepad_binding(DEFAULT_A));
        let restore_ok = match &restored {
            Ok((raw, layout)) => {
                std::fs::write(dir.join("23-unsaved-restore-reply.bin"), raw).ok();
                log("restore A reply.button_a", &button_a_value(layout));
                button_a_value(layout) == Some(DEFAULT_A)
            }
            Err(e) => { println!("restore A error: {e}"); false }
        };
        // End the transient editing session, then independently reload the active layout.
        let stop = b.eval_json_once(&format!("(async()=>{{await SteamClient.Input.StopEditingControllerConfiguration({idx});return true}})()"));
        log("stop editing", &stop);
        let (raw_final, final_layout) = b.editor_start(BALATRO, idx).expect("final readback");
        std::fs::write(dir.join("24-unsaved-final.bin"), raw_final).expect("backup final readback");
        let active_after = b.layout_active(BALATRO, idx).expect("active after");
        let selection_restored = active_after.as_ref().is_some_and(|z| cleanup::same_selection(before, z));
        let final_a = button_a_value(&final_layout);
        log("readback final button_a", &final_a);
        let final_stop = b.eval_json_once(&format!("(async()=>{{await SteamClient.Input.StopEditingControllerConfiguration({idx});return true}})()"));
        log("stop after final readback", &final_stop);
        println!("RESULT unsaved changed={changed_ok} read_b={read_b_value:?} restore_reply={restore_ok} final_a={final_a:?} selection_restored={selection_restored}; no Save/select/delete");
        assert!(selection_restored && final_a == Some(DEFAULT_A) && final_stop.is_ok(), "original layout not verified or editor still open");
        assert!(changed_ok && read_b_value == Some(TEST_B) && restore_ok, "write cycle rejected or readback failed");
        return;
    }
    if args.iter().any(|a| a == "--noop") {
        let (raw, layout) = b.editor_set_input_binding(idx, &raw0, &target, &gamepad_binding(DEFAULT_A))
            .expect("no-op write (no save/cleanup attempted)");
        std::fs::write(dir.join("21-noop-reply.bin"), raw).expect("backup no-op reply");
        assert_eq!(button_a_value(&layout), Some(DEFAULT_A));
        println!("RESULT no-op accepted; no save or cleanup attempted");
        return;
    }
    let Some(before) = active0.as_ref() else {
        println!("ABORT: no original selection to restore; nothing written");
        return;
    };
    // The stock query includes user configs (GetUserConfigsForApp uses this same list).
    // Stable row counts alone are insufficient: require the native query to have finished.
    let inventory = b.layout_list(BALATRO, idx, true).ok().and_then(|list| {
        let complete = b.eval_json("window.controllerConfiguratorStore.BConfigurationQueryInFlight === false").ok()?;
        (complete.as_bool() == Some(true)).then(|| list.into_iter().map(|e| e.url).collect::<Vec<_>>())
    });
    if !cleanup::can_write(before, &l0.url, inventory.as_deref())
        || steam_bridge::controller_list::select_expr(BALATRO, idx, &before.url, before.selection_type).is_err()
    {
        println!("ABORT: unknown baseline, existing personal autosave, or editor/active URL mismatch; nothing written");
        return;
    }
    let still_original = b.layout_active(BALATRO, idx).ok().flatten()
        .is_some_and(|a| cleanup::same_selection(&a, before));
    if !still_original {
        println!("ABORT: selection changed during preflight; nothing written");
        return;
    }
    // Persist selection metadata next to the raw backup before the first write.
    std::fs::write(dir.join("20-start-selection.json"), serde_json::to_vec_pretty(&serde_json::json!({
        "url": before.url, "selection_type": before.selection_type, "urls": inventory,
    })).expect("serialize baseline")).expect("backup selection");
    let mut ok = true;
    match b.editor_set_input_binding(idx, &raw0, &target, &gamepad_binding(TEST_B)) {
        Ok((raw, l)) => {
            std::fs::write(dir.join("21-after-set-b-reply.bin"), &raw).ok();
            let v = button_a_value(&l);
            log("set B reply.button_a", &v);
            ok &= v == Some(TEST_B);
        }
        Err(e) => {
            println!("set B error: {e}");
            ok = false;
        }
    }
    let save_b = b.editor_save(idx, before.selection_type, false);
    log("save after B", &save_b.as_ref().map(|_| ()).map_err(|e| e.to_string()));
    let saved_copy = b.layout_active(BALATRO, idx).ok().flatten();
    ok &= save_b.is_ok();
    if let Ok((raw, l)) = b.editor_start(BALATRO, idx) {
        std::fs::write(dir.join("22-readback-after-save-b.bin"), &raw).ok();
        let v = button_a_value(&l);
        log("readback after save B", &v);
        ok &= v == Some(TEST_B);
    } else {
        ok = false;
    }

    // Restore: A back to its default, then save. Always attempted.
    let restore = b.editor_set_input_binding(idx, &raw0, &target, &gamepad_binding(DEFAULT_A));
    let restore_ok = match &restore {
        Ok((raw, l)) => {
            std::fs::write(dir.join("23-restore-reply.bin"), raw).ok();
            button_a_value(l) == Some(DEFAULT_A)
        }
        Err(_) => false,
    };
    log("restore A reply ok", &restore_ok);
    let save_a = b.editor_save(idx, before.selection_type, false);
    log("save after restore", &save_a.as_ref().map(|_| ()).map_err(|e| e.to_string()));
    let readback = b.editor_start(BALATRO, idx);
    let final_a = readback.as_ref().ok().and_then(|(raw, l)| {
        std::fs::write(dir.join("24-readback-final.bin"), raw).ok();
        button_a_value(l)
    });
    log("readback final button_a", &final_a);
    println!("RESULT write_ok={ok} restored={}", final_a == Some(DEFAULT_A));

    let current = b.layout_active(BALATRO, idx).ok().flatten();
    let copy = cleanup::created_copy(
        before, &l0.url, inventory.as_deref(), saved_copy.as_ref(), current.as_ref(),
        ok && restore_ok && save_a.is_ok() && final_a == Some(DEFAULT_A),
    );
    // Clear only a proven test selection, using its actual type. Always attempt the original
    // selection afterwards, even if clearing failed. Never delete until restoration is read back.
    if let Some(copy) = copy {
        let clear = b.layout_clear(BALATRO, idx, copy.selection_type);
        log("clear test selection", &clear.as_ref().map_err(|e| e.to_string()));
    }
    let select = b.layout_select(BALATRO, idx, &before.url, before.selection_type);
    log("restore original selection", &select.as_ref().map_err(|e| e.to_string()));
    let after = b.layout_active(BALATRO, idx);
    log("active.after cleanup", &after);
    if let Some(copy) = copy.filter(|_| cleanup::restoration_verified(
        before, after.as_ref().ok().and_then(|a| a.as_ref()), select.is_ok(),
    )) {
        let r = b.eval_json_once(&delete_copy_expr(&copy.url));
        log("delete proven new copy", &r.as_ref().map(|_| ()).map_err(|e| e.to_string()));
    } else {
        println!("CLEANUP deletion skipped: new-copy ownership or restoration not verified");
    }
    let after_delete = b.layout_active(BALATRO, idx);
    let restored = matches!(&after_delete, Ok(Some(a)) if cleanup::same_selection(a, before));
    println!("CLEANUP original_selection_restored={restored}");
}

/// Delete only; selection restoration is separate and verified before this call.
fn delete_copy_expr(url: &str) -> String {
    let url_js = serde_json::to_string(url).expect("serialize URL");
    format!("(async()=>{{await SteamClient.Input.DeletePersonalControllerConfiguration({url_js});return true}})()")
}
