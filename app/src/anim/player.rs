//! Playback engine: one video (RGBA frames from the cache, or decoded live on a cache miss) + exactly one audio stream
//! (a rodio `Source` added to the shared mixer of sound.rs), presented through the `AnimState` Slint global.
//!
//! Timing: the UI thread runs a 5 ms timer. The clock is wall time anchored at the moment the mixer first pulled our audio
//! (so audio and picture start together), capped by the audio position when the audio device stalls. The latest frame whose
//! pts <= clock is shown; late frames are dropped, never queued. Everything heavy (file reads, lz4, ffmpeg) is on worker threads.
use super::cache::{self, Dsa, Opts, Writer};
use crate::{AnimPageState, AnimState, AppWindow};
use slint::{ComponentHandle, Image, Rgba8Pixel, SharedPixelBuffer};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

const CHANNEL_FRAMES: usize = 8;
const TICK: Duration = Duration::from_millis(5);
const LOAD_TIMEOUT: Duration = Duration::from_secs(6);
const AUDIO_START_GRACE: Duration = Duration::from_millis(400);
const FADE_AUDIO_US: i64 = 150_000;

/// 0 = overlay does not take input, 1 = swallow input, 2 = any input skips (boot)
pub static INPUT_MODE: AtomicU8 = AtomicU8::new(0);
static SKIP: AtomicBool = AtomicBool::new(false);
pub static ACTIVE_GEN: AtomicU64 = AtomicU64::new(0);

fn debug_on() -> bool {
    static D: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *D.get_or_init(|| std::env::var_os("DECKSHELL_ANIM_DEBUG").is_some())
}

pub fn request_skip(source: &'static str) {
    if !SKIP.swap(true, Ordering::Relaxed) { eprintln!("[anim] skip requested: {source}"); }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// played to the end
    Finished,
    /// cut at the deadline (logind delay budget) with a short audio fade
    Trimmed,
    /// user skipped (boot)
    Skipped,
    /// could not play (decode error, timeout)
    Failed,
    /// replaced by another playback / cancelled
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    /// fade the overlay out (boot, resume)
    FadeOut,
    /// keep the last frame and keep swallowing input (suspend / shutdown: the screen goes off or the machine powers down)
    Hold,
}

pub struct PlayOpts {
    pub label: String,
    pub in_catalog: bool,
    /// This is the initial boot playback that must be ready before mapping.
    pub defer_initial_show: bool,
    pub skippable: bool,
    pub end: End,
    /// maximum time the animation may take, measured from the first shown frame
    pub budget: Option<Duration>,
    pub volume: f32,
    pub video_delay_ms: i64,
    pub decode: Opts,
    pub cache_dir: std::path::PathBuf,
    pub on_done: Box<dyn FnOnce(Outcome)>,
}

// ---------------------------------------------------------------- audio

pub struct AudioShared {
    started: AtomicBool,
    consumed: AtomicU64,
    gain: AtomicU32,
    stop: AtomicBool,
    finished: AtomicBool,
    output_busy: AtomicBool,
}

struct Pcm {
    data: Arc<Vec<i16>>,
    pos: usize,
    gain: f32,
    sh: Arc<AudioShared>,
}

impl Iterator for Pcm {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.sh.stop.load(Ordering::Relaxed) || self.pos >= self.data.len() {
            self.sh.finished.store(true, Ordering::Relaxed);
            return None;
        }
        if self.pos & 127 == 0 {
            self.sh.started.store(true, Ordering::Relaxed);
            self.sh.consumed.store(self.pos as u64, Ordering::Relaxed);
            self.gain = f32::from_bits(self.sh.gain.load(Ordering::Relaxed));
            if self.sh.output_busy.load(Ordering::Relaxed) {
                self.gain = 0.0;
            }
        }
        let s = *self.data.get(self.pos)?;
        self.pos += 1;
        Some(s as f32 * (1.0 / 32768.0) * self.gain)
    }
}

