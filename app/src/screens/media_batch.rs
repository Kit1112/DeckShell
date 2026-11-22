//! Media grid selection and sequential reuse of the individual send/delete actions.
use steam_assets::i18n;
use super::{item_key, key_at, phone_phase_label, MediaOp, St, ST};
use crate::app::App;
use crate::assets::t_or;
use crate::input::Action;
use crate::router::Route;
use crate::screens::library::{MItem, MenuAct, MenuKind};
use std::collections::{HashSet, VecDeque};

pub(super) type Selection = HashSet<String>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum BatchKind {
    Send,
    Delete,
}

pub(super) enum Outcome {
    Done,
    Failed(String),
    Skipped(String),
    Unknown(String),
}

pub(super) struct Batch {
    kind: BatchKind,
    account: Option<u32>,
    pending: VecDeque<String>,
    current: Option<String>,
    total: usize,
    done: usize,
    failed: usize,
    skipped: usize,
    unknown: usize,
    first_problem: Option<String>,
}

impl Batch {
    fn new(kind: BatchKind, account: Option<u32>, keys: Vec<String>) -> Self {
        Self {
            kind,
            account,
            total: keys.len(),
            pending: keys.into(),
            current: None,
            done: 0,
            failed: 0,
            skipped: 0,
            unknown: 0,
            first_problem: None,
        }
    }

    fn record(&mut self, outcome: Outcome) {
        let problem = match outcome {
            Outcome::Done => {
                self.done += 1;
                return;
            }
            Outcome::Failed(reason) => {
                self.failed += 1;
                reason
            }
            Outcome::Skipped(reason) => {
                self.skipped += 1;
                reason
            }
            Outcome::Unknown(reason) => {
                self.unknown += 1;
                reason
            }
        };
        self.first_problem.get_or_insert(problem);
    }

    fn summary(&self) -> String {
        let title = match self.kind {
            BatchKind::Send => i18n::tr_ref("media_batch.item_sent"),
            BatchKind::Delete => i18n::tr_ref("media_batch.deleted"),
        };
        let mut summary = i18n::tr_args("media_batch.of", &[("title", &(title).to_string()), ("done", &(self.done).to_string()), ("total", &(self.total).to_string())]);
        for (count, label) in [
            (self.failed, i18n::tr_ref("media_batch.mistake")),
            (self.skipped, i18n::tr_ref("media_batch.missed")),
            (self.unknown, i18n::tr_ref("media_batch.outcome_unknown")),
        ] {
            if count > 0 {
                summary.push_str(&format!(" · {label} {count}"));
            }
        }
        summary
    }
}

fn selected_keys(s: &St) -> Vec<String> {
    let Some(selection) = &s.selection else {
        return Vec::new();
    };
    s.items
        .iter()
        .map(item_key)
        .filter(|key| selection.contains(key))
        .collect()
}

fn toggle_focused(s: &mut St) -> bool {
    let Some(key) = key_at(s, s.focus) else {
        return false;
    };
    let Some(selection) = &mut s.selection else {
        return false;
    };
    if !selection.remove(&key) {
        selection.insert(key);
    }
    true
}

pub(super) fn selection_status(s: &St) -> String {
    if let Some(batch) = &s.batch {
        let action = match batch.kind {
            BatchKind::Send => i18n::tr_ref("media_batch.sending_it_to_the_phone"),
            BatchKind::Delete => i18n::tr_ref("media_batch.uninstall"),
        };
        let index = batch.total - batch.pending.len();
        let detail = if s.phone_busy {
            format!(
                " · {}",
                phone_phase_label(&s.phone).unwrap_or(i18n::tr_ref("media_batch.checking_your_mobile_device"))
            )
        } else {
            String::new()
        };
        return i18n::tr_args("media_batch.of_2", &[("action", &(action).to_string()), ("index", &(index).to_string()), ("total", &(batch.total).to_string()), ("detail", &(detail).to_string())]);
    }
    let count = s
        .selection
        .as_ref()
        .map(|selection| i18n::tr_args("media_batch.selected", &[("len", &(selection.len()).to_string())]));
    match (count, s.batch_note.is_empty()) {
        (Some(count), true) => count,
        (Some(count), false) => format!("{count} · {}", s.batch_note),
        (None, _) => s.batch_note.clone(),
    }
}

