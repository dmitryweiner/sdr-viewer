//! The alerts the rules gave, for the status line: the rule that gave
//! the latest, how many it gave, and whether a tone is due.

use std::time::Duration;

use sdr_detect::Alert;

/// An alert is told brightly for this long, then dimly.
pub const BRIGHT: Duration = Duration::from_secs(10);

/// A rule sounds the tone at most this often: a rule over a hopper's
/// band would be met 80 times a second.
pub const TONE_EVERY: Duration = Duration::from_secs(10);

/// A rule's name on the status line is cut to this many characters.
const NAME_WIDTH: usize = 20;

/// A rule that has given alerts in this run.
#[derive(Debug, Clone, PartialEq)]
struct Given {
    name: String,
    count: usize,
    /// When its tone last sounded, in stream time.
    toned: Option<Duration>,
}

/// What the status line tells of the alerts.
#[derive(Debug, Clone, PartialEq)]
pub struct Told {
    /// "! gate ×3", or "! gate ×3 of 5" when other rules gave some too.
    pub text: String,
    /// Within [`BRIGHT`] of the latest alert.
    pub bright: bool,
}

/// The alerts of this run, by rule, in stream time.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AlertsView {
    given: Vec<Given>,
    /// The latest alert's rule (an index into `given`) and its time.
    latest: Option<(usize, Duration)>,
    total: usize,
    /// The stream's time as of the latest frame.
    now: Duration,
}

impl AlertsView {
    /// The stream's time, as of the latest frame: what an alert's
    /// brightness is held against.
    pub fn set_now(&mut self, now: Duration) {
        self.now = now;
    }

    /// Takes alerts in, in the order they came, and says whether the tone
    /// is due: one of them is of a rule that has not sounded it in the
    /// last [`TONE_EVERY`]. A batch sounds it once at most.
    pub fn on_alerts(&mut self, alerts: &[Alert]) -> bool {
        let mut tone = false;
        for alert in alerts {
            let time = alert.time();
            let index = match self.given.iter().position(|given| given.name == alert.rule) {
                Some(index) => index,
                None => {
                    self.given.push(Given {
                        name: alert.rule.clone(),
                        count: 0,
                        toned: None,
                    });
                    self.given.len() - 1
                }
            };
            let given = &mut self.given[index];
            given.count += 1;
            self.total += 1;
            if given
                .toned
                .is_none_or(|toned| time.saturating_sub(toned) >= TONE_EVERY)
            {
                given.toned = Some(time);
                tone = true;
            }
            if self.latest.is_none_or(|(_, latest)| time >= latest) {
                self.latest = Some((index, time));
            }
        }
        tone
    }

    /// What the status line says, once there has been an alert.
    pub fn told(&self) -> Option<Told> {
        let (index, time) = self.latest?;
        let given = &self.given[index];
        let name: String = if given.name.chars().count() > NAME_WIDTH {
            let cut: String = given.name.chars().take(NAME_WIDTH - 1).collect();
            format!("{cut}…")
        } else {
            given.name.clone()
        };
        let mut text = format!("! {name} ×{}", given.count);
        if self.total > given.count {
            text += &format!(" of {}", self.total);
        }
        Some(Told {
            text,
            bright: self.now.saturating_sub(time) < BRIGHT,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdr_detect::Event;

    /// An alert of `rule` given at `secs` of the stream.
    fn alert(rule: &str, secs: f64) -> Alert {
        let start = Duration::from_secs_f64(secs - 0.5);
        Alert {
            rule: rule.into(),
            event: Event {
                start,
                duration: Duration::from_millis(500),
                low_hz: 433.9e6,
                high_hz: 433.91e6,
                peak_hz: 433.905e6,
                peak_dbfs: -40.0,
                above_floor_db: 30.0,
            },
        }
    }

    #[test]
    fn nothing_is_told_before_the_first_alert() {
        assert_eq!(AlertsView::default().told(), None);
    }

    #[test]
    fn the_latest_rule_is_told_with_its_count_and_all_the_alerts() {
        let mut view = AlertsView::default();
        view.on_alerts(&[alert("gate", 1.0), alert("gate", 2.0)]);
        view.set_now(Duration::from_secs(2));
        assert_eq!(
            view.told(),
            Some(Told {
                text: "! gate ×2".into(),
                bright: true
            })
        );
        view.on_alerts(&[alert("LPD 17", 3.0)]);
        assert_eq!(view.told().unwrap().text, "! LPD 17 ×1 of 3");
        view.on_alerts(&[alert("gate", 4.0)]);
        assert_eq!(view.told().unwrap().text, "! gate ×3 of 4");
    }

    #[test]
    fn an_alert_is_bright_for_ten_seconds_then_dim() {
        let mut view = AlertsView::default();
        view.on_alerts(&[alert("gate", 5.0)]);
        for (now, bright) in [(5.0, true), (14.9, true), (15.0, false), (100.0, false)] {
            view.set_now(Duration::from_secs_f64(now));
            assert_eq!(view.told().unwrap().bright, bright, "at {now} s");
        }
        // A new one is bright again.
        view.on_alerts(&[alert("gate", 100.0)]);
        assert!(view.told().unwrap().bright);
    }

    #[test]
    fn a_rule_sounds_the_tone_at_most_once_in_ten_seconds() {
        let mut view = AlertsView::default();
        let tones: Vec<bool> = [1.0, 1.1, 10.9, 11.0, 11.5, 30.0]
            .into_iter()
            .map(|secs| view.on_alerts(&[alert("hopper", secs)]))
            .collect();
        assert_eq!(tones, [true, false, false, true, false, true]);
        // Another rule has a tone of its own; a batch sounds once.
        assert!(view.on_alerts(&[alert("gate", 30.1), alert("LPD 17", 30.1)]));
        assert!(!view.on_alerts(&[alert("gate", 30.2), alert("hopper", 30.3)]));
        assert!(!view.on_alerts(&[]));
    }

    #[test]
    fn a_long_name_is_cut_to_fit_the_status_line() {
        let mut view = AlertsView::default();
        view.on_alerts(&[alert("the neighbour's weather station", 1.0)]);
        assert_eq!(view.told().unwrap().text, "! the neighbour's wea… ×1");
    }
}
