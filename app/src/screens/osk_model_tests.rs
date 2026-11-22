use super::*;

fn kb() -> Kb {
    Kb::new(1280.0, 800.0)
}

#[test]
fn qwerty_geometry_matches_deck_screenshot() {
    let k = kb();
    let keys = k.keys();
    let find = |a: Act| keys.iter().find(|x| x.def.act == a).unwrap();
    let tab = find(Act::Tab);
    // Tab: x 5..95 на скриншоте; ряд 1 — y 518..585
    assert!((tab.kx - 4.5).abs() < 2.0 && (tab.kx + tab.kw - 93.5).abs() < 2.0, "tab {} {}", tab.kx, tab.kw);
    assert!((tab.ky - 518.0).abs() < 3.0, "tab y {}", tab.ky);
    let q = find(Act::Char("q".into()));
    assert!((q.x + q.w / 2.0 - 140.0).abs() < 3.0, "q center {}", q.x + q.w / 2.0);
    let sp = find(Act::Space);
    assert!((sp.kx - 222.0).abs() < 4.0 && (sp.kx + sp.kw - 838.0).abs() < 5.0, "space {} {}", sp.kx, sp.kw);
    let paste = find(Act::Paste);
    assert!((paste.kx - 1060.0).abs() < 5.0, "paste {}", paste.kx);
    // нижний край панели — край окна
    let b = keys.iter().map(|x| x.y + x.h).fold(0.0, f32::max);
    assert!((b - 800.0).abs() < 2.5, "bottom {b}");
}

#[test]
fn no_overlap_in_any_layout_or_mode() {
    for layout in 0..2 {
        for mode in [Mode::Standard, Mode::Emoji] {
            let mut k = kb();
            k.layout = layout;
            k.mode = mode;
            k.cat = CAT_STEAM + 1;
            let keys = k.keys();
            for (i, a) in keys.iter().enumerate() {
                assert!(a.x >= -0.5 && a.x + a.w <= 1280.0 + 0.5 || matches!(a.def.act, Act::Emoji(_)), "{i} off-screen");
                for b in keys.iter().skip(i + 1) {
                    let ox = a.x + a.w - 0.5 > b.x && b.x + b.w - 0.5 > a.x;
                    let oy = a.y + a.h - 0.5 > b.y && b.y + b.h - 0.5 > a.y;
                    assert!(!(ox && oy), "overlap {:?} / {:?} (layout {layout} {mode:?})", a.def.act, b.def.act);
                }
            }
        }
    }
}

#[test]
fn rows_fill_the_width() {
    let k = kb();
    let keys = k.keys();
    for r in 0..5 {
        let row: Vec<&PKey> = keys.iter().filter(|x| x.row == r).collect();
        let right = row.iter().map(|x| x.x + x.w).fold(0.0, f32::max);
        assert!((right - (1280.0 - 3.0 * SCALE)).abs() < 1.0, "row {r} ends at {right}");
    }
}

#[test]
fn shift_states_and_typing() {
    let mut k = kb();
    let keys = k.keys();
    let d = |c: &str| keys.iter().find(|x| x.def.act == Act::Char(c.into())).unwrap().def.clone();
    assert_eq!(k.press(&d("q")), Out::Text("q".into()));
    let sh = keys.iter().find(|x| x.def.act == Act::Shift).unwrap().def.clone();
    k.press(&sh);
    assert_eq!(k.shift, 1);
    assert_eq!(k.press(&d("1")), Out::Text("!".into()), "shift symbol");
    assert_eq!(k.shift, 0, "однократный Shift сброшен");
    k.press(&sh);
    k.press(&sh);
    assert_eq!(k.shift, 2);
    assert_eq!(k.press(&d("q")), Out::Text("Q".into()));
    assert_eq!(k.shift, 2, "закреплённый остаётся");
    k.press(&sh);
    assert_eq!(k.shift, 0);
    // Caps инвертирует регистр букв, но не меняет цифры
    let caps = keys.iter().find(|x| x.def.act == Act::Caps).unwrap().def.clone();
    k.press(&caps);
    assert_eq!(k.press(&d("q")), Out::Text("Q".into()));
    assert_eq!(k.press(&d("1")), Out::Text("1".into()));
    k.press(&caps);
    assert_eq!(k.caps, 0);
}

#[test]
fn arrows_swap_under_shift() {
    let mut k = kb();
    let keys = k.keys();
    let left = keys.iter().find(|x| x.def.act == Act::Left).unwrap().def.clone();
    assert_eq!(k.press(&left), Out::Left);
    k.shift = 2;
    assert_eq!(k.press(&left), Out::Up);
}