impl App {
    pub(crate) fn media_batch_busy(&self) -> bool {
        ST.with(|s| s.borrow().batch.is_some())
    }

    pub(crate) fn media_selecting(&self) -> bool {
        ST.with(|s| s.borrow().selection.is_some())
    }

    pub(crate) fn media_selection_hints(
        &self,
    ) -> Vec<(&'static str, &'static str, i32, &'static str, Action)> {
        ST.with(|s| {
            let s = s.borrow();
            let back = ("b", "#ActionButtonLabelBack", 1, "Esc", Action::B);
            if s.batch.is_some() {
                return vec![back];
            }
            let mut hints = Vec::new();
            if let Some(selection) = &s.selection {
                if !selection.is_empty() {
                    hints.push(("y", i18n::tr_ref("media_batch.activity"), 1, "F4", Action::Y));
                }
                if let Some(key) = key_at(&s, s.focus) {
                    let label = if selection.contains(&key) {
                        i18n::tr_ref("media_batch.remove_the_mark")
                    } else {
                        i18n::tr_ref("media_batch.celebrate")
                    };
                    hints.push(("a", label, 1, "Enter", Action::A));
                }
            }
            hints.push(back);
            hints
        })
    }

    pub(super) fn media_select_many(&mut self) {
        let opened = ST.with(|s| {
            let mut s = s.borrow_mut();
            if s.view.is_empty()
                || s.viewer.is_some()
                || s.selection.is_some()
                || s.batch.is_some()
                || s.phone_busy
                || s.deleting.is_some()
            {
                return false;
            }
            s.selection = Some(Selection::new());
            s.confirm_key = None;
            s.batch_note.clear();
            true
        });
        if opened {
            self.push_media();
            self.update_hint();
        }
    }

    pub(super) fn media_activate_card(&mut self) {
        if self.media_batch_busy() {
            return;
        }
        if self.media_selecting() {
            ST.with(|s| {
                let mut s = s.borrow_mut();
                if toggle_focused(&mut s) {
                    s.batch_note.clear();
                }
            });
            self.push_media();
            self.update_hint();
        } else {
            self.media_open_viewer();
        }
    }

    pub(super) fn media_selection_back(&mut self) -> bool {
        let closed = ST.with(|s| s.borrow_mut().selection.take().is_some());
        if closed {
            self.push_media();
            self.update_hint();
        }
        closed
    }

    pub(super) fn media_batch_menu(&mut self) {
        let count = ST.with(|s| selected_keys(&s.borrow()).len());
        if count == 0 {
            return;
        }
        let row = |label, act| MItem {
            label,
            kind: 0,
            sep: false,
            arrow: false,
            act,
        };
        let rows = vec![
            row(
                t_or("#ExportClip_SendToPhone", i18n::tr_ref("media_batch.send_to_phone")),
                MenuAct::Media(MediaOp::BatchSend),
            ),
            row(
                t_or("#MediaPage_ScreenshotContext_Delete", i18n::tr_ref("media_batch.delete")),
                MenuAct::Media(MediaOp::BatchDelete),
            ),
            row(t_or("#Button_Cancel", i18n::tr_ref("media_batch.cancel")), MenuAct::Cancel),
        ];
        self.open_custom_menu(MenuKind::Media, i18n::tr_args("media_batch.selected_2", &[("count", &(count).to_string())]), rows, 0);
    }

    pub(super) fn media_batch_send(&mut self) {
        let keys = ST.with(|s| selected_keys(&s.borrow()));
        self.media_batch_start(BatchKind::Send, self.base.active_user, keys);
    }

