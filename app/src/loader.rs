//! Worker pool: decodes images off the UI thread (newest request first) and posts results back.
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::sync::atomic::{AtomicU64, Ordering};
use crate::screens::media::ThumbTicket;
use std::time::Instant;

use slint::{Rgba8Pixel, SharedPixelBuffer};

use crate::data::DataSource;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Capsule,
    Hero,
    Logo,
    Header,
}

impl Kind {
    pub fn max_size(self) -> (u32, u32) {
        match self {
            Kind::Capsule => (200, 300),
            Kind::Hero => (1280, 640),
            Kind::Logo => (480, 210),
            Kind::Header => (520, 244),
        }
    }
}

pub(crate) enum Job {
    /// Native clip image only; no media decoder, network or recording operation.
    NativeThumbnail { ticket: ThumbTicket, steam_root: PathBuf },
    Still { id: u32, kind: Kind, path: PathBuf },
    Anim { id: u32, path: PathBuf },
    /// Animated hero banner or game logo: built once into an on-disk frame file (media::open_clip), played one frame at a time.
    /// `kind` is `Hero` or `Logo` and selects the size and the art generation the result belongs to.
    Clip { id: u32, kind: Kind, path: PathBuf },
}

pub type AnimFrames = Vec<(SharedPixelBuffer<Rgba8Pixel>, u32)>;

struct Queue {
    native_thumbnail_epoch: AtomicU64,
    jobs: Mutex<VecDeque<(Job, u32, Instant)>>,
    cv: Condvar,
    /// Per (appid, kind) generation, bumped by `invalidate` when the art source changed (Steam resolved a different file
    /// than the file-based guess): a job queued/decoded for an older generation is dropped instead of posted.
    epoch: Mutex<HashMap<(u32, Kind), u32>>,
}

impl Queue {
    fn epoch_of(&self, key: (u32, Kind)) -> u32 {
        self.epoch.lock().unwrap().get(&key).copied().unwrap_or(0)
    }
}

fn apply_if_current(q: &Queue, key: (u32, Kind), epoch: u32, apply: impl FnOnce()) {
    if q.epoch_of(key) == epoch { apply(); }
}

#[derive(Clone)]
pub struct Loader {
    q: Arc<Queue>,
}

fn to_buf(f: &media::Frame) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    if f.rgba.len() != (f.width * f.height * 4) as usize || f.width == 0 {
        return None;
    }
    Some(SharedPixelBuffer::clone_from_slice(&f.rgba, f.width, f.height))
}

/// Animated covers play at the SOURCE frame rate (30/50/60 fps files included). The UI clock (app.rs `anim_tick`) wakes every
/// ANIM_STEP_MS and shows the next frame once its delay has elapsed, so a frame is never on screen shorter than one step: 16 ms
/// = 62 Hz covers a 60 fps source and 30 fps lands within 2 ms (33 -> 32 ms). Frame delays are rounded to that grid with the
/// rounding error carried to the next frame (no drift: a 30 fps clip averages 33.3 ms, not 32 or 48).
pub const ANIM_STEP_MS: u32 = 16;
/// Minimum interval between displayed cover frames = 1000 / DECKSHELL_ANIM_FPS. Default 60 = no practical cap (a source above
/// that is thinned); lower it only to trade smoothness for CPU. Each shown frame costs a whole-window femtovg redraw (no partial
/// rendering), so this is the CPU knob: see docs for measurements.
pub fn anim_min_ms() -> u32 {
    static V: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        let fps: u32 = std::env::var("DECKSHELL_ANIM_FPS").ok().and_then(|v| v.parse().ok()).unwrap_or(60).clamp(1, 120);
        (1000 / fps).max(ANIM_STEP_MS)
    })
}
/// Game logos are drawn over the page and each shown frame redraws the whole window (no partial rendering): logos are
/// capped at 30 fps. Their sources are usually 15-30 fps anyway, so this only halves the cost of a 60 fps source.
pub const LOGO_MIN_MS: u32 = 33;
/// Minimum interval between frames of a clip of `kind`, given the cover interval `base`.
pub fn clip_min_ms(kind: Kind, base: u32) -> u32 {
    if kind == Kind::Logo { base.max(LOGO_MIN_MS) } else { base }
}

