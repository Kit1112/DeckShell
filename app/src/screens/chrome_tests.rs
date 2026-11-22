use super::*;
use std::time::Duration;

#[test]
fn header_switches_steamid_and_retries_the_new_avatar() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        Default::default(),
    ))).unwrap();
    let ui = AppWindow::new().unwrap();
    let top = ui.global::<TopBarState>();
    let root = std::env::temp_dir().join(format!("deckshell-header-profile-{}", std::process::id()));
    let cache = root.join("config/avatarcache");
    std::fs::create_dir_all(&cache).unwrap();
    let first = 76561197960265729;
    let second = 76561197960265730;
    let avatar = |id| cache.join(format!("{id}.png"));
    image::RgbaImage::new(2, 3).save(avatar(first)).unwrap();

    let mut profile = HeaderProfile::default();
    let start = Instant::now();
    let switched = start + Duration::from_secs(1);
    profile.update(&top, &root, first, "Первый", Some(start));
    assert!(top.get_has_avatar());
    assert_eq!(top.get_persona_name(), "Первый");

    profile.update(&top, &root, second, "Второй", Some(switched));
    assert!(!top.get_has_avatar());
    assert_eq!(top.get_avatar().size().width, 0);
    assert_eq!(top.get_persona_name(), "Второй");
    profile.update(&top, &root, first, "Первый", Some(start));
    assert_eq!(top.get_persona_name(), "Второй");
    assert!(!top.get_has_avatar());

    image::RgbaImage::new(4, 5).save(avatar(second)).unwrap();
    profile.update(&top, &root, second, "Второй", Some(switched + Duration::from_secs(5)));
    assert!(top.get_has_avatar());
    assert_eq!(top.get_avatar().size().width, 4);
    std::fs::remove_dir_all(root).unwrap();
}
