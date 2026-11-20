//! Recording on an alert, as the reader sees it: which stretches of the
//! stream go to which recording, from a second before the event to a
//! second after it ends. The samples come from the ring, so a recording
//! begins before its alert; narrowing and writing them is the worker's.

use std::ops::Range;
use std::time::Duration;

use sdr_detect::Alert;
use sdr_detect::channel::MIN_WIDTH_HZ;

/// Kept before an event and after it ends: how it began and ended is a
/// part of it.
pub const AROUND: Duration = Duration::from_secs(1);

/// Recordings at once, at most. Each costs the reader a copy of every
/// read and the worker its narrowing; a rule over a hopper's band would
/// otherwise start one for every hop, 80 a second.
pub const AT_ONCE: usize = 4;

/// What a recording asks of the worker, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// A recording begins with sample `first` of the stream, of a band
    /// `width_hz` wide around `center_hz`.
    Start {
        id: u64,
        first: u64,
        center_hz: f64,
        width_hz: f64,
        alert: Alert,
    },
    /// Another alert of the rule, in the band of a recording going on.
    Mark { id: u64, alert: Alert },
    /// These samples of the stream are the recording's next.
    Samples { id: u64, range: Range<u64> },
    /// The recording is over.
    End { id: u64 },
}

/// A recording going on.
#[derive(Debug, Clone, PartialEq)]
struct Going {
    id: u64,
    rule: String,
    center_hz: f64,
    width_hz: f64,
    /// The detector's numbers of its events still on.
    following: Vec<u64>,
    /// The next sample to hand over.
    next: u64,
    /// Once its events are over: the sample it ends before.
    until: Option<u64>,
}

/// The recordings going on, for a stream at `sample_rate`.
#[derive(Debug, Clone)]
pub struct Recorder {
    sample_rate: u32,
    going: Vec<Going>,
    next_id: u64,
}

impl Recorder {
    pub fn new(sample_rate: u32) -> Self {
        Recorder {
            sample_rate,
            going: Vec::new(),
            next_id: 0,
        }
    }

    fn samples(&self, time: Duration) -> u64 {
        (time.as_secs_f64() * f64::from(self.sample_rate)).round() as u64
    }

    /// Takes an alert in. One that records starts a recording, from
    /// [`AROUND`] before its event as far as `held` (what the ring has)
    /// goes back — or joins one of its rule going on whose band its
    /// event's centre is in. Says whether it was recorded: not when
    /// [`AT_ONCE`] recordings run already. The recording's band is the
    /// event's, or its rule's when the event is wider: a burst keyed hard
    /// splashes wide in the first frame, where a rule without a duration
    /// is met.
    pub fn on_alert(&mut self, alert: &Alert, held: Range<u64>, out: &mut Vec<Step>) -> bool {
        if !alert.record {
            return true;
        }
        let event = &alert.event;
        let (low_hz, high_hz) = alert.band;
        let (center_hz, width_hz) = match event.width_hz() > high_hz - low_hz {
            true => ((low_hz + high_hz) / 2.0, high_hz - low_hz),
            false => (event.center_hz(), event.width_hz()),
        };
        let joined = self.going.iter_mut().find(|going| {
            going.rule == alert.rule && (center_hz - going.center_hz).abs() <= going.width_hz / 2.0
        });
        if let Some(going) = joined {
            going.following.push(alert.number);
            going.until = None;
            out.push(Step::Mark {
                id: going.id,
                alert: alert.clone(),
            });
            return true;
        }
        if self.going.len() >= AT_ONCE {
            return false;
        }
        let first = (self
            .samples(event.start)
            .saturating_sub(self.samples(AROUND)))
        .max(held.start)
        .min(held.end);
        let id = self.next_id;
        self.next_id += 1;
        let width_hz = width_hz.max(MIN_WIDTH_HZ);
        self.going.push(Going {
            id,
            rule: alert.rule.clone(),
            center_hz,
            width_hz,
            following: vec![alert.number],
            next: first,
            until: None,
        });
        out.push(Step::Start {
            id,
            first,
            center_hz,
            width_hz,
            alert: alert.clone(),
        });
        true
    }