fn decimate(v: &[media::Frame]) -> Vec<(&media::Frame, u32)> {
    let mut out: Vec<(&media::Frame, u32)> = Vec::new();
    let (mut acc, mut carry) = (0i64, 0i64);
    let step = ANIM_STEP_MS as i64;
    let grid = |ms: i64, carry: &mut i64| -> u32 {
        let want = ms + *carry;
        let q = ((want + step / 2).div_euclid(step) * step).max(step);
        *carry = want - q;
        q as u32
    };
    for f in v {
        acc += f.delay_ms.max(10) as i64;
        if acc + 5 >= anim_min_ms() as i64 {
            out.push((f, grid(acc, &mut carry)));
            acc = 0;
        }
    }
    if acc > 0 {
        match (out.last_mut(), v.last()) {
            (Some(l), _) => l.1 += acc as u32,
            (None, Some(f)) => out.push((f, (acc as u32).max(anim_min_ms()))),
            _ => {}
        }
    }
    out
}

fn avg_color(f: &media::Frame) -> [u8; 3] {
    let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
    for px in f.rgba.chunks_exact(4).step_by(37) {
        r += px[0] as u64;
        g += px[1] as u64;
        b += px[2] as u64;
        n += 1;
    }
    if n == 0 { return [20, 40, 80]; }
    [(r / n) as u8, (g / n) as u8, (b / n) as u8]
}

impl Loader {
    #[cfg(test)]
    pub(crate) fn take_header_job(&self) -> Option<u32> {
        let mut jobs = self.q.jobs.lock().unwrap();
        let idx = jobs.iter().rposition(|(job, _, _)| matches!(job, Job::Still { kind: Kind::Header, .. }))?;
        let (Job::Still { id, .. }, _, _) = jobs.remove(idx)? else { unreachable!() };
        Some(id)
    }

