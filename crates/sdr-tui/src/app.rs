//! Interface state and what the keys do to it; no drawing here.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use sdr_core::units::parse_hz;
use sdr_core::{GainChange, SourceInfo, SpectrumFrame};
use sdr_detect::{DensityMap, Event, Levels};

use crate::channels::ChannelsView;
use crate::fragment::{Fragment, Picture};
use crate::log::LogView;
use crate::period::{DELAYS, USUAL};
use crate::spectrum::SpectrumView;

/// The screens of the plan (section 5), in the order of their digit keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Spectrum,
    Channels,
    Density,
    Periodicity,
    Log,
    Mesh,
    Panorama,
    Decoders,
}

impl Screen {
    pub const ALL: [Screen; 8] = [
        Screen::Spectrum,
        Screen::Channels,
        Screen::Density,
        Screen::Periodicity,
        Screen::Log,
        Screen::Mesh,
        Screen::Panorama,
        Screen::Decoders,
    ];

    /// The digit that opens it, 1-based.
    pub fn key(self) -> u8 {
        Screen::ALL
            .iter()
            .position(|&s| s == self)
            .map_or(0, |i| i as u8 + 1)
    }

    /// One word for the tab bar.
    pub fn tab(self) -> &'static str {
        match self {
            Screen::Spectrum => "Spectrum",
            Screen::Channels => "Channels",
            Screen::Density => "Density",
            Screen::Periodicity => "Period",
            Screen::Log => "Log",
            Screen::Mesh => "Mesh",
            Screen::Panorama => "Panorama",
            Screen::Decoders => "Decoders",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Screen::Spectrum => "Spectrum and waterfall",
            Screen::Channels => "Channel strip",
            Screen::Density => "Spectral density",
            Screen::Periodicity => "Periodicity",
            Screen::Log => "Signal log",
            Screen::Mesh => "Meshtastic pulse",
            Screen::Panorama => "Panorama",
            Screen::Decoders => "Decoders",
        }
    }

    /// The user's question this screen answers (the plan's section 1).
    pub fn question(self) -> &'static str {
        match self {
            Screen::Spectrum => "What is on the air right now?",
            Screen::Channels | Screen::Log => "What happened while I wasn't looking?",
            Screen::Density | Screen::Panorama => "Which frequencies are busy at all?",
            Screen::Periodicity => "Does this signal repeat? With what period?",
            Screen::Mesh => "Is my mesh network alive?",
            Screen::Decoders => "What exactly is being sent?",
        }
    }

    /// The phase of the plan (section 8) that builds it.
    pub fn phase(self) -> u8 {
        match self {
            Screen::Spectrum => 2,
            Screen::Log => 3,
            Screen::Channels | Screen::Density | Screen::Periodicity => 4,
            Screen::Mesh => 5,
            Screen::Panorama => 6,
            Screen::Decoders => 7,
        }
    }
}

/// How the source thread is doing.
#[derive(Debug, Clone, PartialEq)]
pub enum SourceState {
    Running,
    /// A recording played to its end.
    Ended,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SourceStatus {
    /// Samples per second actually received.
    pub rate: f64,
    pub state: SourceState,
    /// The share of the latest samples that were at full scale, 0 to 1:
    /// the receiver is overloaded.
    pub clipped: f64,
    /// Samples lost since the start because nobody took them in time.
    pub lost: u64,
}

impl SourceStatus {
    /// A source in `state` with nothing clipped and nothing lost.
    pub fn new(state: SourceState, rate: f64) -> Self {
        SourceStatus {
            rate,
            state,
            clipped: 0.0,
            lost: 0,
        }
    }
}

/// What the interface asks of the source; the reader thread carries it out.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Tune(f64),
    Gain(GainChange),
    /// Save the samples of this event, while they are still held.
    SaveIq(Event),
    /// Make the picture of this event from its samples.
    Picture(Event),
    /// Send the density with every frame, or stop: it is a megabyte, and
    /// only its screen needs it.
    WatchDensity(bool),
    /// Forget the density gathered so far.
    ResetDensity,
    /// Forget the events that have ended: those not yet shown, and those
    /// in the database.
    EraseEvents,
}

pub struct App {
    pub screen: Screen,
    pub help: bool,
    pub quit: bool,
    pub source: SourceInfo,
    pub status: SourceStatus,
    pub spectrum: SpectrumView,
    pub log: LogView,
    pub channels: ChannelsView,
    /// The range of delays chosen on the periodicity screen, of
    /// `period::DELAYS`; until one is, the range follows the channel.
    pub delays: Option<usize>,
    /// How often each frequency was at each level; comes only while its
    /// screen is open.
    pub density: Option<DensityMap>,
    /// Asked of the source and not yet sent; the main loop drains it.
    pub commands: Vec<Command>,
    /// The frequency being typed after `f`.
    pub input: Option<String>,
    /// The last thing that went wrong, until the next key.
    pub notice: Option<String>,
    /// The last thing done that was asked for, until the next key.
    pub done: Option<String>,
    /// The event selected in the log and what there is of its picture.
    picture: Option<(Event, Picture)>,
    /// `X` was pressed once on this screen: the next one there does what
    /// was asked about.
    armed: Option<Screen>,
}

