//! Drawing the interface: the tabs, the screen that is open (a placeholder
//! for those not built yet), the status line and the help.

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use sdr_core::units::{format_hz, format_rate};

use crate::app::{App, Screen, SourceState};
use crate::mesh::Link;

/// The plan's minimum terminal; the uConsole gives about 160×45.
pub const MIN_WIDTH: u16 = 80;
pub const MIN_HEIGHT: u16 = 24;

/// Keys that work on every screen.
const KEYS: [(&str, &str); 5] = [
    ("?", "this help"),
    ("1–8", "switch screens"),
    ("Tab, Shift+Tab", "next, previous screen"),
    ("Esc", "close help, put the cursor or selection away, quit"),
    ("q", "quit"),
];

/// The spectrum's keys but the last; the density has them too.
const BAND_KEYS: [(&str, &str); 7] = [
    ("←, →", "move the cursor (Shift: faster)"),
    ("Enter", "tune to the cursor"),
    ("f", "type a frequency"),
    ("[, ]", "tune down, up by half the view"),
    ("+, -", "zoom in, out"),
    ("g, G, a", "gain up, down, automatic"),
    ("Space", "pause"),
];

const CHANNEL_KEYS: [(&str, &str); 6] = [
    ("↑, ↓", "select a channel (PgUp, PgDn, Home, End)"),
    ("+, -", "fewer, more seconds to a column"),
    ("Enter", "tune to the selected channel"),
    ("!", "alert on the selected channel (!), or no more"),
    ("R, R", "its alert records (!R); R once: no more"),
    ("X, X", "let go of the silent channels the app made"),
];

const PERIOD_KEYS: [(&str, &str); 3] = [
    ("↑, ↓", "select a channel (PgUp, PgDn, Home, End)"),
    ("+, -", "shorter, longer delays"),
    ("Enter", "tune to the selected channel"),
];

const MESH_KEYS: [(&str, &str); 6] = [
    ("↑, ↓", "select a node (PgUp, PgDn, Home, End)"),
    ("+, -", "less, more time to a column"),
    ("s", "sort by the last heard, by the fewest hops"),
    ("a", "only the nodes heard in the last 15 m, or all"),
    ("m", "start the list afresh: only what is heard from now"),
    ("X, X", "erase every node: those heard again come back"),
];

const LOG_KEYS: [(&str, &str); 7] = [
    ("↑, ↓", "select an event (PgUp, PgDn, Home, End)"),
    ("+, -", "raise, lower the level that hides weak events"),
    ("s", "sort by time, level, duration, frequency"),
    ("Enter", "tune to the selected event"),
    ("r", "save the selected event's IQ samples"),
    ("!", "alert on the selected event's band, or no more"),
    ("X, X", "erase the ended events, in the database too"),
];

/// The keys of the open screen, then those of every screen: all of them
/// at once no longer fit the smallest terminal.
fn keys(screen: Screen) -> Vec<(&'static str, &'static str)> {
    let own: &[(&str, &str)] = match screen {
        Screen::Spectrum | Screen::Density => &BAND_KEYS,
        Screen::Channels => &CHANNEL_KEYS,
        Screen::Periodicity => &PERIOD_KEYS,
        Screen::Log => &LOG_KEYS,
        Screen::Mesh => &MESH_KEYS,
        _ => &[],
    };
    let m = match screen {
        Screen::Spectrum => Some(("m", "reset max hold")),
        Screen::Density => Some(("m", "start the picture afresh")),
        _ => None,
    };
    own.iter().copied().chain(m).chain(KEYS).collect()
}

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        draw_too_small(frame, area);
        return;
    }
    let [tabs, body, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);
    frame.render_widget(tab_bar(app.screen), tabs);
    let sdr_screen = !matches!(
        app.screen,
        Screen::Mesh | Screen::Panorama | Screen::Decoders
    );
    match app.screen {
        screen if sdr_screen && app.status.state == SourceState::Off => {
            frame.render_widget(no_receiver(screen), body);
        }
        Screen::Spectrum => {
            crate::spectrum_ui::draw(frame, &app.spectrum, app.input.as_deref(), body);
        }
        Screen::Channels => {
            crate::channels_ui::draw(frame, &app.channels, &app.alerts, &app.source, body)
        }
        Screen::Density => {
            let (view, map) = (&app.spectrum, app.density.as_ref());
            crate::density_ui::draw(frame, view, map, app.input.as_deref(), body);
        }
        Screen::Periodicity => {
            crate::period_ui::draw(frame, &app.channels, app.delay_scale(), body);
        }
        Screen::Log => crate::log_ui::draw(frame, &app.log, app.picture(), body),
        Screen::Mesh => crate::mesh_ui::draw(frame, &app.mesh, body),
        screen => frame.render_widget(placeholder(screen), body),
    }
    draw_status(frame, app, status);
    if app.help {
        draw_help(frame, app.screen, body);
    }
}

/// The screen as plain text, one line per row, without colours: for
/// snapshots outside a terminal.
pub fn render_text(app: &App, width: u16, height: u16) -> String {
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height))
        .expect("a test backend cannot fail");
    terminal
        .draw(|frame| draw(frame, app))
        .expect("a test backend cannot fail");
    let buffer = terminal.backend().buffer();
    let mut text = String::new();
    for y in 0..height {
        let row: String = (0..width).map(|x| buffer[(x, y)].symbol()).collect();
        text.push_str(row.trim_end());
        text.push('\n');
    }
    text
}

fn draw_too_small(frame: &mut Frame, area: Rect) {
    let text = format!(
        "The terminal is {}×{}; SDR Viewer needs at least {MIN_WIDTH}×{MIN_HEIGHT}.",
        area.width, area.height
    );
    frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }), area);
}

fn tab_bar(current: Screen) -> Line<'static> {
    let mut spans = Vec::new();
    for screen in Screen::ALL {
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
        }
        let selected = screen == current;
        let base = if selected {
            Style::new()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new()
        };
        let digit = if selected {
            base
        } else {
            base.fg(Color::Yellow)
        };
        spans.push(Span::styled(screen.key().to_string(), digit));
        spans.push(Span::styled(format!(" {}", screen.tab()), base));
    }
    Line::from(spans)
}

fn placeholder(screen: Screen) -> Paragraph<'static> {
    let text = Text::from(vec![
        Line::from(screen.question()).bold(),
        Line::default(),
        Line::from(format!(
            "This screen arrives in phase {} of the plan.",
            screen.phase()
        ))
        .dark_gray(),
    ]);
    Paragraph::new(text)
        .centered()
        .block(Block::bordered().title(format!(" {} ", screen.title())))
}