    pub fn start(ds: Arc<DataSource>, threads: usize) -> Loader {
        let q = Arc::new(Queue { native_thumbnail_epoch: AtomicU64::new(0), jobs: Mutex::new(VecDeque::new()), cv: Condvar::new(), epoch: Mutex::new(HashMap::new()) });
        for n in 0..threads {
            let q = q.clone();
            let ds = ds.clone();
            std::thread::Builder::new()
                .name(format!("img-worker-{n}"))
                .spawn(move || loop {
                    let (job, ep, queued_at) = {
                        let mut g = q.jobs.lock().unwrap();
                        loop {
                            // LIFO: whatever the user is looking at now is loaded first
                            if let Some(j) = g.pop_back() {
                                break j;
                            }
                            g = q.cv.wait(g).unwrap();
                        }
                    };
                    match job {
                        Job::NativeThumbnail { ticket, steam_root } => {
                            if q.native_thumbnail_epoch.load(Ordering::Acquire)!=ticket.epoch { continue; }
                            let started=Instant::now();
                            let pixels=if let Some((full, thumb)) = &ticket.key.screenshot {
                                crate::screens::media::media_thumbs::read_screenshot(full, thumb.as_deref())
                            } else { crate::screens::media::media_thumbs::read_thumbnail(
                                &steam_root,ticket.key.account,&ticket.key.clip_id,&ticket.key.url) };
                            if q.native_thumbnail_epoch.load(Ordering::Acquire)!=ticket.epoch { continue; }
                            if crate::diag::enabled() {
                                crate::diag::log("native_thumbnail_done",serde_json::json!({"success":pixels.is_some(),"decode_ms":started.elapsed().as_millis(),"queue_ms":started.duration_since(queued_at).as_millis()}));
                            }
                            let _=slint::invoke_from_event_loop(move || { crate::with_app(|a| a.on_native_thumbnail_ready(ticket,pixels)); });
                        }
                        Job::Still { id, kind, path } if q.epoch_of((id, kind)) != ep => {
                            if crate::diag::enabled() {
                                crate::diag::log("loader_still_stale", serde_json::json!({"id": id, "kind": format!("{kind:?}"), "epoch": ep, "current_epoch": q.epoch_of((id, kind)), "path": path.display().to_string(), "stage": "before_start"}));
                            }
                            continue;
                        }
                        Job::Still { id, kind, path } => {
                            let (w, h) = kind.max_size();
                            let started_at = Instant::now();
                            if crate::diag::enabled() {
                                crate::diag::log("loader_still_start", serde_json::json!({"id": id, "kind": format!("{kind:?}"), "epoch": ep, "path": path.display().to_string(), "queue_ms": started_at.duration_since(queued_at).as_millis()}));
                            }
                            let fr = ds.thumbnail(&path, w, h);
                            let dimensions = fr.as_ref().map(|f| format!("{}x{}", f.width, f.height)).unwrap_or_else(|| "FAILED".into());
                            if std::env::var_os("DECKSHELL_ARTLOG").is_some() {
                                eprintln!("[art] {id} {kind:?} {} -> {dimensions}", path.display());
                            }
                            let avg = fr.as_ref().filter(|_| kind == Kind::Capsule).map(avg_color);
                            let buf = fr.and_then(|f| to_buf(&f));
                            if q.epoch_of((id, kind)) != ep {
                                if crate::diag::enabled() {
                                    crate::diag::log("loader_still_stale", serde_json::json!({"id": id, "kind": format!("{kind:?}"), "epoch": ep, "current_epoch": q.epoch_of((id, kind)), "path": path.display().to_string(), "stage": "after_decode", "decode_ms": started_at.elapsed().as_millis()}));
                                }
                                continue; // art source changed while decoding
                            }
                            if crate::diag::enabled() {
                                crate::diag::log("loader_still_done", serde_json::json!({"id": id, "kind": format!("{kind:?}"), "epoch": ep, "path": path.display().to_string(), "dimensions": dimensions, "success": buf.is_some(), "decode_ms": started_at.elapsed().as_millis()}));
                            }
                            let callback_q = q.clone();
                            let _ = slint::invoke_from_event_loop(move || {
                                apply_if_current(&callback_q, (id, kind), ep, || {
                                    crate::with_app(|a| a.on_still(id, kind, buf.map(|b| (slint::Image::from_rgba8(b), avg.unwrap_or([20, 40, 80])))));
                                });
                            });
                        }
                        Job::Clip { id, kind, path } => {
                            if q.epoch_of((id, kind)) != ep { continue; }
                            let (w, h) = kind.max_size();
                            let clip = media::open_clip(&path, w, h, clip_min_ms(kind, anim_min_ms())).ok().map(Arc::new);
                            if std::env::var_os("DECKSHELL_ARTLOG").is_some() {
                                eprintln!("[clip] {id} {kind:?} {} -> {}", path.display(), clip.as_ref().map(|c| format!("{} frames {}x{} {} ms", c.len(), c.width, c.height, c.total_ms())).unwrap_or_else(|| "FAILED".into()));
                            }
                            let callback_q = q.clone();
                            let _ = slint::invoke_from_event_loop(move || {
                                apply_if_current(&callback_q, (id, kind), ep, || { crate::with_app(|a| a.on_clip(id, kind, clip)); });
                            });
                        }
                        Job::Anim { id, path } => {
                            if q.epoch_of((id, Kind::Capsule)) != ep { continue; }
                            let frames: Option<AnimFrames> = ds.animation(&path, 200, 300).map(|v| {
                                decimate(&v).into_iter().filter_map(|(f, d)| to_buf(f).map(|b| (b, d))).collect()
                            });
                            let callback_q = q.clone();
                            let _ = slint::invoke_from_event_loop(move || {
                                apply_if_current(&callback_q, (id, Kind::Capsule), ep, || { crate::with_app(|a| a.on_anim(id, frames)); });
                            });
                        }
                    }
                })
                .expect("spawn worker");
        }
        Loader { q }
    }

    /// Existing worker pool, distinct bounded image jobs. Old filter/entry
    /// generations are discarded without decoding or repainting the new view.
    pub(crate) fn set_native_thumbnail_epoch(&self, epoch: u64) {
        self.q.native_thumbnail_epoch.store(epoch,Ordering::Release);
        self.q.jobs.lock().unwrap().retain(|(job,_,_)| match job {
            Job::NativeThumbnail { ticket,.. } => ticket.epoch==epoch,
            _ => true,
        });
    }
    pub(crate) fn push_native_thumbnail(&self, ticket: ThumbTicket, steam_root: PathBuf) -> bool {
        let mut jobs=self.q.jobs.lock().unwrap();
        if ticket.epoch!=self.q.native_thumbnail_epoch.load(Ordering::Acquire)
            || jobs.len()>=400
            || jobs.iter().filter(|(job,_,_)| matches!(job,Job::NativeThumbnail {..})).count()>=24 { return false; }
        jobs.push_back((Job::NativeThumbnail {ticket,steam_root},0,Instant::now()));
        self.q.cv.notify_one(); true
    }

