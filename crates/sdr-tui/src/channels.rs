//! The channel strip's state: the channels, how busy each was in every
//! second of the last hours, and the selection. Fed with the detector's
//! events; all times are stream time.

use std::collections::VecDeque;
use std::ops::Range;
use std::time::Duration;

use sdr_detect::{Channel, Channels, Event};

/// Seconds of occupancy kept per channel: what the coarsest scale shows
/// on the widest screen.
pub const HISTORY_S: u64 = 4 * 3600;

/// Seconds a column of the strip can stand for.
pub const SCALES: [u64; 4] = [1, 10, 60, 300];

/// An event still going on makes a channel once it has lasted this long:
/// by then its band is what it will be. A carrier that never ends would
/// otherwise never get one.
pub const OPEN_LONG_ENOUGH: Duration = Duration::from_secs(1);

/// How busy one channel was, second by second.
#[derive(Debug, Clone, Default)]
struct Occupancy {
    /// Milliseconds of signal in each second, the oldest first.
    busy_ms: VecDeque<u16>,
    /// The second of the stream the first of them is.
    first: u64,
    /// When its latest event ended.
    last_end: Option<Duration>,
}

impl Occupancy {
    fn add(&mut self, start: Duration, end: Duration) {
        self.last_end = Some(self.last_end.map_or(end, |last| last.max(end)));
        let (start_ms, end_ms) = (start.as_millis() as u64, end.as_millis() as u64);
        if self.busy_ms.is_empty() {
            self.first = start_ms / 1000;
        }
        let last_second = end_ms.saturating_sub(1) / 1000;
        while self.first + (self.busy_ms.len() as u64) <= last_second {
            self.busy_ms.push_back(0);
        }
        while self.busy_ms.len() as u64 > HISTORY_S {
            self.busy_ms.pop_front();
            self.first += 1;
        }
        for second in (start_ms / 1000).max(self.first)..=last_second {
            let from = start_ms.max(second * 1000);
            let to = end_ms.min((second + 1) * 1000);
            let slot = &mut self.busy_ms[(second - self.first) as usize];
            // Two events of one channel may overlap; a second is no
            // busier than full.
            *slot = (u64::from(*slot) + to.saturating_sub(from)).min(1000) as u16;
        }
    }

    /// Milliseconds of signal in these seconds of the stream.
    fn busy_ms(&self, seconds: Range<u64>) -> u64 {
        let from = seconds.start.max(self.first) - self.first;
        let to = (seconds.end.max(self.first) - self.first).min(self.busy_ms.len() as u64);
        (from..to)
            .map(|slot| u64::from(self.busy_ms[slot as usize]))
            .sum()
    }
}

/// What the file of channels is to be told.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// A channel was made here: add it.
    Made(Channel),
    /// The channels made by the app are let go: remove them.
    ForgetMade,
}

/// A receiver's band: where its own spike is.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Band {
    center_hz: f64,
    sample_rate: u32,
    /// The stream time of the first frame seen of it.
    since: Duration,
}

/// A row of the strip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Row<'a> {
    pub channel: &'a Channel,
    /// Its number among all channels: what the other questions take.
    pub index: usize,
}

#[derive(Default)]
pub struct ChannelsView {
    channels: Channels,
    /// One per channel, in the channels' order.
    history: Vec<Occupancy>,
    /// The events going on now: the channel and when it began.
    open: Vec<(usize, Duration)>,
    /// The stream time of the latest frame: the right edge of the strip.
    now: Duration,
    scale: usize,
    /// The selected channel, by its number.
    selected: Option<usize>,
    /// What has changed here and is not yet handed over to the file.
    changes: Vec<Change>,
    /// The band being received, and the one before it: events come a
    /// moment after the frames, the last of a band after a retune.
    band: Option<Band>,
    band_before: Option<Band>,
}

impl ChannelsView {
    /// Starts from the channels of the user's file.
    pub fn new(channels: Vec<Channel>) -> Self {
        ChannelsView {
            history: vec![Occupancy::default(); channels.len()],
            channels: Channels::new(channels),
            ..ChannelsView::default()
        }
    }

    fn admit(&mut self, event: &Event) -> Option<usize> {
        let (index, made) = self.channels.admit(event)?;
        if made {
            self.history.push(Occupancy::default());
            let made = self.channels.list()[index].clone();
            self.changes.push(Change::Made(made));
        }
        Some(index)
    }

