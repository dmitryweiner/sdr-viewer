//! Does an envelope repeat: its autocorrelation, and the delays at which
//! that peaks.

use rustfft::FftPlanner;
use rustfft::num_complex::Complex;

/// The autocorrelation of `envelope` at delays of 0 to `max_lag` steps
/// (fewer when the envelope is shorter), its mean taken off, as shares of
/// what delay 0 gives. The sums are not made up for the shorter overlap
/// at a longer delay, so N equal pulses give (N − 1) / N at their period:
/// two bursts are not yet a rhythm. An envelope that never changes
/// repeats nothing and gives zeros.
pub fn autocorrelation(envelope: &[f32], max_lag: usize) -> Vec<f32> {
    let n = envelope.len();
    let lags = max_lag.min(n.saturating_sub(1));
    if n == 0 {
        return Vec::new();
    }
    let mean = envelope.iter().sum::<f32>() / n as f32;
    // Zeros after the envelope keep its end from meeting its start.
    let size = (n + lags).next_power_of_two();
    let mut buffer = vec![Complex::new(0.0f32, 0.0); size];
    for (slot, value) in buffer.iter_mut().zip(envelope) {
        slot.re = value - mean;
    }
    let mut planner = FftPlanner::new();
    planner.plan_fft_forward(size).process(&mut buffer);
    for slot in &mut buffer {
        *slot = Complex::new(slot.norm_sqr(), 0.0);
    }
    planner.plan_fft_inverse(size).process(&mut buffer);
    let zero = buffer[0].re;
    // What is left of a constant is rounding, not a signal.
    if zero <= size as f32 * n as f32 * 1e-9 {
        return vec![0.0; lags + 1];
    }
    buffer[..=lags].iter().map(|slot| slot.re / zero).collect()
}

/// A delay at which the envelope looks like itself again.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Peak {
    /// In steps of the envelope, between two of them when the top is.
    pub lag: f32,
    /// The autocorrelation there, 1 at most.
    pub strength: f32,
}

/// The main peak is the shortest delay nearly as strong as the strongest:
/// a signal that repeats every T repeats every 2T too.
const NEARLY: f32 = 0.85;

/// The peaks of an autocorrelation that reach `min_strength`, at most
/// `most` of them: the main one first, then the others from the
/// strongest down, without the multiples of those before them. The hump
/// around delay 0 — the signal against itself — is no peak.
///
/// A peak's strength is what its hump holds beside what the hump at 0
/// holds, not its one highest point: a period between two steps (pulses
/// 20 and 21 steps apart in turn) is split between two delays, half in
/// each, and is no weaker for it.
pub fn peaks(correlation: &[f32], min_strength: f32, most: usize) -> Vec<Peak> {
    let humps = humps(correlation);
    let Some(first_dip) =
        (1..humps.len().saturating_sub(1)).find(|&lag| humps[lag] < humps[lag + 1])
    else {
        return Vec::new();
    };
    let mut tops: Vec<Peak> = (first_dip + 1..humps.len() - 1)
        .filter(|&lag| {
            let (before, here, after) = (humps[lag - 1], humps[lag], humps[lag + 1]);
            here >= min_strength && here > before && here >= after
        })
        .map(|lag| {
            let (before, here, after) = (humps[lag - 1], humps[lag], humps[lag + 1]);
            // The top of the parabola through the three points.
            let bend = before - 2.0 * here + after;
            let shift = if bend < 0.0 {
                (0.5 * (before - after) / bend).clamp(-0.5, 0.5)
            } else {
                0.0
            };
            Peak {
                lag: lag as f32 + shift,
                strength: here.min(1.0),
            }
        })
        .collect();
    let strongest = tops.iter().map(|peak| peak.strength).fold(0.0, f32::max);
    let Some(main) = tops
        .iter()
        .position(|peak| peak.strength >= NEARLY * strongest)
    else {
        return Vec::new();
    };
    let mut kept = vec![tops.remove(main)];
    tops.sort_by(|a, b| b.strength.total_cmp(&a.strength));
    for peak in tops {
        if kept.len() >= most {
            break;
        }
        if !kept.iter().any(|of| is_multiple(peak.lag, of.lag)) {
            kept.push(peak);
        }
    }
    kept.truncate(most);
    kept
}