    /// The source file of (id, kind) changed: results of earlier jobs for it are discarded.
    pub fn invalidate(&self, id: u32, kind: Kind) {
        let mut epochs = self.q.epoch.lock().unwrap();
        let epoch = epochs.entry((id, kind)).or_insert(0);
        *epoch += 1;
        let current = *epoch;
        drop(epochs);
        if crate::diag::enabled() {
            crate::diag::log("loader_invalidate", serde_json::json!({"id": id, "kind": format!("{kind:?}"), "epoch": current}));
        }
    }

    pub(crate) fn push(&self, job: Job) {
        let ep = match &job {
            Job::Still { id, kind, .. } => self.q.epoch_of((*id, *kind)),
            Job::Anim { id, .. } => self.q.epoch_of((*id, Kind::Capsule)),
            Job::Clip { id, kind, .. } => self.q.epoch_of((*id, *kind)),
            Job::NativeThumbnail {..} => 0,
        };
        let mut g = self.q.jobs.lock().unwrap();
        if g.len() > 400 {
            if let Some((job, dropped_ep, _)) = g.pop_front() {
                if let Job::NativeThumbnail {ticket,..} = job {
                    let _=slint::invoke_from_event_loop(move || { crate::with_app(|a| a.on_native_thumbnail_ready(ticket,None)); });
                } else if crate::diag::enabled() {
                    match job {
                        Job::NativeThumbnail {..} => unreachable!(),
                        Job::Still { id, kind, path } => crate::diag::log("loader_queue_overflow", serde_json::json!({"id": id, "kind": format!("{kind:?}"), "epoch": dropped_ep, "path": path.display().to_string(), "queue_len": g.len()})),
                        Job::Clip { id, kind, path } => crate::diag::log("loader_queue_overflow", serde_json::json!({"id": id, "kind": format!("{kind:?}"), "path": path.display().to_string(), "queue_len": g.len()})),
                        Job::Anim { id, path } => crate::diag::log("loader_queue_overflow", serde_json::json!({"id": id, "kind": "Anim", "path": path.display().to_string(), "queue_len": g.len()})),
                    }
                }
            }
        }
        if let Job::Still { id, kind, path } = &job {
            if crate::diag::enabled() {
                crate::diag::log("loader_still_queued", serde_json::json!({"id": id, "kind": format!("{kind:?}"), "epoch": ep, "path": path.display().to_string(), "queue_len": g.len()}));
            }
        }
        g.push_back((job, ep, Instant::now()));
        self.q.cv.notify_one();
    }
}

