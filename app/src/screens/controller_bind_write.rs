//! Writes of bindings and activators into the native editing copy, with readback and rollback (task 578).
//!
//! Every write is: send one request, read the returned layout, compare with what was asked. A mismatch or an error
//! triggers one rollback request that is read back the same way. Outcomes are explicit: `Done`, `RolledBack`
//! (previous state restored and confirmed) or `Unsure` (the editing copy cannot be trusted any more).
//! Wire shapes are those of the stock configurator (`SetControllerInputBinding` / `SetControllerInputActivator`
//! in sp.js): a new command appends with binding index -1; a removal omits `new_binding`; an activator without
//! `new_activation` is deleted; `EControllerActivationType` numbers come from the stock enum module.
use super::controller_live as live;
use crate::assets::t_or;
use steam_bridge::controller_edit::BindingTarget;
use steam_bridge::{Bridge, Result};

/// The three native calls a write needs. `raw` is the editing copy the request is addressed against.
pub trait Editor {
    fn set_binding(&self, raw: &[u8], target: &BindingTarget, binding: &[u8]) -> Result<Vec<u8>>;
    fn remove_binding(&self, raw: &[u8], target: &BindingTarget) -> Result<Vec<u8>>;
    fn set_activator(&self, raw: &[u8], target: &BindingTarget, activation: Option<i64>) -> Result<Vec<u8>>;
    fn set_mode(&self, target: &BindingTarget, mode: i64) -> Result<Vec<u8>>;
}

/// The live Steam client: controller index plus the CDP bridge.
pub struct Live<'a>(pub &'a Bridge, pub u32);

impl Editor for Live<'_> {
    fn set_binding(&self, raw: &[u8], target: &BindingTarget, binding: &[u8]) -> Result<Vec<u8>> {
        self.0.editor_set_input_binding(self.1, raw, target, binding).map(|r| r.0)
    }
    fn remove_binding(&self, raw: &[u8], target: &BindingTarget) -> Result<Vec<u8>> {
        self.0.editor_remove_input_binding(self.1, raw, target).map(|r| r.0)
    }
    fn set_activator(&self, raw: &[u8], target: &BindingTarget, activation: Option<i64>) -> Result<Vec<u8>> {
        self.0.editor_set_input_activator(self.1, raw, target, activation).map(|r| r.0)
    }
    fn set_mode(&self, target: &BindingTarget, mode: i64) -> Result<Vec<u8>> {
        self.0.editor_set_source_mode(self.1, target, mode).map(|r| r.0)
    }
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// Steam returned exactly what was asked.
    Done(Vec<u8>),
    /// The request failed or differed; the previous state was restored and confirmed by a second readback.
    RolledBack(String, Vec<u8>),
    /// Neither the request nor the rollback could be confirmed.
    Unsure(String),
}

fn at_first(target: &BindingTarget) -> BindingTarget {
    let mut t = target.clone();
    if t.binding_index < 0 { t.binding_index = 0; }
    t
}

/// Remove a binding that this session created and the activator it left empty; confirm both by readback.
/// `target` addresses the binding (binding index 0 of its activator).
pub fn remove_new_binding(ed: &dyn Editor, raw: &[u8], target: &BindingTarget) -> Result<Vec<u8>> {
    let target = at_first(target);
    let mut current = ed.remove_binding(raw, &target)?;
    if live::binding_at(&current, &target).is_ok() {
        return Err(steam_bridge::Error::Protocol("Steam не удалил команду".into()));
    }
    // The stock deletes an emptied activator too; leave the input exactly as it was (no activators).
    if live::activator_count(&current, &target) > target.activator_index as usize {
        current = ed.set_activator(&current, &target, None)?;
        if live::activator_count(&current, &target) > target.activator_index as usize {
            return Err(steam_bridge::Error::Protocol("Steam не удалил пустой активатор".into()));
        }
    }
    Ok(current)
}

