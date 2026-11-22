//! A native void reply proves dispatch only. Completion comes from Steam's overview;
//! missing data and timeouts stay uncertain. Never retry a destructive request.
use steam_assets::i18n::{tr_args, tr_ref};

pub(crate) trait Backend {
    fn dispatch(&mut self, id: u32) -> Result<bool, String>;
    fn installed(&mut self, id: u32) -> Result<Option<bool>, String>;
    fn wait(&mut self);
}
impl Backend for steam_bridge::Bridge {
    fn dispatch(&mut self, id: u32) -> Result<bool, String> {
        let accepted = self.uninstall(id).map_err(|e| tr_args("uninstall.steam_error", &[("e", &e.to_string())]))?;
        if accepted {
            crate::screens::defer(|a| a.toast(tr_ref("uninstall.waiting_for_update")));
        }
        Ok(accepted)
    }
    fn installed(&mut self, id: u32) -> Result<Option<bool>, String> {
        self.uninstall_installed(id).map_err(|e| e.to_string())
    }
    fn wait(&mut self) { std::thread::sleep(std::time::Duration::from_millis(500)); }
}

pub(crate) fn observe(b: &mut impl Backend, id: u32) -> Result<(), String> {
    if !b.dispatch(id)? { return Err(tr_ref("uninstall.request_unconfirmed").into()); }
    for _ in 0..60 {
        b.wait();
        match b.installed(id) {
            Ok(Some(false)) => return Ok(()),
            Ok(_) => {},
            Err(e) => return Err(tr_args("uninstall.steam_disconnected", &[("e", &e)])),
        }
    }
    Err(tr_ref("uninstall.completion_unconfirmed").into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    struct Fake { sent: Vec<u32>, reply: Result<bool, String>, states: VecDeque<Result<Option<bool>, String>>, polls: usize }
    impl Backend for Fake {
        fn dispatch(&mut self, id: u32) -> Result<bool, String> { self.sent.push(id); self.reply.clone() }
        fn installed(&mut self, _: u32) -> Result<Option<bool>, String> { self.polls += 1; self.states.pop_front().unwrap_or(Ok(None)) }
        fn wait(&mut self) {}
    }
    fn fake(reply: Result<bool, String>, states: Vec<Result<Option<bool>, String>>) -> Fake {
        Fake { sent: vec![], reply, states: states.into(), polls: 0 }
    }
    #[test]
    fn completion_requires_observed_not_installed() {
        let mut b = fake(Ok(true), vec![Ok(Some(true)), Ok(None), Ok(Some(false))]);
        assert_eq!(observe(&mut b, 42), Ok(()));
        assert_eq!(b.sent, vec![42]); assert_eq!(b.polls, 3);
    }
    #[test]
    fn false_or_exception_never_polls_or_retries() {
        for reply in [Ok(false), Err("native rejection / lost response".into())] {
            let mut b = fake(reply, vec![Ok(Some(false))]);
            assert!(observe(&mut b, 42).is_err());
            assert_eq!(b.sent, vec![42]); assert_eq!(b.polls, 0);
        }
    }
    #[test]
    fn unknown_or_unchanged_state_is_not_completion() {
        for states in [vec![], vec![Ok(Some(true)); 60]] {
            let mut b = fake(Ok(true), states);
            assert!(observe(&mut b, 42).unwrap_err().contains(crate::screens::expected_text("неизвестен", "unknown")));
            assert_eq!(b.sent, vec![42]); assert_eq!(b.polls, 60);
        }
    }
    #[test]
    fn disconnect_after_dispatch_is_uncertain_without_retry() {
        let mut b = fake(Ok(true), vec![Err("offline".into())]);
        assert!(observe(&mut b, 42).unwrap_err().contains(crate::screens::expected_text("неизвестен", "unknown")));
        assert_eq!(b.sent, vec![42]);
    }
}
