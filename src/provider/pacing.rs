//! Spacing a provider's requests out over time, so a run of many requests
//! (e.g. a multi-seed `refs`) stays under the provider's rate limit instead
//! of only reacting to its 429s.
//!
//! [`Pacer`] is pure bookkeeping: it's handed the current time rather than
//! reading a clock, so it's tested with made-up instants. [`Paced`] puts one
//! behind a lock and does the waiting.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// When the next request may be sent.
#[derive(Debug, Default)]
pub(crate) struct Pacer {
    /// The earliest the next request may go out; `None` before the first.
    next: Option<Instant>,
}

impl Pacer {
    pub(crate) const fn new() -> Self {
        Pacer { next: None }
    }

    /// Books a slot for one request at or after `now`, keeping `interval`
    /// after it free, and returns how long to wait for that slot.
    pub(crate) fn reserve(&mut self, now: Instant, interval: Duration) -> Duration {
        let slot = self.next.map_or(now, |next| next.max(now));
        self.next = Some(slot + interval);
        slot - now
    }

    /// Holds every request back until at least `now + wait`, e.g. the
    /// provider's `Retry-After`. A later slot already booked stands.
    pub(crate) fn hold_off(&mut self, now: Instant, wait: Duration) {
        let until = now + wait;
        self.next = Some(self.next.map_or(until, |next| next.max(until)));
    }
}

/// A [`Pacer`] shared by every request to one provider, whichever client
/// value sends it.
pub(crate) struct Paced(Mutex<Pacer>);

impl Paced {
    pub(crate) const fn new() -> Self {
        Paced(Mutex::new(Pacer::new()))
    }

    /// Waits for this request's slot, `interval` after the previous one's.
    pub(crate) async fn wait(&self, interval: Duration) {
        let wait = self.lock().reserve(Instant::now(), interval);
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }

    /// Holds every later request back `wait` from now.
    pub(crate) fn hold_off(&self, wait: Duration) {
        self.lock().hold_off(Instant::now(), wait);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Pacer> {
        // The pacer is only ever left consistent, so a panic elsewhere while
        // it was held doesn't make it unusable.
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: Duration = Duration::from_secs(1);

    #[test]
    fn the_first_request_goes_out_at_once() {
        let mut pacer = Pacer::new();
        assert_eq!(pacer.reserve(Instant::now(), SECOND), Duration::ZERO);
    }

    #[test]
    fn back_to_back_requests_are_spaced_by_the_interval() {
        let mut pacer = Pacer::new();
        let start = Instant::now();
        let waits: Vec<Duration> = (0..4).map(|_| pacer.reserve(start, SECOND)).collect();
        assert_eq!(waits, [0, 1, 2, 3].map(Duration::from_secs));
    }

    #[test]
    fn a_request_after_a_quiet_spell_does_not_wait() {
        let mut pacer = Pacer::new();
        let start = Instant::now();
        pacer.reserve(start, SECOND);
        assert_eq!(
            pacer.reserve(start + Duration::from_secs(5), SECOND),
            Duration::ZERO
        );
    }

    #[test]
    fn a_request_part_way_through_the_interval_waits_the_rest() {
        let mut pacer = Pacer::new();
        let start = Instant::now();
        pacer.reserve(start, SECOND);
        let wait = pacer.reserve(start + Duration::from_millis(300), SECOND);
        assert_eq!(wait, Duration::from_millis(700));
    }

    #[test]
    fn hold_off_delays_the_next_request_past_the_interval() {
        let mut pacer = Pacer::new();
        let start = Instant::now();
        pacer.reserve(start, SECOND);
        pacer.hold_off(start, Duration::from_secs(5));
        assert_eq!(pacer.reserve(start, SECOND), Duration::from_secs(5));
        // And the request after it is spaced from there.
        assert_eq!(pacer.reserve(start, SECOND), Duration::from_secs(6));
    }

    #[test]
    fn hold_off_shorter_than_a_booked_slot_changes_nothing() {
        let mut pacer = Pacer::new();
        let start = Instant::now();
        for _ in 0..3 {
            pacer.reserve(start, SECOND);
        }
        pacer.hold_off(start, Duration::from_millis(500));
        assert_eq!(pacer.reserve(start, SECOND), Duration::from_secs(3));
    }

    #[test]
    fn a_zero_interval_never_waits() {
        let mut pacer = Pacer::new();
        let start = Instant::now();
        for _ in 0..3 {
            assert_eq!(pacer.reserve(start, Duration::ZERO), Duration::ZERO);
        }
    }

    #[test]
    fn twenty_seeds_of_two_requests_take_about_forty_seconds() {
        // A 20-seed `refs` run asks Semantic Scholar for each seed's counts
        // and then its list: 40 requests, booked as fast as they're made.
        let mut pacer = Pacer::new();
        let start = Instant::now();
        let last = (0..40).map(|_| pacer.reserve(start, SECOND)).last();
        assert_eq!(last, Some(Duration::from_secs(39)));
    }
}