/// Set a command. `target.binding_index` -1 appends a new command on an empty input; otherwise the addressed
/// command is replaced. Rolls back (previous command restored, or the new one removed) when readback differs.
pub fn write_binding(ed: &dyn Editor, raw: &[u8], target: &BindingTarget, binding: &[u8]) -> Outcome {
    let read = at_first(target);
    let old = (target.binding_index >= 0).then(|| live::binding_at(raw, target).ok()).flatten();
    if target.binding_index >= 0 && old.is_none() {
        return Outcome::Unsure("Исходная команда не найдена в копии раскладки".into());
    }
    let mut base = raw.to_vec();
    let why = match ed.set_binding(raw, target, binding) {
        Ok(reply) => match live::binding_at(&reply, &read).map_err(|e| e.to_string()).and_then(|got| live::same_command(&got, binding).map_err(|e| e.to_string())) {
            Ok(true) => return Outcome::Done(reply),
            Ok(false) => { base = reply; "Steam вернул другую команду".to_string() }
            Err(e) => { base = reply; e }
        },
        Err(e) => e.to_string(),
    };
    // Rollback: restore the old command, or take the new one away again.
    let rolled = match &old {
        Some(previous) => ed.set_binding(&base, &read, previous).ok()
            .filter(|r| live::binding_at(r, &read).ok().is_some_and(|got| live::same_command(&got, previous).unwrap_or(false))),
        None if live::binding_at(&base, &read).is_ok() || live::activator_count(&base, &read) > read.activator_index as usize => remove_new_binding(ed, &base, &read).ok(),
        // The request never reached the copy: nothing was added, so the base is already the original.
        None => Some(base.clone()),
    };
    match rolled {
        Some(r) => Outcome::RolledBack(why, r),
        None => Outcome::Unsure(why),
    }
}

/// Entries of the stock «Способ включения» list in the stock order (`vr` in sp.js): (`EControllerActivationType`, name id).
/// Digital inputs omit «Слабое нажатие» and «Аналоговый» (`filter_digital`); `always_on` inputs omit those and
/// «Двойное нажатие» (`filter_alwayson`). A digital input therefore gets the six values 1, 6, 5, 3, 4, 8.
/// A trigger click or pressure-capable trackpad click can use analog activators.
pub fn pressure_activator(target: &BindingTarget, deck: bool) -> bool {
    let source = target.source_binding_key;
    match target.input_key {
        13 => matches!(source, 6 | 7) || (deck && matches!(source, 1 | 2)),
        1 | 3 | 5 | 7 | 15 => matches!(source, 1 | 2 | 3 | 6 | 7 | 12),
        _ => false,
    }
}

pub fn activation_options(analog: bool, always_on: bool) -> Vec<(i64, &'static str)> {
    const ALL: [(i64, &str, bool, bool); 8] = [
        (1, "FullPress", false, false), (6, "DoublePress", false, true), (5, "LongPress", false, false), (3, "StartPress", false, false),
        (4, "Release", false, false), (2, "SoftPress", true, true), (7, "Analog", true, true), (8, "Chord", false, false),
    ];
    ALL.iter().filter(|&&(_, _, filter_digital, filter_alwayson)| (!filter_digital || analog) && (!filter_alwayson || !always_on))
        .map(|&(value, id, _, _)| (value, id)).collect()
}

/// Localized name of an activation type (`#ControllerBinding_ActivatorDropDown_<id>`).
pub fn activation_label(value: i64) -> String {
    let (id, fallback) = match value {
        1 => ("FullPress", "Обычное нажатие"), 2 => ("SoftPress", "Слабое нажатие"), 3 => ("StartPress", "Начало нажатия"),
        4 => ("Release", "Завершение нажатия"), 5 => ("LongPress", "Долгое нажатие"), 6 => ("DoublePress", "Двойное нажатие"),
        7 => ("Analog", "Аналоговый"), 8 => ("Chord", "Кнопка активатора"), _ => return format!("Тип {value}"),
    };
    t_or(&format!("#ControllerBinding_ActivatorDropDown_{id}"), fallback)
}

