//! Does a channel repeat: its bursts kept as spans of time, the envelope
//! made of them — on and off, in even steps — and the periods found in
//! that envelope's autocorrelation. All times are stream time.

use std::collections::VecDeque;

use sdr_dsp::period::{Peak, autocorrelation, peaks};

/// A range of delays to look for a period in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Delays {
    /// The longest delay, in seconds.
    pub longest_s: u64,
    /// The step of the envelope and of the delays, in milliseconds. The
    /// detector's frames are 10 ms: nothing is finer.
    pub step_ms: u64,
}

/// The ranges the screen can show, the shortest first. A period shorter
/// than two steps of a range is not seen in it.
pub const DELAYS: [Delays; 4] = [
    Delays {
        longest_s: 1,
        step_ms: 10,
    },
    Delays {
        longest_s: 10,
        step_ms: 10,
    },
    Delays {
        longest_s: 60,
        step_ms: 50,
    },
    Delays {
        longest_s: 600,
        step_ms: 500,
    },
];

/// The range shown when nothing says which: no period found yet.
pub const USUAL: usize = 1;

/// The envelope is this many times the longest delay: a period at the
/// far end of the range has that many turns to show itself.
pub const TURNS: u64 = 6;

/// How long a burst is kept: the envelope of the longest range.
const KEPT_MS: u64 = DELAYS[DELAYS.len() - 1].longest_s * TURNS * 1000;

/// The bursts kept per channel at most; a channel busier than that has a
/// shorter past.
const KEPT: usize = 8192;

/// A peak this strong is listed on the screen.
pub const WORTH_LISTING: f32 = 0.25;

/// A period this strong is told on the channel strip, and chooses the
/// screen's range: three of every five bursts have their next one.
pub const SURE: f32 = 0.6;

/// The periods listed at most.
const MOST: usize = 4;

/// No period is read out of an envelope of fewer steps: in a few dozen
/// of them chance alone makes peaks. The shortest range's full length.
pub const ENOUGH: usize = 600;

impl Delays {
    /// Delays in the full range, in steps.
    pub fn lags(self) -> usize {
        (self.longest_s * 1000 / self.step_ms) as usize
    }

    /// Steps of the envelope when all of it is there.
    pub fn steps(self) -> usize {
        self.lags() * TURNS as usize
    }
}

/// When one channel had a signal.
#[derive(Debug, Clone, Default)]
pub struct Bursts {
    /// Start and end of each, in milliseconds, about in the order they
    /// ended.
    spans: VecDeque<(u64, u64)>,
    /// Bursts that ended before this may have been dropped: nothing is
    /// known of the time before it.
    since_ms: u64,
}

impl Bursts {
    pub fn add(&mut self, start_ms: u64, end_ms: u64) {
        self.spans.push_back((start_ms, end_ms));
        let oldest = end_ms.saturating_sub(KEPT_MS);
        while let Some(&(_, end)) = self.spans.front() {
            if end > oldest && self.spans.len() <= KEPT {
                break;
            }
            self.since_ms = self.since_ms.max(end);
            self.spans.pop_front();
        }
    }

    /// The share of each step the channel was on, the oldest first, up
    /// to the last whole step before `now_ms`: as much of the range's
    /// envelope as is known. `going` are the starts of bursts not ended.
    fn envelope(&self, going: &[u64], now_ms: u64, delays: Delays) -> Vec<f32> {
        let step = delays.step_ms;
        let end = now_ms / step;
        let first = end
            .saturating_sub(delays.steps() as u64)
            .max(self.since_ms.div_ceil(step));
        let mut envelope = vec![0.0f32; end.saturating_sub(first) as usize];
        let ended = self.spans.iter().copied();
        let going = going.iter().map(|&start| (start, now_ms));
        for (from, to) in ended.chain(going) {
            let (from, to) = (from.max(first * step), to.min(end * step));
            if from >= to {
                continue;
            }
            for slot in from / step..=(to - 1) / step {
                let on = to.min((slot + 1) * step) - from.max(slot * step);
                envelope[(slot - first) as usize] += on as f32 / step as f32;
            }
        }
        // Two bursts at once are no more than on.
        for share in &mut envelope {
            *share = share.min(1.0);
        }
        envelope
    }