#[cfg(test)]
mod native_thumbnail_queue_tests {
    use super::*;
    use crate::screens::media::ThumbKey;
    fn loader() -> Loader {
        Loader { q:Arc::new(Queue {native_thumbnail_epoch:AtomicU64::new(7),jobs:Mutex::new(VecDeque::new()),
            cv:Condvar::new(),epoch:Mutex::new(HashMap::new())}) }
    }
    fn ticket(n:usize,epoch:u64) -> ThumbTicket {
        ThumbTicket {epoch,key:ThumbKey {account:42,clip_id:format!("fixture-{n}"),url:"https://steamloopback.host/synthetic".into(),screenshot:None}}
    }
    #[test]
    fn invalidation_after_decode_rejects_queued_callback() {
        let loader = loader();
        let key = (1, Kind::Header);
        let epoch = loader.q.epoch_of(key); // decoded and queued
        let mut applied = false;
        loader.invalidate(key.0, key.1);
        apply_if_current(&loader.q, key, epoch, || applied = true);
        assert!(!applied);
        apply_if_current(&loader.q, key, loader.q.epoch_of(key), || applied = true);
        assert!(applied);
    }
    #[test]
    fn logo_clips_are_capped_at_30_fps_and_hero_keeps_the_cover_rate() {
        assert_eq!(clip_min_ms(Kind::Logo, 16), LOGO_MIN_MS);
        assert_eq!(clip_min_ms(Kind::Logo, 40), 40);
        assert_eq!(clip_min_ms(Kind::Hero, 16), 16);
    }
    #[test]
    fn animated_jobs_capture_their_art_generation() {
        let loader = loader();
        loader.push(Job::Clip { id: 3, kind: Kind::Hero, path: PathBuf::from("old-hero") });
        loader.push(Job::Anim { id: 3, path: PathBuf::from("old-cover") });
        loader.invalidate(3, Kind::Hero);
        loader.invalidate(3, Kind::Capsule);
        let jobs = loader.q.jobs.lock().unwrap();
        assert_ne!(jobs[0].1, loader.q.epoch_of((3, Kind::Hero)));
        assert_ne!(jobs[1].1, loader.q.epoch_of((3, Kind::Capsule)));
        let mut applied = false;
        apply_if_current(&loader.q, (3, Kind::Hero), jobs[0].1, || applied = true);
        assert!(!applied);
    }
    #[test]
    fn native_admission_bounded_and_generation_invalidates_only_native_jobs() {
        let loader=loader();
        for n in 0..24 { assert!(loader.push_native_thumbnail(ticket(n,7),PathBuf::from("/not-opened"))); }
        assert!(!loader.push_native_thumbnail(ticket(25,7),PathBuf::from("/not-opened")));
        loader.push(Job::Still {id:1,kind:Kind::Header,path:PathBuf::from("/not-opened")});
        loader.set_native_thumbnail_epoch(8);
        assert_eq!(loader.q.jobs.lock().unwrap().len(),1); // art kept
        assert!(!loader.push_native_thumbnail(ticket(0,7),PathBuf::from("/not-opened")));
        assert!(loader.push_native_thumbnail(ticket(0,8),PathBuf::from("/not-opened")));
    }
    #[test]
    fn full_shared_pool_rejects_without_allocating_or_marking_a_request_inflight() {
        let loader=loader();
        for n in 0..400 { loader.push(Job::Still {id:n,kind:Kind::Header,path:PathBuf::from("/not-opened")}); }
        assert!(!loader.push_native_thumbnail(ticket(0,7),PathBuf::from("/not-opened")));
        assert_eq!(loader.q.jobs.lock().unwrap().len(),400);
        loader.q.jobs.lock().unwrap().pop_back();
        assert!(loader.push_native_thumbnail(ticket(0,7),PathBuf::from("/not-opened")));
    }
}

#[cfg(test)]
mod storage_spin_regression {
    use super::*;
    use slint::ComponentHandle;
    use steam_data::{Art, Game, GameKind, Library};

    #[test]
    fn storage_completions_stop_without_input() {
        let _ = slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions { renderer_name: Some("software".into()), mock_time: true, ..Default::default() },
        )));
        let ui = crate::AppWindow::new().unwrap();
        let ds = Arc::new(DataSource::new());
        let games = (0..12).map(|id| Game {
            app_id: id, kind: GameKind::Steam, name: format!("game {id}"), sort_name: format!("game {id}"),
            installed: true, install_dir: None, size_on_disk: 0, playtime_minutes: 0, last_played: None,
            update_pending: false, collections: vec![], art: Art { header: Some("fixture.png".into()), ..Default::default() },
        }).collect();
        let lib = Arc::new(Library { games, ..Default::default() });
        // Zero workers makes the actual loader queue deterministic; each iteration delivers one completion.
        let loader = Loader::start(ds.clone(), 0);
        let mut app = crate::app::App::new(ui.as_weak(), ds, lib, loader.clone());
        app.router.push(crate::router::Route::Settings("storage".into()));
        app.se.rows = (0..12).map(|id| crate::screens::settings::PRow {
            tok: format!("storage-art:{id}"), y: id as f32 * 40., h: 40., ..Default::default()
        }).collect();
        app.push_settings();
        let mut count = 0;
        loop {
            let job = loader.q.jobs.lock().unwrap().pop_front();
            let Some((Job::Still { id, kind, .. }, _, _)) = job else { break };
            count += 1;
            assert!(count <= 12, "art completions repeated without input: {count}");
            app.on_still(id, kind, Some((slint::Image::default(), [0, 0, 0])));
        }
        assert_eq!(count, 12);
    }
}
