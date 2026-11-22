//! DECKSHELL_CONTROL=/path.sock: line protocol, one reply line per command. Network thread parses nothing UI-related;
//! every command runs on the UI thread via invoke_from_event_loop and goes through the same App paths as real input.
//!   route <path> | focus app <appid> | focus index <n> | key <name> | snapshot <file.png> | dumptree <file.json>
//!   props <appid> [page]   (open the Properties page, e.g. `props 227300 localfiles`)
//!   click <x> <y> | drag <x0> <y0> <x1> <y1> [steps] | longpress <x> <y> [ms]   (logical px; real WindowEvent pointer events)
//!   state | quit            (reply: `ok ...` / `err ...` / JSON for state)
use steam_assets::i18n::tr_ref;
use crate::app::App;
use crate::input::Action;
use crate::router::Route;
use slint::ComponentHandle;
use serde_json::json;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::sync::mpsc;
use std::time::Duration;

pub fn spawn(path: String) {
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => return eprintln!("[deckshell] control: bind {path}: {e}"),
    };
    eprintln!("[deckshell] control socket {path}");
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut w = match conn.try_clone() {
                    Ok(w) => w,
                    Err(_) => return,
                };
                for line in BufReader::new(conn).lines().map_while(Result::ok) {
                    let line = line.trim().to_string();
                    if line.is_empty() {
                        continue;
                    }
                    let (tx, rx) = mpsc::channel::<String>();
                    let l2 = line.clone();
                    if slint::invoke_from_event_loop(move || {
                        let r = crate::with_app(|a| exec(a, &l2)).unwrap_or_else(|| "err app busy".into());
                        let _ = tx.send(r);
                    })
                    .is_err()
                    {
                        let _ = writeln!(w, "err no event loop");
                        return;
                    }
                    let r = rx.recv_timeout(Duration::from_secs(20)).unwrap_or_else(|_| "err timeout".into());
                    let _ = writeln!(w, "{r}");
                    if line == "quit" {
                        return;
                    }
                }
            });
        }
    });
}

/// (route path as in Steam, sub-state) for the tree export / `state`.
pub fn route_state(a: &App) -> (String, String) {
    if a.sm.open {
        return ("popup:MainMenu".into(), "default".into());
    }
    if a.qm.open {
        return ("popup:QuickAccess".into(), "default".into());
    }
    let st = if a.lb.sort_open { "sort-menu" } else if a.lb.search { "search" } else { "default" };
    (a.router.current().path(), st.into())
}

pub fn parse_route(p: &str) -> Option<Route> {
    let p = p.trim().trim_end_matches('/');
    let rest = p.strip_prefix("/routes/").or_else(|| p.strip_prefix("routes/"))?;
    let parts: Vec<&str> = rest.split('/').collect();
    Some(match parts.as_slice() {
        ["library", "home"] => Route::Home,
        ["library", "tab", id] => Route::Library((*id).into()),
        ["library", "app", id] => Route::App(id.parse().ok()?),
        ["library", "downloads"] => Route::Downloads,
        ["settings", pg] => Route::Settings((*pg).into()),
        ["media", ..] => Route::Media,
        ["search"] => Route::Search,
        _ => return None,
    })
}

