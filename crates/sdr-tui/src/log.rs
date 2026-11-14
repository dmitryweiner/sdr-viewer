//! The signal log's state: the detector's events, filtered and sorted.

use std::cmp::Reverse;
use std::collections::VecDeque;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sdr_detect::Event;

/// Ended events kept in memory; older ones leave the list.
pub const MAX_EVENTS: usize = 10_000;

/// The level filter as the log starts, and how far the keys move it.
pub const DEFAULT_MIN_ABOVE_DB: f32 = 10.0;
const MIN_ABOVE_STEP_DB: f32 = 1.0;
const MIN_ABOVE_MAX_DB: f32 = 60.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    /// The newest first.
    Time,
    /// The strongest first.
    Level,
    /// The longest first.
    Duration,
    /// The lowest first.
    Frequency,
}

impl Sort {
    pub fn name(self) -> &'static str {
        match self {
            Sort::Time => "time",
            Sort::Level => "level",
            Sort::Duration => "duration",
            Sort::Frequency => "frequency",
        }
    }

    fn next(self) -> Sort {
        match self {
            Sort::Time => Sort::Level,
            Sort::Level => Sort::Duration,
            Sort::Duration => Sort::Frequency,
            Sort::Frequency => Sort::Time,
        }
    }
}

/// One line of the log.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Row<'a> {
    pub event: &'a Event,
    /// When it began by the wall clock, if the log knows the time.
    pub at: Option<SystemTime>,
    /// Still on the air: its duration is so far.
    pub on: bool,
    /// Found by this run, not read from the database: only such an event
    /// can still have its samples.
    pub this_run: bool,
}

/// What ties this run's stream to the wall clock.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WallClock {
    /// The moment of the stream's time zero.
    pub stream_start: SystemTime,
    /// Local time minus UTC, in seconds: times are shown as local.
    pub utc_offset_s: i32,
}

/// An event and when it began by the wall clock.
#[derive(Debug, Clone, PartialEq)]
struct Logged {
    event: Event,
    at: Option<SystemTime>,
    this_run: bool,
}

pub struct LogView {
    ended: VecDeque<Logged>,
    open: Vec<Logged>,
    /// Without it the log shows time in the stream: right for a recording.
    clock: Option<WallClock>,
    /// Events weaker than this above the noise floor are hidden, not
    /// forgotten: lowering it brings them back.
    min_above_db: f32,
    sort: Sort,
    /// The rows as they were when one was selected, and which: a list
    /// that moves cannot be read.
    held: Option<(Vec<(Logged, bool)>, usize)>,
    /// Events the filter passes that ended while the list was held.
    arrived: usize,
}

impl Default for LogView {
    fn default() -> Self {
        LogView {
            ended: VecDeque::new(),
            open: Vec::new(),
            clock: None,
            min_above_db: DEFAULT_MIN_ABOVE_DB,
            sort: Sort::Time,
            held: None,
            arrived: 0,
        }
    }
}

impl LogView {
    /// Gives the log the wall clock: events of this run are shown, and
    /// ordered among those of earlier runs, by the time of day.
    pub fn set_clock(&mut self, clock: WallClock) {
        self.clock = Some(clock);
    }

    pub fn clock(&self) -> Option<WallClock> {
        self.clock
    }

    /// Puts events of earlier runs, each with when it began, before those
    /// of this run.
    pub fn load_history(&mut self, history: impl IntoIterator<Item = (SystemTime, Event)>) {
        let mut history: Vec<Logged> = history
            .into_iter()
            .map(|(at, event)| Logged {
                event,
                at: Some(at),
                this_run: false,
            })
            .collect();
        history.sort_by_key(|logged| Reverse(logged.at));
        for logged in history {
            self.ended.push_front(logged);
        }
        self.trim();
    }

    /// Takes in events that have ended.
    pub fn on_events(&mut self, events: impl IntoIterator<Item = Event>) {
        for event in events {
            if self.held.is_some() && self.passes(&event) {
                self.arrived += 1;
            }
            let logged = self.stamp(event);
            self.ended.push_back(logged);
        }
        self.trim();
    }