    /// What repeats in range `scale` of [`DELAYS`], as of `now_ms`.
    pub fn analyse(&self, going: &[u64], now_ms: u64, scale: usize) -> Analysis {
        let delays = DELAYS[scale];
        let envelope = self.envelope(going, now_ms, delays);
        // A delay is tried only where the envelope is twice as long.
        let lags = delays.lags().min(envelope.len() / 2);
        let correlation = autocorrelation(&envelope, lags);
        let peaks = if envelope.len() >= ENOUGH {
            peaks(&correlation, WORTH_LISTING, MOST)
        } else {
            Vec::new()
        };
        Analysis {
            scale,
            envelope,
            correlation,
            peaks,
        }
    }

    /// The channel's period, if it has one for sure: of the ranges, the
    /// finest that shows one, unless a coarser shows it clearly better.
    pub fn period(&self, going: &[u64], now_ms: u64) -> Option<Period> {
        // Two bursts are not a rhythm.
        if self.spans.len() + going.len() < 3 {
            return None;
        }
        let mut best: Option<Period> = None;
        for scale in 0..DELAYS.len() {
            let analysis = self.analyse(going, now_ms, scale);
            let Some(period) = analysis.period().filter(|period| period.strength >= SURE) else {
                continue;
            };
            if best.is_none_or(|best| period.strength > best.strength + 0.1) {
                best = Some(period);
            }
        }
        best
    }
}

/// How a channel repeats.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Period {
    pub seconds: f64,
    /// The share of the signal that came again after that time, 0 to 1.
    pub strength: f32,
    /// The range of [`DELAYS`] it was found in.
    pub scale: usize,
}

/// One range of delays looked into.
#[derive(Debug, Clone, PartialEq)]
pub struct Analysis {
    pub scale: usize,
    /// The share of each step the channel was on, the oldest first, the
    /// last one ending now.
    pub envelope: Vec<f32>,
    /// The envelope's autocorrelation by delay, from 0, in steps.
    pub correlation: Vec<f32>,
    /// The main peak first.
    pub peaks: Vec<Peak>,
}

impl Analysis {
    pub fn delays(&self) -> Delays {
        DELAYS[self.scale]
    }

    /// The periods found, the main one first.
    pub fn periods(&self) -> impl Iterator<Item = Period> + '_ {
        let step = self.delays().step_ms as f64 / 1000.0;
        self.peaks.iter().map(move |peak| Period {
            seconds: f64::from(peak.lag) * step,
            strength: peak.strength,
            scale: self.scale,
        })
    }

    /// The main period.
    pub fn period(&self) -> Option<Period> {
        self.periods().next()
    }

    /// The autocorrelation for `dots` points across the full range of
    /// delays: the highest in each point's delays, or the nearest when a
    /// point is finer than a step; nothing where no delay was tried.
    pub fn dots(&self, dots: usize) -> Vec<Option<f32>> {
        let lags = self.delays().lags();
        (0..dots)
            .map(|dot| {
                let from = dot * lags / dots;
                let to = ((dot + 1) * lags / dots).max(from + 1);
                let to = to.min(self.correlation.len());
                let within = self.correlation.get(from..to)?;
                within.iter().copied().reduce(f32::max)
            })
            .collect()
    }

    /// The envelope for `columns` columns across its full length, now at
    /// the right: the mean share in each column's steps; nothing for a
    /// column from before what is known.
    pub fn bars(&self, columns: usize) -> Vec<Option<f32>> {
        let steps = self.delays().steps();
        let unknown = steps - self.envelope.len().min(steps);
        (0..columns)
            .map(|column| {
                let from = column * steps / columns;
                let to = ((column + 1) * steps / columns).max(from + 1);
                let within = self
                    .envelope
                    .get(from.checked_sub(unknown)?..to.checked_sub(unknown)?)?;
                Some(within.iter().sum::<f32>() / within.len() as f32)
            })
            .collect()
    }
}

