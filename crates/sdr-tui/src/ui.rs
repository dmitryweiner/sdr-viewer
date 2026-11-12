//! Drawing the interface. Phase 1 has the frame of it: tabs, a placeholder
//! per screen, the status line and the help.

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use sdr_core::units::{format_hz, format_rate};

use crate::app::{App, Screen, SourceState};

/// The plan's minimum terminal; the uConsole gives about 160×45.
pub const MIN_WIDTH: u16 = 80;
pub const MIN_HEIGHT: u16 = 24;

const KEYS: [(&str, &str); 18] = [
    ("?", "this help"),
    ("1–8", "switch screens"),
    ("Tab, Shift+Tab", "next, previous screen"),
    ("←, →", "spectrum: move the cursor (Shift: faster)"),
    ("Enter", "spectrum: tune to the cursor"),
    ("f", "spectrum: type a frequency"),
    ("[, ]", "spectrum: tune down, up by half the view"),
    ("+, -", "spectrum: zoom in, out"),
    ("g, G, a", "spectrum: gain up, down, automatic"),
    ("Space", "spectrum: pause"),
    ("m", "spectrum: reset max hold"),
    ("↑, ↓", "log: select an event (PgUp, PgDn, Home, End)"),
    ("+, -", "log: raise, lower the level that hides weak events"),
    ("s", "log: sort by time, level, duration, frequency"),
    ("Enter", "log: tune to the selected event"),
    ("r", "log: save the selected event's IQ samples"),
    ("Esc", "close help, put the cursor or selection away, quit"),
    ("q", "quit"),
];

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
    match app.screen {
        Screen::Spectrum => {
            crate::spectrum_ui::draw(frame, &app.spectrum, app.input.as_deref(), body);
        }
        Screen::Log => crate::log_ui::draw(frame, &app.log, app.picture(), body),
        screen => frame.render_widget(placeholder(screen), body),
    }
    draw_status(frame, app, status);
    if app.help {
        draw_help(frame, body);
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

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let separator = || Span::raw(" │ ").dark_gray();
    let state = match (&app.notice, &app.done, &app.status.state) {
        (Some(notice), ..) => Span::raw(notice.clone()).red(),
        (None, Some(done), _) => Span::raw(done.clone()).green(),
        (None, None, state) => match state {
            SourceState::Running => Span::raw(format!("rx {}", format_rate(app.status.rate))),
            SourceState::Ended => Span::raw("ended").yellow(),
            SourceState::Failed(message) => Span::raw(format!("error: {message}")).red(),
        },
    };
    let left = Line::from(vec![
        Span::raw(app.source.description.clone()).bold(),
        separator(),
        Span::raw(format_hz(app.source.center_hz)),
        separator(),
        Span::raw(format_rate(f64::from(app.source.sample_rate))),
        separator(),
        state,
    ]);
    let right = Line::from(vec![
        Span::raw("?").yellow(),
        Span::raw(" help  "),
        Span::raw("q").yellow(),
        Span::raw(" quit"),
    ]);
    let right_width = u16::try_from(right.width()).unwrap_or(u16::MAX);
    let [left_area, right_area] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(right_width)]).areas(area);
    frame.render_widget(left, left_area);
    frame.render_widget(right, right_area);
}

fn draw_help(frame: &mut Frame, body: Rect) {
    let key_width = KEYS
        .iter()
        .map(|(key, _)| key.chars().count())
        .max()
        .unwrap_or(0);
    let lines: Vec<Line> = KEYS
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
        Paragraph::new(lines).block(Block::bordered().title(" Keys ")),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::SourceStatus;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use sdr_core::SourceInfo;

    fn app() -> App {
        let mut app = App::new(SourceInfo {
            sample_rate: 2_400_000,
            center_hz: 433.92e6,
            description: "generator".into(),
        });
        app.status = SourceStatus {
            rate: 2_398_000.0,
            state: SourceState::Running,
        };
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
        use sdr_detect::{Detector, DetectorConfig};
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
        let mut buf = vec![Iq::new(0.0, 0.0); 1_024_000];
        for _ in 0..2 {
            generator.read(&mut buf).unwrap();
            analyzer.push(&buf, &generator.info(), |frame| {
                detector.push(&frame, |_| {});
                app.spectrum.on_frame(frame, detector.levels());
            });
        }
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
        app.screen = Screen::Mesh;
        app.status.state = SourceState::Failed("usb device not found".into());
        insta::assert_snapshot!(render(&app, 160, 45).backend());
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
