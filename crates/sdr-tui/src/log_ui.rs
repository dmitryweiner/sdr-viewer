//! Drawing the signal log: a line of counts, then one row per event, and
//! under them the picture of the selected one.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use sdr_core::units::format_hz;

use crate::fragment::{Fragment, Picture};
use crate::log::{
    LogView, Row, WallClock, format_clock, format_duration, format_time, format_width,
};
use crate::spectrum::{cursor_column, heat, level};
use sdr_detect::Event;

/// Columns for when an event began: the time in the stream, or the date
/// and the time of day.
const STREAM_TIME_WIDTH: usize = 10;
const CLOCK_WIDTH: usize = 16;

/// Columns left of the picture for its frequencies: "433.645 MHz ".
const PICTURE_GUTTER: u16 = 12;

/// The picture's share of the rows under the header, and its limits: a
/// title, the picture, the marks of the event.
fn picture_height(rows: u16) -> u16 {
    (rows * 2 / 5).clamp(6, 14)
}

pub(crate) fn draw(frame: &mut Frame, log: &LogView, picture: Option<&Picture>, area: Rect) {
    let [summary, header, body] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(area);
    let selected = log.selected_event().zip(picture);
    let panel = selected.map_or(0, |_| picture_height(body.height));
    let [list, panel] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(panel)]).areas(body);
    if let Some((event, picture)) = selected {
        draw_picture(frame, event, picture, panel);
    }
    let rows = log.rows();
    frame.render_widget(summary_line(log, rows.len()), summary);
    if rows.is_empty() {
        let text = format!("No events above +{:.0} dB yet.", log.min_above_db());
        frame.render_widget(Paragraph::new(text).centered().dark_gray(), list);
        return;
    }
    let clock = log.clock();
    let time_width = match clock {
        Some(_) => CLOCK_WIDTH,
        None => STREAM_TIME_WIDTH,
    };
    frame.render_widget(
        Line::from(format!(
            "  {:>time_width$}  {:>8}  {:>13}  {:>9}  {:>10}  above floor",
            "start", "duration", "centre", "width", "peak"
        ))
        .dark_gray(),
        header,
    );
    let height = usize::from(list.height);
    let selected = log.selected();
    // Scroll just far enough to keep the selected row in sight.
    let first = selected.map_or(0, |index| (index + 1).saturating_sub(height));
    let lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .skip(first)
        .take(height)
        .map(|(index, row)| row_line(row, clock, time_width, selected == Some(index)))
        .collect();
    frame.render_widget(Paragraph::new(lines), list);
}

/// The selected event as a small spectrogram: time across, frequency up,
/// two frequency pixels to a character cell.
fn draw_picture(frame: &mut Frame, event: &Event, picture: &Picture, area: Rect) {
    let fragment = match picture {
        Picture::Ready(fragment) => fragment,
        Picture::Waiting | Picture::Gone => {
            let text = match picture {
                Picture::Waiting => "…",
                _ => "The samples of this event are no longer held.",
            };
            let [_, middle, _] = Layout::vertical([
                Constraint::Fill(1),
                Constraint::Length(1),
                Constraint::Fill(1),
            ])
            .areas(area);
            frame.render_widget(Paragraph::new(text).centered().dark_gray(), middle);
            return;
        }
    };
    let [title, rows, marks] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);
    let plot = |rect: Rect| Rect {
        x: rect.x + PICTURE_GUTTER,
        width: rect.width.saturating_sub(PICTURE_GUTTER),
        ..rect
    };
    frame.render_widget(
        Line::from(format!(
            "{} from {}, {} wide",
            format_duration(fragment.duration()),
            format_time(fragment.start),
            format_width(fragment.high_hz - fragment.low_hz)
        ))
        .dark_gray(),
        plot(title),
    );
    for (y, hz) in [
        (rows.y, fragment.high_hz),
        (rows.y + rows.height.saturating_sub(1), fragment.low_hz),
    ] {
        let label = Rect {
            y,
            height: 1,
            width: PICTURE_GUTTER,
            ..rows
        };
        frame.render_widget(Line::from(format_hz(hz)).dark_gray(), label);
    }
    draw_pixels(frame, fragment, plot(rows));
    // Where the event begins and ends.
    let marks = plot(marks);
    let width = usize::from(marks.width);
    if width > 0 {
        for moment in [event.start, event.start + event.duration] {
            let x = cursor_column(fragment.across(moment), width);
            frame.buffer_mut()[(marks.x + x as u16, marks.y)]
                .set_symbol("▲")
                .set_style(Style::new().fg(Color::White));
        }
    }
}

fn draw_pixels(frame: &mut Frame, fragment: &Fragment, area: Rect) {
    let range = fragment.range();
    let pixels = fragment.pixels(usize::from(area.width), usize::from(area.height) * 2);
    let buffer = frame.buffer_mut();
    for (y, [upper, lower]) in pixels.as_chunks::<2>().0.iter().enumerate() {
        for (x, (&upper, &lower)) in upper.iter().zip(lower).enumerate() {
            buffer[(area.x + x as u16, area.y + y as u16)]
                .set_symbol("▀")
                .set_style(
                    Style::new()
                        .fg(heat(level(upper, range)))
                        .bg(heat(level(lower, range))),
                );
        }
    }
}

fn summary_line(log: &LogView, shown: usize) -> Line<'static> {
    let separator = || Span::raw(" · ").dark_gray();
    let mut spans = vec![
        Span::raw(format!(" {shown} events")).bold(),
        separator(),
        Span::raw(format!(
            "{} hidden below +{:.0} dB",
            log.hidden(),
            log.min_above_db()
        )),
        separator(),
        Span::raw(format!("{} on now", log.on_now())),
        separator(),
        Span::raw(format!("by {}", log.sort().name())),
    ];
    if log.selected().is_some() {
        spans.push(separator());
        spans.push(
            Span::raw(format!(
                "held, {} new — Esc follows again",
                log.arrived_since_held()
            ))
            .yellow(),
        );
    }
    Line::from(spans)
}

fn row_line(
    row: &Row,
    clock: Option<WallClock>,
    time_width: usize,
    selected: bool,
) -> Line<'static> {
    let event = &row.event;
    let mark = if row.on {
        Span::raw("● ").green()
    } else {
        Span::raw("  ")
    };
    let start = match (row.at, clock) {
        (Some(at), Some(clock)) => format_clock(at, clock.utc_offset_s),
        _ => format_time(event.start),
    };
    let text = format!(
        "{start:>time_width$}  {:>8}  {:>13}  {:>9}  {:>5.1} dBFS  +{:.1} dB",
        format_duration(event.duration),
        format_hz(event.center_hz()),
        format_width(event.width_hz()),
        event.peak_dbfs,
        event.above_floor_db
    );
    let line = Line::from(vec![mark, Span::raw(text)]);
    if selected {
        line.style(Style::new().add_modifier(Modifier::REVERSED))
    } else {
        line
    }
}
