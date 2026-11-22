//! Playback of one native clip in the Media viewer. One ffmpeg process per start decodes the clip's recordings in real time:
//! RGBA frames (640x400, 30 fps) go to its stdout, a reader thread keeps only the newest frame, and the audio goes to
//! PulseAudio/PipeWire from the same process, so both outputs share one clock. Pause stops the process and remembers the
//! position; resume and seek start it again at that position (-ss). Steam's own files are only read.
//! Nothing here touches the Steam recording or its chord: Media is the only caller, and the process ends with the viewer.
use steam_assets::i18n;
use std::cell::RefCell;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Rgba8Pixel, SharedPixelBuffer, TimerMode};

use super::media_clips::{self, PlayPlan};
use super::{Item, ST};
use crate::app::App;
use crate::MediaState;

/// Decoded frame size and rate. 640x400 keeps one ffmpeg process near 43% of one Deck core (measured, see handoff).
pub(crate) const FRAME_W: u32 = 640;
pub(crate) const FRAME_H: u32 = 400;
pub(crate) const FPS: u32 = 30;
/// Left/Right in the viewer.
const SEEK_MS: i64 = 10_000;
/// UI refresh while playing: one frame per tick at most.
const TICK: Duration = Duration::from_millis(33);

#[derive(Default)]
struct Slot {
    seq: u64,
    frame: Option<SharedPixelBuffer<Rgba8Pixel>>,
    eof: bool,
}

/// One running ffmpeg process and the position it started from.
pub(crate) struct Player {
    child: Child,
    slot: Arc<Mutex<Slot>>,
    seen: u64,
    started: Instant,
    base_ms: u64,
    duration_ms: u64,
    first_frame: Option<Duration>,
}