impl rodio::Source for Pcm {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> rodio::ChannelCount {
        cache::AUDIO_CH
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        cache::AUDIO_RATE
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

// ---------------------------------------------------------------- loading (worker thread)

pub struct Frame {
    pub pts_us: u64,
    pub buf: SharedPixelBuffer<Rgba8Pixel>,
    pub dup: bool,
}

struct Loaded {
    rx: Receiver<Frame>,
    audio: Option<Arc<Vec<i16>>>,
    duration_us: u64,
    cached: bool,
    desc: String,
}

fn load(path: &std::path::Path, dir: &std::path::Path, o: &Opts, cache_live: bool) -> Result<Loaded, String> {
    let t0 = Instant::now();
    let cp = cache::cache_path(dir, path, o);
    let (tx, rx) = sync_channel::<Frame>(CHANNEL_FRAMES);
    if let Some(d) = Dsa::open(&cp) {
        let audio = d.has_audio().then(|| Arc::new(d.audio()));
        let duration_us = d.duration_us;
        let desc = format!("cache {}x{} {} frames, opened in {} ms", d.w, d.h, d.index.len(), t0.elapsed().as_millis());
        if let Ok(f) = std::fs::File::options().write(true).open(&cp) {
            let _ = f.set_modified(std::time::SystemTime::now());
        }
        std::thread::Builder::new().name("anim-read".into()).spawn(move || {
            let (mut tmp, mut prev): (Vec<u8>, Option<SharedPixelBuffer<Rgba8Pixel>>) = (vec![], None);
            for i in 0..d.index.len() {
                let pts = d.index[i].pts_us as u64;
                let (buf, dup) = if d.is_dup(i) && prev.is_some() {
                    (prev.clone().unwrap(), true)
                } else {
                    let mut b = SharedPixelBuffer::<Rgba8Pixel>::new(d.w, d.h);
                    if !d.frame_into(i, &mut tmp, b.make_mut_bytes(), prev.as_ref().map(|p| p.as_bytes())) {
                        break;
                    }
                    (b, false)
                };
                prev = Some(buf.clone());
                if tx.send(Frame { pts_us: pts, buf, dup }).is_err() {
                    break;
                }
            }
        })
        .map_err(|e| e.to_string())?;
        return Ok(Loaded { rx, audio, duration_us, cached: true, desc });
    }
    // cache miss: decode live and write the cache on the side
    let p = cache::probe(path)?;
    let audio = if p.has_audio {
        let a = cache::decode_audio(path, o).unwrap_or_default();
        (!a.is_empty()).then(|| Arc::new(a))
    } else {
        None
    };
    let audio_us = audio.as_ref().map(|a| (a.len() as u64 / 2) * 1_000_000 / cache::AUDIO_RATE as u64).unwrap_or(0);
    let duration_us = ((p.duration_s * 1e6) as u64).max(audio_us);
    let (w, h) = cache::fit(p.w, p.h, o.maxw, o.maxh);
    let desc = format!("live decode {w}x{h}, audio {} ms, ready in {} ms", audio_us / 1000, t0.elapsed().as_millis());
    let o2 = o.clone();
    let (src, cp2, audio2) = (path.to_path_buf(), cp, audio.clone());
    std::thread::Builder::new().name("anim-live".into()).spawn(move || {
        let (wtx, wrx) = sync_channel::<(u64, Option<SharedPixelBuffer<Rgba8Pixel>>)>(48);
        let wtx = std::sync::Mutex::new(Some(wtx));
        let writer = cache_live.then(|| Writer::create(&cp2, w, h).ok()).flatten().map(|mut wr| {
            std::thread::spawn(move || {
                let mut ok = true;
                while let Ok((pts, f)) = wrx.recv() {
                    if ok && wr.push(pts, f.as_ref().map(|b| b.as_bytes())).is_err() {
                        ok = false;
                    }
                }
                (wr, ok)
            })
        });
        let mut cache_ok = writer.is_some();
        let mut complete = true;
        let mut last = 0u64;
        let r = cache::decode_video(&src, &p, &o2, |_, pts, buf, dup| {
            last = pts;
            if cache_ok {
                let g = wtx.lock().unwrap();
                if let Some(t) = g.as_ref() {
                    if t.try_send((pts, if dup { None } else { Some(buf.clone()) })).is_err() {
                        cache_ok = false; // writer too slow: give up caching rather than stall playback
                    }
                }
            }
            if tx.send(Frame { pts_us: pts, buf: buf.clone(), dup }).is_err() {
                complete = false;
                return false;
            }
            true
        });
        drop(wtx.lock().unwrap().take());
        if let Some(handle) = writer {
            if let Ok((wr, ok)) = handle.join() {
                if r.is_ok() && complete && cache_ok && ok {
                    let a = audio2.as_ref().map(|a| a.as_slice()).unwrap_or(&[]);
                    let _ = wr.finish(a, duration_us.max(last));
                } else {
                    wr.abort();
                }
            }
        }
    })
    .map_err(|e| e.to_string())?;
    Ok(Loaded { rx, audio, duration_us, cached: false, desc })
}

// ---------------------------------------------------------------- UI-thread state

struct Play {
    generation: u64,
    label: String,
    in_catalog: bool,
    preview_audio: Option<crate::sound::PreviewAudio>,
    output_busy: bool,
    defer_initial_show: bool,
    opts_end: End,
    skippable: bool,
    budget: Option<Duration>,
    volume: f32,
    video_delay_us: i64,
    requested: Instant,
    on_done: Option<Box<dyn FnOnce(Outcome)>>,
    // set by attach()
    rx: Option<Receiver<Frame>>,
    audio: Option<Arc<Vec<i16>>>,
    duration_us: u64,
    shared: Option<Arc<AudioShared>>,
    // running state
    pending: Option<Frame>,
    started: bool,
    t0: Instant,
    anchor: Option<Instant>,
    audio_given_up: bool,
    reader_done: bool,
    shown: bool,
    clock_started: bool,
    last_pts: u64,
    deadline_us: Option<i64>,
    wall_deadline: Option<Instant>,
    dbg_sec: u128,
}

thread_local! {
    static PLAY: RefCell<Option<Play>> = const { RefCell::new(None) };
    static TIMER: slint::Timer = slint::Timer::default();
    static UI: RefCell<Option<slint::Weak<AppWindow>>> = const { RefCell::new(None) };
    // Set by main() before entering the event loop for a boot animation. The
    // window stays unmapped until its first frame (or playback fails).
    static DEFERRED_SHOW: RefCell<bool> = const { RefCell::new(false) };
}

pub fn set_ui(ui: slint::Weak<AppWindow>) {
    UI.with(|u| *u.borrow_mut() = Some(ui));
}

fn with_ui<R>(f: impl FnOnce(&AppWindow) -> R) -> Option<R> {
    UI.with(|u| u.borrow().as_ref().and_then(|w| w.upgrade())).map(|ui| f(&ui))
}

pub fn is_playing() -> bool {
    PLAY.with(|p| p.borrow().is_some())
}
pub fn playing_label() -> Option<String> {
    PLAY.with(|p| p.borrow().as_ref().filter(|p| !p.in_catalog).map(|p| p.label.clone()))
}

pub fn catalog_playing() -> bool {
    PLAY.with(|p| p.borrow().as_ref().is_some_and(|p| p.in_catalog))
}

pub fn cancel_catalog() {
    if catalog_playing() {
        finish(Outcome::Cancelled);
    }
}

/// Defer mapping the main window until the boot animation's first frame is ready.
/// Must be called after anim::init() and before entering Slint's event loop.
pub fn defer_initial_show() -> bool {
    let boot_playing = PLAY.with(|p| p.borrow().as_ref().map(|p| p.defer_initial_show && !p.shown).unwrap_or(false));
    if boot_playing {
        DEFERRED_SHOW.with(|d| *d.borrow_mut() = true);
    }
    boot_playing
}

/// Consume the deferred mapping request. Called only on the UI thread.
fn show_deferred_window() {
    let deferred = DEFERRED_SHOW.with(|d| std::mem::replace(&mut *d.borrow_mut(), false));
    if !deferred { return; }
    with_ui(|ui| {
        eprintln!("[anim] mapping initial window (frame_ready={})", ui.global::<AnimState>().get_has_frame());
        if let Err(e) = ui.window().show() {
            eprintln!("[anim] cannot show deferred window: {e}");
            let _ = slint::quit_event_loop();
        }
    });
}

/// Remaining play time budget change (logind told us the delay window started now).
pub fn tighten_budget(max_from_now: Duration) {
    PLAY.with(|p| {
        if let Some(p) = p.borrow_mut().as_mut() {
            let deadline = Instant::now() + max_from_now;
            p.wall_deadline = Some(p.wall_deadline.map(|d| d.min(deadline)).unwrap_or(deadline));
            let now = clock_us(p);
            let dl = now + max_from_now.as_micros() as i64;
            p.deadline_us = Some(p.deadline_us.map(|d| d.min(dl)).unwrap_or(dl));
        }
    });
}

/// Show the overlay immediately (black) so nothing of the UI flashes while the worker opens the file.
pub fn cover(fade_in_ms: i32, input_mode: u8) {
    with_ui(|ui| {
        let s = ui.global::<AnimState>();
        s.set_fade_ms(fade_in_ms);
        s.set_has_frame(false);
        s.set_active(true);
    });
    INPUT_MODE.store(input_mode, Ordering::Relaxed);
}

/// Hide the overlay (fade) and release input.
pub fn uncover(fade_ms: i32) {
    with_ui(|ui| {
        let s = ui.global::<AnimState>();
        s.set_fade_ms(fade_ms);
        s.set_active(false);
    });
    INPUT_MODE.store(0, Ordering::Relaxed);
}

pub fn start(path: std::path::PathBuf, o: PlayOpts) {
    cancel(Outcome::Cancelled);
    let generation = ACTIVE_GEN.fetch_add(1, Ordering::Relaxed) + 1;
    SKIP.store(false, Ordering::Relaxed);
    if !o.in_catalog {
        cover(if o.end == End::FadeOut { 0 } else { 120 }, if o.skippable { 2 } else { 1 });
    }
    let play = Play {
        generation,
        label: o.label.clone(),
        in_catalog: o.in_catalog,
        preview_audio: None,
        output_busy: false,
        defer_initial_show: o.defer_initial_show,
        opts_end: o.end,
        skippable: o.skippable,
        budget: o.budget,
        volume: o.volume,
        video_delay_us: o.video_delay_ms * 1000,
        requested: Instant::now(),
        on_done: Some(o.on_done),
        rx: None,
        audio: None,
        duration_us: 0,
        shared: None,
        pending: None,
        started: false,
        t0: Instant::now(),
        anchor: None,
        audio_given_up: false,
        reader_done: false,
        shown: false,
        clock_started: false,
        last_pts: 0,
        deadline_us: None,
        wall_deadline: o.budget.map(|b| Instant::now() + b),
        dbg_sec: u128::MAX,
    };
    PLAY.with(|p| *p.borrow_mut() = Some(play));
    let (dir, dec, label) = (o.cache_dir.clone(), o.decode.clone(), o.label.clone());
    std::thread::Builder::new()
        .name("anim-load".into())
        .spawn(move || {
            let r = load(&path, &dir, &dec, !o.in_catalog);
            let output_busy = o.in_catalog && o.volume > 0.0 && r.as_ref().is_ok_and(|l| l.audio.is_some())
                && crate::sound::mixer().is_some() && crate::sound::preview_output_busy().unwrap_or(false);
            let _ = slint::invoke_from_event_loop(move || attach(generation, label, r, output_busy));
        })
        .ok();
    TIMER.with(|t| t.start(slint::TimerMode::Repeated, TICK, tick));
}

fn attach(generation: u64, label: String, r: Result<Loaded, String>, output_busy: bool) {
    match r {
        Ok(l) => {
            eprintln!("[anim] {label}: {}", l.desc);
            PLAY.with(|p| {
                if let Some(pl) = p.borrow_mut().as_mut().filter(|pl| pl.generation == generation) {
                    pl.rx = Some(l.rx);
                    pl.audio = l.audio.filter(|_| !pl.in_catalog || pl.volume > 0.0);
                    pl.output_busy = output_busy;
                    pl.duration_us = l.duration_us;
                    let _ = l.cached;
                }
            });
        }
        Err(e) => {
            eprintln!("[anim] {label}: cannot play: {e}");
            if PLAY.with(|p| p.borrow().as_ref().map(|pl| pl.generation == generation).unwrap_or(false)) {
                finish(Outcome::Failed);
            }
        }
    }
}

fn clock_us(p: &Play) -> i64 {
    if !p.started {
        return 0;
    }
    let base = match p.anchor {
        Some(a) => a,
        None if p.audio.is_none() || p.audio_given_up => p.t0,
        None => return 0, // first frame waits for the audio device to start pulling
    };
    let wall = base.elapsed().as_micros() as i64 - p.video_delay_us;
    // device stalled / underrun: follow the audio position instead of running away from it
    if let (Some(sh), Some(_)) = (&p.shared, &p.anchor) {
        let apos = (sh.consumed.load(Ordering::Relaxed) / cache::AUDIO_CH as u64) as i64 * 1_000_000 / cache::AUDIO_RATE as i64;
        if sh.started.load(Ordering::Relaxed) && (!p.in_catalog || !sh.finished.load(Ordering::Relaxed)) && apos + 80_000 < wall {
            return (apos + 80_000).max(0);
        }
    }
    wall.max(0)
}

fn tick() {
    let mut done: Option<Outcome> = None;
    let mut show: Option<SharedPixelBuffer<Rgba8Pixel>> = None;
    PLAY.with(|cell| {
        let mut g = cell.borrow_mut();
        let Some(p) = g.as_mut() else { return };
        if p.wall_deadline.is_some_and(|d| Instant::now() >= d) {
            done = Some(Outcome::Trimmed);
            return;
        }
        if SKIP.swap(false, Ordering::Relaxed) && p.skippable {
            done = Some(Outcome::Skipped);
            return;
        }
        let Some(rx) = p.rx.as_ref() else {
            if p.requested.elapsed() > LOAD_TIMEOUT {
                done = Some(Outcome::Failed);
            }
            return;
        };
        if !p.started {
            // wait for the first frame, then start audio + clock together
            match rx.try_recv() {
                Ok(f) => p.pending = Some(f),
                Err(TryRecvError::Empty) => {
                    if p.requested.elapsed() > LOAD_TIMEOUT {
                        done = Some(Outcome::Failed);
                    }
                    return;
                }
                Err(TryRecvError::Disconnected) => {
                    done = Some(Outcome::Failed);
                    return;
                }
            }
            p.started = true;
            let f = p.pending.take().unwrap();
            p.last_pts = f.pts_us;
            p.shown = true;
            show = Some(f.buf);
            if let Some(b) = p.budget {
                p.deadline_us = Some(b.as_micros() as i64);
            }
            return;
        }
        // audio anchor
        if p.anchor.is_none() {
            if let Some(sh) = &p.shared {
                if sh.started.load(Ordering::Relaxed) {
                    p.anchor = Some(Instant::now());
                } else if p.t0.elapsed() > AUDIO_START_GRACE && !p.audio_given_up {
                    eprintln!("[anim] {}: audio device did not start, continuing silent", p.label);
                    sh.stop.store(true, Ordering::Relaxed);
                    p.audio_given_up = true;
                    // Time spent waiting for the audio device held the first frame.
                    p.t0 = Instant::now();
                }
            }
        }
        let now = clock_us(p);
        if debug_on() && p.t0.elapsed().as_millis() / 1000 != p.dbg_sec {
            p.dbg_sec = p.t0.elapsed().as_millis() / 1000;
            let apos = p.shared.as_ref().map(|sh| (sh.consumed.load(Ordering::Relaxed) / 2) as i64 * 1000 / 48000).unwrap_or(-1);
            eprintln!("[anim.dbg] wall={} ms clock={} ms audio_pos={} ms pending_pts={:?} last_shown={} reader_done={}", p.t0.elapsed().as_millis(), now / 1000, apos, p.pending.as_ref().map(|f| f.pts_us / 1000), p.last_pts / 1000, p.reader_done);
        }
        // newest frame that is due
        let mut latest: Option<Frame> = None;
        loop {
            if p.pending.is_none() && !p.reader_done {
                match rx.try_recv() {
                    Ok(f) => p.pending = Some(f),
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => p.reader_done = true,
                }
            }
            match p.pending.take() {
                Some(f) if f.pts_us as i64 <= now => {
                    p.last_pts = f.pts_us;
                    latest = Some(f);
                }
                Some(f) => {
                    p.pending = Some(f);
                    break;
                }
                None => break,
            }
        }
        if let Some(f) = latest {
            if !f.dup {
                show = Some(f.buf);
            }
        }
        // trimmed audio fade
        if let (Some(sh), Some(dl)) = (&p.shared, p.deadline_us) {
            let left = (dl - now).clamp(0, FADE_AUDIO_US) as f32 / FADE_AUDIO_US as f32;
            sh.gain.store((p.volume * left).to_bits(), Ordering::Relaxed);
        }
        let audio_end_us = p.audio.as_ref().map(|a| (a.len() as i64 / 2) * 1_000_000 / cache::AUDIO_RATE as i64).unwrap_or(0);
        let end_us = (p.duration_us as i64).max(audio_end_us).max(p.last_pts as i64);
        if let Some(dl) = p.deadline_us {
            if now >= dl && now < end_us {
                done = Some(Outcome::Trimmed);
                return;
            }
        }
        if p.reader_done && p.pending.is_none() && now >= end_us {
            done = Some(Outcome::Finished);
            return;
        }
        // wall guard: never hang the power flow on a stalled clock
        if p.t0.elapsed().as_micros() as i64 > end_us + 2_500_000 + p.video_delay_us.max(0) {
            done = Some(Outcome::Finished);
        }
    });
    if let Some(buf) = show {
        let in_catalog = PLAY.with(|cell| cell.borrow().as_ref().is_some_and(|p| p.in_catalog));
        with_ui(|ui| {
            let img = Image::from_rgba8(buf);
            if in_catalog {
                let s = ui.global::<AnimPageState>();
                s.set_preview(img);
                s.set_has_preview(true);
                return;
            }
            let s = ui.global::<AnimState>();
            s.set_frame(img);
            s.set_has_frame(true);
            if DEFERRED_SHOW.with(|d| *d.borrow()) {
                // Only the initial boot frame needs to be opaque before mapping.
                // Leave all subsequent fade and suspend playback semantics intact.
                s.set_fade_ms(0);
                s.set_active(true);
            }
        });
        show_deferred_window();
        // Start the clock and mixer only after the first frame has been handed
        // to the window, including the deferred initial mapping.
        PLAY.with(|cell| {
            if let Some(p) = cell.borrow_mut().as_mut() {
                if !p.clock_started {
                    p.clock_started = true;
                    p.t0 = Instant::now();
                    if let (Some(a), Some(m)) = (p.audio.clone(), crate::sound::mixer()) {
                        let sh = Arc::new(AudioShared {
                            started: AtomicBool::new(false),
                            consumed: AtomicU64::new(0),
                            gain: AtomicU32::new(p.volume.to_bits()),
                            stop: AtomicBool::new(false),
                            finished: AtomicBool::new(false),
                            output_busy: AtomicBool::new(p.output_busy),
                        });
                        if p.in_catalog {
                            p.preview_audio = Some(crate::sound::PreviewAudio::begin());
                            watch_catalog_audio(sh.clone());
                        }
                        m.add(Pcm { data: a, pos: 0, gain: p.volume, sh: sh.clone() });
                        p.shared = Some(sh);
                    } else {
                        p.audio = None;
                    }
                }
            }
        });
    }
    if let Some(o) = done {
        finish(o);
    }
}

// Если другой плеер запустился во время ролика, приглушаем только предпросмотр.
fn watch_catalog_audio(sh: Arc<AudioShared>) {
    std::thread::spawn(move || {
        while !sh.stop.load(Ordering::Relaxed) && !sh.finished.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(400));
            if sh.stop.load(Ordering::Relaxed) || sh.finished.load(Ordering::Relaxed) {
                break;
            }
            if let Some(busy) = crate::sound::preview_output_busy() {
                sh.output_busy.store(busy, Ordering::Relaxed);
            }
        }
    });
}