/// Cursor steps as shares of the width: fine, and coarse with Shift.
const CURSOR_STEP: f64 = 0.005;
const CURSOR_LEAP: f64 = 0.05;

/// Rows a page key moves the log's selection.
const LOG_PAGE: isize = 10;

impl App {
    pub fn new(source: SourceInfo) -> Self {
        App {
            screen: Screen::Spectrum,
            help: false,
            quit: false,
            source,
            status: SourceStatus::new(SourceState::Running, 0.0),
            spectrum: SpectrumView::default(),
            log: LogView::default(),
            channels: ChannelsView::default(),
            delays: None,
            density: None,
            commands: Vec::new(),
            input: None,
            notice: None,
            done: None,
            picture: None,
            armed: None,
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        let watched = self.screen == Screen::Density;
        self.handle_key(key);
        self.follow_selection();
        if self.screen == Screen::Periodicity {
            // The channel in sight is not to wait for its turn.
            self.channels.refresh_selected_period();
        }
        let watch = self.screen == Screen::Density;
        if watch != watched {
            self.commands.push(Command::WatchDensity(watch));
        }
    }

    /// A frame for the screen, and the stream's time with it.
    pub fn on_frame(&mut self, frame: SpectrumFrame, levels: Levels) {
        self.channels
            .on_frame(frame.time, frame.center_hz, frame.sample_rate);
        self.spectrum.on_frame(frame, levels);
    }

    /// Events that have ended: rows of the log, and the channels' time.
    pub fn on_events(&mut self, events: Vec<Event>) {
        self.channels.on_events(&events);
        self.log.on_events(events);
    }

    /// The events going on now.
    pub fn on_open_events(&mut self, open: Vec<Event>) {
        self.channels.set_open(&open);
        self.log.set_open(open);
    }

    /// Takes the density as of the latest frame; like the spectrum, the
    /// picture stands still while paused.
    pub fn on_density(&mut self, map: DensityMap) {
        if !self.spectrum.paused {
            self.density = (map.bins > 0).then_some(map);
        }
    }

    /// The picture of the event selected in the log, if one is.
    pub fn picture(&self) -> Option<&Picture> {
        self.picture.as_ref().map(|(_, picture)| picture)
    }

    /// Takes the picture made of `event`, or the word that its samples
    /// are gone; one that was asked for before the selection moved on is
    /// dropped.
    pub fn on_picture(&mut self, event: &Event, fragment: Option<Fragment>) {
        if let Some((_, picture)) = self.picture.as_mut().filter(|(of, _)| of == event) {
            *picture = fragment.map_or(Picture::Gone, Picture::Ready);
        }
    }

    /// Asks for the picture of the event the log's selection has come to.
    fn follow_selection(&mut self) {
        let selected = self.log.selected_row();
        let event = selected.map(|row| row.event);
        if event == self.picture.as_ref().map(|(of, _)| of) {
            return;
        }
        self.picture = selected.map(|row| {
            let picture = if row.this_run {
                self.commands.push(Command::Picture(row.event.clone()));
                Picture::Waiting
            } else {
                // An earlier run's samples went with it.
                Picture::Gone
            };
            (row.event.clone(), picture)
        });
    }

    fn handle_key(&mut self, key: KeyEvent) {
        self.notice = None;
        self.done = None;
        let armed = self.armed.take();
        if armed == Some(self.screen) && key.code == KeyCode::Char('X') {
            self.clean_slate();
            return;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.quit = true;
            return;
        }
        if self.input.is_some() {
            self.on_input_key(key);
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Esc if self.help => self.help = false,
            KeyCode::Esc if self.screen == Screen::Log && self.log.selected().is_some() => {
                self.log.release();
            }
            KeyCode::Esc
                if matches!(self.screen, Screen::Channels | Screen::Periodicity)
                    && self.channels.selected().is_some() =>
            {
                self.channels.release();
            }
            KeyCode::Esc if self.spectrum.cursor.is_some() => self.spectrum.cursor = None,
            KeyCode::Esc => self.quit = true,
            KeyCode::Char('?') => self.help = !self.help,
            KeyCode::Tab => self.step_screen(1),
            KeyCode::BackTab => self.step_screen(Screen::ALL.len() - 1),
            KeyCode::Char(c @ '1'..='8') => {
                self.screen = Screen::ALL[usize::from(c as u8 - b'1')];
            }
            _ if self.screen == Screen::Spectrum => self.on_spectrum_key(key),
            _ if self.screen == Screen::Channels => self.on_channels_key(key),
            _ if self.screen == Screen::Density => self.on_density_key(key),
            _ if self.screen == Screen::Periodicity => self.on_period_key(key),
            _ if self.screen == Screen::Log => self.on_log_key(key),
            _ => {}
        }
    }

    /// The range of delays the periodicity screen shows: the one chosen
    /// there, or else the one the selected channel's period is in.
    pub fn delay_scale(&self) -> usize {
        let selected = self.channels.selected_index();
        let found = selected.and_then(|index| self.channels.period(index));
        self.delays
            .or(found.map(|period| period.scale))
            .unwrap_or(USUAL)
    }

    /// The keys that select a channel, on the strip and on the
    /// periodicity screen alike. Another channel has its own delays.
    fn on_channel_selection_key(&mut self, key: KeyEvent) -> bool {
        let by = match key.code {
            KeyCode::Up => -1,
            KeyCode::Down => 1,
            KeyCode::PageUp => -LOG_PAGE,
            KeyCode::PageDown => LOG_PAGE,
            KeyCode::Home => isize::MIN,
            KeyCode::End => isize::MAX,
            _ => return false,
        };
        self.channels.move_selection(by);
        self.delays = None;
        true
    }

    fn tune_to_selected_channel(&mut self) {
        if let Some(center_hz) = self.channels.selected().map(|channel| channel.center_hz) {
            self.tune(center_hz);
            self.screen = Screen::Spectrum;
        }
    }

    fn on_period_key(&mut self, key: KeyEvent) {
        if self.on_channel_selection_key(key) {
            return;
        }
        match key.code {
            KeyCode::Char('+' | '=') => self.delays = Some(self.delay_scale().saturating_sub(1)),
            KeyCode::Char('-') => {
                self.delays = Some((self.delay_scale() + 1).min(DELAYS.len() - 1));
            }
            KeyCode::Enter => self.tune_to_selected_channel(),
            _ => {}
        }
    }

    fn on_channels_key(&mut self, key: KeyEvent) {
        if self.on_channel_selection_key(key) {
            return;
        }
        match key.code {
            KeyCode::Char('+' | '=') => self.channels.zoom_in(),
            KeyCode::Char('-') => self.channels.zoom_out(),
            // Not to be undone: asked twice.
            KeyCode::Char('X') => match self.channels.made() {
                0 => self.notice = Some("no silent channel made by the app: nothing to go".into()),
                made => {
                    self.armed = Some(Screen::Channels);
                    self.notice = Some(format!(
                        "X again lets go of the {} the app made; named, hidden and those on now stay",
                        count_channels(made)
                    ));
                }
            },
            KeyCode::Enter => self.tune_to_selected_channel(),
            _ => {}
        }
    }

    fn on_log_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up => self.log.move_selection(-1),
            KeyCode::Down => self.log.move_selection(1),
            KeyCode::PageUp => self.log.move_selection(-LOG_PAGE),
            KeyCode::PageDown => self.log.move_selection(LOG_PAGE),
            KeyCode::Home => self.log.move_selection(isize::MIN),
            KeyCode::End => self.log.move_selection(isize::MAX),
            KeyCode::Char('+' | '=') => self.log.raise_filter(),
            KeyCode::Char('-') => self.log.lower_filter(),
            KeyCode::Char('s') => self.log.next_sort(),
            KeyCode::Char('r') => self.save_selected(),
            // Not to be undone: asked twice.
            KeyCode::Char('X') => match self.log.ended() {
                0 => self.notice = Some("the log has no ended events: nothing to erase".into()),
                ended => {
                    self.armed = Some(Screen::Log);
                    let kept = match self.log.clock() {
                        Some(_) => ", in the database too",
                        None => "",
                    };
                    self.notice = Some(format!(
                        "X again erases the {ended} ended events of the log{kept}; those on now stay"
                    ));
                }
            },
            KeyCode::Enter => {
                if let Some(center_hz) = self.log.selected_event().map(|event| event.center_hz()) {
                    self.tune(center_hz);
                    self.screen = Screen::Spectrum;
                }
            }
            _ => {}
        }
    }

    /// Asks for the selected event's samples to be saved.
    fn save_selected(&mut self) {
        match self.log.selected_row() {
            Some(row) if row.this_run => self.commands.push(Command::SaveIq(row.event.clone())),
            Some(_) => self.notice = Some("an earlier run's samples are gone".into()),
            None => self.notice = Some("select an event first: ↑ ↓".into()),
        }
    }

    /// What the second `X` on a screen does.
    fn clean_slate(&mut self) {
        match self.screen {
            Screen::Channels => {
                let gone = self.channels.forget_made();
                self.done = Some(format!(
                    "{} let go; new signals make new ones",
                    count_channels(gone)
                ));
            }
            Screen::Log => {
                let gone = self.log.clear();
                self.commands.push(Command::EraseEvents);
                self.done = Some(format!("{gone} events erased"));
            }
            _ => {}
        }
    }

    /// The density shows the band the spectrum shows: the same cursor,
    /// zoom and tuning. Only `m` is its own.
    fn on_density_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('m') => {
                self.density = None;
                self.commands.push(Command::ResetDensity);
            }
            _ => self.on_spectrum_key(key),
        }
    }

    fn on_spectrum_key(&mut self, key: KeyEvent) {
        let step = if key.modifiers.contains(KeyModifiers::SHIFT) {
            CURSOR_LEAP
        } else {
            CURSOR_STEP
        };
        match key.code {
            KeyCode::Left => self.spectrum.move_cursor(-step),
            KeyCode::Right => self.spectrum.move_cursor(step),
            KeyCode::Char(' ') => self.spectrum.paused = !self.spectrum.paused,
            KeyCode::Char('m') => self.spectrum.reset_max_hold(),
            KeyCode::Char('+' | '=') => self.spectrum.zoom_in(),
            KeyCode::Char('-') => self.spectrum.zoom_out(),
            KeyCode::Char('f') => self.input = Some(String::new()),
            KeyCode::Char('g') => self.commands.push(Command::Gain(GainChange::Step(1))),
            KeyCode::Char('G') => self.commands.push(Command::Gain(GainChange::Step(-1))),
            KeyCode::Char('a') => self.commands.push(Command::Gain(GainChange::Auto)),
            KeyCode::Char('[') => self.tune_by_view(-0.5),
            KeyCode::Char(']') => self.tune_by_view(0.5),
            KeyCode::Enter => self.tune_to_cursor(),
            _ => {}
        }
    }

    /// Typing a frequency: digits, a point and a k/M/G suffix.
    fn on_input_key(&mut self, key: KeyEvent) {
        let Some(typed) = &mut self.input else { return };
        match key.code {
            KeyCode::Char(c) if c.is_ascii_digit() || ".kKmMgG".contains(c) => typed.push(c),
            KeyCode::Backspace => {
                typed.pop();
            }
            KeyCode::Esc => self.input = None,
            KeyCode::Enter => {
                let typed = self.input.take().unwrap_or_default();
                match parse_hz(&typed) {
                    Some(hz) => self.tune(hz),
                    None if typed.is_empty() => {}
                    None => self.notice = Some(format!("not a frequency: {typed}")),
                }
            }
            _ => {}
        }
    }

    fn tune(&mut self, center_hz: f64) {
        self.commands.push(Command::Tune(center_hz));
        self.spectrum.recenter();
    }

    /// Moves the centre by a share of what is visible.
    fn tune_by_view(&mut self, share: f64) {
        if let Some(latest) = self.spectrum.latest() {
            let center = latest.center_hz + share * self.spectrum.span_hz(latest);
            self.tune(center);
        }
    }

    fn tune_to_cursor(&mut self) {
        if let (Some(latest), Some(cursor)) = (self.spectrum.latest(), self.spectrum.cursor) {
            let center = self.spectrum.hz(latest, cursor);
            self.tune(center);
        }
    }

    fn step_screen(&mut self, by: usize) {
        let index = usize::from(self.screen.key() - 1);
        self.screen = Screen::ALL[(index + by) % Screen::ALL.len()];
    }
}

