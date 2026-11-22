//! Media grid selection and sequential reuse of the individual send/delete actions.
use super::{item_key, key_at, MediaOp, St, ST};
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
            BatchKind::Send => "Отправлено",
            BatchKind::Delete => "Удалено",
        };
        let mut summary = format!("{title} {} из {}", self.done, self.total);
        for (count, label) in [
            (self.failed, "ошибок"),
            (self.skipped, "пропущено"),
            (self.unknown, "исход неизвестен"),
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
            BatchKind::Send => "Отправка на телефон",
            BatchKind::Delete => "Удаление",
        };
        let index = batch.total - batch.pending.len();
        let detail = if s.phone_busy {
            format!(
                " · {}",
                s.phone.label().unwrap_or("Проверка мобильного устройства…")
            )
        } else {
            String::new()
        };
        return format!("{action} · {index} из {}{detail}", batch.total);
    }
    let count = s
        .selection
        .as_ref()
        .map(|selection| format!("Выбрано {}", selection.len()));
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
                    hints.push(("y", "Действия", 1, "F4", Action::Y));
                }
                if let Some(key) = key_at(&s, s.focus) {
                    let label = if selection.contains(&key) {
                        "Снять отметку"
                    } else {
                        "Отметить"
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
                t_or("#ExportClip_SendToPhone", "Отправить на телефон…"),
                MenuAct::Media(MediaOp::BatchSend),
            ),
            row(
                t_or("#MediaPage_ScreenshotContext_Delete", "Удалить"),
                MenuAct::Media(MediaOp::BatchDelete),
            ),
            row(t_or("#Button_Cancel", "Отмена"), MenuAct::Cancel),
        ];
        self.open_custom_menu(MenuKind::Media, format!("Выбрано {count}"), rows, 0);
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
            "Удалить выбранные медиафайлы?".into(),
            format!(
                "Выбрано элементов — {}. Удаление нельзя отменить.",
                keys.len()
            ),
            t_or("#MediaPage_ScreenshotContext_Delete", "Удалить"),
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
                    .get_or_insert_with(|| "Профиль Steam изменился.".into());
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
                "Выбранный элемент больше недоступен.".into(),
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
mod tests {
    use super::*;
    use crate::{AppWindow, MediaState, PropsState};
    use slint::{ComponentHandle, Model};
    use std::sync::Arc;

    #[test]
    fn gamepad_batch_flow_uses_stable_keys_and_confirms_delete() {
        // The shared confirmation has process-global focus flags.
        const CHILD: &str = "DECKSHELL_MEDIA_BATCH_TEST_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "screens::media::media_batch::tests::gamepad_batch_flow_uses_stable_keys_and_confirms_delete"])
                .env(CHILD, "1")
                .env("DECKSHELL_DRYRUN", "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stdout)
            );
            return;
        }

        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                ..Default::default()
            },
        )))
        .unwrap();
        let ui = AppWindow::new().unwrap();
        let ds = Arc::new(crate::data::DataSource::new());
        let loader = crate::loader::Loader::start(ds.clone(), 0);
        let mut app = App::new(ui.as_weak(), ds, Arc::new(Default::default()), loader);
        app.router.push(Route::Media);
        let shot = super::super::delete_tests::shot("chosen");
        let other = super::super::delete_tests::shot("other");
        let mut clip = super::super::delete_tests::clip("clip_730_20250101_120000");
        clip.native.as_mut().unwrap().duration_ms = 61_000;
        ST.with(|s| {
            let mut s = s.borrow_mut();
            s.items = vec![shot.clone(), clip.clone(), other.clone()];
            super::super::refilter_state(&mut s);
        });
        app.push_media();

        let media = ui.global::<MediaState>();
        let props = ui.global::<PropsState>();
        app.handle_pad(Action::Y);
        assert_eq!(
            app.lb.menu.as_ref().unwrap().items[1].act,
            MenuAct::Media(MediaOp::SelectMany)
        );
        app.handle_pad(Action::Down);
        app.handle_pad(Action::A);
        assert!(media.get_selecting());
        assert_eq!(app.focus_scope(), crate::nav::scope::Scope::PagePopup);
        app.handle_pad(Action::A);
        assert!(!media.get_viewer_open());
        assert_eq!(app.hint_spec()[1].1, "Снять отметку");
        app.handle_pad(Action::Right);
        assert_eq!(app.hint_spec()[1].1, "Отметить");
        app.handle_pad(Action::A);
        assert_eq!(media.get_cards().row_data(1).unwrap().on, 1);
        assert_eq!(media.get_cards().row_data(3).unwrap().on, 1);
        assert_eq!(media.get_cards().row_data(5).unwrap().on, 0);
        assert_eq!(media.get_filter_text(), "Выбрано 2");

        // The two selected keys survive a reorder of the cards.
        ST.with(|s| {
            let mut s = s.borrow_mut();
            s.items.swap(0, 2);
            assert_eq!(selected_keys(&s), vec![item_key(&clip), item_key(&shot)]);
        });
        app.push_media();
        assert_eq!(media.get_cards().row_data(1).unwrap().on, 0);
        assert_eq!(media.get_cards().row_data(5).unwrap().on, 1);

        let open_delete = |app: &mut App| {
            app.handle_pad(Action::Y);
            app.handle_pad(Action::Down);
            app.handle_pad(Action::A);
        };
        open_delete(&mut app);
        assert!(props.get_dlg_open());
        assert!(props.get_dlg_text().contains("2"));
        assert_eq!(props.get_dlg_idx(), 0);
        app.handle_pad(Action::A);
        assert!(!props.get_dlg_open());
        assert!(!app.media_batch_busy());
        assert_eq!(media.get_filter_text(), "Выбрано 2");

        open_delete(&mut app);
        app.handle_pad(Action::Right);
        app.handle_pad(Action::A);
        assert!(app.media_batch_busy());
        // Drain the deferred steps without starting the UI event loop.
        app.media_batch_next();
        app.media_batch_next();
        assert!(!app.media_batch_busy());
        assert_eq!(
            media.get_filter_text(),
            "Выбрано 2 · Удалено 0 из 2 · ошибок 2"
        );
        assert_eq!(ST.with(|s| s.borrow().items.len()), 3);

        app.handle_pad(Action::Y);
        app.handle_pad(Action::A);
        assert!(app.media_batch_busy());
        app.media_batch_next();
        app.media_batch_next();
        assert_eq!(
            media.get_filter_text(),
            "Выбрано 2 · Отправлено 0 из 2 · ошибок 1 · пропущено 1"
        );

        app.handle_pad(Action::B);
        assert!(!media.get_selecting());
        assert_eq!(app.router.current(), &Route::Media);
        app.handle_pad(Action::A);
        assert!(media.get_viewer_open());
    }

    #[test]
    fn batch_counts_confirmed_successes_and_unknown_outcomes_independently() {
        let mut batch = Batch::new(
            BatchKind::Send,
            Some(1),
            vec![
                "clip:a".into(),
                "shot:b".into(),
                "shot:c".into(),
                "clip:d".into(),
            ],
        );
        for outcome in [
            Outcome::Done,
            Outcome::Unknown("Steam reply lost".into()),
            Outcome::Failed("Steam rejected file".into()),
            Outcome::Skipped("Clip too long".into()),
        ] {
            batch.record(outcome);
        }
        assert_eq!(
            batch.summary(),
            "Отправлено 1 из 4 · ошибок 1 · пропущено 1 · исход неизвестен 1"
        );
        assert_eq!(batch.first_problem.as_deref(), Some("Steam reply lost"));
    }
}
