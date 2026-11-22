//! Проверка отдельного процесса со звуком, заменённым файловым макетом pactl.
use std::{fs, os::unix::fs::PermissionsExt, path::{Path, PathBuf}, process::{Child, Command, Stdio}, time::{Duration, Instant}};

struct Probe { dir: PathBuf, parent: Child, guardian: Option<Child> }
impl Probe {
    fn start(original: bool) -> Self {
        let dir = std::env::temp_dir().join(format!("deckshell-mic-guardian-{}-{}",std::process::id(),original));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("mute"),if original {"1"} else {"0"}).unwrap();
        let mock = dir.join("pactl");
        fs::write(&mock, r#"#!/bin/sh
set -eu
case "$1" in
 get-default-source) echo test-microphone ;;
 get-source-mute) if test "$(cat "$XDG_RUNTIME_DIR/mute")" = 1; then echo 'Mute: yes'; else echo 'Mute: no'; fi ;;
 set-source-mute) printf %s "$3" > "$XDG_RUNTIME_DIR/mute" ;;
 *) exit 1 ;;
esac
"#).unwrap();
        fs::set_permissions(mock, fs::Permissions::from_mode(0o700)).unwrap();
        let parent = Command::new("sleep").arg("60").spawn().unwrap();
        let mut s = Self { dir, parent, guardian:None };
        s.request("true"); s.launch(); s
    }
    fn request(&self, value: &str) {
        let temp = self.dir.join("lease.tmp");
        fs::write(&temp, format!("{{\"mute\":{value}}}")).unwrap();
        fs::rename(temp,self.dir.join("lease.json")).unwrap();
    }
    fn launch(&mut self) {
        let stat = fs::read_to_string(format!("/proc/{}/stat",self.parent.id())).unwrap();
        let identity = stat.rsplit_once(") ").unwrap().1.split_whitespace().nth(19).unwrap();
        self.guardian = Some(Command::new(env!("CARGO_BIN_EXE_deckshell"))
            .args(["--recording-mic-guardian",&self.parent.id().to_string(),identity])
            .arg(self.dir.join("lease.json")).env("XDG_RUNTIME_DIR",&self.dir)
            .env("PATH",format!("{}:/usr/bin:/bin",self.dir.display()))
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
    }
    fn expect(&self, muted: bool) {
        let end = Instant::now()+Duration::from_secs(5);
        loop {
            if fs::read_to_string(self.dir.join("mute")).unwrap_or_default() == if muted {"1"} else {"0"} { return; }
            assert!(Instant::now()<end,"Не подтверждён mute={muted}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    fn journal(&self) -> PathBuf { self.dir.join("deckshell-recording-mic/restore.json") }
}
impl Drop for Probe {
    fn drop(&mut self) {
        let _ = self.parent.kill(); let _ = self.parent.wait();
        if let Some(mut g) = self.guardian.take() { let _ = g.kill(); let _ = g.wait(); }
        let _ = fs::remove_dir_all(&self.dir);
    }
}
fn wait_file(path: &Path, exists: bool) {
    let end = Instant::now()+Duration::from_secs(5);
    while path.exists()!=exists { assert!(Instant::now()<end,"Не изменился журнал восстановления"); std::thread::sleep(Duration::from_millis(20)); }
}
#[test]
fn independent_guardian_closes_stale_lease_and_restores_after_crashes() {
    for original in [false,true] {
        let mut p = Probe::start(original);
        p.expect(true); wait_file(&p.journal(),true);
        p.request("false"); p.expect(false);
        // Живой UI без heartbeat не оставляет микрофон открытым.
        std::thread::sleep(Duration::from_millis(2200)); p.expect(true);
        // Явное освобождение восстанавливает и исходный true, и исходный false.
        p.request("null"); p.expect(original); wait_file(&p.journal(),false);
        p.request("true"); p.expect(true); wait_file(&p.journal(),true);
        // Авария сторожа сохраняет исходное состояние в журнале.
        let mut g = p.guardian.take().unwrap(); g.kill().unwrap(); g.wait().unwrap();
        assert!(p.journal().exists()); p.launch();
        p.request("false"); p.expect(false);
        // Смерть UI проверяется PID+starttime; отдельный сторож продолжает работу.
        p.parent.kill().unwrap(); p.parent.wait().unwrap();
        p.expect(original); wait_file(&p.journal(),false);
        let end = Instant::now()+Duration::from_secs(5);
        while p.guardian.as_mut().unwrap().try_wait().unwrap().is_none() {
            assert!(Instant::now()<end,"Сторож не завершился после восстановления");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
