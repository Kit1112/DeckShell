use super::*;

#[test]
fn cache_evicts_old_entries_and_keeps_visible_glyph() {
    let mut cache = Cache { wanted: vec!["😀".into()], ..Default::default() };
    cache.insert("😀".to_owned(), Some(Pixels::new(1, 1)));
    for n in 0..CACHE_LIMIT {
        cache.insert(n.to_string(), None);
    }
    assert_eq!(cache.pixels.len(), CACHE_LIMIT);
    assert!(cache.pixels.get("😀").is_some_and(Option::is_some));
    assert!(!cache.pixels.contains_key("0"));
}

#[test]
fn noto_resolves_simple_and_sequence_emoji() {
    if font_data().is_none() {
        eprintln!("Noto Color Emoji не найден — пропуск");
        return;
    }
    let face = Face::parse(font_data().unwrap(), 0).unwrap();
    for s in ["😀", "👍", "❤\u{FE0F}", "🇷🇺", "👨\u{200D}👩\u{200D}👧", "👍\u{1F3FD}", "#\u{FE0F}\u{20E3}"] {
        assert!(glyph_for(&face, s).is_some(), "нет глифа для {s:?}");
    }
    let (list, _) = super::super::osk_model::emoji_list();
    let missing = list.iter().filter(|(e, _)| glyph_for(&face, e).is_none()).count();
    assert!(missing * 20 < list.len(), "слишком много эмодзи без глифа: {missing}/{}", list.len());
}