fn focus_json(a: &App) -> serde_json::Value {
    let games = a.games();
    match a.router.current() {
        Route::Home => {
            let g = a.hm.recent.get(a.hm.idx).and_then(|&i| games.get(i));
            json!({"kind": if g.is_none() && a.hm.idx == a.hm.recent.len() { "home-libtile" } else { "home-card" }, "index": a.hm.idx, "appid": g.map(|g| g.app_id), "name": g.map(|g| g.name.clone())})
        }
        Route::Search => a.search_focus_report(),
        Route::Library(_) => {
            let g = a.lb.flat.get(a.lb.frow).and_then(|r| r.get(a.lb.fcol)).and_then(|&i| games.get(i));
            json!({"kind": if a.lb.zone == 1 { "grid-card" } else { "tab" }, "row": a.lb.frow, "col": a.lb.fcol, "zone": a.lb.zone,
                   "tab": a.lb.tab, "appid": if a.lb.zone == 1 { g.map(|g| g.app_id) } else { None }, "name": g.map(|g| g.name.clone())})
        }
        Route::App(id) => json!({"kind": "game-page", "appid": id, "index": a.gm.focus}),
        Route::Downloads => json!({"kind": "downloads", "index": a.dl.focus}),
        Route::Settings(_) => json!({"kind": "settings", "index": a.se.focus}),
        _ => json!({"kind": "other"}),
    }
}

