//! Offline regression coverage for the live example's deletion boundary.
#[path = "../examples/support/editor_write_cleanup.rs"]
mod cleanup;

use steam_bridge::controller_list::ActiveLayout;

fn active(url: &str, selection_type: u32) -> ActiveLayout {
    ActiveLayout { url: url.into(), title: String::new(), author_name: String::new(), published_file_id: None, selection_type, enabled: true }
}

#[test]
fn only_the_new_copy_observed_after_save_is_eligible() {
    let before = active("template://original.vdf", 0);
    let saved = active("autosave://new.vdf", 1);
    let urls = vec![before.url.clone()];
    let copy = cleanup::created_copy(&before, &before.url, Some(&urls), Some(&saved), Some(&saved), true);
    assert_eq!(copy, Some(&saved));
}

#[test]
fn existing_personal_copy_is_never_written_or_deleted_even_with_default_a() {
    for before in [active("autosave://existing.vdf", 1), active("template://original.vdf", 0)] {
        // Also cover a personal autosave which exists but was not selected at startup.
        let urls = vec!["autosave://existing.vdf".into()];
        let saved = active("autosave://existing.vdf", 1);
        assert!(!cleanup::can_write(&before, &before.url, Some(&urls)));
        assert!(cleanup::created_copy(&before, &before.url, Some(&urls), Some(&saved), Some(&saved), true).is_none());
    }
}

#[test]
fn unknown_baseline_or_mismatched_editor_refuses_writes_and_deletion() {
    let before = active("workshop://1234", 2);
    let saved = active("autosave://new.vdf", 1);
    assert!(!cleanup::can_write(&before, &before.url, None));
    assert!(!cleanup::can_write(&before, "template://other.vdf", Some(&[])));
    assert!(cleanup::created_copy(&before, &before.url, None, Some(&saved), Some(&saved), true).is_none());
}

#[test]
fn final_url_alone_and_failed_cycles_do_not_authorize_deletion() {
    let before = active("template://original.vdf", 0);
    let saved = active("autosave://new.vdf", 1);
    let other = active("autosave://someone-elses.vdf", 1);
    let changed_type = active(&saved.url, 2);
    let unchanged = before.clone();
    for (observed, final_active, verified) in [
        (None, Some(&saved), true),
        (Some(&saved), None, true),
        (Some(&saved), Some(&other), true),
        (Some(&saved), Some(&changed_type), true),
        (Some(&saved), Some(&saved), false),
        (Some(&unchanged), Some(&unchanged), true),
    ] {
        assert!(cleanup::created_copy(&before, &before.url, Some(&[]), observed, final_active, verified).is_none());
    }
}

#[test]
fn restoration_requires_original_url_and_selection_type() {
    let before = active("workshop://1234", 2);
    assert!(cleanup::restoration_verified(&before, Some(&before), true));
    assert!(!cleanup::restoration_verified(&before, Some(&before), false));
    assert!(!cleanup::restoration_verified(&before, None, true));
    assert!(!cleanup::restoration_verified(&before, Some(&active(&before.url, 0)), true));
    assert!(!cleanup::restoration_verified(&before, Some(&active("template://original.vdf", 2)), true));
    assert!(!cleanup::same_selection(&before, &active(&before.url, 0)));
    assert!(!cleanup::same_selection(&before, &active("template://original.vdf", 2)));
}