#[test]
fn russian_layout_and_altgr() {
    let mut k = kb();
    k.layout = 1;
    let keys = k.keys();
    let find = |c: &str| keys.iter().find(|x| x.def.act == Act::Char(c.into())).unwrap().def.clone();
    assert_eq!(k.press(&find("ф")), Out::Text("ф".into()));
    k.shift = 2;
    assert_eq!(k.press(&find("ф")), Out::Text("Ф".into()));
    assert_eq!(k.press(&find("3")), Out::Text("№".into()));
    k.shift = 0;
    assert!(keys.iter().any(|x| x.def.act == Act::AltGr), "AltGr у русской раскладки");
    let ag = keys.iter().find(|x| x.def.act == Act::AltGr).unwrap().def.clone();
    k.press(&ag);
    assert_eq!(k.press(&find("8")), Out::Text("₽".into()));
}

#[test]
fn single_layout_hides_layout_key() {
    let mut k = kb();
    k.layouts = vec![1];
    k.layout = 1;
    let keys = k.keys();
    assert!(!keys.iter().any(|x| x.def.act == Act::ToLayout));
    let sp = keys.iter().find(|x| x.def.act == Act::Space).unwrap();
    assert!(sp.kx < 150.0, "пробел расширился влево: {}", sp.kx);
    assert_eq!(layout_from_steam(18), Some(1));
    assert_eq!(layout_from_steam(7), None);
}

#[test]
fn layout_key_cycles_layouts() {
    let mut k = kb();
    let l = k.keys().iter().find(|x| x.def.act == Act::ToLayout).unwrap().def.clone();
    k.press(&l);
    assert_eq!(k.layout, 1);
    k.press(&l);
    assert_eq!(k.layout, 0);
}

#[test]
fn dpad_navigation_visits_neighbours_and_keeps_column() {
    let mut k = kb();
    let keys = k.keys();
    assert!(matches!(&keys[k.focus].def.act, Act::Char(c) if c == "g"), "стартовый фокус g");
    assert!(k.nav(3));
    assert!(matches!(&k.keys()[k.focus].def.act, Act::Char(c) if c == "h"));
    assert!(k.nav(1));
    let k2 = k.keys();
    assert!(matches!(&k2[k.focus].def.act, Act::Char(c) if c == "b" || c == "n"), "{:?}", k2[k.focus].def.act);
    assert!(k.nav(0));
    assert!(k.nav(0));
    assert!(k.nav(0));
    assert!(!k.nav(0), "верх числового ряда — выход");
    // слева край: Caps
    k.focus_initial();
    for _ in 0..5 {
        k.nav(2);
    }
    assert_eq!(k.keys()[k.focus].def.act, Act::Caps);
    assert!(!k.nav(2));
}

#[test]
fn emoji_mode_flow() {
    let mut k = kb();
    let emo = k.keys().iter().find(|x| x.def.act == Act::ToEmoji).unwrap().def.clone();
    k.press(&emo);
    assert_eq!(k.mode, Mode::Emoji);
    let first = k.keys().iter().find(|x| matches!(x.def.act, Act::Emoji(_))).unwrap().clone();
    assert!(first.x < 100.0);
    let d = first.def.clone();
    let Out::Text(t) = k.press(&d) else { panic!() };
    assert!(!t.is_empty());
    assert_eq!(k.recent.len(), 1);
    k.cat_step(1);
    k.cat_step(-1);
    let abc = k.keys().iter().find(|x| x.def.act == Act::ToAbc).unwrap().def.clone();
    k.press(&abc);
    assert_eq!(k.mode, Mode::Standard);
    let (v, s) = emoji_list();
    assert_eq!(v.len(), 1714);
    assert_eq!(s[0], 0);
    assert!(s.windows(2).all(|w| w[0] < w[1]));
}

#[test]
fn emoji_text_has_no_steam_placeholders() {
    let mut k = kb();
    let n = emoji_list().0.len();
    for i in 0..n {
        let t = k.emoji_text(i);
        assert!(!t.contains("PLACEHOLDER"), "{i}: {t}");
    }
    // оттенок встаёт на место метки или в конец; по умолчанию — вариант-селектор FE0F
    assert_eq!(tinted("🖐*DEFAULT_PLACEHOLDER*", 0), "🖐\u{FE0F}");
    assert_eq!(tinted("🖐*DEFAULT_PLACEHOLDER*", 3), "🖐\u{1F3FD}");
    assert_eq!(tinted("👨%TINT_PLACEHOLDER%\u{200D}🦰", 5), "👨\u{1F3FF}\u{200D}🦰");
    let wave = emoji_list().0.iter().position(|(e, _)| e == "👋").unwrap();
    k.tints.insert(wave, 2);
    assert_eq!(k.emoji_text(wave), "👋\u{1F3FC}");
}

