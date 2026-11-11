//! The sample rate actually achieved, to show whether the source keeps up.

use std::time::Duration;

/// Counts samples and turns them into a rate once per `window`.
pub struct RateMeter {
    window: Duration,
    since: Option<Duration>,
    count: u64,
    rate: f64,
}

impl RateMeter {
    pub fn new(window: Duration) -> Self {
        RateMeter {
            window,
            since: None,
            count: 0,
            rate: 0.0,
        }
    }

    /// Adds `samples` that arrived at `now`; true when the rate was updated.
    pub fn record(&mut self, now: Duration, samples: u64) -> bool {
        let Some(since) = self.since else {
            self.since = Some(now);
            return false;
        };
        self.count += samples;
        let elapsed = now.saturating_sub(since);
        if elapsed < self.window {
            return false;
        }
        self.rate = self.count as f64 / elapsed.as_secs_f64();
        self.since = Some(now);
        self.count = 0;
        true
    }

    /// Samples per second over the last complete window; 0 before the first.
    pub fn rate(&self) -> f64 {
        self.rate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn is_zero_until_a_window_has_passed() {
        let mut meter = RateMeter::new(ms(500));
        assert!(!meter.record(ms(0), 100));
        assert!(!meter.record(ms(400), 100));
        assert_eq!(meter.rate(), 0.0);
    }

    #[test]
    fn counts_samples_after_the_first_arrival() {
        let mut meter = RateMeter::new(ms(500));
        meter.record(ms(1000), 999); // the first chunk only starts the clock
        meter.record(ms(1250), 1000);
        assert!(meter.record(ms(1500), 1000));
        assert_eq!(meter.rate(), 4000.0);
    }

    #[test]
    fn each_window_starts_from_zero() {
        let mut meter = RateMeter::new(ms(500));
        meter.record(ms(0), 0);
        meter.record(ms(500), 1000);
        assert_eq!(meter.rate(), 2000.0);
        meter.record(ms(1000), 100);
        assert_eq!(meter.rate(), 200.0);
    }
}
