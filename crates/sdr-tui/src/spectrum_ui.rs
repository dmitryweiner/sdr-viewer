//! Drawing the spectrum screen: bars with max hold and the detector's
//! threshold behind them, the waterfall below, two waterfall rows per
//! character cell.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use sdr_core::SpectrumFrame;
use sdr_core::units::format_hz;

use crate::spectrum::{
    BARS, LINE, SpectrumView, bar_eighths, columns, cursor_column, heat, level, line_cell,
};

/// Columns left of the plot for the dB labels: "-100 ".
const GUTTER: u16 = 5;

const HELD: Color = Color::Rgb(70, 70, 80);

/// The detector's threshold: unlike anything the palette gives a bar.
const THRESHOLD: Color = Color::Rgb(160, 100, 50);

pub(crate) fn draw(frame: &mut Frame, view: &SpectrumView, input: Option<&str>, area: Rect) {
    let (Some(latest), Some(range)) = (view.latest(), view.range()) else {
        frame.render_widget(
            Paragraph::new("Waiting for samples…")
                .centered()
                .dark_gray(),
            area,
        );
        return;
    };
    let bars_height = (area.height * 2 / 5).max(4);
    let [header, bars, axis, waterfall] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(bars_height),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(area);
    let plot = |rect: Rect| Rect {
        x: rect.x + GUTTER,
        width: rect.width - GUTTER,
        ..rect
    };
    let width = usize::from(plot(bars).width);
    let cursor = view.cursor.map(|at| cursor_column(at, width));

    match input {
        Some(typed) => draw_input(frame, typed, plot(header)),
        None => draw_header(frame, view, latest, cursor, width, plot(header)),
    }
    draw_scale(frame, range, bars);
    draw_bars(frame, view, latest, range, cursor, plot(bars));
    draw_axis(frame, view, latest, cursor, plot(axis));
    draw_waterfall(frame, view, range, plot(waterfall));
}

fn draw_header(
    frame: &mut Frame,
    view: &SpectrumView,
    latest: &SpectrumFrame,
    cursor: Option<usize>,
    width: usize,
    area: Rect,
) {
    let floor = view.floor().map(|floor| view.visible(floor));
    let (label, hz, db, floor) = match cursor {
        Some(column) => {
            let hz = view.hz(latest, (column as f64 + 0.5) / width as f64);
            let db = columns(view.visible(&latest.power_db), width)[column];
            let floor = floor.map(|floor| columns(floor, width)[column]);
            ("cursor", hz, db, floor)
        }
        None => {
            let visible = view.visible(&latest.power_db);
            let (bin, db) = strongest(visible);
            (
                "peak",
                view.hz(latest, bin as f64 / visible.len() as f64),
                db,
                floor.map(|floor| floor[bin]),
            )
        }
    };
    let mut spans = vec![
        Span::raw(format!("{label} ")).dark_gray(),
        Span::raw(format_hz(hz)).bold(),
        Span::raw(format!("  {db:.1} dBFS")),
    ];
    if let Some(floor) = floor {
        spans.push(Span::raw(format!("  {:+.0} dB above floor", db - floor)).dark_gray());
    }
    if view.magnification() > 1 {
        spans.push(Span::raw(format!("  ×{}", view.magnification())).cyan());
    }
    if view.paused {
        spans.push(Span::raw("  paused").yellow());
    }
    frame.render_widget(Line::from(spans), area);
}

/// The frequency being typed, in place of the header.
fn draw_input(frame: &mut Frame, typed: &str, area: Rect) {
    let line = Line::from(vec![
        Span::raw("frequency ").dark_gray(),
        Span::raw(format!("{typed}▏")).bold(),
        Span::raw("  e.g. 433.92M · Enter tunes · Esc cancels").dark_gray(),
    ]);
    frame.render_widget(line, area);
}

fn strongest(power_db: &[f32]) -> (usize, f32) {
    power_db
        .iter()
        .copied()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap_or((0, f32::MIN))
}