    /// Whether `event` is the receiver's own spike: a few bins wide,
    /// with its peak on the centre of the band it was received in. An
    /// RTL-SDR has one always (3.5 kHz wide, 15 dB over the noise on the
    /// user's device); it is not a signal on the air, and after `Enter`
    /// on a channel it sits exactly on that channel. The price: a real
    /// carrier this narrow, exactly on the centre, is not counted either.
    fn is_spike(&self, event: &Event) -> bool {
        let band = match (self.band, self.band_before) {
            (Some(band), _) if event.start >= band.since => band,
            (Some(_), Some(before)) => before,
            _ => return false,
        };
        let rate = f64::from(band.sample_rate);
        (event.peak_hz - band.center_hz).abs() <= rate / 1000.0 && event.width_hz() <= rate / 400.0
    }

    /// Takes the band and the stream time of a frame.
    pub fn on_frame(&mut self, time: Duration, center_hz: f64, sample_rate: u32) {
        let same = |band: &Band| band.center_hz == center_hz && band.sample_rate == sample_rate;
        if !self.band.as_ref().is_some_and(same) {
            self.band_before = self.band;
            self.band = Some(Band {
                center_hz,
                sample_rate,
                since: time,
            });
        }
        self.set_now(time);
    }

    /// Lets go of the channels made here — those without a name that are
    /// not hidden — with what was counted on them; the user's stay. Says
    /// how many went.
    pub fn forget_made(&mut self) -> usize {
        let list = self.channels.list().to_vec();
        let mine = |channel: &Channel| channel.name.is_some() || channel.hidden;
        let history = std::mem::take(&mut self.history);
        let kept = list
            .iter()
            .zip(history)
            .filter(|(channel, _)| mine(channel));
        let (channels, history): (Vec<Channel>, Vec<Occupancy>) = kept
            .map(|(channel, history)| (channel.clone(), history))
            .unzip();
        let gone = list.len() - channels.len();
        self.channels = Channels::new(channels);
        self.history = history;
        // The numbers have changed; the next word on what is on now
        // comes within half a second.
        self.open.clear();
        self.selected = None;
        self.changes
            .retain(|change| !matches!(change, Change::Made(_)));
        self.changes.push(Change::ForgetMade);
        gone
    }

    /// Counts events that have ended. The stream has come at least as
    /// far as their ends.
    pub fn on_events(&mut self, events: &[Event]) {
        for event in events {
            self.set_now(event.start + event.duration);
            if self.is_spike(event) {
                continue;
            }
            let Some(index) = self.admit(event) else {
                continue;
            };
            self.history[index].add(event.start, event.start + event.duration);
            // It is no longer going on, whatever the last word was.
            self.open.retain(|&open| open != (index, event.start));
        }
    }

    /// Takes the events going on now in place of those before.
    pub fn set_open(&mut self, open: &[Event]) {
        self.open.clear();
        for event in open {
            if self.is_spike(event) {
                continue;
            }
            let index = if event.duration >= OPEN_LONG_ENOUGH {
                self.admit(event)
            } else {
                self.channels.find(event.center_hz())
            };
            if let Some(index) = index {
                self.open.push((index, event.start));
            }
        }
    }

    /// The stream time the strip ends at: of the latest frame, or later
    /// if an event is known to have ended later. Never goes back.
    pub fn set_now(&mut self, now: Duration) {
        self.now = self.now.max(now);
    }

    /// What has changed since the last call: for the file.
    pub fn take_changes(&mut self) -> Vec<Change> {
        std::mem::take(&mut self.changes)
    }

    /// How many channels would go if those made here were let go.
    pub fn made(&self) -> usize {
        let list = self.channels.list().iter();
        list.filter(|channel| channel.name.is_none() && !channel.hidden)
            .count()
    }

