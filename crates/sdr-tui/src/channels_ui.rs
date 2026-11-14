//! Drawing the channel strip: one row per channel, its name or frequency
//! on the left, how busy it was through time in the middle — all rows on
//! one time axis, now at the right — and on the right its share of that
//! time and how long it has been quiet.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use sdr_core::SourceInfo;
use sdr_core::units::format_hz;

use crate::channels::{ChannelsView, Row, format_ago};
use crate::density_ui::percent;
use crate::spectrum::BARS;

/// Columns for the channel's name: "1090.000 MHz" and a space.
const LABEL: u16 = 13;

/// Columns right of the strip: " 12.3%  now".
const FIGURES: u16 = 12;

const BUSY: Color = Color::Rgb(90, 200, 120);

pub(crate) fn draw(frame: &mut Frame, view: &ChannelsView, source: &SourceInfo, area: Rect) {
    let [header, list, axis] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);
    let rows = view.rows();
    let columns = usize::from(area.width - LABEL - FIGURES);
    frame.render_widget(header_line(view, rows.len(), columns), header);
    if rows.is_empty() {
        let text = "No channels yet: one is made where a signal is found.";
        frame.render_widget(Paragraph::new(text).centered().dark_gray(), list);
        return;
    }
    // The selection is kept in view, in the middle when it can be.
    let height = usize::from(list.height);
    let selected = view.selected_row();
    let first = selected
        .map_or(0, |row| row.saturating_sub(height / 2))
        .min(rows.len().saturating_sub(height));
    for (y, row) in rows.iter().enumerate().skip(first).take(height) {
        let area = Rect {
            y: list.y + (y - first) as u16,
            height: 1,
            ..list
        };
        draw_row(
            frame,
            view,
            source,
            *row,
            selected == Some(y),
            columns,
            area,
        );
    }
    draw_axis(frame, view, columns, axis);
}

fn header_line(view: &ChannelsView, rows: usize, columns: usize) -> Line<'static> {
    let dim = |text: String| Span::raw(text).dark_gray();
    Line::from(vec![
        Span::raw(format!(" {rows}")).bold(),
        dim(format!(" channel{}", if rows == 1 { "" } else { "s" })),
        dim(" · ".into()),
        Span::raw(format!("{} s", view.seconds_per_column())),
        dim(" a column · the last ".into()),
        Span::raw(format_ago(view.window(columns))),
    ])
}

fn draw_row(
    frame: &mut Frame,
    view: &ChannelsView,
    source: &SourceInfo,
    row: Row,
    selected: bool,
    columns: usize,
    area: Rect,
) {
    let width = usize::from(LABEL) - 1;
    let label = match &row.channel.name {
        Some(name) => name.chars().take(width).collect(),
        None => format_hz(row.channel.center_hz),
    };
    // A channel outside the band being received can show nothing new.
    let received =
        (row.channel.center_hz - source.center_hz).abs() <= f64::from(source.sample_rate) / 2.0;
    let active = view.active(row.index);
    let mut style = match (active, received) {
        (true, _) => Style::new().fg(BUSY).add_modifier(Modifier::BOLD),
        (false, true) => Style::new(),
        (false, false) => Style::new().fg(Color::DarkGray),
    };
    if selected {
        style = style.add_modifier(Modifier::REVERSED);
    }
    let quiet = match (active, view.quiet_for(row.index)) {
        (true, _) => "now".to_string(),
        (false, Some(quiet)) => format_ago(quiet),
        (false, None) => "—".to_string(),
    };
    let share = percent(view.share(row.index, columns));
    let figures = format!(" {share:>5} {quiet:>5}");
    frame.render_widget(Span::styled(format!("{label:<width$}"), style), area);
    frame.render_widget(
        Line::from(figures).right_aligned(),
        Rect {
            x: area.x + area.width - FIGURES,
            width: FIGURES,
            ..area
        },
    );
    let buffer = frame.buffer_mut();
    for (column, cell) in view.cells(row.index, columns).into_iter().enumerate() {
        let Some(share) = cell else { continue };
        let at = &mut buffer[(area.x + LABEL + column as u16, area.y)];
        if share > 0.0 {
            // The shortest burst still shows as the lowest bar.
            let eighths = ((share * 8.0).round() as usize).clamp(1, 8);
            at.set_symbol(BARS[eighths])
                .set_style(Style::new().fg(BUSY));
        } else {
            at.set_symbol("·")
                .set_style(Style::new().fg(Color::DarkGray));
        }
    }
}

/// The time axis under the strip: how long ago its left edge and its
/// middle were.
fn draw_axis(frame: &mut Frame, view: &ChannelsView, columns: usize, area: Rect) {
    let strip = Rect {
        x: area.x + LABEL,
        width: columns as u16,
        ..area
    };
    let scale = view.seconds_per_column();
    let ago = |columns: usize| {
        let time = std::time::Duration::from_secs(columns as u64 * scale);
        format!("−{}", format_ago(time))
    };
    frame.render_widget(Line::from(ago(columns)).dark_gray(), strip);
    frame.render_widget(Line::from(ago(columns / 2)).dark_gray().centered(), strip);
    frame.render_widget(Line::from("now").dark_gray().right_aligned(), strip);
}
