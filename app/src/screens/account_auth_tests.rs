use crate::screens::expected_text;
use super::*;
#[test]
fn qr_render_is_card_sized_with_stock_colors_and_decodable_geometry() {
    let buf = render_qr("https://s.team/q/1/3190591570734873285").unwrap();
    assert_eq!((buf.width(), buf.height()), (QR_CARD, QR_CARD));
    let px = buf.as_slice();
    assert!(px.iter().any(|p| (p.r, p.g, p.b) == (21, 23, 28)), "тёмные модули стока");
    assert_eq!((px[0].r, px[0].g, px[0].b), (255, 255, 255), "тихая зона белая");
    // Код помещается в ~200px по центру: поля не меньше 20px.
    let first_dark = px.iter().position(|p| p.r == 21).unwrap() as u32;
    assert!(first_dark % QR_CARD >= 20 && first_dark / QR_CARD >= 20);
    let b = blur(&buf);
    assert!(b.as_slice().iter().any(|p| p.r > 30 && p.r < 220), "размытие даёт полутона");
}
#[test]
fn eresult_texts_are_stock_russian() {
    assert_eq!(eresult_text(&steam_auth::Error::Steam(5)), expected_text("Неверный пароль.", "Incorrect Password"));
    assert!(eresult_text(&steam_auth::Error::Steam(88)).contains(expected_text("Неверный код", "Incorrect code")));
    assert!(eresult_text(&steam_auth::Error::Network("x".into())).contains(expected_text("подключении", "communicating with Steam")));
}

/// Мок-транспорт с готовыми ответами: QR-воркер доходит до успеха, токены уходят в общую модель.
struct Script(std::sync::Mutex<Vec<(&'static str, (i32, Vec<u8>))>>);
impl Transport for Script {
    fn call(&self, _s: &str, m: &str, _b: &[u8], _p: bool) -> steam_auth::Result<(i32, Vec<u8>)> {
        let mut v = self.0.lock().unwrap();
        let i = v.iter().position(|(n, _)| *n == m).expect(m);
        Ok(v.remove(i).1)
    }
}
#[test]
fn qr_worker_walks_begin_poll_scan_success_without_ui() {
    use steam_auth::proto::Writer;
    let mut begin = Writer::default().uint(1, 5).string(2, "https://s.team/q/1/5").bytes(3, b"r").0;
    begin.extend_from_slice(&[0x25]); begin.extend_from_slice(&1.0f32.to_bits().to_le_bytes());
    let t = Script(std::sync::Mutex::new(vec![
        ("BeginAuthSessionViaQR", (1, begin)),
        ("PollAuthSessionStatus", (1, Writer::default().uint(5, 1).0)),
        ("PollAuthSessionStatus", (1, Writer::default().string(3, "R").string(6, "fixture").0)),
    ]));
    let stop = Arc::new(AtomicBool::new(false));
    // Без цикла событий `post` ничего не применяет; проверяем, что воркер завершается сам на Done.
    let started = Instant::now();
    qr_worker(1, stop, &t);
    assert!(t.0.lock().unwrap().is_empty(), "все ответы израсходованы: begin → scan → done");
    assert!(started.elapsed() < Duration::from_secs(5));
}
