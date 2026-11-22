//! DECKSHELL_FRAMESTATS=1: log inter-frame intervals (BeforeRendering -> AfterRendering span = render cost, plus gaps).
//! Control commands `framestats reset` / `framestats` print count, fps, render-time p50/p99/max and frame-interval p99.
use slint::{ComponentHandle, RenderingState};
use std::cell::RefCell;
use std::time::Instant;

#[derive(Default)]
struct Stats {
    start: Option<Instant>,
    before: Option<Instant>,
    last_end: Option<Instant>,
    render_us: Vec<u32>,
    interval_us: Vec<u32>,
}

thread_local! { static ST: RefCell<Stats> = RefCell::new(Stats::default()); }

pub fn init(ui: &crate::AppWindow) {
    if !crate::env_flag("DECKSHELL_FRAMESTATS") {
        return;
    }
    let r = ui.window().set_rendering_notifier(|state, _| {
        let now = Instant::now();
        ST.with(|s| {
            let mut s = s.borrow_mut();
            match state {
                RenderingState::BeforeRendering => s.before = Some(now),
                RenderingState::AfterRendering => {
                    if let Some(b) = s.before.take() {
                        s.render_us.push(now.duration_since(b).as_micros() as u32);
                    }
                    if let Some(l) = s.last_end {
                        s.interval_us.push(now.duration_since(l).as_micros() as u32);
                    }
                    s.last_end = Some(now);
                    s.start.get_or_insert(now);
                }
                _ => {}
            }
        })
    });
    if r.is_err() {
        eprintln!("[deckshell] framestats: rendering notifier unavailable");
    }
}

pub fn reset() {
    ST.with(|s| *s.borrow_mut() = Stats { start: Some(Instant::now()), ..Default::default() });
}

fn pct(v: &[u32], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let mut v = v.to_vec();
    v.sort_unstable();
    v[((v.len() - 1) as f64 * p) as usize] as f64 / 1000.0
}

pub fn report() -> String {
    ST.with(|s| {
        let s = s.borrow();
        let secs = s.start.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0);
        // interval gaps while idle are not stalls; only report intervals < 250ms for p99 of "frame to frame" under load
        let busy: Vec<u32> = s.interval_us.iter().copied().filter(|&x| x < 250_000).collect();
        format!(
            "frames={} fps={:.1} render_ms p50={:.2} p99={:.2} max={:.2} interval_ms p50={:.2} p99={:.2} max={:.2}",
            s.render_us.len(),
            s.render_us.len() as f64 / secs.max(0.001),
            pct(&s.render_us, 0.5),
            pct(&s.render_us, 0.99),
            pct(&s.render_us, 1.0),
            pct(&busy, 0.5),
            pct(&busy, 0.99),
            pct(&busy, 1.0)
        )
    })
}

/// Отдельные участки между кадрами: статистика рендера сама по себе не видит
/// время обработки ввода и применения результатов на UI-потоке.
pub fn search_span(phase: &str, started: Instant) {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ENABLED.get_or_init(|| crate::env_flag("DECKSHELL_FRAMESTATS")) {
        let us = started.elapsed().as_micros();
        eprintln!("[deckshell] search.profile phase={phase} us={us}");
    }
}