    /// Forgets every event that has ended — this run's and the earlier
    /// ones' — and lets the list go; those still on stay. Says how many
    /// went.
    pub fn clear(&mut self) -> usize {
        let gone = self.ended.len();
        self.ended.clear();
        self.release();
        gone
    }

    /// How many ended events the log holds, hidden ones too.
    pub fn ended(&self) -> usize {
        self.ended.len()
    }

    /// Replaces the events still going on.
    pub fn set_open(&mut self, open: Vec<Event>) {
        self.open = open.into_iter().map(|event| self.stamp(event)).collect();
    }

    fn stamp(&self, event: Event) -> Logged {
        let at = self.clock.map(|clock| clock.stream_start + event.start);
        Logged {
            event,
            at,
            this_run: true,
        }
    }

    /// Lets the oldest go beyond `MAX_EVENTS`.
    fn trim(&mut self) {
        let extra = self.ended.len().saturating_sub(MAX_EVENTS);
        self.ended.drain(..extra);
    }

    pub fn min_above_db(&self) -> f32 {
        self.min_above_db
    }

    pub fn sort(&self) -> Sort {
        self.sort
    }

    /// The rows to show: live ones, or the held ones while one is selected.
    pub fn rows(&self) -> Vec<Row<'_>> {
        match &self.held {
            Some((rows, _)) => rows.iter().map(|(logged, on)| row(logged, *on)).collect(),
            None => self.live_rows(),
        }
    }

    /// The selected row's place in `rows`.
    pub fn selected(&self) -> Option<usize> {
        self.held.as_ref().map(|(_, index)| *index)
    }

    pub fn selected_event(&self) -> Option<&Event> {
        self.selected_row().map(|row| row.event)
    }

    pub fn selected_row(&self) -> Option<Row<'_>> {
        let (rows, index) = self.held.as_ref()?;
        rows.get(*index).map(|(logged, on)| row(logged, *on))
    }

    /// Events the level filter hides.
    pub fn hidden(&self) -> usize {
        self.all()
            .filter(|(logged, _)| !self.passes(&logged.event))
            .count()
    }

    /// Events on the air now, hidden ones included.
    pub fn on_now(&self) -> usize {
        self.open.len()
    }

    /// Events that ended after the list was held and are not in it.
    pub fn arrived_since_held(&self) -> usize {
        self.arrived
    }

    pub fn raise_filter(&mut self) {
        self.set_filter(self.min_above_db + MIN_ABOVE_STEP_DB);
    }

    pub fn lower_filter(&mut self) {
        self.set_filter(self.min_above_db - MIN_ABOVE_STEP_DB);
    }

    pub fn next_sort(&mut self) {
        self.sort = self.sort.next();
        self.hold_again();
    }

    /// Moves the selection by `by` rows; the first move selects the top
    /// row and holds the list.
    pub fn move_selection(&mut self, by: isize) {
        match &mut self.held {
            Some((rows, index)) => {
                let last = rows.len().saturating_sub(1);
                *index = index.saturating_add_signed(by).min(last);
            }
            None => self.hold(0),
        }
    }

    /// Lets the list go: it follows the air again.
    pub fn release(&mut self) {
        self.held = None;
        self.arrived = 0;
    }

    fn set_filter(&mut self, min_above_db: f32) {
        self.min_above_db = min_above_db.clamp(0.0, MIN_ABOVE_MAX_DB);
        self.hold_again();
    }

    /// After the filter or the order changed under a held list: the same
    /// event stays selected if it is still shown, else the top row.
    fn hold_again(&mut self) {
        let Some(selected) = self.selected_event().cloned() else {
            return;
        };
        let same =
            |row: &Row| row.event.start == selected.start && row.event.peak_hz == selected.peak_hz;
        let index = self.live_rows().iter().position(same).unwrap_or(0);
        self.hold(index);
    }

    /// Holds the live rows with the one at `index` selected; an empty
    /// list is not held.
    fn hold(&mut self, index: usize) {
        let logged = |row: &Row| Logged {
            event: row.event.clone(),
            at: row.at,
            this_run: row.this_run,
        };
        let rows: Vec<(Logged, bool)> = self
            .live_rows()
            .iter()
            .map(|row| (logged(row), row.on))
            .collect();
        self.held = (!rows.is_empty()).then_some((rows, index));
        self.arrived = 0;
    }

    fn all(&self) -> impl Iterator<Item = (&Logged, bool)> {
        let ended = self.ended.iter().map(|logged| (logged, false));
        ended.chain(self.open.iter().map(|logged| (logged, true)))
    }

    fn passes(&self, event: &Event) -> bool {
        event.above_floor_db >= self.min_above_db
    }

    fn live_rows(&self) -> Vec<Row<'_>> {
        let mut rows: Vec<Row> = self
            .all()
            .filter(|(logged, _)| self.passes(&logged.event))
            .map(|(logged, on)| row(logged, on))
            .collect();
        // Among equals the newest first, whatever the order.
        rows.sort_by_key(|row| Reverse((row.at, row.event.start)));
        match self.sort {
            Sort::Time => {}
            Sort::Level => {
                rows.sort_by(|a, b| b.event.above_floor_db.total_cmp(&a.event.above_floor_db));
            }
            Sort::Duration => rows.sort_by_key(|row| Reverse(row.event.duration)),
            Sort::Frequency => {
                rows.sort_by(|a, b| a.event.center_hz().total_cmp(&b.event.center_hz()));
            }
        }
        rows
    }
}

