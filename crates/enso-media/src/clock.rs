//! The clock a file plays by: where in it playback is, counted on from
//! when that was last known. The sound sets it as the server plays (it is
//! what's heard); without sound, it simply runs.

use std::{
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

/// Differences smaller than this are eased away rather than jumped:
/// the server's reports wobble by a few milliseconds.
const SNAP: Duration = Duration::from_millis(40);

#[derive(Clone, Copy, Debug)]
pub(crate) struct Clock {
    at: Duration,
    since: Instant,
    running: bool,
}

impl Clock {
    pub fn new(at: Duration, now: Instant) -> Self {
        Self {
            at,
            since: now,
            running: false,
        }
    }

    pub fn now(&self, now: Instant) -> Duration {
        if self.running {
            self.at + now.saturating_duration_since(self.since)
        } else {
            self.at
        }
    }

    /// Start or stop it where it is.
    pub fn run(&mut self, running: bool, now: Instant) {
        self.at = self.now(now);
        self.since = now;
        self.running = running;
    }

    /// Put it at `at`, running or not as it was.
    pub fn set(&mut self, at: Duration, now: Instant) {
        self.at = at;
        self.since = now;
    }

    /// What's heard is at `at`: follow it, gently unless far off, and run.
    pub fn heard(&mut self, at: Duration, now: Instant) {
        let mine = self.now(now);
        let off = at.as_secs_f64() - mine.as_secs_f64();
        self.at = if !self.running || off.abs() > SNAP.as_secs_f64() {
            at
        } else {
            Duration::from_secs_f64((mine.as_secs_f64() + off * 0.2).max(0.0))
        };
        self.since = now;
        self.running = true;
    }
}

/// The clock, and how the sound stands, shared with the sound's thread.
#[derive(Debug)]
pub(crate) struct Timing {
    pub clock: Clock,
    /// The sound has all been heard.
    pub sound_ended: bool,
    /// The sound stopped for good (the server went away): the clock runs
    /// on its own.
    pub sound_lost: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct Shared(Arc<Mutex<Timing>>);

impl Shared {
    pub fn new(at: Duration, now: Instant) -> Self {
        Self(Arc::new(Mutex::new(Timing {
            clock: Clock::new(at, now),
            sound_ended: false,
            sound_lost: false,
        })))
    }

    pub fn lock(&self) -> MutexGuard<'_, Timing> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_runs_stops_and_follows_what_is_heard() {
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let mut clock = Clock::new(Duration::from_secs(10), at(0));
        assert_eq!(clock.now(at(500)), Duration::from_secs(10));
        clock.run(true, at(500));
        assert_eq!(clock.now(at(1500)), Duration::from_secs(11));
        clock.run(false, at(1500));
        assert_eq!(clock.now(at(9000)), Duration::from_secs(11));
        // Far off: jumped to. Near: eased towards.
        clock.heard(Duration::from_secs(20), at(9000));
        assert_eq!(clock.now(at(9000)), Duration::from_secs(20));
        clock.heard(Duration::from_millis(20_010), at(9000));
        let now = clock.now(at(9000)).as_secs_f64();
        assert!(now > 20.0 && now < 20.01, "{now}");
    }
}
