//! Interface state and what the keys do to it; no drawing here.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use sdr_core::units::parse_hz;
use sdr_core::{GainChange, SourceInfo};

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
}

/// What the interface asks of the source; the reader thread carries it out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    Tune(f64),
    Gain(GainChange),
}

pub struct App {
    pub screen: Screen,
    pub help: bool,
    pub quit: bool,
    pub source: SourceInfo,
    pub status: SourceStatus,
    pub spectrum: SpectrumView,
    /// Asked of the source and not yet sent; the main loop drains it.
    pub commands: Vec<Command>,
    /// The frequency being typed after `f`.
    pub input: Option<String>,
    /// The last thing that went wrong, until the next key.
    pub notice: Option<String>,
}

/// Cursor steps as shares of the width: fine, and coarse with Shift.
const CURSOR_STEP: f64 = 0.005;
const CURSOR_LEAP: f64 = 0.05;

impl App {
    pub fn new(source: SourceInfo) -> Self {
        App {
            screen: Screen::Spectrum,
            help: false,
            quit: false,
            source,
            status: SourceStatus {
                rate: 0.0,
                state: SourceState::Running,
            },
            spectrum: SpectrumView::default(),
            commands: Vec::new(),
            input: None,
            notice: None,
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        self.notice = None;
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
            KeyCode::Esc if self.spectrum.cursor.is_some() => self.spectrum.cursor = None,
            KeyCode::Esc => self.quit = true,
            KeyCode::Char('?') => self.help = !self.help,
            KeyCode::Tab => self.step_screen(1),
            KeyCode::BackTab => self.step_screen(Screen::ALL.len() - 1),
            KeyCode::Char(c @ '1'..='8') => {
                self.screen = Screen::ALL[usize::from(c as u8 - b'1')];
            }
            _ if self.screen == Screen::Spectrum => self.on_spectrum_key(key),
            _ => {}
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
        app.spectrum.on_frame(frame());
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
        app.spectrum.on_frame(frame());
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