    /// The rows from the lowest frequency up; hidden channels have none.
    pub fn rows(&self) -> Vec<Row<'_>> {
        let list = self.channels.list().iter().enumerate();
        let mut rows: Vec<Row> = list
            .filter(|(_, channel)| !channel.hidden)
            .map(|(index, channel)| Row { channel, index })
            .collect();
        rows.sort_by(|a, b| a.channel.center_hz.total_cmp(&b.channel.center_hz));
        rows
    }

    pub fn seconds_per_column(&self) -> u64 {
        SCALES[self.scale]
    }

    /// Fewer seconds to a column.
    pub fn zoom_in(&mut self) {
        self.scale = self.scale.saturating_sub(1);
    }

    pub fn zoom_out(&mut self) {
        self.scale = (self.scale + 1).min(SCALES.len() - 1);
    }

    /// Milliseconds channel `index` was busy between two moments, the
    /// first a whole second: what has ended, and what is still going on.
    fn busy_ms(&self, index: usize, from_s: u64, to: Duration) -> u64 {
        let to_ms = to.as_millis() as u64;
        let ended = self.history[index].busy_ms(from_s..to_ms.div_ceil(1000));
        let open = self.open.iter().filter(|(of, _)| *of == index);
        let going: u64 = open
            .map(|(_, start)| to_ms.saturating_sub((start.as_millis() as u64).max(from_s * 1000)))
            .sum();
        ended + going
    }

    /// The column, counted from the stream's start, that the latest
    /// moment before now falls in: at a whole second the newest column
    /// is the one just filled, not an empty one.
    fn newest_column(&self) -> u64 {
        (self.now.as_millis() as u64).saturating_sub(1) / 1000 / self.seconds_per_column()
    }

    /// The share of each of `columns` columns channel `index` was busy,
    /// the oldest first and the one still filling last; nothing for a
    /// column from before the stream began. Columns begin at whole
    /// multiples of the scale, so a bar never changes once its column is
    /// full.
    pub fn cells(&self, index: usize, columns: usize) -> Vec<Option<f32>> {
        let scale = self.seconds_per_column();
        let newest = self.newest_column();
        (0..columns as u64)
            .map(|column| {
                let column = (newest + 1 + column).checked_sub(columns as u64)?;
                let from = column * scale;
                let to = Duration::from_secs(from + scale).min(self.now);
                let span_ms = (to.as_millis() as u64 - from * 1000).max(1);
                let busy = self.busy_ms(index, from, to);
                Some((busy as f32 / span_ms as f32).min(1.0))
            })
            .collect()
    }

    /// How far back `columns` columns reach from now, at most to the
    /// stream's start.
    pub fn window(&self, columns: usize) -> Duration {
        let scale = self.seconds_per_column();
        let first = (self.newest_column() + 1).saturating_sub(columns as u64) * scale;
        self.now.saturating_sub(Duration::from_secs(first))
    }

    /// The share of the last `columns` columns' time the channel was busy.
    pub fn share(&self, index: usize, columns: usize) -> f32 {
        let window = self.window(columns);
        let from = self.now.saturating_sub(window).as_secs();
        let busy = self.busy_ms(index, from, self.now);
        (busy as f32 / window.as_millis().max(1) as f32).min(1.0)
    }

    /// Whether a signal is on the channel now.
    pub fn active(&self, index: usize) -> bool {
        self.open.iter().any(|(of, _)| *of == index)
    }

    /// How long ago the channel's latest signal ended; nothing if it
    /// never had one.
    pub fn quiet_for(&self, index: usize) -> Option<Duration> {
        let end = self.history[index].last_end?;
        Some(self.now.saturating_sub(end))
    }

    /// The selected channel.
    pub fn selected(&self) -> Option<&Channel> {
        self.selected.map(|index| &self.channels.list()[index])
    }

    /// The row the selection is on.
    pub fn selected_row(&self) -> Option<usize> {
        let selected = self.selected?;
        self.rows().iter().position(|row| row.index == selected)
    }

    /// Moves the selection by rows; it appears on the first row.
    pub fn move_selection(&mut self, by: isize) {
        let rows = self.rows();
        let Some(last) = rows.len().checked_sub(1) else {
            return;
        };
        let row = match self.selected_row() {
            Some(row) => row.saturating_add_signed(by).min(last),
            None => 0,
        };
        self.selected = Some(rows[row].index);
    }

    pub fn release(&mut self) {
        self.selected = None;
    }
}

