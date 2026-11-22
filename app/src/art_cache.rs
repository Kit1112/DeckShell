//! Least-recently-used bookkeeping for decoded artwork. Pure, so the eviction rules and the storage
//! feedback loop can be tested without a Slint window (`App` needs one).

/// Records `key` as the most recently used entry and, if its class holds more than `cap` entries,
/// removes and returns the least recently used entry of that class that is not protected. The key
/// that was just touched is never the victim.
pub fn touch<K: Copy + PartialEq>(
    lru: &mut Vec<K>,
    key: K,
    same_class: impl Fn(K) -> bool,
    cap: usize,
    protected: impl Fn(K) -> bool,
) -> Option<K> {
    if let Some(p) = lru.iter().position(|k| *k == key) {
        lru.remove(p);
    }
    lru.push(key);
    if lru.iter().filter(|&&k| same_class(k)).count() <= cap {
        return None;
    }
    let p = lru.iter().position(|&k| same_class(k) && !protected(k) && k != key)?;
    Some(lru.remove(p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    type Key = (u32, u8);

    /// What the app does on a request: a resident key is touched, an absent key is queued once.
    /// Evictions forget the key, so it can be requested again.
    struct Model {
        lru: Vec<Key>,
        resident: HashSet<Key>,
        requested: HashSet<Key>,
        queue: Vec<Key>,
        loads: usize,
        cap: usize,
    }

    impl Model {
        fn new(cap: usize) -> Self {
            Model { lru: vec![], resident: HashSet::new(), requested: HashSet::new(), queue: vec![], loads: 0, cap }
        }

        fn request(&mut self, key: Key) {
            if self.resident.contains(&key) {
                self.evict_from(key);
            } else if self.requested.insert(key) {
                self.queue.push(key);
                self.loads += 1;
            }
        }

        fn evict_from(&mut self, key: Key) {
            let cap = self.cap;
            if let Some(old) = touch(&mut self.lru, key, |k| k.1 == 1, cap, |_| false) {
                self.resident.remove(&old);
                self.requested.remove(&old);
            }
        }

        /// A finished load: the image becomes resident and is touched.
        fn complete(&mut self, key: Key) {
            self.queue.retain(|k| *k != key);
            self.resident.insert(key);
            self.evict_from(key);
        }

        /// One storage pass: request every row of the working set, in row order.
        fn pass(&mut self, working_set: &[Key]) {
            for &k in working_set {
                self.request(k);
            }
        }
    }

    /// Storage shows about 12 Header art keys (visible rows plus Home rows) while the Header cache holds 10.
    fn working_set() -> Vec<Key> {
        (0..12).map(|i| (1000 + i, 1)).collect()
    }

    /// Runs completions until nothing is queued, with a budget. `refill_on_complete` models a completion
    /// that starts another storage pass (the shipped behaviour is `false`: only input starts a pass).
    fn drain(refill_on_complete: bool, budget: usize) -> Model {
        let ws = working_set();
        let mut m = Model::new(10);
        m.pass(&ws);
        let mut events = 0;
        while let Some(&key) = m.queue.first() {
            m.complete(key);
            if refill_on_complete {
                m.pass(&ws);
            }
            events += 1;
            if events >= budget { break; }
        }
        m
    }

    #[test]
    fn evicts_least_recent_of_the_same_class_only() {
        let mut lru = vec![(1, 0), (2, 1), (3, 1)];
        let victim = touch(&mut lru, (4, 1), |k| k.1 == 1, 2, |_| false);
        assert_eq!(victim, Some((2, 1)));
        assert_eq!(lru, vec![(1, 0), (3, 1), (4, 1)]);
    }

    #[test]
    fn protected_key_is_not_evicted() {
        let mut lru = vec![(1, 1), (2, 1)];
        let victim = touch(&mut lru, (3, 1), |k| k.1 == 1, 2, |k| k.0 == 1);
        assert_eq!(victim, Some((2, 1)));
    }

    #[test]
    fn no_eviction_within_cap() {
        let mut lru = vec![(1, 1)];
        assert_eq!(touch(&mut lru, (2, 1), |k| k.1 == 1, 2, |_| false), None);
    }

    /// Regression: completing a load must not start another storage pass. Without that, each completion
    /// evicts a working-set member that the next pass re-requests, so loads never settle while the owner
    /// does nothing. The shipped policy stays within one load per working-set key.
    #[test]
    fn completions_without_input_do_not_reload_the_storage_working_set() {
        let ws = working_set();
        let m = drain(false, 1000);
        assert!(m.queue.is_empty());
        assert!(m.loads <= ws.len(), "loads={} for {} keys", m.loads, ws.len());
    }

    /// The model reproduces the spin: with a pass after every completion, the queue never drains within the budget.
    #[test]
    fn refilling_on_every_completion_spins() {
        let m = drain(true, 1000);
        assert!(!m.queue.is_empty(), "expected the thrash to continue, loads={}", m.loads);
        assert!(m.loads > 1000 / 2);
    }
}
