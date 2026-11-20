//! Alerts: the user's rules, which the events going on are held against
//! with every frame, and the alert a rule gives the moment an event meets
//! it — while the signal is still on.

use std::time::Duration;

use crate::detector::Event;

/// A rule's least level above the noise floor when it names none: the
/// level from which an event makes a channel, below which events are
/// mostly a bin of noise for a frame.
pub const DEFAULT_MIN_ABOVE_DB: f32 = crate::channel::MIN_ABOVE_DB;

/// What makes an alert: an event whose centre is in the band, that has
/// gone on for `min_duration` and whose peak stood `min_above_db` over
/// the noise floor.
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub name: String,
    pub low_hz: f64,
    pub high_hz: f64,
    pub min_duration: Duration,
    pub min_above_db: f32,
}

/// Bands whose edges are this close, in hertz, are one: a band read back
/// from a file is a hair off the one written.
const SAME_HZ: f64 = 1.0;

impl Rule {
    /// Whether `other` watches the same band, whatever else it asks.
    pub fn same_band(&self, other: &Rule) -> bool {
        (self.low_hz - other.low_hz).abs() < SAME_HZ
            && (self.high_hz - other.high_hz).abs() < SAME_HZ
    }

    pub fn is_met_by(&self, event: &Event) -> bool {
        let center = event.center_hz();
        self.low_hz <= center
            && center <= self.high_hz
            && event.duration >= self.min_duration
            && event.above_floor_db >= self.min_above_db
    }
}

/// A rule met by an event, the event as it was then.
#[derive(Debug, Clone, PartialEq)]
pub struct Alert {
    /// The rule's name.
    pub rule: String,
    pub event: Event,
}

impl Alert {
    /// When the rule was met, in stream time: the end of the event's
    /// latest frame.
    pub fn time(&self) -> Duration {
        self.event.start + self.event.duration
    }
}

/// The rules, and which events going on have given their alerts.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Alerts {
    rules: Vec<Rule>,
    /// (the event's number, the rule's index) of every alert given for
    /// an event still going on.
    given: Vec<(u64, usize)>,
}

impl Alerts {
    pub fn new(rules: Vec<Rule>) -> Self {
        Alerts {
            rules,
            given: Vec::new(),
        }
    }

    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// Holds the events against `rule` too, from the next check on.
    pub fn add(&mut self, rule: Rule) {
        self.rules.push(rule);
    }

