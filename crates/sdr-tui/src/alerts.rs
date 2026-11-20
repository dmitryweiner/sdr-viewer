//! The alerts: the rules there are, what `!` and `R` changed of them for
//! the file, and what the rules gave, for the status line — the rule that gave the
//! latest, how many it gave, and whether a tone is due.

use std::time::Duration;

use sdr_detect::{Alert, Rule};

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

/// A change of the rules made here, for the alerts file.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// A rule made with `!`: add it.
    Made(Rule),
    /// `!` again: remove the rules of this one's band.
    Removed(Rule),
    /// `R`: the rules of this one's band record as it does, or not.
    Record(Rule),
}

/// What the status line tells of the alerts.
#[derive(Debug, Clone, PartialEq)]
pub struct Told {
    /// "! gate ×3", or "! gate ×3 of 5" when other rules gave some too.
    pub text: String,
    /// Within [`BRIGHT`] of the latest alert.
    pub bright: bool,
}

/// The rules, and the alerts of this run by rule, in stream time.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AlertsView {
    rules: Vec<Rule>,
    /// A file keeps the rules made here.
    kept: bool,
    /// Made here and not yet handed to the file.
    changes: Vec<Change>,
    given: Vec<Given>,
    /// The latest alert's rule (an index into `given`) and its time.
    latest: Option<(usize, Duration)>,
    total: usize,
    /// The stream's time as of the latest frame.
    now: Duration,
    /// The alerts' recordings have reached their limit.
    full: bool,
}

impl AlertsView {
    /// The rules the app started with; `kept` when a file keeps those
    /// made in it.
    pub fn new(rules: Vec<Rule>, kept: bool) -> Self {
        AlertsView {
            rules,
            kept,
            ..AlertsView::default()
        }
    }

    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// The rule of a channel's band: `width_hz` around `center_hz`.
    pub fn rule_on(&self, center_hz: f64, width_hz: f64) -> Option<&Rule> {
        let (low_hz, high_hz) = (center_hz - width_hz / 2.0, center_hz + width_hz / 2.0);
        self.rules
            .iter()
            .find(|rule| rule.has_band(low_hz, high_hz))
    }

    /// How a change is told: kept in the file, or for this run alone.
    fn as_kept(&self, what: &str, in_file: &str) -> String {
        match self.kept {
            true => format!("{what}{in_file}"),
            false => format!("{what} for this run: there is no alerts file"),
        }
    }

    /// Takes in a rule made here, and says so; one for a band that has a
    /// rule already is refused, naming that rule.
    pub fn add(&mut self, rule: Rule) -> Result<String, String> {
        if let Some(there) = self.rules.iter().find(|there| there.same_band(&rule)) {
            return Err(format!(
                "an alert watches that band already: {}",
                there.name
            ));
        }
        let told = self.as_kept(
            &format!("alert on {}", rule.name),
            " added to the alerts file",
        );
        if self.kept {
            self.changes.push(Change::Made(rule.clone()));
        }
        self.rules.push(rule);
        Ok(told)
    }

    /// Lets go of the rules of `band`'s band, and says so by the name of
    /// the first.
    pub fn remove(&mut self, band: &Rule) -> String {
        let name = self.name_on(band);
        self.rules.retain(|rule| !rule.same_band(band));
        if self.kept {
            self.changes.push(Change::Removed(band.clone()));
        }
        self.as_kept(&format!("alert on {name} removed"), " from the alerts file")
    }

    /// Makes the rules of `band`'s band record as it does, or not, and
    /// says so.
    pub fn set_record(&mut self, band: &Rule) -> String {
        let name = self.name_on(band);
        for rule in self.rules.iter_mut().filter(|rule| rule.same_band(band)) {
            rule.record = band.record;
        }
        if self.kept {
            self.changes.push(Change::Record(band.clone()));
        }
        match band.record {
            true => self.as_kept(
                &format!("alert on {name} records"),
                ": record = true in the alerts file",
            ),
            false => self.as_kept(
                &format!("alert on {name} records no more"),
                ": record = false in the alerts file",
            ),
        }
    }

    /// The name of the first rule of `band`'s band, or its own.
    fn name_on(&self, band: &Rule) -> String {
        let rule = self.rules.iter().find(|rule| rule.same_band(band));
        rule.unwrap_or(band).name.clone()
    }

    /// The alerts' recordings have reached their limit: told from now on.
    pub fn set_full(&mut self) {
        self.full = true;
    }

    /// The changes made since the last call, in order: for the file.
    pub fn take_changes(&mut self) -> Vec<Change> {
        std::mem::take(&mut self.changes)
    }

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
        let full = match self.full {
            true => " · recordings full",
            false => "",
        };
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
        text += full;
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
            record: false,
            band: (433.9e6, 434.0e6),
            number: 1,
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