/// What the autocorrelation holds above nothing over as many delays each
/// way as the hump at 0 is wide (one at least), as shares of what the
/// hump at 0 holds. Only what is above nothing is summed: the dips
/// beside a hump are not a part of it. The last delays, whose sum would
/// reach past the end, are left out: a hump cut off there is no peak.
fn humps(correlation: &[f32]) -> Vec<f32> {
    let len = correlation.len();
    let falls = (1..len).find(|&lag| {
        correlation[lag] <= 0.0
            || correlation
                .get(lag + 1)
                .is_some_and(|&next| next > correlation[lag])
    });
    let half = falls.map_or(1, |lag| lag - 1).clamp(1, (len / 8).max(1)) as isize;
    // Before delay 0 the autocorrelation is its own mirror.
    let at = |lag: isize| {
        let value = correlation.get(lag.unsigned_abs()).copied();
        value.unwrap_or(0.0).max(0.0)
    };
    let sum = |lag: usize| -> f32 { (-half..=half).map(|by| at(lag as isize + by)).sum() };
    let zero = sum(0);
    let whole = len.saturating_sub(half.unsigned_abs());
    if zero <= 0.0 {
        return vec![0.0; whole];
    }
    (0..whole).map(|lag| sum(lag) / zero).collect()
}

/// Whether `lag` is a whole number of `base`s, as nearly as a peak can
/// be placed.
fn is_multiple(lag: f32, base: f32) -> bool {
    let times = (lag / base).round();
    times >= 1.0 && (lag - times * base).abs() <= 1.0 + 0.02 * lag
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `on` steps of signal at the start of every `period`.
    fn pulses(steps: usize, period: usize, on: usize) -> Vec<f32> {
        (0..steps)
            .map(|step| if step % period < on { 1.0 } else { 0.0 })
            .collect()
    }

    /// The sums as they are written down, one delay at a time.
    fn slowly(envelope: &[f32], max_lag: usize) -> Vec<f32> {
        let n = envelope.len();
        let mean = envelope.iter().sum::<f32>() / n as f32;
        let at = |lag: usize| -> f32 {
            (0..n - lag)
                .map(|i| (envelope[i] - mean) * (envelope[i + lag] - mean))
                .sum()
        };
        (0..=max_lag).map(|lag| at(lag) / at(0)).collect()
    }

    #[test]
    fn the_fast_way_gives_what_the_sums_give() {
        let envelope: Vec<f32> = (0..300)
            .map(|i| ((i * 7919) % 13) as f32 / 12.0 + if i % 50 < 5 { 1.0 } else { 0.0 })
            .collect();
        let fast = autocorrelation(&envelope, 120);
        let slow = slowly(&envelope, 120);
        assert_eq!(fast.len(), 121);
        for (lag, (fast, slow)) in fast.iter().zip(&slow).enumerate() {
            assert!((fast - slow).abs() < 1e-3, "lag {lag}: {fast} and {slow}");
        }
        assert!((fast[0] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn pulses_peak_at_their_period_by_the_share_that_have_a_next_one() {
        // 12 pulses: 11 of them have one a period later.
        let correlation = autocorrelation(&pulses(600, 50, 3), 200);
        assert!(
            (correlation[50] - 11.0 / 12.0).abs() < 0.01,
            "{}",
            correlation[50]
        );
        assert!((correlation[100] - 10.0 / 12.0).abs() < 0.01);
        assert!(correlation[25] < 0.0);
        // Two bursts are half a rhythm.
        let two = autocorrelation(&pulses(100, 50, 3), 60);
        assert!((two[50] - 0.5).abs() < 0.03, "{}", two[50]);
    }

    #[test]
    fn what_never_changes_repeats_nothing() {
        assert_eq!(autocorrelation(&[1.0; 64], 10), [0.0; 11]);
        assert_eq!(autocorrelation(&[0.0; 64], 10), [0.0; 11]);
        assert_eq!(autocorrelation(&[0.3; 1000], 2), [0.0; 3]);
        assert!(autocorrelation(&[], 10).is_empty());
        // No delay is as long as the envelope.
        assert_eq!(autocorrelation(&[0.0, 1.0, 0.0], 10).len(), 3);
    }

    #[test]
    fn the_main_peak_is_the_period_and_its_multiples_are_not_listed() {
        let correlation = autocorrelation(&pulses(1200, 50, 3), 400);
        let found = peaks(&correlation, 0.25, 5);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!((found[0].lag - 50.0).abs() < 0.01, "{found:?}");
        assert!(found[0].strength > 0.9);
    }

    #[test]
    fn a_rhythm_inside_the_rhythm_is_listed_after_the_main_one() {
        // Three pulses 10 steps apart, every 200 steps.
        let envelope: Vec<f32> = (0..2400)
            .map(|step| {
                let within = step % 200;
                if within < 30 && within % 10 < 2 {
                    1.0
                } else {
                    0.0
                }
            })
            .collect();
        let found = peaks(&autocorrelation(&envelope, 500), 0.25, 5);
        assert!((found[0].lag - 200.0).abs() < 0.01, "{found:?}");
        assert!((found[1].lag - 10.0).abs() < 0.01, "{found:?}");
        // Two of three have a next one, within a group.
        assert!((found[1].strength - 0.66).abs() < 0.05, "{found:?}");
        assert!(found.iter().all(|peak| (peak.lag - 400.0).abs() > 1.0));
        assert!(found.iter().all(|peak| (peak.lag - 20.0).abs() > 1.0));
        assert_eq!(peaks(&autocorrelation(&envelope, 500), 0.25, 1).len(), 1);
    }

    #[test]
    fn a_period_between_two_steps_is_placed_between_them() {
        // Every 20.5 steps: the pulses fall on 20 and 21 in turn.
        let envelope: Vec<f32> = (0..4000)
            .map(|step| {
                let phase = (step as f32 / 20.5).fract() * 20.5;
                (2.0 - phase).clamp(0.0, 1.0)
            })
            .collect();
        let found = peaks(&autocorrelation(&envelope, 100), 0.25, 5);
        assert!((found[0].lag - 20.5).abs() < 0.2, "{found:?}");
    }

    #[test]
    fn pulses_a_step_early_and_late_in_turn_are_as_strong_as_even_ones() {
        // One step wide, 20 and 21 steps apart in turn: every 20.5.
        let envelope: Vec<f32> = (0..4100)
            .map(|step| match step % 41 {
                0 | 20 => 1.0,
                _ => 0.0,
            })
            .collect();
        let found = peaks(&autocorrelation(&envelope, 100), 0.25, 5);
        assert!((found[0].lag - 20.5).abs() < 0.01, "{found:?}");
        assert!(found[0].strength > 0.9, "{found:?}");
        assert_eq!(found.len(), 1, "{found:?}");
    }

    #[test]
    fn a_hump_cut_off_by_the_end_of_the_delays_is_no_peak() {
        // 30 steps on of every 500: at delay 1000 the third hump is at
        // its top, and only its rising side is there.
        let correlation = autocorrelation(&pulses(6000, 500, 30), 1000);
        let found = peaks(&correlation, 0.25, 5);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!((found[0].lag - 500.0).abs() < 0.5, "{found:?}");
    }

    #[test]
    fn noise_and_a_lone_burst_have_no_peaks() {
        let mut seed = 7u32;
        let noise: Vec<f32> = (0..2000)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                if seed >> 28 == 0 { 1.0 } else { 0.0 }
            })
            .collect();
        assert_eq!(peaks(&autocorrelation(&noise, 500), 0.25, 5), []);
        let mut lone = vec![0.0; 500];
        lone[100..110].fill(1.0);
        assert_eq!(peaks(&autocorrelation(&lone, 200), 0.25, 5), []);
        assert_eq!(peaks(&[], 0.25, 5), []);
        assert_eq!(peaks(&[1.0, 0.5], 0.25, 5), []);
    }
}