/// A screen of the receiver in a run that has none.
fn no_receiver(screen: Screen) -> Paragraph<'static> {
    let text = Text::from(vec![
        Line::from(screen.question()).bold(),
        Line::default(),
        Line::from("No receiver in this run: it was started with --mesh alone.").dark_gray(),
        Line::from("A recording or --rtlsdr beside --mesh fills this screen.").dark_gray(),
    ]);
    Paragraph::new(text)
        .centered()
        .block(Block::bordered().title(format!(" {} ", screen.title())))
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let separator = || Span::raw(" │ ").dark_gray();
    if app.status.state == SourceState::Off && app.notice.is_none() && app.done.is_none() {
        let link = match &app.mesh.link {
            Link::Off => Span::raw("no meshtasticd").yellow(),
            Link::Connecting(addr) => Span::raw(format!("connecting to {addr}")).yellow(),
            Link::Connected => Span::raw("meshtasticd connected"),
            Link::Lost(why) => Span::raw(format!("meshtasticd: {why}")).red(),
        };
        let left = Line::from(vec![Span::raw("mesh only").bold(), separator(), link]);
        frame.render_widget(left, area);
        frame.render_widget(help_and_quit().right_aligned(), area);
        return;
    }
    let state = match (&app.notice, &app.done, &app.status.state) {
        (Some(notice), ..) => Span::raw(notice.clone()).red(),
        (None, Some(done), _) => Span::raw(done.clone()).green(),
        (None, None, state) => match state {
            SourceState::Running => Span::raw(format!("rx {}", format_rate(app.status.rate))),
            SourceState::Ended => Span::raw("ended").yellow(),
            SourceState::Failed(message) => Span::raw(format!("error: {message}")).red(),
            SourceState::Off => Span::raw("no receiver").yellow(),
        },
    };
    let mut left = vec![
        Span::raw(app.source.description.clone()).bold(),
        separator(),
        Span::raw(format_hz(app.source.center_hz)),
        separator(),
        Span::raw(format_rate(f64::from(app.source.sample_rate))),
        separator(),
        state,
    ];
    if app.notice.is_none() && app.done.is_none() {
        left.extend(health(app));
    }
    let left = Line::from(left);
    let right = alert_and_help(app);
    let right_width = u16::try_from(right.width()).unwrap_or(u16::MAX);
    let [left_area, right_area] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(right_width)]).areas(area);
    frame.render_widget(left, left_area);
    frame.render_widget(right, right_area);
}

/// The latest alert, if there was one, before the keys of help and quit.
fn alert_and_help(app: &App) -> Line<'static> {
    let mut line = help_and_quit();
    if let Some(told) = app.alerts.told() {
        let alert = Span::raw(told.text);
        let alert = if told.bright {
            alert.white().on_red().bold()
        } else {
            alert.red()
        };
        line.spans.splice(0..0, [alert, Span::raw("  ")]);
    }
    line
}

fn help_and_quit() -> Line<'static> {
    Line::from(vec![
        Span::raw("?").yellow(),
        Span::raw(" help  "),
        Span::raw("q").yellow(),
        Span::raw(" quit"),
    ])
}

fn draw_help(frame: &mut Frame, screen: Screen, body: Rect) {
    let keys = keys(screen);
    let key_width = keys
        .iter()
        .map(|(key, _)| key.chars().count())
        .max()
        .unwrap_or(0);
    let lines: Vec<Line> = keys
        .iter()
        .map(|(key, what)| {
            Line::from(vec![
                Span::raw(format!(" {key:key_width$}  ")).yellow(),
                Span::raw(format!("{what} ")),
            ])
        })
        .collect();
    let width = lines.iter().map(Line::width).max().unwrap_or(0) + 2;
    let height = lines.len() + 2;
    let [area] = Layout::horizontal([Constraint::Length(width.try_into().unwrap_or(u16::MAX))])
        .flex(Flex::Center)
        .areas(body);
    let [area] = Layout::vertical([Constraint::Length(height.try_into().unwrap_or(u16::MAX))])
        .flex(Flex::Center)
        .areas(area);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(format!(" Keys: {} ", screen.title()))),
        area,
    );
}

/// Clipped samples from this share on are told; from `OVERLOAD` in red.
const CLIP_WORTH_TELLING: f64 = 0.001;
const OVERLOAD: f64 = 0.01;

