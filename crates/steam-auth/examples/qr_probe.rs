//! Живая проверка транспорта без входа: начать QR-сессию, один опрос, остановиться.
//! `cargo run -p steam-auth --example qr_probe` — печатает только домен ссылки и статус, не токены.
fn main() {
    let t = steam_auth::WebApi::default();
    let mut s = steam_auth::qr::begin(&t, &steam_auth::Device::default()).expect("BeginAuthSessionViaQR");
    println!("challenge host: {}", s.challenge_url.split('/').take(3).collect::<Vec<_>>().join("/"));
    println!("interval: {:?}", s.interval);
    std::thread::sleep(s.interval);
    match s.poll(&t) {
        Ok(steam_auth::qr::Event::Waiting { url_changed }) => println!("poll: waiting (url_changed={url_changed})"),
        Ok(other) => println!("poll: unexpected {other:?}"),
        Err(f) => println!("poll: failure {f:?}"),
    }
}
