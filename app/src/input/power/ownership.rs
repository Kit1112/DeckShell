//! Ordered handoff contract. This does not create the independent systemd guard;
//! no caller may use it until the guard and its recovery action are verified.
use std::io;

pub trait Handoff {
    /// Listener, UI delivery and independent recovery guard must already be ready.
    fn ready(&mut self) -> io::Result<()>;
    fn stop_stock(&mut self) -> io::Result<()>;
    fn stock_stopped(&mut self) -> io::Result<bool>;
    fn drain_and_key_up(&mut self) -> io::Result<bool>;
    fn arm(&mut self) -> io::Result<()>;
    fn disarm(&mut self);
    fn start_stock(&mut self) -> io::Result<()>;
}

/// On any uncertain stop/arm failure, disarm before attempting stock recovery.
/// The independent guard remains responsible for recovery on process death.
pub fn acquire(h: &mut impl Handoff) -> io::Result<()> {
    h.ready()?;
    let result = (|| {
        h.stop_stock()?;
        if !h.stock_stopped()? { return Err(io::Error::other("stock handler did not stop")); }
        if !h.drain_and_key_up()? { return Err(io::Error::other("power key held at handoff")); }
        h.arm()
    })();
    if let Err(e) = result {
        h.disarm();
        if let Err(recovery) = h.start_stock() {
            return Err(io::Error::other(format!("handoff failed: {e}; stock recovery failed: {recovery}")));
        }
        return Err(e);
    }
    Ok(())
}

pub fn release(h: &mut impl Handoff) -> io::Result<()> {
    h.disarm();
    h.start_stock()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Fake { calls: Vec<&'static str>, fail: Option<&'static str>, recovery_fails: bool, stopped: bool, up: bool }
    impl Fake {
        fn call(&mut self, name: &'static str) -> io::Result<()> {
            self.calls.push(name);
            if self.fail == Some(name) { Err(io::Error::other(name)) } else { Ok(()) }
        }
    }
    impl Handoff for Fake {
        fn ready(&mut self) -> io::Result<()> { self.call("ready") }
        fn stop_stock(&mut self) -> io::Result<()> { self.call("stop") }
        fn stock_stopped(&mut self) -> io::Result<bool> { self.call("stopped")?; Ok(self.stopped) }
        fn drain_and_key_up(&mut self) -> io::Result<bool> { self.call("drain")?; Ok(self.up) }
        fn arm(&mut self) -> io::Result<()> { self.call("arm") }
        fn disarm(&mut self) { self.calls.push("disarm"); }
        fn start_stock(&mut self) -> io::Result<()> {
            self.call("start")?;
            if self.recovery_fails { Err(io::Error::other("recovery")) } else { Ok(()) }
        }
    }
    fn ok() -> Fake { Fake { stopped: true, up: true, ..Default::default() } }

    #[test]
    fn success_arms_only_after_stop_and_release_restores_stock() {
        let mut f = ok(); acquire(&mut f).unwrap(); release(&mut f).unwrap();
        assert_eq!(f.calls, ["ready", "stop", "stopped", "drain", "arm", "disarm", "start"]);
    }
    #[test]
    fn readiness_failure_does_not_stop_stock() {
        let mut f = ok(); f.fail = Some("ready"); assert!(acquire(&mut f).is_err());
        assert_eq!(f.calls, ["ready"]);
    }
    #[test]
    fn recovery_failure_is_visible_to_caller() {
        let mut f = ok(); f.fail = Some("arm"); f.recovery_fails = true;
        let err = acquire(&mut f).unwrap_err().to_string();
        assert!(err.contains("handoff failed"));
        assert!(err.contains("stock recovery failed"));
        assert_eq!(&f.calls[f.calls.len()-2..], ["disarm", "start"]);
    }
    #[test]
    fn failed_or_uncertain_handoff_disarms_and_recovers() {
        for stage in ["stop", "stopped", "drain", "arm"] {
            let mut f = ok(); f.fail = Some(stage); assert!(acquire(&mut f).is_err());
            assert_eq!(&f.calls[f.calls.len()-2..], ["disarm", "start"]);
            if stage != "arm" { assert!(!f.calls.contains(&"arm")); }
        }
        for (stopped, up) in [(false, true), (true, false)] {
            let mut f = ok(); f.stopped = stopped; f.up = up; assert!(acquire(&mut f).is_err());
            assert!(!f.calls.contains(&"arm"));
            assert_eq!(&f.calls[f.calls.len()-2..], ["disarm", "start"]);
        }
    }
}
