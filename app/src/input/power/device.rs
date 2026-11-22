//! Steam Deck Galileo power input.  This module never grabs the keyboard or
//! changes service ownership; its caller must complete the handoff first.
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::time::Duration;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::path::Path;

const EVENT_PATH: &str = "/dev/input/by-path/platform-i8042-serio-0-event-kbd";
const SYS_INPUT: &str = "/sys/class/input";
const DMI_PRODUCT: &str = "/sys/class/dmi/id/product_name";
// Linux EVIOCGKEY: query 256 key bits, including KEY_POWER (116).
const EVIOCGKEY_32: libc::c_ulong = 0x8020_4518;
const EVIOCSCLOCKID: libc::c_ulong = 0x4004_45a0;

#[derive(Debug, Clone, Copy)]
pub struct Event {
    pub event_type: u16,
    pub code: u16,
    pub value: i32,
    pub at: Duration,
}

pub struct Device { file: File }

impl Device {
    pub fn open() -> io::Result<Self> {
        let product = fs::read_to_string(DMI_PRODUCT)?;
        let real = fs::canonicalize(EVENT_PATH)?;
        let name = real.file_name().and_then(|s| s.to_str()).ok_or_else(|| io::Error::other("invalid input node"))?;
        if !name.starts_with("event") || name.len() == 5 || !name[5..].bytes().all(|c| c.is_ascii_digit()) {
            return Err(io::Error::other("invalid event node name"));
        }
        let sys = Path::new(SYS_INPUT).join(name);
        let phys = fs::read_to_string(sys.join("device/phys"))?;
        if !super::is_traced_power_source(product.trim(), phys.trim()) {
            return Err(io::Error::other("unverified physical power source"));
        }
        let file = OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC).open(&real)?;
        let expected = fs::read_to_string(sys.join("dev"))?;
        let meta = file.metadata()?;
        if !meta.file_type().is_char_device() { return Err(io::Error::other("input node is not a character device")); }
        let actual = format!("{}:{}", libc::major(meta.rdev()), libc::minor(meta.rdev()));
        if expected.trim() != actual { return Err(io::Error::other("input device changed during open")); }
        let clock_id: libc::c_int = libc::CLOCK_MONOTONIC;
        if unsafe { libc::ioctl(file.as_raw_fd(), EVIOCSCLOCKID, &clock_id) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { file })
    }

    /// Drop pre-handoff events, then refuse to arm while KEY_POWER is held.
    /// Only call after confirming the stock handler has stopped.
    pub fn drain_and_key_up(&mut self) -> io::Result<bool> {
        let mut buffer = [0u8; 24 * 64];
        let mut batches = 0;
        loop {
            if batches >= 128 { return Err(io::Error::other("input queue did not drain")); }
            match self.file.read(&mut buffer) {
                Ok(0) => return Err(io::Error::other("input device closed")),
                Ok(n) if n % std::mem::size_of::<libc::input_event>() != 0 => return Err(io::Error::other("partial input event")),
                Ok(_) => { batches += 1; },
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        let mut keys = [0u8; 32];
        let rc = unsafe { libc::ioctl(self.file.as_raw_fd(), EVIOCGKEY_32, keys.as_mut_ptr()) };
        if rc < 0 { return Err(io::Error::last_os_error()); }
        Ok(keys[116 / 8] & (1 << (116 % 8)) == 0)
    }

    /// Read one complete evdev event, without consuming or grabbing other clients.
    /// A timeout allows the caller to check the long-press deadline and liveness.
    pub fn next(&mut self, timeout_ms: i32) -> io::Result<Option<Event>> {
        let mut pollfd = libc::pollfd { fd: self.file.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        let n = unsafe { libc::poll(&mut pollfd, 1, timeout_ms) };
        if n < 0 {
            let e = io::Error::last_os_error();
            return if e.kind() == io::ErrorKind::Interrupted { Ok(None) } else { Err(e) };
        }
        if n == 0 { return Ok(None); }
        if pollfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            return Err(io::Error::other("power input lost"));
        }
        if pollfd.revents & libc::POLLIN == 0 { return Ok(None); }
        let mut raw = std::mem::MaybeUninit::<libc::input_event>::uninit();
        let size = std::mem::size_of::<libc::input_event>();
        let bytes = unsafe { std::slice::from_raw_parts_mut(raw.as_mut_ptr().cast::<u8>(), size) };
        match self.file.read(bytes) {
            Ok(n) if n == size => {},
            Err(e) if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::Interrupted => return Ok(None),
            Err(e) => return Err(e),
            Ok(_) => return Err(io::Error::other("incomplete power input event")),
        }
        let raw = unsafe { raw.assume_init() };
        if raw.time.tv_sec < 0 || !(0..1_000_000).contains(&raw.time.tv_usec) {
            return Err(io::Error::other("invalid power event timestamp"));
        }
        Ok(Some(Event { event_type: raw.type_, code: raw.code, value: raw.value,
            at: Duration::new(raw.time.tv_sec as u64, raw.time.tv_usec as u32 * 1000) }))
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::fd::{FromRawFd, IntoRawFd};
    use std::os::unix::net::UnixStream;

    #[test]
    fn reads_one_timestamped_event_and_reports_disconnect() {
        let (reader, mut writer) = UnixStream::pair().unwrap();
        let file = unsafe { File::from_raw_fd(reader.into_raw_fd()) };
        let mut dev = Device { file };
        assert!(dev.next(0).unwrap().is_none());
        let raw = libc::input_event {
            time: libc::timeval { tv_sec: 12, tv_usec: 345_678 },
            type_: 1, code: 116, value: 1,
        };
        let data = unsafe { std::slice::from_raw_parts((&raw as *const libc::input_event).cast::<u8>(), std::mem::size_of_val(&raw)) };
        writer.write_all(data).unwrap();
        let event = dev.next(0).unwrap().unwrap();
        assert_eq!((event.event_type, event.code, event.value), (1, 116, 1));
        assert_eq!(event.at, Duration::new(12, 345_678_000));
        drop(writer);
        assert!(dev.next(0).is_err());
    }
}
