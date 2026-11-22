use crate::screens::expected_text;
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
    assert_eq!(app.hint_spec()[1].1, expected_text("Снять отметку", "Deselect"));
    app.handle_pad(Action::Right);
    assert_eq!(app.hint_spec()[1].1, expected_text("Отметить", "Select"));
    app.handle_pad(Action::A);
    assert_eq!(media.get_cards().row_data(1).unwrap().on, 1);
    assert_eq!(media.get_cards().row_data(3).unwrap().on, 1);
    assert_eq!(media.get_cards().row_data(5).unwrap().on, 0);
    assert_eq!(media.get_filter_text(), expected_text("Выбрано 2", "2 selected"));

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
    assert_eq!(media.get_filter_text(), expected_text("Выбрано 2", "2 selected"));

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
        expected_text("Выбрано 2 · Удалено 0 из 2 · ошибок 2", "2 selected · Deleted 0 of 2 · failed 2")
    );
    assert_eq!(ST.with(|s| s.borrow().items.len()), 3);

    app.handle_pad(Action::Y);
    app.handle_pad(Action::A);
    assert!(app.media_batch_busy());
    app.media_batch_next();
    app.media_batch_next();
    assert_eq!(
        media.get_filter_text(),
        expected_text("Выбрано 2 · Отправлено 0 из 2 · ошибок 1 · пропущено 1", "2 selected · Sent 0 of 2 · failed 1 · skipped 1")
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
        expected_text("Отправлено 1 из 4 · ошибок 1 · пропущено 1 · исход неизвестен 1", "Sent 1 of 4 · failed 1 · skipped 1 · outcome unknown 1")
    );
    assert_eq!(batch.first_problem.as_deref(), Some("Steam reply lost"));
}