    pub(super) fn media_batch_confirm_delete(&mut self) {
        let keys = ST.with(|s| selected_keys(&s.borrow()));
        if keys.is_empty() || self.media_batch_busy() {
            return;
        }
        let account = self.base.active_user;
        crate::screens::properties::confirm(
            self,
            i18n::tr_ref("media_batch.delete_selected_media_files").into(),
            i18n::tr_args("media_batch.items_selected_this_cannot_be_undone", &[("len", &(keys.len()).to_string())]),
            t_or("#MediaPage_ScreenshotContext_Delete", i18n::tr_ref("media_batch.delete")),
            Box::new(move |a| a.media_batch_start(BatchKind::Delete, account, keys)),
        );
    }

    fn media_batch_start(&mut self, kind: BatchKind, account: Option<u32>, keys: Vec<String>) {
        if keys.is_empty()
            || account != self.base.active_user
            || self.router.current() != &Route::Media
        {
            return;
        }
        let started = ST.with(|s| {
            let mut s = s.borrow_mut();
            if s.selection.is_none() || s.batch.is_some() || s.phone_busy || s.deleting.is_some() {
                return false;
            }
            s.batch = Some(Batch::new(kind, account, keys));
            s.batch_note.clear();
            true
        });
        if started {
            self.update_hint();
            self.media_batch_next();
        }
    }

    fn media_batch_next(&mut self) {
        let next = ST.with(|s| {
            let mut s = s.borrow_mut();
            let batch = s.batch.as_mut()?;
            if batch.account != self.base.active_user {
                batch.failed += batch.pending.len();
                batch.pending.clear();
                batch
                    .first_problem
                    .get_or_insert_with(|| i18n::tr_ref("media_batch.steam_s_profile_has_changed").into());
            }
            let kind = batch.kind;
            let key = batch.pending.pop_front()?;
            batch.current = Some(key.clone());
            let item = s.items.iter().find(|it| item_key(it) == key).cloned();
            Some((kind, item))
        });
        let Some((kind, item)) = next else {
            self.media_batch_finish();
            return;
        };
        self.push_media();
        let Some(item) = item else {
            self.media_batch_outcome(Outcome::Failed(
                i18n::tr_ref("media_batch.the_selected_item_is_no_longer_available").into(),
            ));
            return;
        };
        match kind {
            BatchKind::Send => self.media_phone_send_item(item),
            BatchKind::Delete => self.media_delete_item(item),
        }
    }

    pub(super) fn media_batch_outcome(&mut self, outcome: Outcome) {
        let account_epoch = ST.with(|s| {
            let mut s = s.borrow_mut();
            let batch = s.batch.as_mut().expect("batch item in progress");
            let key = batch.current.take().expect("current batch item");
            let done = matches!(outcome, Outcome::Done);
            batch.record(outcome);
            if done {
                if let Some(selection) = &mut s.selection {
                    selection.remove(&key);
                }
            }
            s.account_epoch
        });
        // An immediate refusal or a local file deletion can finish synchronously.
        // Schedule the next item so the UI can process events between items.
        super::super::defer(move |a| {
            if ST.with(|s| s.borrow().account_epoch == account_epoch) {
                a.media_batch_next();
            }
        });
    }

    fn media_batch_finish(&mut self) {
        let result = ST.with(|s| {
            let mut s = s.borrow_mut();
            let batch = s.batch.take()?;
            let summary = batch.summary();
            s.batch_note.clone_from(&summary);
            if s.selection.as_ref().is_some_and(Selection::is_empty) {
                s.selection = None;
            }
            Some((summary, batch.first_problem))
        });
        if let Some((summary, detail)) = result {
            self.push_media();
            self.update_hint();
            self.toast(&detail.map_or_else(|| summary.clone(), |msg| format!("{summary}. {msg}")));
        }
    }
}

#[cfg(test)]
#[path = "media_batch_tests.rs"]
mod tests;
