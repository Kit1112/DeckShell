use super::*;
use std::collections::BTreeSet;

fn placeholders(text: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut tail = text;
    while let Some((_, after_open)) = tail.split_once('{') {
        let (name, rest) = after_open.split_once('}').expect("unclosed placeholder");
        assert!(valid_name(name), "invalid placeholder {name:?}");
        names.insert(name.to_string());
        tail = rest;
    }
    names
}

fn signature(locale: &Locale, message: &Message) -> (bool, BTreeSet<String>) {
    match message {
        Message::Text(text) => (false, placeholders(text)),
        Message::Plural(forms) => {
            assert_eq!(&forms.keys().copied().collect::<Vec<_>>(), locale.forms);
            let names = placeholders(&forms[&Category::Other]);
            assert!(names.contains("count"));
            for text in forms.values() {
                assert_eq!(placeholders(text), names);
            }
            (true, names)
        }
    }
}

#[test]
fn catalogs_match_english() {
    let canonical = english().catalog();
    for locale in &LOCALES {
        let catalog = locale.catalog();
        assert!(
            catalog.keys().eq(canonical.keys()),
            "{} catalog differs from en: missing {:?}, extra {:?}",
            locale.code(),
            canonical
                .keys()
                .filter(|key| !catalog.contains_key(*key))
                .collect::<Vec<_>>(),
            catalog
                .keys()
                .filter(|key| !canonical.contains_key(*key))
                .collect::<Vec<_>>()
        );
        for (key, source) in canonical {
            assert_eq!(
                signature(locale, &catalog[key]),
                signature(english(), source),
                "{}: {key}",
                locale.code()
            );
        }
    }
}

#[test]
fn resolves_overrides_then_steam() {
    for (env, file, steam, expected) in [
        (Some(" RU "), Some("english"), Some("english"), "ru"),
        (Some("english"), Some("ru"), Some("russian"), "en"),
        (None, Some(" russian\n"), Some("english"), "ru"),
        (Some(" "), Some("unknown"), Some("russian"), "ru"),
        (Some("unknown"), None, Some("english"), "en"),
        (None, None, Some("russian"), "ru"),
        (None, None, Some("german"), "en"),
        (None, None, Some("ru"), "en"),
        (None, None, Some(""), "en"),
        (None, None, None, "en"),
    ] {
        let locale = select(env, || file.map(str::to_owned), || steam.map(str::to_owned));
        assert_eq!(locale.code(), expected);
    }
}

#[test]
fn registry_missing_and_blank_resolve_to_english() {
    let dir = tempfile::tempdir().unwrap();
    let primary = dir.path().join("primary.vdf");
    let fallback = dir.path().join("fallback.vdf");
    let paths = [primary.clone(), fallback.clone()];
    let detected = || select(None, || None, || crate::registry_language(&paths)).code();
    assert_eq!(detected(), "en");
    std::fs::write(&primary, "\"Steam\"\n{\n \"language\" \"   \"\n}\n").unwrap();
    std::fs::write(&fallback, "\"language\" \"\"").unwrap();
    assert_eq!(detected(), "en");
    std::fs::write(&fallback, "\"language\" \"russian\"").unwrap();
    assert_eq!(detected(), "ru");
    std::fs::write(&primary, "\"language\" \"english\"").unwrap();
    assert_eq!(detected(), "en");
}

#[test]
fn formats_plural_and_named_arguments_and_falls_back() {
    for (number, form) in [
        (0, Category::Many),
        (1, Category::One),
        (2, Category::Few),
        (4, Category::Few),
        (5, Category::Many),
        (11, Category::Many),
        (14, Category::Many),
        (21, Category::One),
        (24, Category::Few),
        (-22, Category::Few),
    ] {
        assert_eq!(russian_plural(number), form, "{number}");
    }
    assert_eq!(english_plural(1), Category::One);
    assert_eq!(english_plural(21), Category::Other);
    assert_eq!(english_plural(0), Category::Other);

    let ru = by_code("ru").unwrap();
    assert_eq!(
        ru.tr_args("power.time.minutes_seconds", &[("1", "30"), ("0", "2")]),
        "2 мин. 30 сек."
    );
    assert_eq!(
        ru.tr_plural("power.time.seconds", 22, &[("count", "ignored")]),
        "22 сек."
    );
    assert_eq!(
        english().tr_plural("power.time.seconds", 1, &[]),
        "1 second"
    );
    assert_eq!(
        english().tr_plural("power.time.seconds", 22, &[]),
        "22 seconds"
    );
    assert_eq!(
        render("{first}, {second} {missing}", |name| match name {
            "first" => Some("{second}"),
            "second" => Some("done"),
            _ => None,
        }),
        "{second}, done {missing}"
    );

    let incomplete = Locale {
        code: "xx",
        steam: "test",
        source: "{}",
        plural: english_plural,
        forms: &[Category::One, Category::Other],
        catalog: OnceLock::new(),
    };
    assert_eq!(incomplete.tr("Settings_Page_Power"), "Power");
    assert_eq!(
        incomplete.tr_plural("power.time.seconds", 1, &[]),
        "1 second"
    );
    assert_eq!(incomplete.tr("test.missing"), "test.missing");
}