/// The dB at the top, the middle and the bottom of the bars.
fn draw_scale(frame: &mut Frame, (bottom, top): (f32, f32), bars: Rect) {
    let rows = bars.height;
    for (row, db) in [
        (0, top),
        (rows / 2, (top + bottom) / 2.0),
        (rows - 1, bottom),
    ] {
        let label = Line::from(format!("{db:>4.0}")).dark_gray();
        frame.render_widget(
            label,
            Rect {
                y: bars.y + row,
                height: 1,
                width: GUTTER,
                ..bars
            },
        );
    }
}

fn draw_bars(
    frame: &mut Frame,
    view: &SpectrumView,
    latest: &SpectrumFrame,
    range: (f32, f32),
    cursor: Option<usize>,
    area: Rect,
) {
    let width = usize::from(area.width);
    let rows = usize::from(area.height);
    let now = columns(view.visible(&latest.power_db), width);
    let held = columns(view.visible(view.max_hold()), width);
    let threshold = view
        .threshold()
        .map(|threshold| columns(view.visible(&threshold), width));
    let buffer = frame.buffer_mut();
    for (column, (&now, &held)) in now.iter().zip(&held).enumerate() {
        let (now, held) = (level(now, range), level(held, range));
        let line = threshold
            .as_ref()
            .map(|threshold| line_cell(level(threshold[column], range), rows));
        for row in 0..rows {
            let x = area.x + column as u16;
            let y = area.y + (rows - 1 - row) as u16;
            let fill = bar_eighths(now, rows, row);
            let held_fill = bar_eighths(held, rows, row);
            let cell = &mut buffer[(x, y)];
            // The threshold's line runs behind the bars and over what
            // max hold fills.
            let line = line
                .filter(|&(line_row, _)| line_row == row)
                .map(|(_, quarter)| LINE[quarter]);
            if fill > 0 {
                // What max hold reached shows behind the top of the bar.
                let behind = if held_fill == 8 && fill < 8 {
                    HELD
                } else {
                    Color::Reset
                };
                cell.set_symbol(BARS[fill])
                    .set_style(Style::new().fg(heat(now)).bg(behind));
            } else if let Some(line) = line.filter(|_| held_fill == 0 || held_fill == 8) {
                let behind = if held_fill == 8 { HELD } else { Color::Reset };
                cell.set_symbol(line)
                    .set_style(Style::new().fg(THRESHOLD).bg(behind));
            } else if held_fill > 0 {
                cell.set_symbol(BARS[held_fill])
                    .set_style(Style::new().fg(HELD));
            } else if cursor == Some(column) {
                cell.set_symbol("│")
                    .set_style(Style::new().fg(Color::DarkGray));
            }
        }
    }
}

/// Frequencies of the left edge, the centre and the right edge, and the
/// cursor's mark.
fn draw_axis(
    frame: &mut Frame,
    view: &SpectrumView,
    latest: &SpectrumFrame,
    cursor: Option<usize>,
    area: Rect,
) {
    let [left, center, right] = [0.0, 0.5, 1.0].map(|at| format_hz(view.hz(latest, at)));
    frame.render_widget(Line::from(left).dark_gray(), area);
    frame.render_widget(Line::from(center).dark_gray().centered(), area);
    frame.render_widget(Line::from(right).dark_gray().right_aligned(), area);
    if let Some(column) = cursor {
        let cell = &mut frame.buffer_mut()[(area.x + column as u16, area.y)];
        cell.set_symbol("▲")
            .set_style(Style::new().fg(Color::White));
    }
}

/// Newest at the top. A cell is "▀": its foreground is the newer of two
/// frames, its background the older one.
fn draw_waterfall(frame: &mut Frame, view: &SpectrumView, range: (f32, f32), area: Rect) {
    let width = usize::from(area.width);
    let mut rows = view
        .frames()
        .take(usize::from(area.height) * 2)
        .map(|frame| columns(view.visible(&frame.power_db), width));
    let buffer = frame.buffer_mut();
    for y in 0..area.height {
        let Some(upper) = rows.next() else { break };
        let lower = rows.next();
        for (column, &db) in upper.iter().enumerate() {
            let below = lower
                .as_ref()
                .map_or(Color::Reset, |row| heat(level(row[column], range)));
            buffer[(area.x + column as u16, area.y + y)]
                .set_symbol("▀")
                .set_style(Style::new().fg(heat(level(db, range))).bg(below));
        }
    }
}