    fn rule(name: &str, low_hz: f64) -> Rule {
        Rule {
            name: name.into(),
            low_hz,
            high_hz: low_hz + 12.5e3,
            min_duration: Duration::ZERO,
            min_above_db: 10.0,
            record: false,
        }
    }

    #[test]
    fn a_rule_made_here_goes_to_the_file_unless_its_band_has_one() {
        let mut view = AlertsView::new(vec![rule("gate", 433.9e6)], true);
        assert_eq!(
            view.add(rule("LPD 17", 433.46875e6)),
            Ok("alert on LPD 17 added to the alerts file".into())
        );
        assert_eq!(
            view.add(rule("433.906 MHz", 433.9e6)),
            Err("an alert watches that band already: gate".into())
        );
        assert_eq!(
            view.add(rule("again", 433.46875e6)),
            Err("an alert watches that band already: LPD 17".into())
        );
        assert_eq!(view.rules().len(), 2);
        assert_eq!(
            view.take_changes(),
            [Change::Made(rule("LPD 17", 433.46875e6))]
        );
        assert!(view.take_changes().is_empty());
    }

    #[test]
    fn a_band_s_rule_is_found_by_the_channel_s_centre_and_width() {
        let view = AlertsView::new(vec![rule("LPD 17", 433.46875e6)], true);
        let found = view.rule_on(433.475e6, 12.5e3);
        assert_eq!(found.map(|rule| rule.name.as_str()), Some("LPD 17"));
        assert_eq!(view.rule_on(433.475e6, 25e3), None);
        assert_eq!(view.rule_on(433.5e6, 12.5e3), None);
    }

    #[test]
    fn a_band_s_rules_record_and_go_and_the_file_gets_it_all_in_order() {
        let mut view = AlertsView::new(vec![rule("gate", 433.9e6)], true);
        view.add(rule("LPD 17", 433.46875e6)).unwrap();
        let recording = Rule {
            record: true,
            ..rule("LPD 17", 433.46875e6)
        };
        assert_eq!(
            view.set_record(&recording),
            "alert on LPD 17 records: record = true in the alerts file"
        );
        assert!(view.rule_on(433.475e6, 12.5e3).unwrap().record);
        let quiet = rule("LPD 17", 433.46875e6);
        assert_eq!(
            view.set_record(&quiet),
            "alert on LPD 17 records no more: record = false in the alerts file"
        );
        // By its band, under whatever name.
        assert_eq!(
            view.remove(&rule("433.475 MHz", 433.46875e6)),
            "alert on LPD 17 removed from the alerts file"
        );
        assert_eq!(view.rule_on(433.475e6, 12.5e3), None);
        assert_eq!(view.rules(), [rule("gate", 433.9e6)]);
        assert_eq!(
            view.take_changes(),
            [
                Change::Made(quiet.clone()),
                Change::Record(recording),
                Change::Record(quiet.clone()),
                Change::Removed(rule("433.475 MHz", 433.46875e6)),
            ]
        );
    }

    #[test]
    fn without_a_file_a_band_s_rules_record_and_go_for_this_run() {
        let mut view = AlertsView::new(vec![rule("gate", 433.9e6)], false);
        let recording = Rule {
            record: true,
            ..rule("gate", 433.9e6)
        };
        assert_eq!(
            view.set_record(&recording),
            "alert on gate records for this run: there is no alerts file"
        );
        assert_eq!(
            view.remove(&recording),
            "alert on gate removed for this run: there is no alerts file"
        );
        assert!(view.rules().is_empty());
        assert!(view.take_changes().is_empty());
    }

    #[test]
    fn without_a_file_a_rule_made_here_holds_for_this_run() {
        let mut view = AlertsView::default();
        assert_eq!(
            view.add(rule("LPD 17", 433.46875e6)),
            Ok("alert on LPD 17 for this run: there is no alerts file".into())
        );
        assert_eq!(view.rules().len(), 1);
        assert!(view.take_changes().is_empty());
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
    fn recordings_that_reached_their_limit_are_told_with_the_alert() {
        let mut view = AlertsView::default();
        view.on_alerts(&[alert("gate", 1.0)]);
        view.set_full();
        assert_eq!(view.told().unwrap().text, "! gate ×1 · recordings full");
    }

    #[test]
    fn a_long_name_is_cut_to_fit_the_status_line() {
        let mut view = AlertsView::default();
        view.on_alerts(&[alert("the neighbour's weather station", 1.0)]);
        assert_eq!(view.told().unwrap().text, "! the neighbour's wea… ×1");
    }
}