    /// After a read: hands over what the ring now holds beyond what each
    /// recording has, and ends those whose events have been over for
    /// [`AROUND`]. `is_on` tells whether the detector's event of that
    /// number is still going on.
    pub fn on_read(&mut self, held: Range<u64>, is_on: impl Fn(u64) -> bool, out: &mut Vec<Step>) {
        let around = self.samples(AROUND);
        for going in &mut self.going {
            going.following.retain(|&number| is_on(number));
            if going.following.is_empty() && going.until.is_none() {
                going.until = Some(held.end + around);
            }
            let end = held.end.min(going.until.unwrap_or(u64::MAX));
            // Never what the ring has let go of.
            let from = going.next.max(held.start);
            if from < end {
                out.push(Step::Samples {
                    id: going.id,
                    range: from..end,
                });
                going.next = end;
            }
        }
        self.going.retain(|going| {
            let over = going.until.is_some_and(|until| going.next >= until);
            if over {
                out.push(Step::End { id: going.id });
            }
            !over
        });
    }

    /// Ends every recording where it is: the band was retuned, or the
    /// stream is over.
    pub fn end_all(&mut self, out: &mut Vec<Step>) {
        out.extend(self.going.drain(..).map(|going| Step::End { id: going.id }));
    }

    pub fn is_recording(&self) -> bool {
        !self.going.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdr_detect::Event;

    /// A stream of 1000 samples a second: a sample is a millisecond.
    const RATE: u32 = 1000;

    fn alert(rule: &str, number: u64, start_ms: u64, center_hz: f64) -> Alert {
        Alert {
            rule: rule.into(),
            record: true,
            // The whole ISM band.
            band: (433.05e6, 434.79e6),
            number,
            event: Event {
                start: Duration::from_millis(start_ms),
                duration: Duration::from_millis(500),
                low_hz: center_hz - 2_500.0,
                high_hz: center_hz + 2_500.0,
                peak_hz: center_hz,
                peak_dbfs: -40.0,
                above_floor_db: 30.0,
            },
        }
    }

    #[test]
    fn a_recording_runs_from_a_second_before_its_event_to_a_second_after() {
        let mut recorder = Recorder::new(RATE);
        let mut out = Vec::new();
        // The event began at 5 s, the rule was met at 5.5 s; the ring
        // holds the last 3 s.
        let gate = alert("gate", 7, 5_000, 433.9e6);
        recorder.on_alert(&gate, 2_500..5_500, &mut out);
        assert_eq!(
            out,
            [Step::Start {
                id: 0,
                first: 4_000,
                center_hz: 433.9e6,
                width_hz: 10_000.0,
                alert: gate,
            }]
        );
        out.clear();
        // What the ring has goes at once; the event is still on.
        recorder.on_read(2_600..5_600, |_| true, &mut out);
        assert_eq!(
            out,
            [Step::Samples {
                id: 0,
                range: 4_000..5_600
            }]
        );
        out.clear();
        recorder.on_read(3_000..6_000, |_| true, &mut out);
        // It has ended: a second more.
        recorder.on_read(3_500..6_500, |_| false, &mut out);
        recorder.on_read(4_000..7_000, |_| false, &mut out);
        recorder.on_read(4_500..7_600, |_| false, &mut out);
        assert_eq!(
            out,
            [
                Step::Samples {
                    id: 0,
                    range: 5_600..6_000
                },
                Step::Samples {
                    id: 0,
                    range: 6_000..6_500
                },
                Step::Samples {
                    id: 0,
                    range: 6_500..7_000
                },
                Step::Samples {
                    id: 0,
                    range: 7_000..7_500
                },
                Step::End { id: 0 },
            ]
        );
        assert!(!recorder.is_recording());
    }

    #[test]
    fn a_recording_takes_no_wider_a_band_than_its_rule_watches() {
        // A burst keyed hard splashes wide in its first frame, when a
        // rule without a duration is met: 100 kHz, its centre off.
        let channel = (433.46875e6, 433.48125e6);
        let splash = Alert {
            band: channel,
            event: Event {
                low_hz: 433.43e6,
                high_hz: 433.53e6,
                ..alert("LPD 17", 1, 1_000, 433.48e6).event
            },
            ..alert("LPD 17", 1, 1_000, 433.48e6)
        };
        let mut recorder = Recorder::new(RATE);
        let mut out = Vec::new();
        recorder.on_alert(&splash, 0..1_500, &mut out);
        assert!(
            matches!(
                out[0],
                Step::Start {
                    center_hz: 433.475e6,
                    width_hz: 12_500.0,
                    ..
                }
            ),
            "{out:?}"
        );
        // One narrower than the rule's band is its own.
        let narrow = Alert {
            band: channel,
            ..alert("LPD 17", 2, 1_000, 433.476e6)
        };
        let mut recorder = Recorder::new(RATE);
        out.clear();
        recorder.on_alert(&narrow, 0..1_500, &mut out);
        assert!(
            matches!(
                out[0],
                Step::Start {
                    center_hz: 433.476e6,
                    width_hz: 10_000.0,
                    ..
                }
            ),
            "{out:?}"
        );
    }

    #[test]
    fn the_ring_s_start_bounds_what_comes_before() {
        let mut recorder = Recorder::new(RATE);
        let mut out = Vec::new();
        recorder.on_alert(&alert("gate", 1, 300, 433.9e6), 0..800, &mut out);
        assert!(matches!(out[0], Step::Start { first: 0, .. }), "{out:?}");
        // A ring that holds nothing: the recording begins with what comes.
        let mut recorder = Recorder::new(RATE);
        out.clear();
        recorder.on_alert(&alert("gate", 1, 300, 433.9e6), 800..800, &mut out);
        assert!(matches!(out[0], Step::Start { first: 800, .. }), "{out:?}");
    }

    #[test]
    fn an_alert_of_the_same_rule_in_the_band_goes_on_in_the_same_recording() {
        let mut recorder = Recorder::new(RATE);
        let mut out = Vec::new();
        recorder.on_alert(&alert("gate", 1, 1_000, 433.9e6), 0..1_500, &mut out);
        recorder.on_read(0..1_600, |number| number == 1, &mut out);
        recorder.on_read(0..2_000, |_| false, &mut out);
        // Before its second after: the next burst, 3 kHz off.
        let again = alert("gate", 2, 2_400, 433.903e6);
        recorder.on_alert(&again, 0..2_900, &mut out);
        // Another rule's, or one out of the band, is a recording of its own.
        recorder.on_alert(&alert("other", 3, 2_400, 433.9e6), 0..2_900, &mut out);
        recorder.on_alert(&alert("gate", 4, 2_400, 433.95e6), 0..2_900, &mut out);
        out.clear();
        // The others' events are over at once, and they end a second
        // later; the second burst is seen over at 4.6 s, and the first
        // recording ends a second after that.
        recorder.on_read(0..3_500, |number| number == 2, &mut out);
        recorder.on_read(0..4_600, |_| false, &mut out);
        recorder.on_read(0..5_600, |_| false, &mut out);
        let ends: Vec<&Step> = out
            .iter()
            .filter(|step| matches!(step, Step::End { .. }))
            .collect();
        assert_eq!(
            ends,
            [
                &Step::End { id: 1 },
                &Step::End { id: 2 },
                &Step::End { id: 0 }
            ]
        );
        let first: Vec<Range<u64>> = out
            .iter()
            .filter_map(|step| match step {
                Step::Samples { id: 0, range } => Some(range.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(first.last().unwrap().end, 5_600);
    }

    #[test]
    fn no_more_than_four_record_at_once_but_one_going_on_may_still_be_joined() {
        let mut recorder = Recorder::new(RATE);
        let mut out = Vec::new();
        let recorded: Vec<bool> = (0..6)
            .map(|hop| {
                let hz = 433.1e6 + 25e3 * hop as f64;
                recorder.on_alert(&alert("hopper", hop, 1_000, hz), 0..1_500, &mut out)
            })
            .collect();
        assert_eq!(recorded, [true, true, true, true, false, false]);
        assert!(recorder.on_alert(&alert("hopper", 9, 1_200, 433.1e6), 0..1_700, &mut out));
        assert_eq!(out.len(), 5, "{out:?}");
        assert!(matches!(out[4], Step::Mark { id: 0, .. }));
    }

    #[test]
    fn the_mark_of_a_joining_alert_is_told() {
        let mut recorder = Recorder::new(RATE);
        let mut out = Vec::new();
        recorder.on_alert(&alert("gate", 1, 1_000, 433.9e6), 0..1_500, &mut out);
        let again = alert("gate", 2, 1_200, 433.9e6);
        recorder.on_alert(&again, 0..1_700, &mut out);
        assert_eq!(
            out[1],
            Step::Mark {
                id: 0,
                alert: again
            }
        );
    }

    #[test]
    fn an_alert_that_does_not_record_is_nothing_here_and_all_end_on_a_retune() {
        let mut recorder = Recorder::new(RATE);
        let mut out = Vec::new();
        let quiet = Alert {
            record: false,
            ..alert("gate", 1, 1_000, 433.9e6)
        };
        recorder.on_alert(&quiet, 0..1_500, &mut out);
        assert!(out.is_empty() && !recorder.is_recording());
        recorder.on_alert(&alert("a", 2, 1_000, 433.9e6), 0..1_500, &mut out);
        recorder.on_alert(&alert("b", 3, 1_000, 434.1e6), 0..1_500, &mut out);
        out.clear();
        recorder.end_all(&mut out);
        assert_eq!(out, [Step::End { id: 0 }, Step::End { id: 1 }]);
        assert!(!recorder.is_recording());
    }
}
