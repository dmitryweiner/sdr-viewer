//! Drawing the periodicity screen: the selected channel's envelope — when
//! it was on — and under it that envelope's autocorrelation by delay, in
//! Braille dots, with a cursor on the main peak.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use sdr_core::units::format_hz;

use crate::channels::{ChannelsView, format_ago};
use crate::channels_ui::draw_bar;
use crate::period::{Analysis, ENOUGH, Period, TURNS, format_period};

/// The plot's top is all of the signal coming again; under nothing there
/// is room for a quarter as much the other way.
const TOP: f32 = 1.0;
const BOTTOM: f32 = -0.25;

const CURVE: Color = Color::Rgb(110, 170, 230);
const CURSOR: Color = Color::Yellow;

pub(crate) fn draw(frame: &mut Frame, view: &ChannelsView, scale: usize, area: Rect) {
    let Some((index, channel)) = view.selected_index().zip(view.selected()) else {
        let text = if view.rows().is_empty() {
            "No channels yet: one is made where a signal is found."
        } else {
            "Select a channel: ↑ ↓, here or on the channel strip."
        };
        frame.render_widget(Paragraph::new(text).centered().dark_gray(), area);
        return;
    };
    let [header, found, envelope, envelope_axis, plot, plot_axis] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);
    let analysis = view.analysis(index, scale);
    let delays = analysis.delays();
    let dim = |text: String| Span::raw(text).dark_gray();
    let longest = std::time::Duration::from_secs(delays.longest_s);

    let mut title = vec![Span::raw(" ")];
    if let Some(name) = &channel.name {
        title.push(Span::raw(name.clone()).bold());
        title.push(dim(" · ".into()));
    }
    title.push(Span::raw(format_hz(channel.center_hz)).bold());
    title.push(dim(" · delays to ".into()));
    title.push(Span::raw(format_ago(longest)));
    title.push(dim(format!(" in {} ms steps", delays.step_ms)));
    frame.render_widget(Line::from(title), header);
    frame.render_widget(found_line(&analysis), found);

    for (column, share) in analysis
        .bars(usize::from(envelope.width))
        .iter()
        .enumerate()
    {
        if let Some(share) = share {
            draw_bar(frame, *share, envelope.x + column as u16, envelope.y);
        }
    }
    draw_ends(
        frame,
        &format!("−{}", format_ago(longest * TURNS as u32)),
        "when the channel was on",
        "now",
        envelope_axis,
    );

    let main = analysis.period();
    draw_plot(frame, &analysis, main, plot);
    draw_ends(
        frame,
        "0",
        "how like itself the channel is after a delay",
        &format_ago(longest),
        plot_axis,
    );
}

/// What was found: the main period, then the others.
fn found_line(analysis: &Analysis) -> Line<'static> {
    let dim = |text: String| Span::raw(text).dark_gray();
    let longest = format_ago(std::time::Duration::from_secs(analysis.delays().longest_s));
    let known = analysis.envelope.len();
    if known < ENOUGH {
        let step_ms = analysis.delays().step_ms;
        let seconds = |steps: usize| format_period((steps as u64 * step_ms) as f64 / 1000.0);
        return Line::from(dim(format!(
            " Too little yet for these delays: {} of the {} it takes",
            seconds(known),
            seconds(ENOUGH)
        )));
    }
    let mut periods = analysis.periods();
    let Some(main) = periods.next() else {
        return Line::from(dim(format!(
            " No period within {longest}: + and - look at shorter and longer delays"
        )));
    };
    let strength = |period: Period| format!(" {:.0}%", period.strength * 100.0);
    let mut spans = vec![
        dim(" Repeats every ".into()),
        Span::raw(format_period(main.seconds)).fg(CURSOR).bold(),
        dim(strength(main)),
    ];
    for (nth, other) in periods.enumerate() {
        spans.push(dim(if nth == 0 { " · also " } else { ", " }.into()));
        spans.push(Span::raw(format_period(other.seconds)));
        spans.push(dim(strength(other)));
    }
    Line::from(spans)
}

