use super::*;
#[test]
fn widths_match_measured_stock_lines() {
    // (text, px, ink width measured on the 1280x800 stock captures); advance is ~3 px wider than ink
    for (t, px, ink) in [("Завершать скачивание с выключенным экраном перед", 24., 696.), ("Процент заряда батареи", 24., 314.),
        ("Показывать вверху процент заряд батареи", 18., 411.), ("Выключать телевизор при переходе в спящий режим", 24., 677.),
        ("Если ваше устройство перейдёт в спящее состояние, скачивание игр", 18., 661.)] {
        let w = text_width(t, px);
        assert!(w >= ink && w - ink <= 9., "{t}: {w} vs {ink}");
    }
}
#[test]
fn wraps_like_a_browser() {
    assert_eq!(wrap("", 24., 100.), Vec::<String>::new());
    assert_eq!(wrap("aaa bbb", 24., 1000.), ["aaa bbb"]);
    let l = wrap("Выходить из спящего режима при включении телевизора", 24., 710.);
    assert_eq!(l, ["Выходить из спящего режима при включении", "телевизора"]);
    assert_eq!(wrap("a\nb", 24., 1000.), ["a", "b"]); // explicit break
    assert_eq!(wrap("supercalifragilistic", 24., 50.), ["supercalifragilistic"]); // overlong word keeps its line
    assert_eq!(wrap("one-two-three four", 24., text_width("one-two-", 24.) + 1.), ["one-two-", "three", "four"]);
}