/// Change the mode of any source in action set `set_index` (gyro, pads, sticks, triggers, face buttons, D-pad):
/// write, read the mode back from the reply, and on any difference restore the previous mode and read that back too.
pub fn write_mode(ed: &dyn Editor, raw: &[u8], set_index: usize, source: i64, mode: i64) -> Outcome {
    use super::controller_quick as quick;
    let old = quick::mode_in_set(raw, set_index, source);
    let target = match quick::mode_target_at(raw, set_index, source) { Ok(t) => t, Err(e) => return Outcome::Unsure(e.to_string()) };
    let (why, base) = match ed.set_mode(&target, mode) {
        Ok(reply) if quick::mode_in_set(&reply, set_index, source) == mode => return Outcome::Done(reply),
        Ok(reply) => ("Steam вернул другой режим".to_string(), reply),
        Err(e) => (e.to_string(), raw.to_vec()),
    };
    let rolled = quick::mode_target_at(&base, set_index, source).and_then(|t| ed.set_mode(&t, old)).ok()
        .filter(|r| quick::mode_in_set(r, set_index, source) == old);
    match rolled { Some(r) => Outcome::RolledBack(why, r), None => Outcome::Unsure(why) }
}

/// Change the activation type of an existing activator; rolls back to the previous type when readback differs.
pub fn write_activation(ed: &dyn Editor, raw: &[u8], target: &BindingTarget, activation: i64) -> Outcome {
    let Some(old) = live::activation_at(raw, target) else {
        return Outcome::Unsure("Активатор не найден в копии раскладки".into());
    };
    let (why, base) = match ed.set_activator(raw, target, Some(activation)) {
        Ok(reply) if live::activation_at(&reply, target) == Some(activation) => return Outcome::Done(reply),
        Ok(reply) => ("Steam вернул другой способ включения".to_string(), reply),
        Err(e) => (e.to_string(), raw.to_vec()),
    };
    match ed.set_activator(&base, target, Some(old)) {
        Ok(r) if live::activation_at(&r, target) == Some(old) => Outcome::RolledBack(why, r),
        _ => Outcome::Unsure(why),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use steam_bridge::controller_edit::{gamepad_binding, Writer};

    /// Layout with one source (key 7, mode 10 id 4) and one input (13) holding `activators` as (activation, bindings).
    fn layout(activators: &[(i64, Vec<Vec<u8>>)]) -> Vec<u8> {
        let mut group = Writer::new(); group.int(1, 10); group.int(5, 4);
        let mut source = Writer::new(); source.int(1, 7); source.message(2, &group.finish());
        let mut set = Writer::new(); set.string(1, "Default"); set.message(6, &source.finish());
        let mut input = Writer::new(); input.int(1, 13);
        for (activation, bindings) in activators {
            let mut a = Writer::new(); a.int(1, *activation);
            for b in bindings { a.message(2, b); }
            input.message(2, &a.finish());
        }
        let mut mode = Writer::new(); mode.int(1, 10); mode.int(2, 4); mode.message(4, &input.finish());
        let mut out = Writer::new(); out.message(8, &set.finish()); out.message(9, &mode.finish());
        out.finish()
    }
    fn target(activator: i64, binding: i64) -> BindingTarget {
        BindingTarget { action_set_key: "Default".into(), action_set_layer_key: None, source_binding_key: 7, mode_shift: false,
            input_key: 13, activator_index: activator, binding_index: binding, source_mode: 10, modeid: 4 }
    }
    /// Scripted Steam: returns the queued replies in order and records the request kinds.
    struct Fake { replies: RefCell<Vec<Result<Vec<u8>>>>, calls: RefCell<Vec<&'static str>> }
    impl Fake {
        fn new(replies: Vec<Result<Vec<u8>>>) -> Self { Self { replies: RefCell::new(replies.into_iter().rev().collect()), calls: RefCell::new(Vec::new()) } }
        fn next(&self, what: &'static str) -> Result<Vec<u8>> { self.calls.borrow_mut().push(what); self.replies.borrow_mut().pop().expect("unexpected request") }
    }
    impl Editor for Fake {
        fn set_binding(&self, _: &[u8], _: &BindingTarget, _: &[u8]) -> Result<Vec<u8>> { self.next("set") }
        fn remove_binding(&self, _: &[u8], _: &BindingTarget) -> Result<Vec<u8>> { self.next("remove") }
        fn set_activator(&self, _: &[u8], _: &BindingTarget, a: Option<i64>) -> Result<Vec<u8>> { self.next(if a.is_some() { "activation" } else { "drop-activator" }) }
        fn set_mode(&self, _: &BindingTarget, _: i64) -> Result<Vec<u8>> { self.next("mode") }
    }

    /// Layout where each (source key, mode, modeid) is one source group of the Default set.
    fn modes_layout(sources: &[(i64, i64, i64)]) -> Vec<u8> {
        let mut set = Writer::new(); set.string(1, "Default");
        for &(key, mode, id) in sources {
            let mut g = Writer::new(); g.int(1, mode); g.int(5, id);
            let mut src = Writer::new(); src.int(1, key); src.message(2, &g.finish());
            set.message(6, &src.finish());
        }
        let mut out = Writer::new(); out.message(8, &set.finish()); out.finish()
    }

    #[test]
    fn mode_write_reads_back_and_rolls_back_for_every_source_kind() {
        // gyro 10, left/right pad 1/2, sticks 3/12, triggers 6/7, face buttons 4, D-pad 13
        for (source, from, to) in [(10, 0, 22), (2, 6, 12), (1, 1, 4), (12, 6, 7), (3, 6, 13), (7, 10, 0), (4, 3, 6), (13, 1, 6)] {
            let before = modes_layout(&[(source, from, 5)]);
            let after = modes_layout(&[(source, to, 5)]);
            let fake = Fake::new(vec![Ok(after.clone())]);
            assert_eq!(write_mode(&fake, &before, 0, source, to), Outcome::Done(after), "source {source}");
            // Steam answers with a different mode: restored and confirmed.
            let other = modes_layout(&[(source, 9, 5)]);
            let fake = Fake::new(vec![Ok(other), Ok(before.clone())]);
            assert_eq!(write_mode(&fake, &before, 0, source, to), Outcome::RolledBack("Steam вернул другой режим".into(), before.clone()), "source {source}");
            assert_eq!(*fake.calls.borrow(), ["mode", "mode"]);
            // Refusal then a failed restore: the copy is flagged.
            let fake = Fake::new(vec![err(), err()]);
            assert!(matches!(write_mode(&fake, &before, 0, source, to), Outcome::Unsure(_)), "source {source}");
        }
    }

    fn err() -> Result<Vec<u8>> { Err(steam_bridge::Error::Protocol("Steam Input 12100".into())) }

    #[test]
    fn appending_to_an_empty_input_is_confirmed_by_readback() {
        let empty = layout(&[]);
        let added = layout(&[(1, vec![gamepad_binding(5)])]);
        let fake = Fake::new(vec![Ok(added.clone())]);
        assert_eq!(write_binding(&fake, &empty, &target(0, -1), &gamepad_binding(5)), Outcome::Done(added));
        assert_eq!(*fake.calls.borrow(), ["set"]);
    }

    #[test]
    fn a_wrong_reply_to_an_append_is_rolled_back_by_removing_binding_and_activator() {
        let empty = layout(&[]);
        let wrong = layout(&[(1, vec![gamepad_binding(6)])]);
        let emptied = layout(&[(1, vec![])]);
        let fake = Fake::new(vec![Ok(wrong), Ok(emptied), Ok(empty.clone())]);
        let out = write_binding(&fake, &empty, &target(0, -1), &gamepad_binding(5));
        assert_eq!(out, Outcome::RolledBack("Steam вернул другую команду".into(), empty));
        assert_eq!(*fake.calls.borrow(), ["set", "remove", "drop-activator"]);
    }

    #[test]
    fn a_native_error_on_append_leaves_nothing_to_roll_back() {
        let empty = layout(&[]);
        let fake = Fake::new(vec![err()]);
        let Outcome::RolledBack(why, raw) = write_binding(&fake, &empty, &target(0, -1), &gamepad_binding(5)) else { panic!() };
        assert!(why.contains("12100"));
        assert_eq!(raw, empty);
        assert_eq!(*fake.calls.borrow(), ["set"]);
    }

    #[test]
    fn replacing_an_existing_command_restores_the_previous_one_on_mismatch() {
        let before = layout(&[(1, vec![gamepad_binding(3)])]);
        let wrong = layout(&[(1, vec![gamepad_binding(9)])]);
        let fake = Fake::new(vec![Ok(wrong), Ok(before.clone())]);
        let out = write_binding(&fake, &before, &target(0, 0), &gamepad_binding(4));
        assert_eq!(out, Outcome::RolledBack("Steam вернул другую команду".into(), before));
        assert_eq!(*fake.calls.borrow(), ["set", "set"]);
    }

    #[test]
    fn a_failed_rollback_marks_the_copy_unreliable() {
        let before = layout(&[(1, vec![gamepad_binding(3)])]);
        let fake = Fake::new(vec![err(), err()]);
        assert!(matches!(write_binding(&fake, &before, &target(0, 0), &gamepad_binding(4)), Outcome::Unsure(_)));
        let empty = layout(&[]);
        let fake = Fake::new(vec![Ok(layout(&[(1, vec![gamepad_binding(9)])])), err()]);
        assert!(matches!(write_binding(&fake, &empty, &target(0, -1), &gamepad_binding(5)), Outcome::Unsure(_)));
    }

    #[test]
    fn undo_of_a_new_command_removes_binding_and_empty_activator() {
        let added = layout(&[(1, vec![gamepad_binding(5)])]);
        let empty = layout(&[]);
        let fake = Fake::new(vec![Ok(layout(&[(1, vec![])])), Ok(empty.clone())]);
        assert_eq!(remove_new_binding(&fake, &added, &target(0, 0)).unwrap(), empty);
        // Steam ignoring the removal is an error, not a silent success.
        let fake = Fake::new(vec![Ok(added.clone())]);
        assert!(remove_new_binding(&fake, &added, &target(0, 0)).is_err());
    }

    #[test]
    fn activation_write_reads_back_and_rolls_back() {
        let before = layout(&[(1, vec![gamepad_binding(3)])]);
        let after = layout(&[(5, vec![gamepad_binding(3)])]);
        let fake = Fake::new(vec![Ok(after.clone())]);
        assert_eq!(write_activation(&fake, &before, &target(0, 0), 5), Outcome::Done(after));
        let fake = Fake::new(vec![Ok(layout(&[(6, vec![gamepad_binding(3)])])), Ok(before.clone())]);
        assert_eq!(write_activation(&fake, &before, &target(0, 0), 5), Outcome::RolledBack("Steam вернул другой способ включения".into(), before.clone()));
        let fake = Fake::new(vec![err(), err()]);
        assert!(matches!(write_activation(&fake, &before, &target(0, 0), 5), Outcome::Unsure(_)));
        assert!(matches!(write_activation(&Fake::new(vec![]), &before, &target(3, 0), 5), Outcome::Unsure(_)));
    }

    #[test]
    fn stock_activation_list_has_six_values_for_a_digital_input() {
        let digital: Vec<i64> = activation_options(false, false).iter().map(|o| o.0).collect();
        assert_eq!(digital, [1, 6, 5, 3, 4, 8]);
        assert_eq!(digital.len(), 6);
        // Analog capable inputs also offer Soft press (2) and Analog (7), as stock `filter_digital` allows.
        assert_eq!(activation_options(true, false).iter().map(|o| o.0).collect::<Vec<_>>(), [1, 6, 5, 3, 4, 2, 7, 8]);
        // filter_alwayson drops DoublePress, SoftPress and Analog.
        assert_eq!(activation_options(true, true).iter().map(|o| o.0).collect::<Vec<_>>(), [1, 5, 3, 4, 8]);
        assert_eq!(activation_options(false, true).iter().map(|o| o.0).collect::<Vec<_>>(), [1, 5, 3, 4, 8]);
    }
}
