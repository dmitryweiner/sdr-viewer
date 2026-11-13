//! Real-time playback: a file or the generator would otherwise run as fast
//! as the CPU allows.

use std::time::{Duration, Instant};

use crate::source::{GainChange, Iq, Source, SourceError, SourceInfo};

/// Time as a parameter, so tests do not sleep.
pub trait Clock: Send {
    /// Time since an arbitrary fixed moment.
    fn now(&self) -> Duration;
    fn sleep(&mut self, duration: Duration);
}

pub struct SystemClock(Instant);

impl SystemClock {
    pub fn new() -> Self {
        SystemClock(Instant::now())
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }

    fn sleep(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// How far behind real time a source may fall before pacing gives up
/// catching up and starts counting afresh (after a suspend, say).
const MAX_LAG: Duration = Duration::from_secs(1);

/// Hands out samples no faster than the source's sample rate.
pub struct Paced<S, C> {
    inner: S,
    clock: C,
    start: Option<Duration>,
    samples: u64,
}

impl<S: Source, C: Clock> Paced<S, C> {
    pub fn new(inner: S, clock: C) -> Self {
        Paced {
            inner,
            clock,
            start: None,
            samples: 0,
        }
    }
}

impl<S: Source, C: Clock> Source for Paced<S, C> {
    fn info(&self) -> SourceInfo {
        self.inner.info()
    }

    fn read(&mut self, buf: &mut [Iq]) -> Result<usize, SourceError> {
        let n = self.inner.read(buf)?;
        let now = self.clock.now();
        let start = *self.start.get_or_insert(now);
        self.samples += n as u64;
        let rate = f64::from(self.inner.info().sample_rate);
        let due = Duration::from_secs_f64(self.samples as f64 / rate);
        let elapsed = now - start;
        if elapsed > due + MAX_LAG {
            self.start = Some(now);
            self.samples = n as u64;
            self.clock.sleep(Duration::from_secs_f64(n as f64 / rate));
        } else if due > elapsed {
            self.clock.sleep(due - elapsed);
        }
        Ok(n)
    }

    fn tune(&mut self, center_hz: f64) -> Result<(), SourceError> {
        self.inner.tune(center_hz)
    }

    fn set_gain(&mut self, change: GainChange) -> Result<(), SourceError> {
        self.inner.set_gain(change)
    }

    fn lost_samples(&self) -> u64 {
        self.inner.lost_samples()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generator::{Generator, GeneratorConfig};
    use std::sync::{Arc, Mutex};

    /// A clock that only moves when slept on or advanced by the test.
    #[derive(Clone, Default)]
    struct FakeClock {
        now: Arc<Mutex<Duration>>,
        slept: Arc<Mutex<Vec<Duration>>>,
    }

    impl FakeClock {
        fn advance(&self, by: Duration) {
            *self.now.lock().unwrap() += by;
        }
        fn slept(&self) -> Vec<Duration> {
            self.slept.lock().unwrap().clone()
        }
    }

    impl Clock for FakeClock {
        fn now(&self) -> Duration {
            *self.now.lock().unwrap()
        }
        fn sleep(&mut self, duration: Duration) {
            self.slept.lock().unwrap().push(duration);
            self.advance(duration);
        }
    }

    fn generator(sample_rate: u32) -> Generator {
        Generator::new(GeneratorConfig {
            sample_rate,
            ..GeneratorConfig::demo()
        })
    }

    #[test]
    fn sleeps_until_the_samples_are_due() {
        let clock = FakeClock::default();
        let mut paced = Paced::new(generator(1000), clock.clone());
        let mut buf = vec![Iq::new(0.0, 0.0); 250];
        paced.read(&mut buf).unwrap();
        paced.read(&mut buf).unwrap();
        assert_eq!(
            clock.slept(),
            [Duration::from_millis(250), Duration::from_millis(250)]
        );
    }

    #[test]
    fn does_not_sleep_when_the_reader_is_slower_than_real_time() {
        let clock = FakeClock::default();
        let mut paced = Paced::new(generator(1000), clock.clone());
        let mut buf = vec![Iq::new(0.0, 0.0); 100];
        paced.read(&mut buf).unwrap();
        clock.advance(Duration::from_millis(300));
        paced.read(&mut buf).unwrap();
        assert_eq!(clock.slept(), [Duration::from_millis(100)]);
    }

    #[test]
    fn starts_afresh_after_falling_far_behind() {
        let clock = FakeClock::default();
        let mut paced = Paced::new(generator(1000), clock.clone());
        let mut buf = vec![Iq::new(0.0, 0.0); 100];
        paced.read(&mut buf).unwrap();
        clock.advance(Duration::from_secs(10));
        // Without a fresh start the next 10 s of samples would come unpaced.
        paced.read(&mut buf).unwrap();
        paced.read(&mut buf).unwrap();
        assert_eq!(clock.slept().last(), Some(&Duration::from_millis(100)));
    }
}