/// "340 ms", "2.04 s", "34.1 s", "340 s": a period with the digits it has.
pub fn format_period(seconds: f64) -> String {
    if seconds < 0.9995 {
        format!("{:.0} ms", seconds * 1000.0)
    } else if seconds < 9.995 {
        format!("{seconds:.2} s")
    } else if seconds < 99.95 {
        format!("{seconds:.1} s")
    } else {
        format!("{seconds:.0} s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `on_ms` of signal every `period_ms`, from 0 to `until_ms`.
    fn bursts(period_ms: u64, on_ms: u64, until_ms: u64) -> Bursts {
        let mut bursts = Bursts::default();
        let mut start = 0;
        while start + on_ms <= until_ms {
            bursts.add(start, start + on_ms);
            start += period_ms;
        }
        bursts
    }

    #[test]
    fn the_envelope_is_the_share_of_each_step_the_channel_was_on() {
        let mut bursts = Bursts::default();
        bursts.add(1000, 1020);
        bursts.add(1025, 1045);
        // Over one that is there already.
        bursts.add(1030, 1040);
        let envelope = bursts.envelope(&[1095], 1108, DELAYS[0]);
        // Steps of 10 ms from 0 to 1100: the 8 ms after are no step yet.
        assert_eq!(envelope.len(), 110);
        assert_eq!(
            envelope[100..],
            [1.0, 1.0, 0.5, 1.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.5]
        );
        assert!(envelope[..100].iter().all(|&share| share == 0.0));
        // A longer past is cut to the range's length.
        let envelope = bursts.envelope(&[], 60_000, DELAYS[0]);
        assert_eq!(envelope.len(), 600);
        // Coarser steps hold shares; two bursts at once count twice
        // there until the step is full.
        let envelope = bursts.envelope(&[], 2000, DELAYS[3]);
        assert_eq!(envelope.len(), 4);
        assert!((envelope[2] - 0.1).abs() < 1e-6 && envelope[3] == 0.0);
    }

    #[test]
    fn a_burst_every_two_seconds_is_found_in_the_range_that_holds_it() {
        let bursts = bursts(2040, 300, 60_000);
        let short = bursts.analyse(&[], 60_000, 0);
        assert_eq!(short.correlation.len(), 101);
        assert_eq!(short.period(), None);
        let analysis = bursts.analyse(&[], 60_000, 1);
        assert_eq!(analysis.envelope.len(), 6000);
        assert_eq!(analysis.correlation.len(), 1001);
        let period = analysis.period().unwrap();
        assert!((period.seconds - 2.04).abs() < 0.005, "{period:?}");
        assert!(period.strength > 0.9, "{period:?}");
        assert_eq!(analysis.periods().count(), 1);
        // The channel's period is that one, though coarser ranges see
        // it too.
        let found = bursts.period(&[], 60_000).unwrap();
        assert_eq!(found.scale, 1);
        assert!((found.seconds - 2.04).abs() < 0.005);
        assert!(bursts.analyse(&[], 60_000, 2).period().is_some());
    }

    #[test]
    fn a_slow_period_is_found_in_a_coarse_range() {
        let bursts = bursts(34_000, 100, 600_000);
        let found = bursts.period(&[], 600_000).unwrap();
        assert!((found.seconds - 34.0).abs() < 0.3, "{found:?}");
        assert!(found.scale >= 2);
    }

    #[test]
    fn a_short_past_tries_only_the_delays_it_has_twice() {
        let bursts = bursts(2040, 300, 7000);
        let analysis = bursts.analyse(&[], 7000, 1);
        assert_eq!(analysis.envelope.len(), 700);
        assert_eq!(analysis.correlation.len(), 351);
        // Four bursts: three have a next one.
        let period = analysis.period().unwrap();
        assert!((period.strength - 0.75).abs() < 0.05, "{period:?}");
        // Two bursts are no rhythm, and nothing is nothing.
        let mut two = Bursts::default();
        two.add(0, 300);
        two.add(2040, 2340);
        assert_eq!(two.period(&[], 7000), None);
        let nothing = Bursts::default().analyse(&[], 0, 1);
        assert!(nothing.envelope.is_empty() && nothing.peaks.is_empty());
    }

    #[test]
    fn a_carrier_and_bursts_at_random_have_no_period() {
        let mut carrier = Bursts::default();
        carrier.add(0, 30_000);
        assert_eq!(carrier.period(&[30_000], 60_000), None);
        let mut random = Bursts::default();
        let mut seed = 3u32;
        let mut at = 0u64;
        while at < 60_000 {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            at += 100 + u64::from(seed >> 22);
            random.add(at, at + 40);
        }
        assert_eq!(random.period(&[], 60_000), None);
    }

    #[test]
    fn only_the_last_hour_and_the_last_thousands_of_bursts_are_kept() {
        let mut bursts = bursts(1000, 100, 100_000);
        bursts.add(3_700_000, 3_700_100);
        // What ended an hour before the latest is gone.
        assert_eq!(bursts.spans.len(), 1);
        assert_eq!(bursts.since_ms, 99_100);
        let mut busy = Bursts::default();
        for burst in 0..10_000 {
            busy.add(burst * 100, burst * 100 + 50);
        }
        assert_eq!(busy.spans.len(), KEPT);
        // The envelope does not reach back into what was dropped.
        let since = busy.since_ms;
        assert_eq!(since, (10_000 - KEPT as u64 - 1) * 100 + 50);
        let envelope = busy.envelope(&[], 1_000_000, DELAYS[3]);
        assert_eq!(envelope.len() as u64, 2000 - since.div_ceil(500));
    }

    #[test]
    fn the_plot_has_the_highest_of_each_points_delays() {
        let bursts = bursts(2040, 300, 60_000);
        let analysis = bursts.analyse(&[], 60_000, 1);
        // 1000 delays in 100 points: the peak at 204 is in point 20.
        let dots = analysis.dots(100);
        assert!(dots[20].unwrap() > 0.9);
        assert!(dots[10].unwrap() < 0.0);
        assert_eq!(dots[0], Some(1.0));
        // More points than delays: each has the delay it is in.
        let fine = bursts.analyse(&[], 60_000, 0).dots(200);
        assert_eq!(fine[0], fine[1]);
        assert_eq!(fine[0], Some(1.0));
        // A short past leaves the far delays empty.
        let short = bursts.analyse(&[], 7000, 1).dots(100);
        assert!(short[34].is_some() && short[36].is_none());
    }

    #[test]
    fn the_envelopes_bars_are_aligned_to_now() {
        let bursts = bursts(2040, 300, 60_000);
        let analysis = bursts.analyse(&[], 60_000, 1);
        // A second to a column; the burst at 59.16 s is in the last.
        let bars = analysis.bars(60);
        assert!((bars[59].unwrap() - 0.3).abs() < 1e-3);
        assert!((bars[0].unwrap() - 0.3).abs() < 1e-3);
        assert_eq!(bars[1], Some(0.0));
        // Of a short past the left part is unknown.
        let short = bursts.analyse(&[], 7000, 1).bars(60);
        assert_eq!(short[52], None);
        assert!((short[53].unwrap() - 0.3).abs() < 1e-3);
        // More columns than steps repeat a step.
        assert_eq!(analysis.bars(12_000).len(), 12_000);
    }

    #[test]
    fn a_period_is_told_with_the_digits_it_has() {
        assert_eq!(format_period(0.04), "40 ms");
        assert_eq!(format_period(0.9994), "999 ms");
        assert_eq!(format_period(0.9996), "1.00 s");
        assert_eq!(format_period(2.04), "2.04 s");
        assert_eq!(format_period(34.02), "34.0 s");
        assert_eq!(format_period(340.4), "340 s");
    }
}