fn take_play() -> Option<Play> {
    PLAY.with(|p| p.borrow_mut().take())
}

fn finish(outcome: Outcome) {
    let Some(mut p) = take_play() else { return };
    TIMER.with(|t| t.stop());
    if let Some(sh) = &p.shared {
        sh.stop.store(true, Ordering::Relaxed);
    }
    eprintln!("[anim] {}: {:?} after {} ms (last pts {} ms)", p.label, outcome, p.t0.elapsed().as_millis(), p.last_pts / 1000);
    drop(p.rx.take()); // stops the reader / live decoder thread
    drop(p.preview_audio.take());
    if !p.in_catalog && outcome != Outcome::Cancelled && (p.opts_end == End::FadeOut || outcome == Outcome::Failed) {
        uncover(300);
    } // Hold: the last frame stays and input stays swallowed until uncover()
    // A failed, skipped, timed out, or cancelled boot can finish before a
    // frame is available. Always map the window in that case so the app does
    // not remain alive but invisible.
    if DEFERRED_SHOW.with(|d| *d.borrow()) {
        uncover(0);
        show_deferred_window();
    }
    if let Some(cb) = p.on_done.take() {
        cb(outcome);
    }
}

/// Abort the running playback (audio stops immediately); the overlay is left to the caller.
pub fn cancel(outcome: Outcome) {
    if is_playing() {
        finish(outcome);
    }
}

