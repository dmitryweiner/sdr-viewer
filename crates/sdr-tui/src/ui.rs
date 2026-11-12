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

const KEYS: [(&str, &str); 8] = [
    ("1–8", "switch screens"),
    ("Tab, Shift+Tab", "next, previous screen"),
    ("←, →", "spectrum: move the cursor (Shift: faster)"),
    ("Space", "spectrum: pause"),
    ("m", "spectrum: reset max hold"),
    ("Esc", "close help, put the cursor away, quit"),
    ("?", "this help"),
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
    if app.screen == Screen::Spectrum {
        crate::spectrum_ui::draw(frame, &app.spectrum, body);
    } else {
        frame.render_widget(placeholder(app.screen), body);
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
    let state = match &app.status.state {
        SourceState::Running => Span::raw(format!("rx {}", format_rate(app.status.rate))),
        SourceState::Ended => Span::raw("ended").yellow(),
        SourceState::Failed(message) => Span::raw(format!("error: {message}")).red(),
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
        let mut buf = vec![Iq::new(0.0, 0.0); 1_024_000];
        for _ in 0..2 {
            generator.read(&mut buf).unwrap();
            analyzer.push(&buf, &generator.info(), |frame| {
                app.spectrum.on_frame(frame)
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