fn exec(a: &mut App, line: &str) -> String {
    let mut it = line.split_whitespace();
    let cmd = it.next().unwrap_or("");
    let args: Vec<&str> = it.collect();
    match (cmd, args.as_slice()) {
        ("route", [p]) => match parse_route(p) {
            Some(r) => {
                a.sm.open = false;
                a.qm.open = false;
                a.lb.sort_open = false;
                while a.router.back() {}
                if r == Route::Home {
                    a.close_search();
                    a.enter_route();
                } else {
                    a.navigate(r);
                }
                format!("ok {}", a.router.current().path())
            }
            None => format!("err bad route {p}"),
        },
        ("controller", [id]) => {
            let Ok(id) = id.parse::<u32>() else { return "err bad appid".into() };
            crate::screens::controller::open(a, id);
            format!("ok controller {id}")
        },
        ("props", [id, rest @ ..]) => {
            let Ok(id) = id.parse::<u32>() else { return "err bad appid".into() };
            crate::screens::properties::open(a, id, rest.first().copied());
            format!("ok props {id}")
        }
        ("focus", ["app", id]) => {
            let Ok(id) = id.parse::<u32>() else { return "err bad appid".into() };
            match a.router.current().clone() {
                Route::Home => match a.hm.recent.iter().position(|&i| a.games().get(i).map(|g| g.app_id) == Some(id)) {
                    Some(n) => {
                        a.hm.idx = n;
                        a.home_refresh();
                        a.focus_changed_home();
                        "ok".into()
                    }
                    None => "err appid not in recent".into(),
                },
                Route::Search if a.search_focus_app(id) => "ok".into(),
                Route::Search => "err appid not in search results".into(),
                Route::Library(_) => match a.lb.view_pos.get(&id).copied() {
                    Some((r, c)) => {
                        a.lb.frow = r;
                        a.lb.fcol = c;
                        a.lb.zone = 1;
                        a.lib_focus_changed();
                        "ok".into()
                    }
                    None => "err appid not in view".into(),
                },
                _ => "err focus app: unsupported route".into(),
            }
        }
        ("focus", ["index", n]) => {
            let Ok(n) = n.parse::<usize>() else { return "err bad index".into() };
            match a.router.current().clone() {
                Route::Home if n <= a.hm.recent.len() => {
                    a.hm.idx = n;
                    a.home_refresh();
                    a.focus_changed_home();
                    "ok".into()
                }
                Route::Search if a.search_focus_index(n) => "ok".into(),
                Route::Search => "err index out of range".into(),
                Route::Library(_) => {
                    let id = a.lb.view.get(n).and_then(|&i| a.games().get(i)).map(|g| g.app_id);
                    match id.and_then(|id| a.lb.view_pos.get(&id).copied()) {
                        Some((r, c)) => {
                            a.lb.frow = r;
                            a.lb.fcol = c;
                            a.lb.zone = 1;
                            a.lib_focus_changed();
                            "ok".into()
                        }
                        None => "err index out of range".into(),
                    }
                }
                _ => "err focus index: unsupported route/index".into(),
            }
        }
        ("key", [k]) => {
            let act = match *k {
                "a" => Action::A,
                "b" => Action::B,
                "x" => Action::X,
                "y" => Action::Y,
                "up" => Action::Up,
                "down" => Action::Down,
                "left" => Action::Left,
                "right" => Action::Right,
                "lb" => Action::LB,
                "rb" => Action::RB,
                "l2" => Action::L2,
                "r2" => Action::R2,
                "menu" => Action::Menu,
                "view" => Action::View,
                "steam" => Action::Steam,
                "qam" => Action::Qam,
                _ => return format!("err bad key {k}"),
            };
            a.handle_pad(act);
            "ok".into()
        }
        ("notification-test", [kind]) if crate::actions::dryrun() => {
            let (title, body) = match *kind {
                "screenshot" => (tr_ref("capture.screenshot_taken"), tr_ref("notification.notification_test")),
                "achievement" => (tr_ref("notification.achievement_unlocked"), tr_ref("notification.achievement_test")),
                "friend" => (tr_ref("notification.friend"), tr_ref("notification.in_game_test")),
                _ => return "err bad notification kind".into(),
            };
            crate::notifications::enqueue(&a.ui(), crate::notifications::Event {
                title: title.into(), body: body.into(), kind: (*kind).into(), ..Default::default()
            }, !a.live.running.is_empty());
            "ok".into()
        }
        ("snapshot", [f]) => match a.ui().window().take_snapshot() {
            Ok(buf) => match image::RgbaImage::from_raw(buf.width(), buf.height(), buf.as_bytes().to_vec()) {
                Some(img) => match img.save(f) {
                    Ok(_) => format!("ok {f} {}x{}", buf.width(), buf.height()),
                    Err(e) => format!("err {e}"),
                },
                None => "err bad buffer".into(),
            },
            Err(e) => format!("err {e}"),
        },
        ("dumptree", [f]) => {
            let (r, s) = route_state(a);
            match crate::dumptree::dump(&a.ui(), &r, &s, Some(f)) {
                Ok(m) => format!("ok {m}"),
                Err(e) => format!("err {e}"),
            }
        }
        ("plugin", names) if !names.is_empty() => {
            // open a Decky plugin panel by name (QAM must be open on the Decky tab); `plugin list` = back to the list
            let name = names.join(" ");
            if name == "list" {
                crate::plugins::open();
                return "ok list".into();
            }
            if crate::plugins::open_plugin(&name) { format!("ok {name}") } else { format!("err no such plugin {name}") }
        }
        ("qamtab", [n]) => match n.parse::<usize>() {
            Ok(n) if n < 5 => {
                a.qam_select_tab(n, false);
                format!("ok {n}")
            }
            _ => "err qamtab 0..4".into(),
        },
        ("plugins", []) => {
            // surfaces the Decky tab can show, from the generated panel spec
            let (_, defs) = crate::plugins::load_defs();
            defs.iter().map(|d| d.name.clone()).collect::<Vec<_>>().join("\n")
        }
        ("anim", a) => crate::anim::command(a),
        ("manage", [id]) => id.parse::<u32>().map(|id| a.manage_dump(id)).unwrap_or_else(|_| "err bad appid".into()),
        ("memstats", []) => {
            let mut by: std::collections::BTreeMap<String, (usize, usize)> = Default::default();
            for ((_, k), img) in a.imgs.iter() {
                let sz = img.size();
                let e = by.entry(format!("{k:?}")).or_default();
                e.0 += 1;
                e.1 += (sz.width * sz.height * 4) as usize;
            }
            let anim: usize = a.anim_cache.values().map(|f| f.iter().map(|(i, _)| { let s = i.size(); (s.width * s.height * 4) as usize }).sum::<usize>()).sum();
            let mi = unsafe { libc::mallinfo2() };
            format!("imgs(count,MB)={:?} anim_cache={}MB ({} anims) malloc_inuse={}MB arena={}MB mmapped={}MB",
                by.iter().map(|(k, v)| (k.clone(), v.0, v.1 >> 20)).collect::<Vec<_>>(), anim >> 20, a.anim_cache.len(), mi.uordblks >> 20, mi.arena >> 20, mi.hblkhd >> 20)
        }
        ("exp", ["bgoff"]) => {
            a.ui().global::<crate::BackdropState>().set_bg_on(false);
            "ok".into()
        }
        ("framestats", []) => crate::framestats::report(),
        ("clockstats", []) => crate::app::clock_report(),
        ("framestats", ["reset"]) => {
            crate::framestats::reset();
            "ok".into()
        }
        ("state", []) => {
            let (r, s) = route_state(a);
            json!({"route": a.router.current().path(), "screen": r, "substate": s, "depth": a.router.depth(),
                   "menu": a.sm.open, "qam": a.qm.open, "focus": focus_json(a)})
            .to_string()
        }
        ("click", [x, y]) => {
            let (Ok(x), Ok(y)) = (x.parse::<f32>(), y.parse::<f32>()) else { return "err bad coords".into() };
            let ui = a.ui();
            let w = ui.window();
            ptr(w, PtrEv::Move(x, y));
            ptr(w, PtrEv::Down(x, y));
            ptr(w, PtrEv::Up(x, y));
            "ok".into()
        }
        ("drag", [x0, y0, x1, y1, rest @ ..]) => {
            let v: Vec<Result<f32, _>> = [x0, y0, x1, y1].iter().map(|s| s.parse::<f32>()).collect();
            if v.iter().any(|r| r.is_err()) {
                return "err bad coords".into();
            }
            let v: Vec<f32> = v.into_iter().map(|r| r.unwrap()).collect();
            let steps = rest.first().and_then(|s| s.parse::<u32>().ok()).unwrap_or(8).max(1);
            let ui = a.ui();
            let w = ui.window();
            ptr(w, PtrEv::Move(v[0], v[1]));
            ptr(w, PtrEv::Down(v[0], v[1]));
            for i in 1..=steps {
                let t = i as f32 / steps as f32;
                ptr(w, PtrEv::Move(v[0] + (v[2] - v[0]) * t, v[1] + (v[3] - v[1]) * t));
            }
            ptr(w, PtrEv::Up(v[2], v[3]));
            "ok".into()
        }
        ("longpress", [x, y, rest @ ..]) => {
            let (Ok(x), Ok(y)) = (x.parse::<f32>(), y.parse::<f32>()) else { return "err bad coords".into() };
            let ms = rest.first().and_then(|s| s.parse::<u64>().ok()).unwrap_or(800);
            let ui = a.ui();
            let w = ui.window();
            ptr(w, PtrEv::Move(x, y));
            ptr(w, PtrEv::Down(x, y));
            // release later from the event loop so Slint's long-press timers can fire in between
            slint::Timer::single_shot(Duration::from_millis(ms), move || {
                crate::with_app(|a| ptr(a.ui().window(), PtrEv::Up(x, y)));
            });
            "ok".into()
        }
        ("quit", []) => {
            let _ = slint::quit_event_loop();
            "ok".into()
        }
        _ => format!("err unknown command: {line}"),
    }
}

enum PtrEv {
    Move(f32, f32),
    Down(f32, f32),
    Up(f32, f32),
}

fn ptr(w: &slint::Window, e: PtrEv) {
    use slint::platform::{PointerEventButton, WindowEvent};
    let ev = match e {
        PtrEv::Move(x, y) => WindowEvent::PointerMoved { position: slint::LogicalPosition::new(x, y) },
        PtrEv::Down(x, y) => WindowEvent::PointerPressed { position: slint::LogicalPosition::new(x, y), button: PointerEventButton::Left },
        PtrEv::Up(x, y) => WindowEvent::PointerReleased { position: slint::LogicalPosition::new(x, y), button: PointerEventButton::Left },
    };
    w.dispatch_event(ev);
}