/// What is wrong with reception, if anything: the receiver overloaded,
/// samples lost.
fn health(app: &App) -> Vec<Span<'static>> {
    let status = &app.status;
    let mut spans = Vec::new();
    if status.clipped >= CLIP_WORTH_TELLING {
        let percent = status.clipped * 100.0;
        let text = if percent < 10.0 {
            format!(" · clip {percent:.1}%")
        } else {
            format!(" · clip {percent:.0}%")
        };
        spans.push(if status.clipped >= OVERLOAD {
            Span::raw(text).red()
        } else {
            Span::raw(text).yellow()
        });
    }
    if status.lost > 0 {
        let seconds = status.lost as f64 / f64::from(app.source.sample_rate.max(1));
        spans.push(Span::raw(format!(" · lost {seconds:.2} s")).red());
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::SourceStatus;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use sdr_core::SourceInfo;
    use sdr_detect::{Alert, Event};
    use sdr_mesh::update::MeshUpdate;
    use std::time::Duration;

    fn app() -> App {
        let mut app = App::new(SourceInfo {
            sample_rate: 2_400_000,
            center_hz: 433.92e6,
            description: "generator".into(),
        });
        app.status = SourceStatus::new(SourceState::Running, 2_398_000.0);
        app
    }

    fn render(app: &App, width: u16, height: u16) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        terminal
    }

    /// The app after two seconds of a quiet band with one carrier and one
    /// burst that has just ended (it stays in max hold).
    fn app_with_signal() -> App {
        use sdr_core::generator::{Gate, Generator, GeneratorConfig, Tone};
        use sdr_core::{Iq, Source};
        use sdr_detect::{Density, DensityConfig, Detector, DetectorConfig};
        use sdr_dsp::{Analyzer, AnalyzerConfig};

        let mut app = app();
        let mut generator = Generator::new(GeneratorConfig {
            sample_rate: 1_024_000,
            center_hz: 433.92e6,
            noise_dbfs: Some(-50.0),
            tones: vec![
                Tone {
                    freq_hz: 433.72e6,
                    level_dbfs: -30.0,
                    gate: None,
                },
                Tone {
                    freq_hz: 434.22e6,
                    level_dbfs: -20.0,
                    gate: Some(Gate {
                        period_s: 5.0,
                        on_s: 1.0,
                    }),
                },
            ],
            seed: 5,
        });
        app.source = generator.info();
        let mut analyzer = Analyzer::new(AnalyzerConfig {
            fft_size: 1024,
            average: 100,
        });
        let mut detector = Detector::new(DetectorConfig::default());
        let mut density = Density::new(DensityConfig::default());
        let mut buf = vec![Iq::new(0.0, 0.0); 1_024_000];
        for _ in 0..2 {
            generator.read(&mut buf).unwrap();
            analyzer.push(&buf, &generator.info(), |frame| {
                detector.push(&frame, |_| {});
                density.push(&frame);
                app.spectrum.on_frame(frame, detector.levels());
            });
        }
        app.on_density(density.map().clone());
        app
    }

    #[test]
    fn spectrum_waits_for_samples() {
        insta::assert_snapshot!(render(&app(), 80, 24).backend());
    }

    #[test]
    fn spectrum_and_waterfall_at_the_minimum_size() {
        insta::assert_snapshot!(render(&app_with_signal(), 80, 24).backend());
    }

    #[test]
    fn spectrum_with_a_cursor_on_the_uconsole_screen() {
        let mut app = app_with_signal();
        app.spectrum.cursor = Some(0.305);
        app.spectrum.paused = true;
        insta::assert_snapshot!(render(&app, 160, 45).backend());
    }

    #[test]
    fn zoomed_in_on_the_carrier_while_typing_a_frequency() {
        let mut app = app_with_signal();
        app.spectrum.cursor = Some(0.305);
        app.spectrum.zoom_in();
        app.spectrum.zoom_in();
        app.input = Some("433.5M".into());
        insta::assert_snapshot!(render(&app, 80, 24).backend());
    }

    /// A minute of a band: a named channel with a burst every 5 s, one
    /// made for a carrier that is still on, one made for a signal that
    /// stopped half a minute ago, and one of the user's far from here.
    fn app_on_the_channels() -> App {
        use crate::channels::ChannelsView;
        use crate::log::tests::event;
        use sdr_detect::Channel;
        use std::time::Duration;

        let mut app = app();
        app.screen = Screen::Channels;
        let mine = |name: &str, center_hz: f64| Channel {
            name: Some(name.into()),
            center_hz,
            width_hz: 25e3,
            hidden: false,
        };
        app.channels = ChannelsView::new(vec![
            mine("LPD 17", 433.475e6),
            mine("a name too long for its column", 144.8e6),
            Channel {
                hidden: true,
                ..mine("spike", 433.92e6)
            },
        ]);
        let mut events: Vec<_> = (0..12)
            .map(|i| event(i * 5000 + 200, 300, 475.0, 20.0))
            .collect();
        events.push(event(10_000, 20_000, 700.0, 30.0));
        events.push(event(0, 60_000, 920.0, 12.0));
        app.on_events(events);
        app.on_frame(
            sdr_core::SpectrumFrame {
                time: Duration::from_secs(60),
                center_hz: 433.92e6,
                sample_rate: 2_400_000,
                power_db: vec![-70.0; 16],
            },
            sdr_detect::Levels::default(),
        );
        app.on_open_events(vec![event(48_500, 11_500, 1100.0, 25.0)]);
        app
    }

    /// The channels' minute with the named channel — a burst every 5 s
    /// — on the periodicity screen.
    fn app_on_the_periodicity() -> App {
        let mut app = app_on_the_channels();
        app.screen = Screen::Periodicity;
        app.channels.move_selection(1);
        app.channels.move_selection(1);
        app.channels.refresh_periods();
        app
    }

    #[test]
    fn periodicity_at_the_minimum_size() {
        insta::assert_snapshot!(render(&app_on_the_periodicity(), 80, 24).backend());
    }

    #[test]
    fn periodicity_on_the_uconsole_screen() {
        insta::assert_snapshot!(render(&app_on_the_periodicity(), 160, 45).backend());
    }

    #[test]
    fn the_cursor_stands_on_the_main_period_and_names_it() {
        let app = app_on_the_periodicity();
        let terminal = render(&app, 80, 24);
        let buffer = terminal.backend().buffer();
        let text = render_text(&app, 80, 24);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines[1],
            " LPD 17 · 433.475 MHz · delays to 10 s in 10 ms steps"
        );
        assert_eq!(lines[2], " Repeats every 5.00 s 92%");
        assert!(lines[4].starts_with("−60 s") && lines[4].ends_with("now"));
        assert!(lines[22].starts_with('0') && lines[22].ends_with("10 s"));
        // 5 s of 10 is the middle of the 80 columns; the plot is rows
        // 5 to 21.
        for y in 5..22 {
            assert_eq!(buffer[(40, y)].style().fg, Some(Color::Yellow), "row {y}");
            assert_ne!(buffer[(30, y)].style().fg, Some(Color::Yellow), "row {y}");
        }
        assert!(lines[5].contains(" 5.00 s"), "{text}");
        // The envelope: the minute in 80 columns, 0.3 s of signal in
        // the first 0.75 s.
        assert_eq!(lines[3].chars().count(), 80);
        assert!(lines[3].starts_with("▃·····"), "{text}");
    }

    #[test]
    fn the_periodicity_screen_says_what_it_waits_for() {
        let mut app = app();
        app.screen = Screen::Periodicity;
        let text = render_text(&app, 80, 24);
        assert!(text.contains("No channels yet"), "{text}");
        let mut app = app_on_the_channels();
        app.screen = Screen::Periodicity;
        let text = render_text(&app, 80, 24);
        assert!(text.contains("Select a channel: ↑ ↓"), "{text}");
        // A minute is too little for delays up to ten: 120 steps.
        let mut app = app_on_the_periodicity();
        app.delays = Some(3);
        let text = render_text(&app, 80, 24);
        assert!(
            text.contains("Too little yet for these delays: 60.0 s of the 300 s it takes"),
            "{text}"
        );
        // The carrier repeats nothing.
        let mut app = app_on_the_periodicity();
        app.channels.move_selection(2);
        let text = render_text(&app, 80, 24);
        assert!(text.contains("No period within 10 s: + and -"), "{text}");
    }

    #[test]
    fn the_strip_tells_the_period_of_a_channel_that_has_one() {
        let mut app = app_on_the_channels();
        app.channels.refresh_periods();
        let text = render_text(&app, 80, 24);
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[3].starts_with("   LPD 17"), "{text}");
        assert!(lines[3].ends_with("▂····  5.00 s  5.5%   4 s"), "{text}");
        // What is on without a rhythm has none.
        assert!(lines[5].ends_with("█           26%   now"), "{text}");
    }

    #[test]
    fn the_strip_marks_a_channel_with_an_alert_and_one_that_records() {
        use crate::alerts::AlertsView;
        use sdr_detect::Rule;
        let mut app = app_on_the_channels();
        let rule = |name: &str, center_hz: f64, width_hz: f64, record| Rule {
            name: name.into(),
            low_hz: center_hz - width_hz / 2.0,
            high_hz: center_hz + width_hz / 2.0,
            min_duration: std::time::Duration::ZERO,
            min_above_db: 10.0,
            record,
        };
        app.alerts = AlertsView::new(
            vec![
                rule("gate", 433.475e6, 25e3, true),
                rule("far", 144.8e6, 25e3, false),
                // A wider band than a channel's is no channel's rule.
                rule("ISM", 433.92e6, 1.74e6, true),
            ],
            true,
        );
        let terminal = render(&app, 80, 24);
        let text = render_text(&app, 80, 24);
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[2].starts_with("!  a name too"), "{text}");
        assert!(lines[3].starts_with("!R LPD 17"), "{text}");
        assert!(lines[4].starts_with("   433.700 MHz"), "{text}");
        let buffer = terminal.backend().buffer();
        for (x, y) in [(0, 2), (0, 3), (1, 3)] {
            assert_eq!(buffer[(x, y)].style().fg, Some(Color::Red), "({x}, {y})");
        }
    }

    #[test]
    fn the_help_of_every_screen_fits_the_smallest_terminal() {
        for screen in Screen::ALL {
            let mut app = app();
            app.screen = screen;
            app.help = true;
            let text = render_text(&app, MIN_WIDTH, MIN_HEIGHT);
            assert!(
                text.contains(&format!("┌ Keys: {} ", screen.title())),
                "{text}"
            );
            assert!(text.contains("│ q ") && text.contains("┘"), "{text}");
            let has = |what: &str| text.contains(what);
            assert_eq!(
                has("zoom in, out"),
                screen == Screen::Spectrum || screen == Screen::Density
            );
            assert_eq!(has("reset max hold"), screen == Screen::Spectrum);
            assert_eq!(has("start the picture afresh"), screen == Screen::Density);
            assert_eq!(has("seconds to a column"), screen == Screen::Channels);
            assert_eq!(has("shorter, longer delays"), screen == Screen::Periodicity);
            assert_eq!(has("sort by time"), screen == Screen::Log);
        }
    }

    #[test]
    fn the_channel_strip_before_any_signal() {
        let mut app = app();
        app.screen = Screen::Channels;
        let text = render_text(&app, 80, 24);
        assert!(text.contains(" 0 channels · 1 s a column"), "{text}");
        assert!(
            text.contains("No channels yet: one is made where a signal is found."),
            "{text}"
        );
    }

    #[test]
    fn the_channel_strip_at_the_minimum_size() {
        insta::assert_snapshot!(render(&app_on_the_channels(), 80, 24).backend());
    }

    #[test]
    fn the_channel_strip_with_a_selection_and_ten_seconds_to_a_column() {
        let mut app = app_on_the_channels();
        app.channels.zoom_out();
        app.channels.move_selection(2);
        app.channels.move_selection(1);
        let terminal = render(&app, 160, 45);
        insta::assert_snapshot!(terminal.backend());
        let buffer = terminal.backend().buffer();
        // The second row is selected; the fourth is on now; the first is
        // not in the received band. The names begin after the alerts'
        // marks, which the selection takes in.
        assert!(
            buffer[(0, 3)]
                .style()
                .add_modifier
                .contains(Modifier::REVERSED)
        );
        let style = |y: u16| buffer[(3, y)].style();
        assert!(style(3).add_modifier.contains(Modifier::REVERSED));
        assert!(!style(4).add_modifier.contains(Modifier::REVERSED));
        assert!(style(5).add_modifier.contains(Modifier::BOLD));
        assert_eq!(style(2).fg, Some(Color::DarkGray));
    }

    #[test]
    fn rows_are_aligned_in_time_and_tell_share_and_silence() {
        let text = render_text(&app_on_the_channels(), 80, 24);
        let rows: Vec<&str> = text.lines().collect();
        assert_eq!(rows[1], " 4 channels · 1 s a column · the last 44 s");
        // 44 columns: seconds 16 to 59, after the alerts' marks.
        // A 0.3 s burst every 5 s, the last at 55.2 s.
        assert!(
            rows[3].starts_with("   LPD 17       ····▂····▂····▂····"),
            "{text}"
        );
        // No period is told before one is worked out.
        assert!(rows[3].ends_with("▂····          5.5%   4 s"), "{text}");
        // On from 10 s to 30 s.
        assert!(
            rows[4].starts_with("   433.700 MHz  ██████████████····"),
            "{text}"
        );
        assert!(rows[4].ends_with("  32%  30 s"), "{text}");
        // On since 48.5 s and still.
        assert!(
            rows[5].ends_with("·▄███████████           26%   now"),
            "{text}"
        );
        // Never heard here.
        assert!(rows[2].starts_with("   a name too l ·····"), "{text}");
        assert!(rows[2].ends_with("····            0%     —"), "{text}");
        assert!(
            rows[22].contains("−44 s") && rows[22].ends_with("now"),
            "{text}"
        );
        // The hidden channel took its event and has no row.
        assert!(
            !text.contains("spike") && !text.contains("433.920 MHz  "),
            "{text}"
        );
    }

    #[test]
    fn a_long_list_scrolls_to_keep_the_selection_in_view() {
        use crate::log::tests::event;
        let mut app = app();
        app.screen = Screen::Channels;
        app.on_events(
            (0..40)
                .map(|i| event(0, 40, f64::from(i) * 25.0, 20.0))
                .collect(),
        );
        let shown = |app: &App, khz: u32| {
            let label = format!("{:.3} MHz  ", 433.0 + f64::from(khz) / 1000.0);
            render_text(app, 80, 24).contains(&label)
        };
        // 20 rows of the 40.
        assert!(shown(&app, 0) && shown(&app, 475) && !shown(&app, 500));
        app.channels.move_selection(1);
        app.channels.move_selection(30);
        assert!(!shown(&app, 475) && shown(&app, 500) && shown(&app, 750));
        app.channels.move_selection(isize::MAX);
        assert!(shown(&app, 975) && shown(&app, 500) && !shown(&app, 475));
    }

    fn app_on_the_density() -> App {
        let mut app = app_with_signal();
        app.screen = Screen::Density;
        app
    }

    #[test]
    fn density_waits_for_samples() {
        let mut app = app();
        app.screen = Screen::Density;
        assert!(render_text(&app, 80, 24).contains("Waiting for samples…"));
        // A spectrum alone is not enough, nor a map of another one.
        let mut app = app_on_the_density();
        app.density = None;
        assert!(render_text(&app, 80, 24).contains("Waiting for samples…"));
    }

    #[test]
    fn density_at_the_minimum_size() {
        insta::assert_snapshot!(render(&app_on_the_density(), 80, 24).backend());
    }

    #[test]
    fn density_with_a_cursor_on_the_burst_on_the_uconsole_screen() {
        let mut app = app_on_the_density();
        // 300 kHz above the centre of 1.024 MHz; on for the first of the
        // 2 s, which by now weighs a little less than the second.
        app.spectrum.cursor = Some(0.793);
        let terminal = render(&app, 160, 45);
        let text = render_text(&app, 160, 45);
        assert!(
            text.contains("cursor 434.217 MHz  busy 49% of the time  half-life 30 s"),
            "{text}"
        );
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn the_density_is_brightest_where_the_signal_always_is() {
        let app = app_on_the_density();
        let terminal = render(&app, 80, 24);
        let buffer = terminal.backend().buffer();
        // The carrier, as in the waterfall's test: always there, at the
        // top of the scale, on one level or two.
        let carrier = 5 + 22;
        let top = &buffer[(carrier, 2)];
        assert_eq!(top.symbol(), "▀");
        assert!(
            matches!(top.fg, Color::Rgb(255, g, _) if g > 235),
            "{top:?}"
        );
        // Between its level and the noise it never was: nothing is drawn.
        assert_eq!(buffer[(carrier + 6, 10)].symbol(), " ");
        // The noise is there all the time too.
        let noise = &buffer[(carrier + 6, 21)];
        assert_eq!(noise.symbol(), "▀");
        assert!(
            matches!(noise.fg, Color::Rgb(r, _, _) if r > 200),
            "{noise:?}"
        );
    }

    #[test]
    fn the_densitys_zoom_and_typing_are_the_spectrums() {
        let mut app = app_on_the_density();
        app.spectrum.zoom_in();
        app.spectrum.paused = true;
        let text = render_text(&app, 80, 24);
        assert!(text.contains("half-life 30 s  ×2  paused"), "{text}");
        assert!(
            text.contains("433.664 MHz") && text.contains("434.176 MHz"),
            "{text}"
        );
        app.input = Some("433.5M".into());
        assert!(render_text(&app, 80, 24).contains("frequency 433.5M▏"));
    }

    #[test]
    fn the_header_names_the_band_one_is_in() {
        // 433.92 MHz: the 433 MHz ISM band inside the amateurs' 70 cm.
        let mut app = app_with_signal();
        let text = render_text(&app, 80, 24);
        assert!(
            text.lines().nth(1).unwrap().ends_with("ISM 433 / LPD"),
            "{text}"
        );
        // The density shows the same piece of the air.
        app.screen = Screen::Density;
        let text = render_text(&app, 80, 24);
        assert!(
            text.lines().nth(1).unwrap().ends_with("ISM 433 / LPD"),
            "{text}"
        );
        // Not while a frequency is typed over the header.
        app.input = Some("433.5M".into());
        assert!(!render_text(&app, 80, 24).contains("ISM 433"));
    }

    #[test]
    fn a_cursor_in_another_band_names_it() {
        // 1.024 MHz around 435 MHz: the middle is the amateurs' 70 cm,
        // the left edge is still ISM 433.
        let mut app = app_with_signal();
        let mut frame = app.spectrum.latest().unwrap().clone();
        frame.center_hz = 435e6;
        app.on_frame(frame, sdr_detect::Levels::default());
        app.spectrum.cursor = Some(0.5);
        let header = |app: &App| {
            render_text(app, 100, 24)
                .lines()
                .nth(1)
                .unwrap()
                .to_string()
        };
        assert!(header(&app).ends_with("amateur 70 cm"), "{}", header(&app));
        assert!(header(&app).contains(" MHz  -"), "{}", header(&app));
        app.spectrum.cursor = Some(0.1);
        assert!(
            header(&app).contains(" MHz  ISM 433 / LPD  -"),
            "{}",
            header(&app)
        );
        app.screen = Screen::Density;
        assert!(header(&app).ends_with("amateur 70 cm"), "{}", header(&app));
        // Between the bands there is nothing to say.
        let mut frame = app.spectrum.latest().unwrap().clone();
        frame.center_hz = 700e6;
        app.on_frame(frame, sdr_detect::Levels::default());
        app.screen = Screen::Spectrum;
        assert!(header(&app).ends_with("dBFS"), "{}", header(&app));
    }

    #[test]
    fn a_zoomed_view_names_its_magnification_and_its_own_edges() {
        let mut app = app_with_signal();
        app.spectrum.zoom_in();
        let text = render_text(&app, 80, 24);
        assert!(text.contains("×2"), "{text}");
        // Half of 1.024 MHz around 433.92.
        assert!(
            text.contains("433.664 MHz") && text.contains("434.176 MHz"),
            "{text}"
        );
    }

    #[test]
    fn a_notice_takes_the_place_of_the_source_state() {
        let mut app = app();
        app.notice = Some("not supported: a recording cannot be retuned".into());
        let text = render_text(&app, 100, 24);
        assert!(
            text.contains("│ not supported: a recording cannot be retuned"),
            "{text}"
        );
        assert!(!text.contains("rx 2.398"), "{text}");
    }

    #[test]
    fn overload_and_lost_samples_are_told_beside_the_rate() {
        let mut app = app();
        let text = render_text(&app, 100, 24);
        assert!(!text.contains("clip") && !text.contains("lost"), "{text}");
        // A few clipped peaks are not worth a word.
        app.status.clipped = 0.0004;
        assert!(!render_text(&app, 100, 24).contains("clip"));
        app.status.clipped = 0.004;
        app.status.lost = 1_200_000;
        let text = render_text(&app, 100, 24);
        assert!(
            text.contains("rx 2.398 MS/s · clip 0.4% · lost 0.50 s"),
            "{text}"
        );
        app.status.clipped = 0.127;
        assert!(render_text(&app, 100, 24).contains("· clip 13% ·"));
        let terminal = render(&app, 100, 24);
        let buffer = terminal.backend().buffer();
        let x = (0..100)
            .find(|&x| buffer[(x, 23)].symbol() == "c" && buffer[(x + 1, 23)].symbol() == "l")
            .unwrap();
        assert_eq!(buffer[(x, 23)].fg, Color::Red);
        // They stay when the stream has ended.
        app.status.state = SourceState::Ended;
        assert!(render_text(&app, 100, 24).contains("ended · clip 13% · lost 0.50 s"));
    }

    #[test]
    fn an_alert_is_told_before_the_help_brightly_then_dimly() {
        let mut app = app();
        let alert = |secs: f64| Alert {
            rule: "gate".into(),
            record: false,
            band: (433.9e6, 434.0e6),
            number: 1,
            event: Event {
                start: Duration::from_secs_f64(secs - 0.5),
                duration: Duration::from_millis(500),
                low_hz: 433.9e6,
                high_hz: 433.91e6,
                peak_hz: 433.905e6,
                peak_dbfs: -40.0,
                above_floor_db: 30.0,
            },
        };
        assert!(app.on_alerts(&[alert(1.0), alert(2.0)]));
        let text = render_text(&app, 80, 24);
        let status = text.lines().last().unwrap();
        assert!(
            status.ends_with("rx 2.398 MS/s   ! gate ×2  ? help  q quit"),
            "{status}"
        );
        let colours = |app: &App| {
            let terminal = render(app, 80, 24);
            let buffer = terminal.backend().buffer();
            let x = (0..80).find(|&x| buffer[(x, 23)].symbol() == "!").unwrap();
            (buffer[(x, 23)].fg, buffer[(x, 23)].bg)
        };
        assert_eq!(colours(&app), (Color::White, Color::Red));
        app.alerts.set_now(Duration::from_secs(12));
        assert_eq!(colours(&app), (Color::Red, Color::Reset));
    }

    #[test]
    fn what_was_done_takes_the_place_of_the_source_state_too() {
        let mut app = app();
        app.done = Some("IQ saved: burst (0.14 s)".into());
        let text = render_text(&app, 100, 24);
        assert!(text.contains("│ IQ saved: burst (0.14 s)"), "{text}");
        assert!(!text.contains("rx 2.398"), "{text}");
        // What went wrong comes first.
        app.notice = Some("the disk is full".into());
        let text = render_text(&app, 100, 24);
        assert!(text.contains("│ the disk is full"), "{text}");
    }

    #[test]
    fn the_waterfall_is_brighter_where_the_carrier_is() {
        let app = app_with_signal();
        let terminal = render(&app, 80, 24);
        let buffer = terminal.backend().buffer();
        let luma = |x: u16, y: u16| match buffer[(x, y)].fg {
            Color::Rgb(r, g, b) => u32::from(r) + u32::from(g) + u32::from(b),
            other => panic!("{other:?} at {x},{y}"),
        };
        // The carrier is 200 kHz below the centre of 1.024 MHz: 30.5% of
        // the 75 plot columns after the 5 of the gutter.
        let carrier = 5 + 22;
        let y = 20;
        assert_eq!(buffer[(carrier, y)].symbol(), "▀");
        assert!(luma(carrier, y) > luma(carrier - 6, y) + 150);
        assert!(luma(carrier, y) > luma(carrier + 6, y) + 150);
        // The noise itself stays dark, so weak signals stand out.
        assert!(luma(carrier - 6, y) < 100, "{}", luma(carrier - 6, y));
    }

    #[test]
    fn the_detectors_threshold_is_a_dotted_line_behind_the_bars() {
        let app = app_with_signal();
        let terminal = render(&app, 80, 24);
        let buffer = terminal.backend().buffer();
        // Noise at -78 dBFS a bin under a scale from -80: the threshold,
        // 8 dB over the floor, is 10 dB up the 50 dB of 8 rows — in the
        // second row from the bottom, past its middle.
        let y = 8;
        let quiet = 5 + 10;
        assert_eq!(buffer[(quiet, y)].symbol(), "⠒");
        assert_eq!(buffer[(quiet, y)].fg, Color::Rgb(160, 100, 50));
        // The carrier's bar stands in front of it.
        let carrier = 5 + 22;
        assert_eq!(buffer[(carrier, y)].symbol(), "█");
        // The line goes on over what max hold kept of the burst.
        let burst = 5 + 59;
        assert_eq!(buffer[(burst, y)].symbol(), "⠒");
        assert_eq!(buffer[(burst, y)].bg, Color::Rgb(70, 70, 80));
    }

    #[test]
    fn the_header_says_how_far_over_the_floor_the_peak_is() {
        let text = render_text(&app_with_signal(), 80, 24);
        assert!(
            text.contains("peak 433.720 MHz  -30.0 dBFS  +48 dB above floor"),
            "{text}"
        );
        let mut app = app_with_signal();
        app.spectrum.cursor = Some(0.9);
        let text = render_text(&app, 80, 24);
        assert!(text.contains("dBFS  +"), "{text}");
        assert!(text.contains(" dB above floor"), "{text}");
    }

    fn app_with_events() -> App {
        use crate::log::tests::event;
        let mut app = app();
        app.screen = Screen::Log;
        app.log.on_events([
            event(2_040, 307, 1_200.0, 46.2),
            event(4_080, 45, 425.0, 13.4),
            event(4_130, 10, 602.0, 8.7),
            event(4_180, 41, 350.0, 11.9),
            event(64_500, 2_300, 924.5, 21.0),
        ]);
        app.log.set_open(vec![event(0, 3_725_000, 500.0, 40.3)]);
        app
    }

    #[test]
    fn the_log_lists_events_newest_first_under_their_counts() {
        insta::assert_snapshot!(render(&app_with_events(), 80, 24).backend());
    }

    #[test]
    fn a_log_with_a_clock_shows_the_date_and_time_of_day_at_the_minimum_size() {
        use crate::log::WallClock;
        use crate::log::tests::{event, noon};
        use std::time::Duration;
        let mut app = app();
        app.screen = Screen::Log;
        app.log.set_clock(WallClock {
            stream_start: noon(),
            utc_offset_s: 7200,
        });
        app.log.on_events([event(64_500, 2_300, 924.5, 21.0)]);
        app.log.set_open(vec![event(0, 3_725_000, 500.0, 40.3)]);
        app.log.load_history([(
            noon() - Duration::from_secs(86_400),
            event(2_040, 307, 1_200.0, 46.2),
        )]);
        insta::assert_snapshot!(render(&app, 80, 24).backend());
    }

    /// The log with its newest row selected and that event's picture: a
    /// burst in the middle of 80 ms, low in a band of 4 kHz.
    fn app_with_a_picture() -> App {
        use crate::fragment::tests::fragment;
        use crate::log::tests::event;
        use ratatui::crossterm::event::{KeyCode, KeyEvent};
        let mut app = app();
        app.screen = Screen::Log;
        app.log
            .on_events([event(500, 50, 100.0, 20.0), event(1_020, 40, 102.0, 48.0)]);
        app.on_key(KeyEvent::from(KeyCode::Down));
        app.on_picture(&event(1_020, 40, 102.0, 48.0), Some(fragment()));
        app
    }

    #[test]
    fn the_selected_event_has_its_picture_under_the_list() {
        insta::assert_snapshot!(render(&app_with_a_picture(), 80, 24).backend());
    }

    #[test]
    fn the_picture_is_bright_where_and_when_the_event_was() {
        let terminal = render(&app_with_a_picture(), 80, 24);
        let buffer = terminal.backend().buffer();
        let luma = |color| match color {
            Color::Rgb(r, g, b) => u32::from(r) + u32::from(g) + u32::from(b),
            other => panic!("{other:?}"),
        };
        // 68 columns after the gutter of 12, and 6 rows from row 16 — 12
        // pixels up the band. The burst fills the middle half of the
        // time and the third quarter of the band from the top: pixels 6
        // to 8, the fourth row and the upper half of the fifth.
        let (middle, early) = (12 + 34, 12 + 5);
        assert!(luma(buffer[(middle, 19)].fg) > 300);
        assert!(luma(buffer[(middle, 19)].bg) > 300);
        assert!(luma(buffer[(middle, 20)].fg) > 300);
        assert!(luma(buffer[(middle, 20)].bg) < 100);
        assert!(luma(buffer[(middle, 16)].fg) < 100);
        assert!(luma(buffer[(early, 19)].fg) < 100);
        // The event's start and end are marked under it.
        assert_eq!(buffer[(12 + 17, 22)].symbol(), "▲");
        assert_eq!(buffer[(12 + 51, 22)].symbol(), "▲");
    }

    #[test]
    fn a_picture_that_is_gone_or_on_its_way_says_so() {
        use crate::log::tests::event;
        let mut app = app_with_a_picture();
        app.on_picture(&event(1_020, 40, 102.0, 48.0), None);
        let text = render_text(&app, 80, 24);
        assert!(
            text.contains("The samples of this event are no longer held."),
            "{text}"
        );
    }

    #[test]
    fn a_held_log_marks_the_selected_row_and_says_how_to_let_go() {
        let mut app = app_with_events();
        app.log.move_selection(1);
        app.log.move_selection(1);
        let terminal = render(&app, 100, 24);
        let buffer = terminal.backend().buffer();
        let reversed = |y: u16| buffer[(10, y)].modifier.contains(Modifier::REVERSED);
        // Tabs, counts, column names, then the rows.
        assert!(!reversed(3) && reversed(4) && !reversed(5));
        let text = render_text(&app, 100, 24);
        assert!(text.contains("held, 0 new — Esc follows again"), "{text}");
    }

    #[test]
    fn a_long_log_scrolls_to_keep_the_selection_in_sight() {
        use crate::log::tests::event;
        let mut app = app();
        app.screen = Screen::Log;
        app.log
            .on_events((0..100).map(|i| event(i * 1000, 50, 100.0, 20.0)));
        app.log.move_selection(1);
        let text = render_text(&app, 80, 24);
        // 20 rows fit: the newest, 1:39, down to 1:20.
        assert!(
            text.contains("1:39.00") && text.contains("1:20.00"),
            "{text}"
        );
        assert!(!text.contains("1:19.00"), "{text}");
        app.log.move_selection(25);
        let text = render_text(&app, 80, 24);
        assert!(
            text.contains("1:14.00") && text.contains("1:33.00"),
            "{text}"
        );
        assert!(
            !text.contains("1:34.00") && !text.contains("1:13.00"),
            "{text}"
        );
    }

    #[test]
    fn an_empty_log_says_what_it_waits_for() {
        let mut app = app();
        app.screen = Screen::Log;
        app.log.raise_filter();
        let text = render_text(&app, 80, 24);
        assert!(text.contains("No events above +11 dB yet."), "{text}");
        assert!(text.contains("0 events · 0 hidden below +11 dB"), "{text}");
    }

    #[test]
    fn help_over_the_log_screen() {
        let mut app = app();
        app.screen = Screen::Log;
        app.help = true;
        insta::assert_snapshot!(render(&app, 80, 24).backend());
    }

    #[test]
    fn a_failed_source_on_the_uconsole_screen() {
        let mut app = app();
        app.screen = Screen::Panorama;
        app.status.state = SourceState::Failed("usb device not found".into());
        insta::assert_snapshot!(render(&app, 160, 45).backend());
    }

    fn mesh_app() -> App {
        let mut app = app();
        app.mesh = crate::mesh::tests::view();
        app.mesh.link = Link::Connected;
        app.screen = Screen::Mesh;
        app
    }

    #[test]
    fn the_mesh_pulse_with_a_node_selected_on_the_uconsole_screen() {
        let mut app = mesh_app();
        app.mesh.move_selection(1);
        app.mesh.move_selection(1);
        insta::assert_snapshot!(render(&app, 160, 45).backend());
    }

    #[test]
    fn the_card_and_the_feed_tell_texts_positions_and_batteries() {
        use crate::mesh::tests::{FAR, GATE, ME, packet, place};
        use sdr_mesh::update::{Content, DeviceMetrics, Position};
        let mut app = mesh_app();
        let rounded = Position {
            within_m: Some(364.8),
            ..place(32.82)
        };
        let metrics = DeviceMetrics {
            battery: Some(80),
            voltage: Some(3.95),
            channel_utilization: Some(12.5),
            air_util_tx: Some(1.5),
        };
        let mut to_me = packet(GATE, 35_940, Content::Text("the gate is open".into()));
        to_me.to = ME;
        // What the node itself sent of where it is.
        let mut own = packet(ME, 35_300, Content::Position(place(32.81)));
        own.hops = Some(0);
        for packet in [
            own,
            packet(GATE, 35_400, Content::Position(rounded)),
            packet(GATE, 35_880, Content::Device(metrics)),
            packet(FAR, 35_700, Content::Text("shalom\nall".into())),
            to_me,
        ] {
            app.on_mesh(MeshUpdate::Packet(packet));
        }
        app.mesh.move_selection(1);
        app.mesh.move_selection(1);
        insta::assert_snapshot!(render(&app, 160, 45).backend());
        let text = render_text(&app, 160, 45);
        for line in [
            " battery 80% 3.95 V · channel 12.5% · air 1.5% · 2 m ago",
            " at 32.82000, 34.99000 ±365 m · 1.1 km away · 10 m ago",
        ] {
            assert!(text.contains(line), "{line:?} in\n{text}");
        }
        let feed = |text: &str| -> Vec<String> {
            text.lines()
                .filter(|line| line.starts_with(" 09:5"))
                .map(String::from)
                .collect()
        };
        // The feed under the list has the selected node's texts alone.
        let selected = feed(&text);
        assert_eq!(selected.len(), 1, "{text}");
        assert!(
            selected[0].starts_with(" 09:59  HG1 → this node · MediumFast  the gate is open"),
            "{text}"
        );
        // It all fits the smallest screen.
        let small = render_text(&app, MIN_WIDTH, MIN_HEIGHT);
        assert!(small.contains(" 09:59  HG1 → this node"), "{small}");
        assert!(small.contains(" at 32.82000"), "{small}");
        // A node that sent none says so.
        app.mesh.move_selection(-1);
        let text = render_text(&app, 160, 45);
        assert!(feed(&text).is_empty(), "{text}");
        assert!(text.contains(" No text heard from 8900."), "{text}");
        // None selected: every channel's, the newest last.
        app.mesh.release();
        let all = feed(&render_text(&app, 160, 45));
        assert_eq!(all.len(), 2);
        assert!(all[0].starts_with(" 09:55  !43396f6c → all · MediumFast  shalom all"));
        assert!(all[1].starts_with(" 09:59  HG1 → this node"));
    }

    #[test]
    fn the_nodes_own_row_tells_what_it_sent_and_what_it_handed_the_app_alone() {
        use crate::mesh::tests::app_only;
        use sdr_mesh::update::{Content, LocalStats, Sent};
        let mut app = mesh_app();
        // Its device telemetry of every minute since 9:30:10.
        for at in (34_210..36_000).step_by(60) {
            app.on_mesh(MeshUpdate::Packet(app_only(at)));
        }
        // Its counts: up since 9:30, one sent by 9:31:15, three by 9:46:15.
        for (at, sent, uptime_s) in [(34_275, 1, 75), (35_175, 3, 975)] {
            let mut stats = app_only(at);
            stats.content = Content::LocalStats(LocalStats { sent, uptime_s });
            app.on_mesh(MeshUpdate::Packet(stats));
        }
        // Packets of its own queued for the air at 9:40 and 9:59:30.
        for (at, id) in [(34_800, 0x10), (35_970, 0x11)] {
            app.on_mesh(MeshUpdate::Sent(Sent { at, id }));
        }
        app.mesh.move_selection(1);
        insta::assert_snapshot!(render(&app, 160, 45).backend());
        let text = render_text(&app, 160, 45);
        let own = text.lines().find(|line| line.starts_with("8900")).unwrap();
        assert!(own.contains("••••"), "{own}");
        assert!(
            text.contains(" · 2 sent in 24 h · 32 to this app only"),
            "{text}"
        );
        assert!(
            text.contains(
                "sent by its own count: 2 from 09:31 to 09:46, 3 since it started at 09:30"
            ),
            "{text}"
        );
        assert!(
            text.contains("last: one of its own, queued for the air · 30 s ago"),
            "{text}"
        );
        // Once it handed the app something newer, that is the last.
        app.on_mesh(MeshUpdate::Packet(app_only(35_990)));
        let text = render_text(&app, 160, 45);
        assert!(
            text.contains("last: telemetry to this app only, not on the air · 10 s ago"),
            "{text}"
        );
    }

    #[test]
    fn the_mesh_pulse_at_the_minimum_size() {
        insta::assert_snapshot!(render_text(&mesh_app(), MIN_WIDTH, MIN_HEIGHT));
    }

    #[test]
    fn without_a_receiver_its_screens_say_so_and_the_status_tells_the_mesh() {
        let mut app = mesh_app();
        app.status.state = SourceState::Off;
        app.screen = Screen::Spectrum;
        let text = render_text(&app, MIN_WIDTH, MIN_HEIGHT);
        assert!(text.contains("No receiver in this run"), "{text}");
        assert!(text.contains("mesh only │ meshtasticd connected"), "{text}");
        app.screen = Screen::Mesh;
        let text = render_text(&app, MIN_WIDTH, MIN_HEIGHT);
        assert!(text.contains("Meshtastic 8900"), "{text}");
        app.mesh.link = Link::Lost("connection refused".into());
        let text = render_text(&app, MIN_WIDTH, MIN_HEIGHT);
        assert!(text.contains("meshtasticd: connection refused"), "{text}");
    }

    #[test]
    fn while_meshtasticd_does_not_answer_the_pulse_tells_how_to_start_it() {
        let mut app = mesh_app();
        app.mesh =
            crate::mesh::MeshView::new(Link::Lost("Connection refused (os error 111)".into()));
        app.mesh.starter = Some("sudo /home/dmw/sdr-viewer-check/lora.sh on".into());
        let text = render_text(&app, MIN_WIDTH, MIN_HEIGHT);
        assert!(
            text.contains("No answer from meshtasticd: Connection refused"),
            "{text}"
        );
        assert!(
            text.contains("Start it: sudo /home/dmw/sdr-viewer-check/lora.sh on"),
            "{text}"
        );
        // Another machine's: no command to give.
        app.mesh.starter = None;
        let text = render_text(&app, 160, 45);
        assert!(text.contains("Is meshtasticd running there?"), "{text}");
        // Once it answers, the hint goes.
        app.mesh = mesh_app().mesh;
        assert!(!render_text(&app, 160, 45).contains("Is meshtasticd running"));
    }

    #[test]
    fn the_pulse_tells_what_its_list_shows_and_how_it_is_sorted() {
        let mut app = mesh_app();
        let text = render_text(&app, 160, 45);
        assert!(!text.contains("nodes, heard"), "{text}");
        app.mesh.toggle_active();
        app.mesh.next_order();
        insta::assert_snapshot!(render_text(&app, MIN_WIDTH, MIN_HEIGHT));
        app.mesh.start_afresh();
        let text = render_text(&app, 160, 45);
        assert!(
            text.contains("1 of 3 nodes, heard in the last 15 m (a) and since 10:00 (m)"),
            "{text}"
        );
    }

    #[test]
    fn too_small_says_how_big_it_must_be() {
        insta::assert_snapshot!(render(&app(), 40, 10).backend());
    }

    #[test]
    fn the_current_tab_is_highlighted_and_the_others_are_not() {
        let mut app = app();
        app.screen = Screen::Channels;
        let terminal = render(&app, 80, 24);
        let buffer = terminal.backend().buffer();
        let x_of = |digit: char| {
            (0..80)
                .find(|&x| buffer[(x, 0)].symbol() == digit.to_string())
                .unwrap()
        };
        assert_eq!(buffer[(x_of('2'), 0)].bg, Color::Cyan);
        assert_eq!(buffer[(x_of('1'), 0)].bg, Color::Reset);
        assert_eq!(buffer[(x_of('1'), 0)].fg, Color::Yellow);
    }

    #[test]
    fn text_rendering_has_a_line_per_row_without_trailing_spaces() {
        let text = render_text(&app(), 80, 24);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 24);
        assert!(lines[0].starts_with("1 Spectrum 2 Channels"));
        assert!(lines[23].ends_with("q quit"));
        assert!(lines.iter().all(|line| !line.ends_with(' ')));
    }

    #[test]
    fn the_tab_bar_fits_the_minimum_width() {
        assert!(tab_bar(Screen::Spectrum).width() <= usize::from(MIN_WIDTH));
    }
}