fn row(logged: &Logged, on: bool) -> Row<'_> {
    Row {
        event: &logged.event,
        at: logged.at,
        on,
        this_run: logged.this_run,
    }
}

/// The local date and time of day, "11-12 15:43:38.2": the year is not in
/// doubt within the week the log keeps.
pub fn format_clock(at: SystemTime, utc_offset_s: i32) -> String {
    let since = at.duration_since(UNIX_EPOCH).unwrap_or_default();
    let tenths = i64::try_from(since.as_millis() / 100).unwrap_or(i64::MAX / 2)
        + i64::from(utc_offset_s) * 10;
    let (days, of_day) = (tenths.div_euclid(864_000), tenths.rem_euclid(864_000));
    // Days since 1970 to a month and a day (the civil-from-days
    // arithmetic of Howard Hinnant), in eras of 400 years from March 0000.
    let shifted = days + 719_468;
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let march_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * march_month + 2) / 5 + 1;
    let month = if march_month < 10 {
        march_month + 3
    } else {
        march_month - 9
    };
    format!(
        "{month:02}-{day:02} {:02}:{:02}:{:02}.{}",
        of_day / 36_000,
        of_day / 600 % 60,
        of_day / 10 % 60,
        of_day % 10
    )
}

/// Stream time as a clock: "0:02.04", "1:02:03.45".
pub fn format_time(time: Duration) -> String {
    let hundredths = time.as_millis() / 10;
    let (hours, minutes) = (hundredths / 360_000, hundredths / 6000 % 60);
    let (seconds, rest) = (hundredths / 100 % 60, hundredths % 100);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}.{rest:02}")
    } else {
        format!("{minutes}:{seconds:02}.{rest:02}")
    }
}

/// "45 ms", "3.2 s", "2:05" — as short as tells the scale.
pub fn format_duration(duration: Duration) -> String {
    let ms = duration.as_millis();
    if ms < 1000 {
        format!("{ms} ms")
    } else if ms < 60_000 {
        format!("{:.1} s", duration.as_secs_f64())
    } else if ms < 3_600_000 {
        format!("{}:{:02}", ms / 60_000, ms / 1000 % 60)
    } else {
        format!(
            "{}:{:02}:{:02}",
            ms / 3_600_000,
            ms / 60_000 % 60,
            ms / 1000 % 60
        )
    }
}

