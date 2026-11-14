//! Channels: the pieces of the band events are sorted into. A channel is
//! the user's, from their file, or made where an event had none.

use crate::detector::Event;

#[derive(Debug, Clone, PartialEq)]
pub struct Channel {
    /// The user's name for it; without one it goes by its frequency.
    pub name: Option<String>,
    pub center_hz: f64,
    pub width_hz: f64,
    /// Not shown, and no channel is made in its place: for what the user
    /// has seen enough of.
    pub hidden: bool,
}

impl Channel {
    pub fn contains(&self, hz: f64) -> bool {
        (hz - self.center_hz).abs() <= self.width_hz / 2.0
    }
}

/// An event this far above the noise makes a channel where there is
/// none: the level from which the log shows events by default. Weaker
/// ones are mostly one bin of noise for one frame.
pub const MIN_ABOVE_DB: f32 = 10.0;

/// The narrowest channel made for an event: a carrier is a bin or two
/// wide, and the next burst of the same transmitter is never exactly
/// there.
pub const MIN_WIDTH_HZ: f64 = 10_000.0;

/// Channels made for events, at most: a band of noise bursts must not
/// grow the list for ever.
pub const MAX_MADE: usize = 500;

/// The channels in the order they came: first the user's file, then
/// those made here.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Channels {
    list: Vec<Channel>,
    made: usize,
}

impl Channels {
    pub fn new(list: Vec<Channel>) -> Self {
        Channels { list, made: 0 }
    }

    pub fn list(&self) -> &[Channel] {
        &self.list
    }

    /// The channel `hz` is in; of several, the one whose centre is
    /// nearest.
    pub fn find(&self, hz: f64) -> Option<usize> {
        let distance = |index: &usize| (self.list[*index].center_hz - hz).abs();
        (0..self.list.len())
            .filter(|&index| self.list[index].contains(hz))
            .min_by(|a, b| distance(a).total_cmp(&distance(b)))
    }

    /// The channel of `event`, made now if it has none and is strong
    /// enough; the second part says whether it was made.
    pub fn admit(&mut self, event: &Event) -> Option<(usize, bool)> {
        let center = event.center_hz();
        if let Some(index) = self.find(center) {
            return Some((index, false));
        }
        if event.above_floor_db < MIN_ABOVE_DB || self.made >= MAX_MADE {
            return None;
        }
        self.made += 1;
        self.list.push(Channel {
            name: None,
            // Whole kilohertz and tenths of one: what a person would
            // write into the file.
            center_hz: (center / 1e3).round() * 1e3,
            width_hz: (event.width_hz().max(MIN_WIDTH_HZ) / 100.0).round() * 100.0,
            hidden: false,
        });
        Some((self.list.len() - 1, true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn event(center_hz: f64, width_hz: f64, above_floor_db: f32) -> Event {
        Event {
            start: Duration::ZERO,
            duration: Duration::from_millis(40),
            low_hz: center_hz - width_hz / 2.0,
            high_hz: center_hz + width_hz / 2.0,
            peak_hz: center_hz,
            peak_dbfs: -40.0,
            above_floor_db,
        }
    }

    fn named(name: &str, center_hz: f64, width_hz: f64) -> Channel {
        Channel {
            name: Some(name.into()),
            center_hz,
            width_hz,
            hidden: false,
        }
    }

    #[test]
    fn an_event_goes_to_the_channel_its_centre_is_in() {
        let mut channels = Channels::new(vec![
            named("LPD 1", 433.075e6, 25e3),
            named("LPD 2", 433.100e6, 25e3),
        ]);
        assert_eq!(
            channels.admit(&event(433.080e6, 9e3, 20.0)),
            Some((0, false))
        );
        assert_eq!(
            channels.admit(&event(433.105e6, 9e3, 20.0)),
            Some((1, false))
        );
        // On the edge both hold it; the nearer centre wins.
        assert_eq!(channels.find(433.0876e6), Some(1));
        assert_eq!(channels.find(433.0874e6), Some(0));
        assert_eq!(channels.find(433.2e6), None);
        // A weak event is counted where there is a channel.
        assert_eq!(
            channels.admit(&event(433.075e6, 1e3, 8.5)),
            Some((0, false))
        );
        assert_eq!(channels.list().len(), 2);
    }

    #[test]
    fn a_strong_event_without_a_channel_makes_one() {
        let mut channels = Channels::default();
        assert_eq!(channels.admit(&event(433.4752e6, 9_375.0, 9.9)), None);
        assert_eq!(
            channels.admit(&event(433.4752e6, 9_375.0, 15.0)),
            Some((0, true))
        );
        assert_eq!(
            channels.list(),
            [Channel {
                name: None,
                center_hz: 433.475e6,
                width_hz: 10_000.0,
                hidden: false,
            }]
        );
        // The next burst, a little off, is the same channel.
        assert_eq!(
            channels.admit(&event(433.4775e6, 9_375.0, 15.0)),
            Some((0, false))
        );
        // The next channel of the grid is another.
        assert_eq!(
            channels.admit(&event(433.5e6, 9_375.0, 15.0)),
            Some((1, true))
        );
        // A wide signal gets a channel of its width.
        assert_eq!(
            channels.admit(&event(434.5e6, 180_468.75, 30.0)),
            Some((2, true))
        );
        assert_eq!(channels.list()[2].width_hz, 180_500.0);
    }

    #[test]
    fn a_hidden_channel_takes_its_events_and_nothing_is_made_there() {
        let mut channels = Channels::new(vec![Channel {
            hidden: true,
            ..named("spike", 433.92e6, 10e3)
        }]);
        assert_eq!(
            channels.admit(&event(433.921e6, 2e3, 40.0)),
            Some((0, false))
        );
        assert_eq!(channels.list().len(), 1);
    }

    #[test]
    fn only_so_many_channels_are_made() {
        let mut channels = Channels::new(vec![named("mine", 100e6, 10e3)]);
        for i in 0..MAX_MADE {
            let made = channels.admit(&event(200e6 + i as f64 * 20e3, 9e3, 20.0));
            assert_eq!(made, Some((i + 1, true)));
        }
        assert_eq!(channels.admit(&event(300e6, 9e3, 20.0)), None);
        // Those there keep taking events.
        assert_eq!(channels.admit(&event(200e6, 9e3, 20.0)), Some((1, false)));
    }
}