#[test]
fn long_press_rows_follow_steam() {
    let mut k = kb();
    let keys = k.keys();
    let idx = |c: &str| keys.iter().position(|x| x.def.act == Act::Char(c.into())).unwrap();
    // левая половина ряда — ряд растёт вправо, первый элемент — сам символ
    let a = k.ext_items(&keys[idx("a")]).unwrap();
    let s: String = a.iter().map(|x| x.0.as_str()).collect();
    assert_eq!(s, "aáàâãäåæāą");
    // правая половина — в обратном порядке, сам символ у клавиши (справа)
    let o = k.ext_items(&keys[idx("o")]).unwrap();
    assert_eq!(o.last().unwrap().0, "o");
    assert_eq!(o.first().unwrap().0, "œ");
    // пары: символ, Shift-символ и таблица (у «0» — «)» и «°»)
    let z: Vec<_> = k.ext_items(&keys[idx("0")]).unwrap().into_iter().map(|x| x.0).collect();
    assert_eq!(z, vec!["°", ")", "0"]);
    // под Caps буквы заглавные
    k.caps = 2;
    assert_eq!(k.ext_items(&keys[idx("e")]).unwrap()[1].0, "É");
    k.caps = 0;
    // открытие: над клавишей, в пределах экрана, автофокус у клавиши; набор выбранного
    assert!(k.open_ext(idx("a"), None));
    let r = k.ext.clone().unwrap();
    assert!((r.y - (keys[idx("a")].ky - EXT_H)).abs() < 0.1);
    assert!(r.x >= 0.0 && r.x + r.items.len() as f32 * EXT_W <= 1280.0);
    assert_eq!(r.sel, Some(0));
    k.ext_nav(3);
    k.ext_nav(3);
    assert_eq!(k.press_ext(k.ext.as_ref().unwrap().sel.unwrap()), Out::Text("à".into()));
    assert!(k.ext.is_none());
    // числовой ряд — ряд вариантов под клавишей (TopRow)
    assert!(k.open_ext(idx("1"), None));
    assert!(k.ext.as_ref().unwrap().y > keys[idx("1")].ky);
    // тачпад: ряд не выходит за свою половину
    assert!(k.open_ext(idx("p"), Some(true)));
    assert!(k.ext.as_ref().unwrap().x >= 640.0);
    // Tab, Shift и пр. — без вариантов
    let tab = keys.iter().find(|x| x.def.act == Act::Tab).unwrap();
    assert!(k.ext_items(tab).is_none());
}

#[test]
fn emoji_tint_row_remembers_choice() {
    let mut k = kb();
    k.mode = Mode::Emoji;
    k.cat = CAT_STEAM + 1;
    let wave = emoji_list().0.iter().position(|(e, _)| e == "👋").unwrap();
    let (a, _) = emoji_range(k.cat);
    k.scroll = (wave - a) / EMOJI_ROWS;
    let keys = k.keys();
    let i = keys.iter().position(|x| x.def.act == Act::Emoji(wave)).unwrap();
    assert!(k.open_ext(i, None));
    let r = k.ext.clone().unwrap();
    assert_eq!(r.items.len(), 6);
    let dark = r.items.iter().position(|x| x.1 == Some(5)).unwrap();
    assert_eq!(k.press_ext(dark), Out::Text("👋\u{1F3FF}".into()));
    assert_eq!(k.tints.get(&wave), Some(&5));
    assert_eq!(k.recent.first(), Some(&wave));
    // без оттенков — без ряда
    let smile = keys.iter().find(|x| x.def.act == Act::Emoji(0)).map(|x| x.def.clone());
    if let Some(d) = smile {
        let pk = keys.iter().find(|x| x.def.act == d.act).unwrap();
        assert!(k.ext_items(pk).is_none());
    }
}

#[test]
fn emoji_scroll_is_clamped() {
    let mut k = kb();
    k.mode = Mode::Emoji;
    k.cat = CAT_STEAM + 1;
    k.set_scroll(-5);
    assert_eq!(k.scroll, 0);
    k.set_scroll(10_000);
    let vis = (1280.0 / EMOJI_COL_W).floor() as usize;
    assert_eq!(k.scroll, k.emoji_cols() - vis);
    // последняя колонка видна целиком
    let last = k.keys().iter().filter(|x| matches!(x.def.act, Act::Emoji(_))).map(|x| x.x + x.w).fold(0.0, f32::max);
    assert!(last <= 1280.0 + 1.0, "{last}");
}

#[test]
fn pad_maps_halves() {
    let k = kb();
    let (x, y) = pad_point(&k, false, -1.0, 1.0, 1.0);
    assert!(x < 1.0 && (y - k.panel_top()).abs() < 1.0);
    let (x, _) = pad_point(&k, true, 0.0, 0.0, 1.0);
    assert!((x - 960.0).abs() < 1.0);
    let (x, y) = pad_point(&k, false, 1.0, -1.0, 1.0);
    assert!(x < 640.0 && (x - 640.0).abs() < 1.0);
    assert!(y < 800.0 && (y - 800.0).abs() < 1.0);
    let (x, y) = pad_point(&k, true, 1.0, -1.0, 2.0);
    assert!(x < 1280.0 && (x - 1280.0).abs() < 1.0);
    assert!(y < 800.0 && (y - 800.0).abs() < 1.0);
}