/// A line of dim words: at the left end, in the middle, at the right end.
fn draw_ends(frame: &mut Frame, left: &str, middle: &str, right: &str, area: Rect) {
    frame.render_widget(Line::from(middle.to_string()).dark_gray().centered(), area);
    frame.render_widget(Line::from(left.to_string()).dark_gray(), area);
    let right = Line::from(right.to_string()).dark_gray().right_aligned();
    frame.render_widget(right, area);
}

/// The dot of a Braille cell, two across and four down.
fn dot(x: usize, y: usize) -> u8 {
    const DOTS: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];
    DOTS[x][y]
}

/// The row of dots, from the top, a value is drawn on.
fn dot_row(value: f32, rows: usize) -> usize {
    let share = (TOP - value.clamp(BOTTOM, TOP)) / (TOP - BOTTOM);
    (share * (rows - 1) as f32).round() as usize
}

/// The autocorrelation as columns of dots standing on the line of
/// nothing — below that line only the curve itself, or the dips between
/// the peaks would weigh as much as the peaks — and the main period as a
/// cursor through the plot with its name at the top.
fn draw_plot(frame: &mut Frame, analysis: &Analysis, main: Option<Period>, area: Rect) {
    let (width, height) = (usize::from(area.width), usize::from(area.height));
    if width == 0 || height == 0 {
        return;
    }
    let rows = height * 4;
    let mut cells = vec![0u8; width * height];
    let nothing = dot_row(0.0, rows);
    for (x, value) in analysis.dots(width * 2).into_iter().enumerate() {
        let Some(value) = value else { continue };
        let at = dot_row(value, rows);
        let filled = if at <= nothing { at..=nothing } else { at..=at };
        for y in filled {
            cells[y / 4 * width + x / 2] |= dot(x % 2, y % 4);
        }
    }
    let longest = analysis.delays().longest_s as f64;
    let cursor = main.map(|period| {
        let column = (period.seconds / longest * width as f64) as usize;
        column.min(width - 1)
    });
    let buffer = frame.buffer_mut();
    for (at, &dots) in cells.iter().enumerate() {
        let (x, y) = (at % width, at / width);
        let cell = &mut buffer[(area.x + x as u16, area.y + y as u16)];
        let on_cursor = cursor == Some(x);
        if dots != 0 {
            let symbol = char::from_u32(0x2800 + u32::from(dots)).unwrap_or(' ');
            let color = if on_cursor { CURSOR } else { CURVE };
            cell.set_char(symbol).set_style(Style::new().fg(color));
        } else if on_cursor {
            cell.set_char('│').set_style(Style::new().fg(CURSOR));
        }
    }
    if let (Some(period), Some(column)) = (main, cursor) {
        // Beside the cursor: right of it, or left where the edge is near.
        let label = format!(" {} ", format_period(period.seconds));
        let length = label.chars().count();
        let x = if column + 1 + length <= width {
            column + 1
        } else {
            column.saturating_sub(length)
        };
        let label_area = Rect {
            x: area.x + x as u16,
            width: (length.min(width - x)) as u16,
            height: 1,
            ..area
        };
        frame.render_widget(Span::raw(label).fg(CURSOR).bold(), label_area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plot_runs_from_all_at_the_top_to_a_quarter_under_nothing() {
        assert_eq!(dot_row(1.0, 41), 0);
        assert_eq!(dot_row(0.0, 41), 32);
        assert_eq!(dot_row(-0.25, 41), 40);
        // What is beyond stays on the edge.
        assert_eq!(dot_row(-1.0, 41), 40);
        assert_eq!(dot_row(1.5, 41), 0);
    }

    #[test]
    fn a_cell_of_dots_is_two_across_and_four_down() {
        let all = (0..8).fold(0, |cell, at| cell | dot(at % 2, at / 2));
        assert_eq!(all, 0xff);
        assert_eq!(dot(0, 0), 0x01);
        assert_eq!(dot(1, 3), 0x80);
    }
}