/// "45 s", "12 m", "3.5 h": a stretch of time at a glance.
pub fn format_ago(time: Duration) -> String {
    let seconds = time.as_secs();
    if seconds < 100 {
        format!("{seconds} s")
    } else if seconds < 6000 {
        format!("{} m", seconds / 60)
    } else {
        format!("{:.1} h", seconds as f64 / 3600.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(start_ms: u64, duration_ms: u64, center_hz: f64) -> Event {
        Event {
            start: Duration::from_millis(start_ms),
            duration: Duration::from_millis(duration_ms),
            low_hz: center_hz - 4_500.0,
            high_hz: center_hz + 4_500.0,
            peak_hz: center_hz,
            peak_dbfs: -40.0,
            above_floor_db: 20.0,
        }
    }

    fn secs(seconds: u64) -> Duration {
        Duration::from_secs(seconds)
    }

    fn shares(view: &ChannelsView, index: usize, columns: usize) -> Vec<Option<f32>> {
        let round = |share: f32| (share * 1000.0).round() / 1000.0;
        let cells = view.cells(index, columns).into_iter();
        cells.map(|cell| cell.map(round)).collect()
    }

    #[test]
    fn events_make_channels_and_rows_run_up_the_band() {
        let mut view = ChannelsView::new(vec![Channel {
            name: Some("mine".into()),
            center_hz: 433.3e6,
            width_hz: 25e3,
            hidden: false,
        }]);
        view.on_events(&[
            event(0, 40, 433.5e6),
            event(100, 40, 433.1e6),
            event(200, 40, 433.5e6),
            event(300, 40, 433.305e6),
        ]);
        let rows = view.rows();
        let centers: Vec<f64> = rows.iter().map(|row| row.channel.center_hz).collect();
        assert_eq!(centers, [433.1e6, 433.3e6, 433.5e6]);
        assert_eq!(rows[1].index, 0);
        assert_eq!(rows[1].channel.name.as_deref(), Some("mine"));
        // Only what was made here is to be saved, and once.
        let changes = view.take_changes();
        assert_eq!(changes.len(), 2);
        assert!(matches!(&changes[0], Change::Made(made) if made.center_hz == 433.5e6));
        assert!(view.take_changes().is_empty());
    }

    #[test]
    fn a_hidden_channel_has_no_row() {
        let mut view = ChannelsView::new(vec![Channel {
            name: None,
            center_hz: 433.92e6,
            width_hz: 10e3,
            hidden: true,
        }]);
        view.on_events(&[event(0, 5000, 433.92e6)]);
        assert!(view.rows().is_empty());
        assert!(view.take_changes().is_empty());
        view.move_selection(1);
        assert_eq!(view.selected(), None);
    }

    #[test]
    fn a_column_is_the_share_of_its_time_the_channel_was_busy() {
        let mut view = ChannelsView::default();
        // 40 ms in second 0; 1.5 s across seconds 2 and 3.
        view.on_events(&[event(500, 40, 433.5e6), event(2250, 1500, 433.5e6)]);
        view.set_now(secs(5));
        assert_eq!(
            shares(&view, 0, 8),
            [
                None,
                None,
                None,
                Some(0.04),
                Some(0.0),
                Some(0.75),
                Some(0.75),
                // Second 4, the latest.
                Some(0.0),
            ]
        );
        assert_eq!(view.window(8), secs(5));
        assert!((view.share(0, 8) - 1.54 / 5.0).abs() < 1e-3);
        assert_eq!(view.window(3), secs(3));
        assert!((view.share(0, 3) - 1.5 / 3.0).abs() < 1e-3);
        assert_eq!(view.quiet_for(0), Some(Duration::from_millis(1250)));
    }

    #[test]
    fn an_ended_event_moves_now_to_its_end() {
        let mut view = ChannelsView::default();
        view.set_now(secs(1));
        view.on_events(&[event(900, 400, 433.5e6)]);
        assert_eq!(view.window(10), Duration::from_millis(1300));
        assert_eq!(shares(&view, 0, 2), [Some(0.1), Some(1.0)]);
        view.set_now(secs(1));
        assert_eq!(view.window(10), Duration::from_millis(1300));
    }

    #[test]
    fn coarser_columns_begin_at_multiples_of_the_scale() {
        let mut view = ChannelsView::default();
        view.on_events(&[event(8_000, 4_000, 433.5e6)]);
        view.set_now(Duration::from_millis(25_000));
        view.zoom_out();
        assert_eq!(view.seconds_per_column(), 10);
        // 0–10 s, 10–20 s, and 5 s of 20–30 s.
        assert_eq!(shares(&view, 0, 4), [None, Some(0.2), Some(0.2), Some(0.0)]);
        assert_eq!(view.window(4), secs(25));
        assert_eq!(view.window(2), secs(15));
        for _ in 0..10 {
            view.zoom_out();
        }
        assert_eq!(view.seconds_per_column(), 300);
        for _ in 0..10 {
            view.zoom_in();
        }
        assert_eq!(view.seconds_per_column(), 1);
    }

    #[test]
    fn an_event_still_on_fills_its_channel_up_to_now() {
        let mut view = ChannelsView::default();
        view.on_events(&[event(0, 40, 433.5e6)]);
        view.set_now(Duration::from_millis(3500));
        assert!(!view.active(0));
        view.set_open(&[event(1500, 1900, 433.5e6)]);
        assert!(view.active(0));
        assert_eq!(
            shares(&view, 0, 4),
            [Some(0.04), Some(0.5), Some(1.0), Some(1.0)]
        );
        assert!((view.share(0, 4) - 2.04 / 3.5).abs() < 1e-3);
        // When it ends it is counted once.
        view.on_events(&[event(1500, 2000, 433.5e6)]);
        assert!(!view.active(0));
        view.set_now(secs(4));
        assert_eq!(shares(&view, 0, 2), [Some(1.0), Some(0.5)]);
        assert_eq!(view.quiet_for(0), Some(Duration::from_millis(500)));
    }

    #[test]
    fn an_event_that_lasts_makes_its_channel_before_it_ends() {
        let mut view = ChannelsView::default();
        view.set_now(secs(2));
        view.set_open(&[event(0, 900, 433.5e6)]);
        assert!(view.rows().is_empty());
        view.set_open(&[event(0, 1000, 433.5e6)]);
        assert_eq!(view.rows().len(), 1);
        assert_eq!(view.take_changes().len(), 1);
        assert_eq!(shares(&view, 0, 2), [Some(1.0), Some(1.0)]);
        assert_eq!(view.quiet_for(0), None);
    }

    #[test]
    fn only_the_last_hours_are_kept() {
        let mut view = ChannelsView::default();
        view.on_events(&[event(0, 500, 433.5e6)]);
        let late = (HISTORY_S + 100) * 1000;
        view.on_events(&[event(late, 500, 433.5e6)]);
        assert_eq!(view.history[0].busy_ms.len() as u64, HISTORY_S);
        assert_eq!(view.history[0].busy_ms(0..u64::MAX), 500);
        // An event from before what is kept changes nothing.
        view.on_events(&[event(10_000, 500, 433.5e6)]);
        assert_eq!(view.history[0].busy_ms(0..u64::MAX), 500);
    }

    #[test]
    fn overlapping_events_do_not_make_a_second_busier_than_full() {
        let mut view = ChannelsView::default();
        view.on_events(&[event(0, 1000, 433.5e6), event(200, 600, 433.502e6)]);
        view.set_now(secs(1));
        assert_eq!(shares(&view, 0, 2), [None, Some(1.0)]);
    }

    #[test]
    fn the_selection_stays_on_its_channel_when_rows_come_in_before_it() {
        let mut view = ChannelsView::default();
        view.move_selection(1);
        assert_eq!(view.selected_row(), None);
        view.on_events(&[event(0, 40, 433.5e6), event(0, 40, 433.7e6)]);
        view.move_selection(1);
        assert_eq!(view.selected_row(), Some(0));
        view.move_selection(5);
        assert_eq!(view.selected_row(), Some(1));
        view.on_events(&[event(0, 40, 433.1e6)]);
        assert_eq!(view.selected_row(), Some(2));
        assert_eq!(view.selected().unwrap().center_hz, 433.7e6);
        view.move_selection(isize::MIN);
        assert_eq!(view.selected().unwrap().center_hz, 433.1e6);
        view.release();
        assert_eq!(view.selected(), None);
    }

    /// The receiver's spike as the user's device has it: 3 bins on the
    /// centre.
    fn spike(start_ms: u64, duration_ms: u64, center_hz: f64) -> Event {
        Event {
            low_hz: center_hz - 1_758.0,
            high_hz: center_hz + 1_758.0,
            ..event(start_ms, duration_ms, center_hz)
        }
    }

    #[test]
    fn the_receivers_own_spike_is_not_a_signal() {
        let mut view = ChannelsView::default();
        view.on_frame(Duration::ZERO, 446.0e6, 2_400_000);
        view.on_events(&[spike(0, 500, 446.0e6)]);
        view.set_open(&[spike(600, 5000, 446.0e6)]);
        assert!(view.rows().is_empty());
        // A signal as wide as a voice there is one, and so is as narrow
        // a one anywhere else.
        view.on_events(&[event(1000, 500, 446.0e6), spike(1000, 500, 446.1e6)]);
        assert_eq!(view.rows().len(), 2);
        assert_eq!(view.share(0, 10), 0.5 / 1.5);
    }

    #[test]
    fn after_a_retune_the_spike_is_where_the_band_was_when_the_event_began() {
        let mut view = ChannelsView::new(vec![Channel {
            name: Some("mine".into()),
            center_hz: 446.1e6,
            width_hz: 12.5e3,
            hidden: false,
        }]);
        view.on_frame(Duration::ZERO, 446.0e6, 2_400_000);
        // Enter on the channel: from 2 s the spike is on it. The frame
        // of the new band comes before the old spike's end is told.
        view.on_frame(secs(2), 446.1e6, 2_400_000);
        view.on_events(&[spike(0, 2000, 446.0e6)]);
        view.set_open(&[spike(2000, 3000, 446.1e6)]);
        view.set_now(secs(5));
        assert_eq!(view.rows().len(), 1);
        assert!(!view.active(0));
        assert_eq!(view.share(0, 10), 0.0);
        // What was a spike is a signal when the centre has moved away...
        view.on_events(&[spike(4000, 500, 446.0e6)]);
        assert_eq!(view.rows().len(), 2);
        // ...and before any frame nothing is known of a centre.
        let mut early = ChannelsView::default();
        early.on_events(&[spike(0, 500, 446.0e6)]);
        assert_eq!(early.rows().len(), 1);
    }

    #[test]
    fn letting_go_of_the_made_channels_keeps_the_users() {
        let mine = |name: Option<&str>, center_hz: f64, hidden: bool| Channel {
            name: name.map(String::from),
            center_hz,
            width_hz: 12.5e3,
            hidden,
        };
        let mut view = ChannelsView::new(vec![
            // Without a name and not hidden: as good as made here.
            mine(None, 433.2e6, false),
            mine(Some("gate"), 433.5e6, false),
            mine(None, 433.92e6, true),
        ]);
        view.on_events(&[
            event(0, 500, 433.5e6),
            event(0, 500, 433.7e6),
            event(0, 500, 433.92e6),
        ]);
        view.set_open(&[event(600, 2000, 433.7e6)]);
        view.set_now(secs(1));
        view.move_selection(isize::MAX);
        assert_eq!(view.take_changes().len(), 1);
        view.on_events(&[event(0, 500, 433.9e6)]);
        assert_eq!(view.made(), 3);
        assert_eq!(view.forget_made(), 3);
        // What was not yet in the file never gets there.
        assert_eq!(view.take_changes(), [Change::ForgetMade]);
        let rows = view.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].channel.name.as_deref(), Some("gate"));
        // The gate keeps what was counted on it; nothing is selected.
        assert_eq!(view.share(rows[0].index, 10), 0.5);
        assert_eq!(view.selected(), None);
        assert_eq!(view.made(), 0);
        // The hidden one still takes its events; a signal makes its
        // channel again.
        view.on_events(&[event(1000, 500, 433.92e6), event(1000, 500, 433.7e6)]);
        assert_eq!(view.rows().len(), 2);
        assert_eq!(view.made(), 1);
    }

    #[test]
    fn a_stretch_of_time_at_a_glance() {
        assert_eq!(format_ago(Duration::from_millis(900)), "0 s");
        assert_eq!(format_ago(secs(99)), "99 s");
        assert_eq!(format_ago(secs(100)), "1 m");
        assert_eq!(format_ago(secs(5999)), "99 m");
        assert_eq!(format_ago(secs(6300)), "1.8 h");
    }
}