/// "1 channel", "2 channels".
fn count_channels(count: usize) -> String {
    format!("{count} channel{}", if count == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        App::new(SourceInfo {
            sample_rate: 2_400_000,
            center_hz: 433.92e6,
            description: "test".into(),
        })
    }

    fn press(app: &mut App, code: KeyCode) {
        app.on_key(KeyEvent::from(code));
    }

    fn app_on_the_log() -> App {
        use crate::log::tests::event;
        let mut app = app();
        app.screen = Screen::Log;
        app.log.on_events([
            event(100, 50, 100.0, 20.0),
            event(200, 10, 300.0, 9.0),
            event(300, 400, 50.0, 30.0),
        ]);
        app
    }

    fn density_map(bins: usize) -> DensityMap {
        use sdr_detect::{Density, DensityConfig};
        let mut density = Density::new(DensityConfig::default());
        if bins > 0 {
            density.push(&sdr_core::SpectrumFrame {
                time: std::time::Duration::ZERO,
                center_hz: 433.92e6,
                sample_rate: 2_400_000,
                power_db: vec![-70.0; bins],
            });
        }
        density.map().clone()
    }

    #[test]
    fn the_density_is_asked_for_while_its_screen_is_open() {
        let mut app = app();
        press(&mut app, KeyCode::Char('3'));
        assert_eq!(app.commands, [Command::WatchDensity(true)]);
        press(&mut app, KeyCode::Char('3'));
        press(&mut app, KeyCode::Right);
        assert_eq!(app.commands.len(), 1);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.commands[1..], [Command::WatchDensity(false)]);
        press(&mut app, KeyCode::BackTab);
        assert_eq!(app.commands[2..], [Command::WatchDensity(true)]);
    }

    #[test]
    fn the_density_screen_has_the_spectrums_keys_and_m_starts_it_afresh() {
        let mut app = app();
        press(&mut app, KeyCode::Char('3'));
        app.commands.clear();
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Char('+'));
        assert!(app.spectrum.cursor.is_some());
        assert_eq!(app.spectrum.magnification(), 2);
        press(&mut app, KeyCode::Esc);
        assert!(app.spectrum.cursor.is_none() && !app.quit);
        app.on_density(density_map(8));
        assert!(app.density.is_some());
        press(&mut app, KeyCode::Char('m'));
        assert_eq!(app.density, None);
        assert_eq!(app.commands, [Command::ResetDensity]);
        // A map of nothing, on its way when the reset came, is no map.
        app.on_density(density_map(0));
        assert_eq!(app.density, None);
    }

    #[test]
    fn a_paused_picture_keeps_its_density() {
        let mut app = app();
        press(&mut app, KeyCode::Char('3'));
        app.on_density(density_map(8));
        press(&mut app, KeyCode::Char(' '));
        app.on_density(density_map(16));
        assert_eq!(app.density.as_ref().map(|map| map.bins), Some(8));
        press(&mut app, KeyCode::Char(' '));
        app.on_density(density_map(16));
        assert_eq!(app.density.as_ref().map(|map| map.bins), Some(16));
    }

    #[test]
    fn channel_keys_select_change_the_time_scale_and_tune() {
        use crate::log::tests::event;
        let mut app = app();
        app.on_events(vec![
            event(100, 50, 100.0, 20.0),
            event(200, 10, 300.0, 20.0),
        ]);
        // The log has them too.
        assert_eq!(app.log.rows().len(), 2);
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Enter);
        assert!(app.commands.is_empty());
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::End);
        assert_eq!(app.channels.selected_row(), Some(1));
        press(&mut app, KeyCode::Char('-'));
        assert_eq!(app.channels.seconds_per_column(), 10);
        press(&mut app, KeyCode::Char('+'));
        assert_eq!(app.channels.seconds_per_column(), 1);
        // Neither the spectrum's zoom nor the log's filter moved.
        assert_eq!(app.spectrum.magnification(), 1);
        assert_eq!(app.log.min_above_db(), 10.0);
        press(&mut app, KeyCode::Esc);
        assert!(app.channels.selected().is_none() && !app.quit);
        press(&mut app, KeyCode::Home);
        press(&mut app, KeyCode::Enter);
        let center = app.channels.selected().unwrap().center_hz;
        assert_eq!(app.commands, [Command::Tune(center)]);
        assert_eq!(app.screen, Screen::Spectrum);
    }

    /// Two channels: a burst every 2.04 s at +100 kHz, one every 34 s at
    /// +300 kHz, ten minutes of both.
    fn app_with_rhythms() -> App {
        use crate::log::tests::event;
        let mut app = app();
        let every = |period_ms: u64, khz: f64| {
            let starts = (0..600_000 / period_ms).map(move |turn| turn * period_ms);
            starts.map(move |start| event(start, 300, khz, 20.0))
        };
        app.on_events(every(2040, 100.0).chain(every(34_000, 300.0)).collect());
        app
    }

    #[test]
    fn the_periodicity_screen_shares_the_strips_selection_and_follows_the_period() {
        let mut app = app_with_rhythms();
        press(&mut app, KeyCode::Char('4'));
        assert_eq!(app.delay_scale(), USUAL);
        press(&mut app, KeyCode::Enter);
        assert!(app.commands.is_empty());
        // The first channel repeats within 10 s, the second within 60:
        // each is shown in its own range at once, without a frame.
        press(&mut app, KeyCode::Down);
        assert_eq!(app.channels.selected_row(), Some(0));
        assert_eq!(app.delay_scale(), 1);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.delay_scale(), 2);
        // The selection is the strip's.
        press(&mut app, KeyCode::Char('2'));
        assert_eq!(app.channels.selected_row(), Some(1));
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Char('4'));
        assert_eq!(app.channels.selected_row(), Some(0));
        assert_eq!(app.delay_scale(), 1);
    }

    #[test]
    fn plus_and_minus_choose_the_delays_until_another_channel_is_selected() {
        let mut app = app_with_rhythms();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('-'));
        assert_eq!(app.delay_scale(), 2);
        for _ in 0..5 {
            press(&mut app, KeyCode::Char('-'));
        }
        assert_eq!(app.delay_scale(), DELAYS.len() - 1);
        for _ in 0..5 {
            press(&mut app, KeyCode::Char('+'));
        }
        assert_eq!(app.delay_scale(), 0);
        // Neither the strip's scale nor the spectrum's zoom moved.
        assert_eq!(app.channels.seconds_per_column(), 1);
        assert_eq!(app.spectrum.magnification(), 1);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.delay_scale(), 2);
        // Enter tunes to the channel; Esc lets it go before it quits.
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Esc);
        assert!(app.channels.selected().is_none() && !app.quit);
        press(&mut app, KeyCode::Home);
        press(&mut app, KeyCode::Enter);
        let center = app.channels.selected().unwrap().center_hz;
        assert_eq!(app.commands, [Command::Tune(center)]);
        assert_eq!(app.screen, Screen::Spectrum);
    }

    #[test]
    fn x_twice_lets_go_of_the_made_channels() {
        use crate::channels::Change;
        use crate::log::tests::event;
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('X'));
        assert!(
            app.notice
                .as_deref()
                .unwrap()
                .starts_with("no silent channel made")
        );
        app.on_events(vec![
            event(100, 50, 100.0, 20.0),
            event(200, 10, 300.0, 20.0),
        ]);
        app.channels.take_changes();
        press(&mut app, KeyCode::Char('X'));
        let asked = app.notice.clone().unwrap();
        assert!(
            asked.starts_with("X again lets go of the 2 channels"),
            "{asked}"
        );
        assert_eq!(app.channels.rows().len(), 2);
        // Any other key calls it off.
        press(&mut app, KeyCode::Down);
        assert_eq!(app.notice, None);
        press(&mut app, KeyCode::Char('X'));
        assert_eq!(app.channels.rows().len(), 2);
        press(&mut app, KeyCode::Char('X'));
        assert!(app.channels.rows().is_empty());
        assert_eq!(
            app.channels.take_changes(),
            [Change::ForgetMade { keep: Vec::new() }]
        );
        assert_eq!(
            app.done.as_deref(),
            Some("2 channels let go; new signals make new ones")
        );
        // On another screen X is nothing.
        app.on_events(vec![event(100, 50, 100.0, 20.0)]);
        press(&mut app, KeyCode::Char('X'));
        let asked = app.notice.clone().unwrap();
        assert!(
            asked.starts_with("X again lets go of the 1 channel the"),
            "{asked}"
        );
        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Char('X'));
        assert_eq!(app.channels.rows().len(), 1);
    }

    #[test]
    fn x_twice_on_the_log_erases_what_has_ended() {
        let mut app = app_on_the_log();
        app.on_open_events(vec![crate::log::tests::event(900, 400, 50.0, 30.0)]);
        press(&mut app, KeyCode::Down);
        app.commands.clear();
        press(&mut app, KeyCode::Char('X'));
        assert_eq!(
            app.notice.as_deref(),
            Some("X again erases the 3 ended events of the log; those on now stay")
        );
        assert!(app.commands.is_empty());
        // The question asked on one screen is not answered on another.
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Char('X'));
        assert_eq!(app.log.ended(), 3);
        press(&mut app, KeyCode::Char('X'));
        assert_eq!(app.log.ended(), 0);
        assert_eq!(app.log.rows().len(), 1);
        assert_eq!(app.commands, [Command::EraseEvents]);
        assert_eq!(app.done.as_deref(), Some("3 events erased"));
        // The picture of the row that was selected went with it.
        assert!(app.picture().is_none());
        press(&mut app, KeyCode::Char('X'));
        assert_eq!(
            app.notice.as_deref(),
            Some("the log has no ended events: nothing to erase")
        );
        // With a database it says so.
        app.log.set_clock(crate::log::WallClock {
            stream_start: crate::log::tests::noon(),
            utc_offset_s: 0,
        });
        app.on_events(vec![crate::log::tests::event(100, 50, 100.0, 20.0)]);
        press(&mut app, KeyCode::Char('X'));
        let asked = app.notice.clone().unwrap();
        assert!(
            asked.contains("1 ended events of the log, in the database too;"),
            "{asked}"
        );
    }

    #[test]
    fn log_keys_select_filter_and_sort() {
        let mut app = app_on_the_log();
        press(&mut app, KeyCode::Down);
        assert_eq!(app.log.selected(), Some(0));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.log.selected(), Some(1));
        press(&mut app, KeyCode::Home);
        assert_eq!(app.log.selected(), Some(0));
        press(&mut app, KeyCode::End);
        assert_eq!(app.log.selected(), Some(1));
        press(&mut app, KeyCode::PageUp);
        assert_eq!(app.log.selected(), Some(0));
        press(&mut app, KeyCode::Char('-'));
        assert_eq!(app.log.min_above_db(), 9.0);
        assert_eq!(app.log.rows().len(), 3);
        press(&mut app, KeyCode::Char('+'));
        press(&mut app, KeyCode::Char('='));
        assert_eq!(app.log.min_above_db(), 11.0);
        press(&mut app, KeyCode::Char('s'));
        assert_eq!(app.log.sort(), crate::log::Sort::Level);
        // The spectrum's zoom did not move.
        assert_eq!(app.spectrum.magnification(), 1);
    }

    #[test]
    fn escape_on_the_log_lets_the_list_go_before_it_quits() {
        let mut app = app_on_the_log();
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.log.selected(), None);
        assert!(!app.quit);
        press(&mut app, KeyCode::Esc);
        assert!(app.quit);
    }

    #[test]
    fn enter_on_a_log_row_tunes_to_the_event_and_shows_the_spectrum() {
        let mut app = app_on_the_log();
        press(&mut app, KeyCode::Enter);
        assert!(app.commands.is_empty());
        assert_eq!(app.screen, Screen::Log);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        app.commands.clear();
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.commands, [Command::Tune(433.1e6)]);
        assert_eq!(app.screen, Screen::Spectrum);
    }

    #[test]
    fn r_on_a_log_row_asks_for_the_events_samples() {
        use crate::log::tests::{event, noon};
        let mut app = app();
        app.screen = Screen::Log;
        app.log.set_clock(crate::log::WallClock {
            stream_start: noon(),
            utc_offset_s: 0,
        });
        app.log.on_events([event(300, 400, 50.0, 30.0)]);
        let hour = std::time::Duration::from_secs(3600);
        app.log
            .load_history([(noon() - hour, event(50, 10, 700.0, 40.0))]);
        press(&mut app, KeyCode::Char('r'));
        assert!(app.commands.is_empty());
        assert_eq!(app.notice.as_deref(), Some("select an event first: ↑ ↓"));
        press(&mut app, KeyCode::Down);
        assert_eq!(app.notice, None);
        app.commands.clear();
        press(&mut app, KeyCode::Char('r'));
        assert_eq!(app.commands, [Command::SaveIq(event(300, 400, 50.0, 30.0))]);
        // The list stays held, for the next one.
        assert_eq!(app.log.selected(), Some(0));
        // What the database remembers of an earlier run has no samples.
        app.commands.clear();
        press(&mut app, KeyCode::End);
        press(&mut app, KeyCode::Char('r'));
        assert!(app.commands.is_empty());
        assert_eq!(
            app.notice.as_deref(),
            Some("an earlier run's samples are gone")
        );
    }

    #[test]
    fn selecting_a_log_row_asks_for_its_picture_once() {
        use crate::fragment::tests::fragment;
        use crate::log::tests::event;
        let mut app = app_on_the_log();
        assert_eq!(app.picture(), None);
        press(&mut app, KeyCode::Down);
        // The newest row is selected first.
        let newest = event(300, 400, 50.0, 30.0);
        assert_eq!(app.commands, [Command::Picture(newest.clone())]);
        assert_eq!(app.picture(), Some(&Picture::Waiting));
        // A key that leaves the selection where it is asks for nothing.
        press(&mut app, KeyCode::Home);
        assert_eq!(app.commands.len(), 1);
        app.on_picture(&newest, Some(fragment()));
        assert_eq!(app.picture(), Some(&Picture::Ready(fragment())));
        // The next row: its own picture, and the old one is put away.
        press(&mut app, KeyCode::Down);
        let older = event(100, 50, 100.0, 20.0);
        assert_eq!(app.commands[1], Command::Picture(older.clone()));
        assert_eq!(app.picture(), Some(&Picture::Waiting));
        // A picture that comes late, for a row already left, is dropped.
        app.on_picture(&newest, Some(fragment()));
        assert_eq!(app.picture(), Some(&Picture::Waiting));
        app.on_picture(&older, None);
        assert_eq!(app.picture(), Some(&Picture::Gone));
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.picture(), None);
        assert_eq!(app.commands.len(), 2);
    }

    #[test]
    fn a_row_of_an_earlier_run_has_no_picture_to_ask_for() {
        use crate::log::tests::{event, noon};
        let mut app = app();
        app.screen = Screen::Log;
        app.log.load_history([(noon(), event(50, 10, 700.0, 40.0))]);
        press(&mut app, KeyCode::Down);
        assert!(app.commands.is_empty());
        assert_eq!(app.picture(), Some(&Picture::Gone));
    }

    #[test]
    fn a_key_puts_away_what_was_done_as_it_does_a_notice() {
        let mut app = app();
        app.done = Some("IQ saved".into());
        press(&mut app, KeyCode::Char('5'));
        assert_eq!(app.done, None);
    }

    #[test]
    fn starts_on_the_spectrum() {
        let app = app();
        assert_eq!(app.screen, Screen::Spectrum);
        assert!(!app.help && !app.quit);
    }

    #[test]
    fn digits_open_their_screens() {
        let mut app = app();
        for screen in Screen::ALL {
            press(&mut app, KeyCode::Char(char::from(b'0' + screen.key())));
            assert_eq!(app.screen, screen);
        }
        press(&mut app, KeyCode::Char('9'));
        press(&mut app, KeyCode::Char('0'));
        assert_eq!(app.screen, Screen::Decoders);
    }

    #[test]
    fn tab_walks_forward_and_back_around_the_ends() {
        let mut app = app();
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.screen, Screen::Channels);
        press(&mut app, KeyCode::BackTab);
        press(&mut app, KeyCode::BackTab);
        assert_eq!(app.screen, Screen::Decoders);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.screen, Screen::Spectrum);
    }

    #[test]
    fn question_mark_toggles_help_and_escape_closes_it_first() {
        let mut app = app();
        press(&mut app, KeyCode::Char('?'));
        assert!(app.help);
        press(&mut app, KeyCode::Char('?'));
        assert!(!app.help);
        press(&mut app, KeyCode::Char('?'));
        press(&mut app, KeyCode::Esc);
        assert!(!app.help && !app.quit);
        press(&mut app, KeyCode::Esc);
        assert!(app.quit);
    }

    #[test]
    fn spectrum_keys_move_the_cursor_pause_and_reset() {
        let mut app = app();
        press(&mut app, KeyCode::Right);
        assert_eq!(app.spectrum.cursor, Some(0.5));
        press(&mut app, KeyCode::Right);
        assert_eq!(app.spectrum.cursor, Some(0.505));
        app.on_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
        assert_eq!(app.spectrum.cursor, Some(0.455));
        press(&mut app, KeyCode::Char(' '));
        assert!(app.spectrum.paused);
        press(&mut app, KeyCode::Char(' '));
        assert!(!app.spectrum.paused);
        // Escape puts the cursor away before it quits.
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.spectrum.cursor, None);
        assert!(!app.quit);
    }

    fn frame() -> sdr_core::SpectrumFrame {
        sdr_core::SpectrumFrame {
            time: std::time::Duration::ZERO,
            center_hz: 433.92e6,
            sample_rate: 2_400_000,
            power_db: vec![-90.0; 2048],
        }
    }

    fn type_keys(app: &mut App, keys: &str) {
        for c in keys.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    #[test]
    fn f_takes_a_frequency_and_enter_tunes_to_it() {
        let mut app = app();
        type_keys(&mut app, "f144.8M");
        assert_eq!(app.input.as_deref(), Some("144.8M"));
        // While typing, the digits do not switch screens and q does not quit.
        press(&mut app, KeyCode::Char('q'));
        assert_eq!(app.screen, Screen::Spectrum);
        assert!(!app.quit);
        press(&mut app, KeyCode::Backspace);
        press(&mut app, KeyCode::Char('M'));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.input, None);
        assert_eq!(app.commands, [Command::Tune(144.8e6)]);
    }

    #[test]
    fn escape_cancels_typing_and_nonsense_is_reported_until_the_next_key() {
        let mut app = app();
        type_keys(&mut app, "f100M");
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.input, None);
        assert!(app.commands.is_empty() && !app.quit);

        type_keys(&mut app, "f1.2.3");
        press(&mut app, KeyCode::Enter);
        assert!(app.commands.is_empty());
        assert_eq!(app.notice.as_deref(), Some("not a frequency: 1.2.3"));
        press(&mut app, KeyCode::Left);
        assert_eq!(app.notice, None);

        type_keys(&mut app, "f");
        press(&mut app, KeyCode::Enter);
        assert!(app.commands.is_empty());
        assert_eq!(app.notice, None);
    }

    #[test]
    fn brackets_move_the_centre_by_half_of_what_is_visible() {
        let mut app = app();
        press(&mut app, KeyCode::Char(']'));
        assert!(
            app.commands.is_empty(),
            "nothing to move before the first frame"
        );
        app.spectrum
            .on_frame(frame(), sdr_detect::Levels::default());
        press(&mut app, KeyCode::Char(']'));
        press(&mut app, KeyCode::Char('+'));
        press(&mut app, KeyCode::Char('['));
        assert_eq!(
            app.commands,
            [Command::Tune(435.12e6), Command::Tune(433.32e6)]
        );
    }

    #[test]
    fn enter_tunes_to_the_cursor_and_brings_it_to_the_middle() {
        let mut app = app();
        app.spectrum
            .on_frame(frame(), sdr_detect::Levels::default());
        press(&mut app, KeyCode::Enter);
        assert!(app.commands.is_empty(), "no cursor, nowhere to go");
        app.spectrum.cursor = Some(0.75);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.commands, [Command::Tune(434.52e6)]);
        assert_eq!(app.spectrum.cursor, Some(0.5));
    }

    #[test]
    fn gain_keys_ask_the_source() {
        let mut app = app();
        type_keys(&mut app, "gGa");
        assert_eq!(
            app.commands,
            [
                Command::Gain(GainChange::Step(1)),
                Command::Gain(GainChange::Step(-1)),
                Command::Gain(GainChange::Auto),
            ]
        );
    }

    #[test]
    fn plus_and_minus_zoom() {
        let mut app = app();
        type_keys(&mut app, "++");
        assert_eq!(app.spectrum.magnification(), 4);
        type_keys(&mut app, "-");
        assert_eq!(app.spectrum.magnification(), 2);
    }

    #[test]
    fn spectrum_keys_do_nothing_on_other_screens() {
        let mut app = app();
        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.spectrum.cursor, None);
        assert!(!app.spectrum.paused);
    }

    #[test]
    fn q_and_ctrl_c_quit() {
        let mut app = app();
        press(&mut app, KeyCode::Char('q'));
        assert!(app.quit);

        let mut app = self::app();
        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.quit);
    }

    #[test]
    fn key_releases_are_ignored() {
        // Terminals with the kitty protocol report releases too.
        let mut app = app();
        let mut release = KeyEvent::from(KeyCode::Char('2'));
        release.kind = KeyEventKind::Release;
        app.on_key(release);
        assert_eq!(app.screen, Screen::Spectrum);
    }

    #[test]
    fn every_screen_has_a_distinct_key_from_one_to_eight() {
        let keys: Vec<u8> = Screen::ALL.iter().map(|s| s.key()).collect();
        assert_eq!(keys, [1, 2, 3, 4, 5, 6, 7, 8]);
    }
}
