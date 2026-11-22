//! Runs one JavaScript file in the Steam UI over CDP and prints the JSON result (task 218).
//! Used for the controller editor state probes; the script itself decides whether it writes.
//! Usage: STEAM_CDP_PORT=18080 cargo run -p steam-bridge --example editor_js_probe -- <script.js>
//! The script is read from disk and must be an expression (an async IIFE returning a value).
use steam_bridge::Bridge;

fn main() {
    let path = std::env::args().nth(1).expect("usage: editor_js_probe <script.js>");
    let expr = std::fs::read_to_string(&path).expect("read script");
    let b = Bridge::connect().expect("connect");
    match b.eval_json_once(expr.trim()) {
        Ok(v) => println!("{v}"),
        Err(e) => println!("error: {e}"),
    }
}