/// "9.4 kHz", "1.2 MHz", "950 Hz": a width needs fewer digits than a
/// frequency.
pub fn format_width(hz: f64) -> String {
    if hz >= 1e6 {
        format!("{:.1} MHz", hz / 1e6)
    } else if hz >= 1e3 {
        format!("{:.1} kHz", hz / 1e3)
    } else {
        format!("{hz:.0} Hz")
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// An event 5 kHz wide: (start ms, duration ms, centre kHz above
    /// 433 MHz, dB above the floor).
    pub(crate) fn event(start_ms: u64, ms: u64, khz: f64, above: f32) -> Event {
        let center = 433e6 + khz * 1e3;
        Event {
            start: Duration::from_millis(start_ms),
            duration: Duration::from_millis(ms),
            low_hz: center - 2_500.0,
            high_hz: center + 2_500.0,
            peak_hz: center,
            peak_dbfs: -71.0 + above,
            above_floor_db: above,
        }
    }

    #[test]
    fn clearing_forgets_what_has_ended_and_keeps_what_is_on() {
        let mut log = LogView::default();
        log.on_events([event(100, 50, 100.0, 20.0), event(200, 10, 300.0, 5.0)]);
        log.set_open(vec![event(300, 400, 50.0, 30.0)]);
        log.move_selection(1);
        assert_eq!(log.ended(), 2);
        assert_eq!(log.clear(), 2);
        assert_eq!(log.ended(), 0);
        assert_eq!(log.selected(), None);
        assert_eq!(log.hidden(), 0);
        let rows = log.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].event.start, Duration::from_millis(300));
        // It goes on being a log.
        log.on_events([event(300, 500, 50.0, 30.0)]);
        assert_eq!(log.ended(), 1);
    }

    fn log() -> LogView {
        let mut log = LogView::default();
        log.on_events([
            event(100, 50, 100.0, 20.0),
            event(200, 10, 300.0, 9.0),
            event(300, 400, 50.0, 30.0),
            event(400, 45, 200.0, 12.0),
        ]);
        log.set_open(vec![event(0, 1000, 500.0, 15.0)]);
        log
    }

    fn starts(log: &LogView) -> Vec<u64> {
        log.rows()
            .iter()
            .map(|row| row.event.start.as_millis() as u64)
            .collect()
    }

    #[test]
    fn shows_the_newest_first_and_hides_the_weak() {
        let log = log();
        assert_eq!(starts(&log), [400, 300, 100, 0]);
        assert_eq!(log.hidden(), 1);
        assert_eq!(log.on_now(), 1);
        let rows = log.rows();
        assert!(rows[3].on && !rows[0].on);
    }

    #[test]
    fn the_filter_moves_by_a_decibel_and_brings_hidden_events_back() {
        let mut log = log();
        assert_eq!(log.min_above_db(), 10.0);
        log.lower_filter();
        assert_eq!(log.min_above_db(), 9.0);
        assert_eq!(starts(&log), [400, 300, 200, 100, 0]);
        assert_eq!(log.hidden(), 0);
        for _ in 0..4 {
            log.raise_filter();
        }
        assert_eq!(starts(&log), [300, 100, 0]);
        assert_eq!(log.hidden(), 2);
    }

    #[test]
    fn the_filter_stays_between_nothing_and_sixty() {
        let mut log = log();
        for _ in 0..100 {
            log.lower_filter();
        }
        assert_eq!(log.min_above_db(), 0.0);
        for _ in 0..100 {
            log.raise_filter();
        }
        assert_eq!(log.min_above_db(), 60.0);
        assert!(log.rows().is_empty());
    }

    #[test]
    fn sorts_by_level_duration_and_frequency_in_turn() {
        let mut log = log();
        assert_eq!(log.sort(), Sort::Time);
        log.next_sort();
        assert_eq!(log.sort(), Sort::Level);
        assert_eq!(starts(&log), [300, 100, 0, 400]);
        log.next_sort();
        assert_eq!(log.sort().name(), "duration");
        assert_eq!(starts(&log), [0, 300, 100, 400]);
        log.next_sort();
        assert_eq!(log.sort(), Sort::Frequency);
        assert_eq!(starts(&log), [300, 100, 400, 0]);
        log.next_sort();
        assert_eq!(log.sort(), Sort::Time);
    }

    #[test]
    fn selecting_holds_the_list_until_it_is_released() {
        let mut log = log();
        assert_eq!(log.selected(), None);
        log.move_selection(1);
        assert_eq!(log.selected(), Some(0));
        log.move_selection(2);
        assert_eq!(log.selected_event().unwrap().start.as_millis(), 100);
        log.on_events([event(500, 20, 100.0, 25.0), event(600, 20, 100.0, 5.0)]);
        log.set_open(Vec::new());
        assert_eq!(starts(&log), [400, 300, 100, 0]);
        // The weak one would not be shown anyway.
        assert_eq!(log.arrived_since_held(), 1);
        log.on_events([event(700, 20, 100.0, 25.0)]);
        assert_eq!(log.arrived_since_held(), 2);
        log.release();
        assert_eq!(log.selected(), None);
        assert_eq!(log.arrived_since_held(), 0);
        assert_eq!(starts(&log), [700, 500, 400, 300, 100]);
    }

    #[test]
    fn the_selection_stops_at_the_ends() {
        let mut log = log();
        log.move_selection(-1);
        log.move_selection(-5);
        assert_eq!(log.selected(), Some(0));
        log.move_selection(100);
        assert_eq!(log.selected(), Some(3));
        log.move_selection(isize::MIN);
        assert_eq!(log.selected(), Some(0));
    }

    #[test]
    fn an_empty_log_has_nothing_to_select() {
        let mut log = LogView::default();
        log.move_selection(1);
        assert_eq!(log.selected(), None);
        assert!(log.rows().is_empty());
    }

    #[test]
    fn the_selected_event_stays_selected_through_sorting_and_filtering() {
        let mut log = log();
        log.move_selection(1);
        log.move_selection(1);
        assert_eq!(log.selected_event().unwrap().start.as_millis(), 300);
        log.next_sort();
        assert_eq!(log.selected(), Some(0));
        log.lower_filter();
        assert_eq!(log.selected_event().unwrap().start.as_millis(), 300);
        // Filtered away: the top row takes over.
        for _ in 0..25 {
            log.raise_filter();
        }
        assert_eq!(log.selected(), None);
        assert!(log.rows().is_empty());
    }

    /// 2026-11-12 12:00:00 UTC.
    pub(crate) fn noon() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_794_484_800)
    }

    #[test]
    fn with_a_clock_events_carry_the_time_of_day() {
        let mut log = LogView::default();
        assert_eq!(log.clock(), None);
        log.on_events([event(100, 50, 100.0, 20.0)]);
        let clock = WallClock {
            stream_start: noon(),
            utc_offset_s: 7200,
        };
        log.set_clock(clock);
        assert_eq!(log.clock(), Some(clock));
        log.on_events([event(1_500, 50, 100.0, 20.0)]);
        log.set_open(vec![event(2_000, 50, 100.0, 20.0)]);
        let at: Vec<Option<SystemTime>> = log.rows().iter().map(|row| row.at).collect();
        let ms = Duration::from_millis;
        // The one from before the clock was set has only its stream time.
        assert_eq!(
            at,
            [Some(noon() + ms(2_000)), Some(noon() + ms(1_500)), None]
        );
    }

    #[test]
    fn history_goes_before_this_run_by_the_wall_clock() {
        let mut log = LogView::default();
        log.set_clock(WallClock {
            stream_start: noon(),
            utc_offset_s: 0,
        });
        log.on_events([event(1_000, 50, 100.0, 20.0)]);
        let hour = Duration::from_secs(3600);
        // Late in yesterday's stream, early in this morning's; given in
        // no order.
        log.load_history([
            (noon() - hour, event(5_000, 10, 300.0, 30.0)),
            (noon() - 24 * hour, event(900_000, 10, 200.0, 25.0)),
            (noon() - 2 * hour, event(50, 10, 400.0, 5.0)),
        ]);
        let rows = log.rows();
        let khz: Vec<f64> = rows
            .iter()
            .map(|row| (row.event.center_hz() - 433e6) / 1e3)
            .collect();
        assert_eq!(khz, [100.0, 300.0, 200.0]);
        assert_eq!(rows[1].at, Some(noon() - hour));
        // Only this run's events can still have their samples.
        let this_run: Vec<bool> = rows.iter().map(|row| row.this_run).collect();
        assert_eq!(this_run, [true, false, false]);
        // The weak one is there, behind the filter.
        assert_eq!(log.hidden(), 1);
        // Tuning to a row of history works as to any other.
        log.move_selection(1);
        assert!(log.selected_row().unwrap().this_run);
        log.move_selection(1);
        assert_eq!(log.selected_event().unwrap().center_hz(), 433.3e6);
        assert!(!log.selected_row().unwrap().this_run);
    }

    #[test]
    fn history_is_the_first_to_go_when_the_log_is_full() {
        let mut log = LogView::default();
        log.set_clock(WallClock {
            stream_start: noon(),
            utc_offset_s: 0,
        });
        log.on_events((0..MAX_EVENTS as u64 - 1).map(|i| event(i, 1, 0.0, 20.0)));
        let day = Duration::from_secs(86_400);
        log.load_history([
            (noon() - day, event(0, 1, 111.0, 20.0)),
            (noon() - 2 * day, event(0, 1, 222.0, 20.0)),
        ]);
        let rows = log.rows();
        assert_eq!(rows.len(), MAX_EVENTS);
        assert_eq!(rows[MAX_EVENTS - 1].at, Some(noon() - day));
    }

    #[test]
    fn formats_the_local_date_and_time_of_day() {
        let ms = Duration::from_millis;
        assert_eq!(format_clock(noon() + ms(38_250), 0), "11-12 12:00:38.2");
        // Israel in winter is two hours ahead; just before midnight there
        // is the next day.
        assert_eq!(
            format_clock(noon() + ms(36_000_000), 7200),
            "11-13 00:00:00.0"
        );
        assert_eq!(format_clock(noon(), -18_000), "11-12 07:00:00.0");
        // Leap day, the end and the start of a year.
        let at = |secs: u64| UNIX_EPOCH + Duration::from_secs(secs);
        assert_eq!(format_clock(at(1_709_208_000), 0), "02-29 12:00:00.0");
        assert_eq!(format_clock(at(1_798_761_599), 0), "12-31 23:59:59.0");
        assert_eq!(format_clock(at(1_798_761_600), 0), "01-01 00:00:00.0");
        assert_eq!(format_clock(UNIX_EPOCH, 0), "01-01 00:00:00.0");
    }

    #[test]
    fn keeps_only_the_latest_events() {
        let mut log = LogView::default();
        log.on_events((0..MAX_EVENTS as u64 + 5).map(|i| event(i, 1, 0.0, 20.0)));
        let rows = log.rows();
        assert_eq!(rows.len(), MAX_EVENTS);
        assert_eq!(rows[0].event.start.as_millis() as usize, MAX_EVENTS + 4);
        assert_eq!(rows[MAX_EVENTS - 1].event.start.as_millis(), 5);
    }

    #[test]
    fn formats_times_durations_and_widths_for_a_narrow_column() {
        assert_eq!(format_time(Duration::from_millis(2_043)), "0:02.04");
        assert_eq!(format_time(Duration::from_millis(754_990)), "12:34.99");
        assert_eq!(format_time(Duration::from_secs(3_723)), "1:02:03.00");
        assert_eq!(format_duration(Duration::from_millis(45)), "45 ms");
        assert_eq!(format_duration(Duration::from_millis(3_240)), "3.2 s");
        assert_eq!(format_duration(Duration::from_secs(125)), "2:05");
        assert_eq!(format_duration(Duration::from_secs(3_725)), "1:02:05");
        assert_eq!(format_width(9_375.0), "9.4 kHz");
        assert_eq!(format_width(1_171.875), "1.2 kHz");
        assert_eq!(format_width(950.0), "950 Hz");
        assert_eq!(format_width(1_200_000.0), "1.2 MHz");
    }
}