    /// Holds the events going on, with their detector's numbers, against
    /// every rule: `alert` gets a rule's alert the first time an event
    /// meets it, once for the event's whole life.
    pub fn check(&mut self, open: &[(u64, Event)], mut alert: impl FnMut(Alert)) {
        // An event that has ended will not come again.
        let going_on = |given: &(u64, usize)| open.iter().any(|(number, _)| *number == given.0);
        self.given.retain(going_on);
        for (number, event) in open {
            for (index, rule) in self.rules.iter().enumerate() {
                let key = (*number, index);
                if rule.is_met_by(event) && !self.given.contains(&key) {
                    self.given.push(key);
                    alert(Alert {
                        rule: rule.name.clone(),
                        event: event.clone(),
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    /// A rule for 433.9–434.0 MHz, a second at least, 20 dB over the
    /// floor: the plan's example.
    fn rule() -> Rule {
        Rule {
            name: "keyfob".into(),
            low_hz: 433.9e6,
            high_hz: 434.0e6,
            min_duration: ms(1000),
            min_above_db: 20.0,
        }
    }

    /// 10 kHz wide around `center_hz`, from 1 s.
    fn event(center_hz: f64, duration: Duration, above_floor_db: f32) -> Event {
        Event {
            start: ms(1000),
            duration,
            low_hz: center_hz - 5e3,
            high_hz: center_hz + 5e3,
            peak_hz: center_hz,
            peak_dbfs: -60.0 + above_floor_db,
            above_floor_db,
        }
    }

    #[test]
    fn a_rule_is_met_by_an_event_centred_in_its_band_long_and_strong_enough() {
        let rule = rule();
        assert!(rule.is_met_by(&event(433.95e6, ms(1000), 20.0)));
        // The edges of the band are in it.
        assert!(rule.is_met_by(&event(433.9e6, ms(5000), 30.0)));
        assert!(rule.is_met_by(&event(434.0e6, ms(5000), 30.0)));
        // The centre decides, not the edges of the event.
        assert!(!rule.is_met_by(&event(433.899e6, ms(5000), 30.0)));
        assert!(!rule.is_met_by(&event(434.001e6, ms(5000), 30.0)));
        assert!(!rule.is_met_by(&event(433.95e6, ms(990), 30.0)));
        assert!(!rule.is_met_by(&event(433.95e6, ms(5000), 19.9)));
    }

    #[test]
    fn a_band_a_hair_off_is_the_same_band() {
        let near = Rule {
            name: "another".into(),
            low_hz: 433.9e6 + 0.5,
            high_hz: 434.0e6 - 0.5,
            min_duration: Duration::ZERO,
            min_above_db: 0.0,
        };
        assert!(rule().same_band(&near));
        let wider = Rule {
            high_hz: 434.0e6 + 1.0,
            ..near
        };
        assert!(!rule().same_band(&wider));
    }

    #[test]
    fn an_event_gives_its_alert_the_moment_it_meets_the_rule_and_only_once() {
        let mut alerts = Alerts::new(vec![rule()]);
        let mut given = Vec::new();
        // The event grows frame by frame; the rule is met at 1 s.
        for duration in [500, 990, 1000, 1010, 3000] {
            let open = [(7, event(433.95e6, ms(duration), 25.0))];
            alerts.check(&open, |alert| given.push(alert));
        }
        assert_eq!(
            given,
            [Alert {
                rule: "keyfob".into(),
                event: event(433.95e6, ms(1000), 25.0),
            }]
        );
        assert_eq!(given[0].time(), ms(2000));
    }

    #[test]
    fn every_event_meeting_a_rule_gives_an_alert_of_its_own() {
        let mut alerts = Alerts::new(vec![rule()]);
        let mut given = Vec::new();
        let long = |center_hz| event(center_hz, ms(1500), 25.0);
        // Two at once, one out of the band; the first ends, a new one
        // comes.
        let frames = [
            vec![(1, long(433.92e6)), (2, long(433.98e6)), (3, long(433.5e6))],
            vec![(1, long(433.92e6)), (2, long(433.98e6))],
            vec![(2, long(433.98e6))],
            vec![(2, long(433.98e6)), (4, long(433.92e6))],
        ];
        for open in frames {
            alerts.check(&open, |alert| given.push(alert.event.center_hz()));
        }
        assert_eq!(given, [433.92e6, 433.98e6, 433.92e6]);
    }

    #[test]
    fn an_event_meeting_two_rules_gives_both_alerts() {
        let wide = Rule {
            name: "ISM".into(),
            low_hz: 433.05e6,
            high_hz: 434.79e6,
            min_duration: Duration::ZERO,
            min_above_db: DEFAULT_MIN_ABOVE_DB,
        };
        let mut alerts = Alerts::new(vec![wide, rule()]);
        let mut given = Vec::new();
        for duration in [10, 1200] {
            let open = [(1, event(433.95e6, ms(duration), 25.0))];
            alerts.check(&open, |alert| {
                given.push((alert.rule, alert.event.duration));
            });
        }
        assert_eq!(
            given,
            [
                ("ISM".to_string(), ms(10)),
                ("keyfob".to_string(), ms(1200))
            ]
        );
    }

    #[test]
    fn no_rules_give_no_alerts() {
        let mut alerts = Alerts::default();
        let open = [(1, event(433.95e6, ms(1500), 25.0))];
        alerts.check(&open, |alert| panic!("{alert:?}"));
        assert!(alerts.rules().is_empty());
        // A rule added holds from then on, for the events already going
        // on too.
        alerts.add(rule());
        let mut given = 0;
        alerts.check(&open, |_| given += 1);
        assert_eq!((alerts.rules(), given), (&[rule()][..], 1));
    }
}