#[cfg(test)]
mod playback_tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn fixture(done: Rc<Cell<Option<Outcome>>>) {
        let (tx, rx) = sync_channel(2);
        tx.send(Frame { pts_us: 0, buf: SharedPixelBuffer::new(1, 1), dup: false }).unwrap();
        tx.send(Frame { pts_us: 1_000_000, buf: SharedPixelBuffer::new(1, 1), dup: false }).unwrap();
        drop(tx);
        let now = Instant::now();
        PLAY.with(|p| *p.borrow_mut() = Some(Play {
            generation: 1, label: "test".into(), in_catalog: false, preview_audio: None, output_busy: false, defer_initial_show: false,
            opts_end: End::FadeOut, skippable: true, budget: None, volume: 1.0,
            video_delay_us: 0, requested: now - Duration::from_secs(5),
            on_done: Some(Box::new(move |o| done.set(Some(o)))), rx: Some(rx), audio: None,
            duration_us: 1_000_000, shared: None, pending: None, started: false,
            t0: now, anchor: None, audio_given_up: false, reader_done: false,
            shown: false, clock_started: false, last_pts: 0, deadline_us: None, wall_deadline: None, dbg_sec: u128::MAX,
        }));
        SKIP.store(false, Ordering::Relaxed);
    }

    #[test]
    fn catalog_webm_decodes_audio_and_cancel_stops_the_shared_pcm() {
        let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("deckshell-423-webm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("preview.webm");
        let Ok(status) = std::process::Command::new("ffmpeg").args([
            "-v", "error", "-y", "-f", "lavfi", "-i", "color=blue:size=64x64:rate=24",
            "-f", "lavfi", "-i", "sine=frequency=440", "-t", "0.25",
            "-c:v", "libvpx", "-c:a", "libopus",
        ]).arg(&path).status() else {
            std::fs::remove_dir_all(dir).unwrap();
            eprintln!("Пропущена проверка генерации тестового WebM: ffmpeg недоступен");
            return;
        };
        assert!(status.success());

        let loaded = load(&path, &dir, &Opts { maxw: 64, maxh: 64, background: false }, false).unwrap();
        assert!(loaded.rx.recv_timeout(Duration::from_secs(3)).is_ok());
        let data = loaded.audio.expect("WebM должен содержать звуковую дорожку");
        assert!(data.iter().any(|sample| *sample != 0));
        let done = Rc::new(Cell::new(None));
        fixture(done.clone());
        let shared = Arc::new(AudioShared {
            started: AtomicBool::new(false),
            consumed: AtomicU64::new(0),
            gain: AtomicU32::new(0.25f32.to_bits()),
            stop: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            output_busy: AtomicBool::new(false),
        });
        PLAY.with(|p| {
            let mut p = p.borrow_mut();
            let p = p.as_mut().unwrap();
            p.in_catalog = true;
            p.shared = Some(shared.clone());
        });
        let mut pcm = Pcm { data: data.clone(), pos: 0, gain: 1.0, sh: shared.clone() };
        for sample in data.iter().take(128) {
            assert_eq!(pcm.next(), Some(*sample as f32 * (0.25 / 32768.0)));
        }
        shared.output_busy.store(true, Ordering::Relaxed);
        assert!(pcm.by_ref().take(128).all(|sample| sample == 0.0));
        cancel_catalog();
        assert_eq!(done.get(), Some(Outcome::Cancelled));
        assert!(!catalog_playing());
        assert_eq!(pcm.next(), None);
        drop(loaded.rx);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn first_frame_starts_clock_and_fresh_skip_remains_available() {
        let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let done = Rc::new(Cell::new(None));
        fixture(done.clone());
        tick();
        PLAY.with(|p| {
            let mut p = p.borrow_mut();
            let p = p.as_mut().unwrap();
            assert!(p.shown && p.clock_started);
            assert!(p.t0.elapsed() < Duration::from_secs(1));
            p.t0 = Instant::now() - Duration::from_millis(500);
        });
        tick();
        assert_eq!(done.get(), None);
        PLAY.with(|p| p.borrow_mut().as_mut().unwrap().t0 = Instant::now() - Duration::from_millis(1100));
        tick();
        assert_eq!(done.get(), Some(Outcome::Finished));
        done.set(None);
        fixture(done.clone());
        tick();
        PLAY.with(|p| {
            let mut p = p.borrow_mut();
            let p = p.as_mut().unwrap();
            p.audio = Some(Arc::new(vec![0; 2]));
            p.shared = Some(Arc::new(AudioShared { started: AtomicBool::new(false), consumed: AtomicU64::new(0), gain: AtomicU32::new(1.0f32.to_bits()), stop: AtomicBool::new(false), finished: AtomicBool::new(false), output_busy: AtomicBool::new(false) }));
            p.t0 = Instant::now() - Duration::from_millis(500);
        });
        tick();
        PLAY.with(|p| {
            let p = p.borrow();
            let p = p.as_ref().unwrap();
            assert!(p.audio_given_up);
            assert!(p.t0.elapsed() < Duration::from_millis(100));
        });
        request_skip("test");
        tick();
        assert_eq!(done.get(), Some(Outcome::Skipped));

        // A long boot has no implicit five/ten-second limit. Advance its clock
        // without rendering or waiting; ignored keyboard noise never sets SKIP.
        done.set(None);
        fixture(done.clone());
        tick();
        PLAY.with(|p| p.borrow_mut().as_mut().unwrap().duration_us = 120_000_000);
        let mut keyboard = crate::input::AnimationKeyboard::default();
        for seconds in [6, 12, 60, 119] {
            assert!(!keyboard.event(1, 0, true, false, false));
            assert!(!keyboard.event(1, 1, true, true, false));
            PLAY.with(|p| p.borrow_mut().as_mut().unwrap().t0 = Instant::now() - Duration::from_secs(seconds));
            tick();
            assert_eq!(done.get(), None);
        }
        PLAY.with(|p| p.borrow_mut().as_mut().unwrap().t0 = Instant::now() - Duration::from_secs(121));
        tick();
        assert_eq!(done.get(), Some(Outcome::Finished));

        // A delayed loader must not hold logind past the wall budget.
        done.set(None);
        fixture(done.clone());
        PLAY.with(|p| {
            let mut p = p.borrow_mut();
            let p = p.as_mut().unwrap();
            p.rx = None;
            p.wall_deadline = Some(Instant::now() - Duration::from_millis(1));
        });
        tick();
        assert_eq!(done.get(), Some(Outcome::Trimmed));

        // Late worker failure from the previous generation cannot end the new playback.
        done.set(None);
        fixture(done.clone());
        attach(0, "old".into(), Err("late failure".into()), false);
        assert_eq!(done.get(), None);
        cancel(Outcome::Cancelled);
        assert_eq!(done.get(), Some(Outcome::Cancelled));
        done.set(None);
        fixture(done.clone());
        cancel(Outcome::Cancelled);
        assert_eq!(done.get(), Some(Outcome::Cancelled));
    }
}