impl Player {
    pub(crate) fn start(program: &Path, args: &[String], base_ms: u64, duration_ms: u64, w: u32, h: u32) -> std::io::Result<Player> {
        let mut cmd = Command::new(program);
        cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
        // If DeckShell exits without stopping the viewer, the decoder and its audio stream must not outlive it.
        // SAFETY: runs in the child between fork and exec and only calls prctl.
        unsafe {
            use std::os::unix::process::CommandExt;
            cmd.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                Ok(())
            });
        }
        let mut child = cmd.spawn()?;
        let mut out = child.stdout.take().ok_or_else(|| std::io::Error::other("ffmpeg has no stdout"))?;
        let slot = Arc::new(Mutex::new(Slot::default()));
        let writer = slot.clone();
        let len = w as usize * h as usize * 4;
        std::thread::Builder::new().name("clip-frames".into()).spawn(move || {
            let mut buf = vec![0u8; len];
            while out.read_exact(&mut buf).is_ok() {
                let px = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&buf, w, h);
                let mut g = writer.lock().unwrap_or_else(|e| e.into_inner());
                g.seq += 1;
                g.frame = Some(px);
            }
            writer.lock().unwrap_or_else(|e| e.into_inner()).eof = true;
        })?;
        Ok(Player { child, slot, seen: 0, started: Instant::now(), base_ms, duration_ms, first_frame: None })
    }

    /// Media time on screen: the start position plus the wall time since start, never past the media end.
    pub(crate) fn position_ms(&self) -> u64 {
        let p = self.base_ms + self.started.elapsed().as_millis() as u64;
        if self.duration_ms > 0 { p.min(self.duration_ms) } else { p }
    }

    /// The newest frame, once per decoded frame.
    pub(crate) fn take_frame(&mut self) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        let g = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        if g.seq == self.seen { return None; }
        self.seen = g.seq;
        if self.first_frame.is_none() { self.first_frame = Some(self.started.elapsed()); }
        g.frame.clone()
    }

    pub(crate) fn finished(&self) -> bool {
        self.slot.lock().unwrap_or_else(|e| e.into_inner()).eof
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The clip being viewed: where it is, whether it runs, and the timer that refreshes the viewer.
pub(crate) struct PlayState {
    pub clip_id: String,
    duration_ms: u64,
    /// Position while no process runs (paused, not started, or ended).
    pos_ms: u64,
    player: Option<Player>,
    ended: bool,
    timer: Option<slint::Timer>,
    shown_sec: Option<u64>,
    logged_first: bool,
}

thread_local! { static PLAY: RefCell<Option<PlayState>> = const { RefCell::new(None) }; }

/// Cache root, as the media crate resolves it (DECKSHELL_CACHE, else XDG_CACHE_HOME/deckshell, else ~/.cache/deckshell).
fn cache_root() -> PathBuf {
    if let Some(d) = std::env::var_os("DECKSHELL_CACHE") { return PathBuf::from(d); }
    let base = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("deckshell")
}

pub(crate) fn mmss(ms: u64) -> String {
    let s = ms / 1000;
    format!("{:02}:{:02}", s / 60, s % 60)
}

/// The clip id of a native clip entry in the viewer (None for screenshots).
pub(crate) fn native_clip_id(it: &Item) -> Option<&str> {
    it.native.as_ref().map(|c| c.clip_id.as_str())
}

/// True while this clip's process runs (the viewer must then keep the frame that is on screen).
pub(crate) fn is_playing(clip_id: &str) -> bool {
    PLAY.with(|p| p.borrow().as_ref().is_some_and(|s| s.clip_id == clip_id && s.player.is_some()))
}

/// Label and time line for the viewer, when this clip has a playback state.
pub(crate) fn status_for(clip_id: &str) -> Option<(bool, bool, u64, u64)> {
    PLAY.with(|p| p.borrow().as_ref().filter(|s| s.clip_id == clip_id).map(|s| {
        let pos = s.player.as_ref().map(|pl| pl.position_ms()).unwrap_or(s.pos_ms);
        (s.player.is_some(), s.ended, pos, s.duration_ms)
    }))
}

impl App {
    fn clip_dir_of(&self, clip_id: &str) -> Option<PathBuf> {
        let acct = self.base.active_user?;
        Some(self.base.steam_root.join("userdata").join(acct.to_string()).join("gamerecordings").join("clips").join(clip_id))
    }

    /// Whether a native clip has recordings that can be played (reads the folder only).
    pub(crate) fn clip_playable(&self, clip_id: &str) -> bool {
        self.clip_dir_of(clip_id).and_then(|d| media_clips::play_plan(&d)).is_some()
    }

    /// Left / Right seek while a playable native clip is running. Otherwise they move between the viewer's buttons,
    /// so Delete stays reachable for a clip that is playable but not playing.
    pub(crate) fn media_clip_key(&mut self, a: crate::input::Action) -> bool {
        let Some(it) = self.media_current_for_player() else { return false };
        let Some(cid) = native_clip_id(&it).map(str::to_owned) else { return false };
        if !is_playing(&cid) || !self.clip_playable(&cid) { return false; }
        match a {
            crate::input::Action::Left => self.media_clip_seek(-SEEK_MS),
            crate::input::Action::Right => self.media_clip_seek(SEEK_MS),
            _ => return false,
        }
        true
    }

    /// A on a playable clip: start, or pause while it runs.
    pub(crate) fn media_clip_toggle(&mut self) {
        let Some(it) = self.media_current_for_player() else { return };
        let Some(cid) = native_clip_id(&it).map(str::to_owned) else { return };
        if is_playing(&cid) {
            self.media_clip_pause();
        } else {
            self.media_clip_start(&cid, None);
        }
    }

    fn media_clip_start(&mut self, clip_id: &str, at: Option<u64>) {
        let Some(dir) = self.clip_dir_of(clip_id) else { return };
        let Some(plan) = media_clips::play_plan(&dir) else { return self.media_clip_note(i18n::tr_ref("media_player.record_files_are_not_found_playback_is")) };
        if crate::actions::dryrun() {
            return self.media_clip_note(i18n::tr_ref("media_player.dryrun_the_playback_is_not_running"));
        }
        let start = at.unwrap_or_else(|| PLAY.with(|p| p.borrow().as_ref().filter(|s| s.clip_id == clip_id && !s.ended).map(|s| s.pos_ms).unwrap_or(0)));
        let start = start.min(plan.duration_ms);
        match self.spawn_clip(clip_id, &plan, start) {
            Ok(player) => {
                let timer = slint::Timer::default();
                timer.start(TimerMode::Repeated, TICK, || crate::screens::defer(|a| a.media_clip_tick()));
                crate::diag::log("clip_play.start", serde_json::json!({"start_ms": start, "duration_ms": plan.duration_ms, "manifests": plan.manifests.len()}));
                PLAY.with(|p| *p.borrow_mut() = Some(PlayState {
                    clip_id: clip_id.to_owned(), duration_ms: plan.duration_ms, pos_ms: start, player: Some(player), ended: false,
                    timer: Some(timer), shown_sec: None, logged_first: false,
                }));
                self.push_media();
            }
            Err(e) => {
                crate::diag::log("clip_play.error", serde_json::json!({"stage": "spawn", "kind": format!("{:?}", e.kind())}));
                self.media_clip_note(i18n::tr_ref("media_player.it_was_not_possible_to_run_ffmpeg"));
            }
        }
    }

    fn spawn_clip(&self, clip_id: &str, plan: &PlayPlan, start: u64) -> std::io::Result<Player> {
        let input = media_clips::materialize(plan, clip_id, &cache_root())?;
        let program = media::find_ffmpeg().unwrap_or_else(|| PathBuf::from("ffmpeg"));
        let args = media_clips::ffmpeg_args(&input, start, FRAME_W, FRAME_H, FPS, true);
        Player::start(&program, &args, start, plan.duration_ms, FRAME_W, FRAME_H)
    }

    fn media_clip_pause(&mut self) {
        let paused = PLAY.with(|p| {
            let mut p = p.borrow_mut();
            let Some(s) = p.as_mut() else { return None };
            let Some(player) = s.player.take() else { return None };
            s.pos_ms = player.position_ms();
            s.timer = None;
            Some(s.pos_ms)
        });
        if let Some(pos) = paused {
            crate::diag::log("clip_play.pause", serde_json::json!({"pos_ms": pos}));
            self.push_media();
        }
    }

    fn media_clip_seek(&mut self, delta: i64) {
        let Some(it) = self.media_current_for_player() else { return };
        let Some(cid) = native_clip_id(&it).map(str::to_owned) else { return };
        let (pos, duration, running) = PLAY.with(|p| match p.borrow().as_ref().filter(|s| s.clip_id == cid) {
            Some(s) => (s.player.as_ref().map(|pl| pl.position_ms()).unwrap_or(s.pos_ms), s.duration_ms, s.player.is_some()),
            None => (0, 0, false),
        });
        let duration = if duration > 0 { duration } else {
            self.clip_dir_of(&cid).and_then(|d| media_clips::play_plan(&d)).map(|p| p.duration_ms).unwrap_or(0)
        };
        let target = (pos as i64 + delta).clamp(0, duration as i64) as u64;
        crate::diag::log("clip_play.seek", serde_json::json!({"to_ms": target, "running": running}));
        if running {
            self.media_clip_start(&cid, Some(target));
        } else {
            PLAY.with(|p| {
                let mut p = p.borrow_mut();
                match p.as_mut() {
                    Some(s) if s.clip_id == cid => { s.pos_ms = target; s.ended = false; }
                    _ => *p = Some(PlayState { clip_id: cid.clone(), duration_ms: duration, pos_ms: target, player: None, ended: false, timer: None, shown_sec: None, logged_first: false }),
                }
            });
            self.push_media();
        }
    }

    /// Timer: shows the newest frame, the time line, and the end of the clip; stops when the viewer no longer shows the clip.
    fn media_clip_tick(&mut self) {
        let Some(it) = self.media_current_for_player() else { return self.media_play_stop() };
        let Some(cid) = native_clip_id(&it).map(str::to_owned) else { return self.media_play_stop() };
        let on_media = self.router.current() == &crate::router::Route::Media && self.media_viewer_open();
        let state_is_this = PLAY.with(|p| p.borrow().as_ref().is_some_and(|s| s.clip_id == cid));
        if !on_media || !state_is_this { return self.media_play_stop(); }

        let (frame, finished, first) = PLAY.with(|p| {
            let mut p = p.borrow_mut();
            let Some(s) = p.as_mut() else { return (None, false, None) };
            let Some(player) = s.player.as_mut() else { return (None, false, None) };
            let frame = player.take_frame();
            let first = if !s.logged_first { player.first_frame.map(|d| d.as_millis() as u64) } else { None };
            if first.is_some() { s.logged_first = true; }
            (frame, player.finished(), first)
        });
        if let Some(ms) = first { crate::diag::log("clip_play.first_frame", serde_json::json!({"ms": ms})); }
        if let Some(px) = frame {
            let ui = self.ui();
            let st = ui.global::<MediaState>();
            st.set_viewer_img(slint::Image::from_rgba8(px));
            st.set_viewer_has(true);
        }
        if finished {
            PLAY.with(|p| if let Some(s) = p.borrow_mut().as_mut() {
                s.pos_ms = s.duration_ms;
                s.player = None;
                s.timer = None;
                s.ended = true;
            });
            crate::diag::log("clip_play.end", serde_json::json!({}));
            self.push_media();
            return;
        }
        // The time line changes once a second, not per frame.
        let sec = PLAY.with(|p| {
            let mut p = p.borrow_mut();
            let s = p.as_mut()?;
            let pos = s.player.as_ref().map(|pl| pl.position_ms()).unwrap_or(s.pos_ms);
            let sec = pos / 1000;
            if s.shown_sec == Some(sec) { return None; }
            s.shown_sec = Some(sec);
            Some((pos, s.duration_ms))
        });
        if let Some((pos, duration)) = sec {
            let sub = self.media_clip_sub(&it, pos, duration);
            self.ui().global::<MediaState>().set_viewer_sub(sub.as_str().into());
        }
    }

    /// Stops the process and forgets the clip (viewer closed, another item shown, or the route left).
    pub(crate) fn media_play_stop(&mut self) {
        let had = PLAY.with(|p| p.borrow_mut().take());
        if let Some(s) = had {
            let pos = s.player.as_ref().map(|pl| pl.position_ms()).unwrap_or(s.pos_ms);
            crate::diag::log("clip_play.stop", serde_json::json!({"pos_ms": pos}));
        }
    }

    /// The viewer's subtitle for a clip: kind, age, and while a state exists the time line.
    pub(crate) fn media_clip_sub(&self, it: &Item, pos: u64, duration: u64) -> String {
        format!("{}  -  {}  -  {} / {}", super::kind_label(it.kind), super::ago(it.mtime), mmss(pos), mmss(duration))
    }

    fn media_clip_note(&mut self, text: &str) {
        ST.with(|s| s.borrow_mut().note = text.to_owned());
        self.push_media();
    }

    /// The item the viewer shows, without the grid's focus rules.
    pub(crate) fn media_current_for_player(&self) -> Option<Item> {
        ST.with(|s| {
            let s = s.borrow();
            let v = s.viewer?;
            s.view.get(v).and_then(|&i| s.items.get(i)).cloned()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real ffmpeg on a synthetic 1 s file: frames arrive at the requested size, the reader sees the end, and the
    /// process is gone after the player is dropped. Skipped where ffmpeg is not installed.
    #[test]
    fn player_decodes_real_frames_and_reports_the_end() {
        let Some(ffmpeg) = media::find_ffmpeg() else { eprintln!("ffmpeg not installed: skipped"); return };
        let dir = std::env::temp_dir().join(format!("deckshell-player-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("synthetic.mp4");
        let made = Command::new(&ffmpeg)
            .args(["-y", "-loglevel", "error", "-f", "lavfi", "-i", "testsrc=size=1280x800:rate=60", "-t", "1", "-c:v", "mpeg4"])
            .arg(&file)
            .status();
        if !made.map(|s| s.success()).unwrap_or(false) { eprintln!("ffmpeg cannot write the fixture: skipped"); return; }

        let input = media_clips::Input { path: file, concat: false };
        let args = media_clips::ffmpeg_args(&input, 0, 640, 400, 30, false);
        let mut player = Player::start(&ffmpeg, &args, 0, 1000, 640, 400).expect("ffmpeg starts");
        let t0 = Instant::now();
        let mut frames = 0;
        while t0.elapsed() < Duration::from_secs(8) && !player.finished() {
            if let Some(f) = player.take_frame() {
                assert_eq!((f.width(), f.height()), (640, 400));
                frames += 1;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(frames >= 5, "frames seen: {frames}");
        assert!(player.first_frame.is_some());
        assert!(player.finished(), "the end of a 1 s file is reported");
        assert!(player.position_ms() <= 1000);
        drop(player);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
