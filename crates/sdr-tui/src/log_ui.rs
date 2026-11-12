//! Drawing the signal log: a line of counts, then one row per event.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use sdr_core::units::format_hz;

use crate::log::{LogView, Row, format_duration, format_time, format_width};

pub(crate) fn draw(frame: &mut Frame, log: &LogView, area: Rect) {
    let [summary, header, list] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(area);
    let rows = log.rows();
    frame.render_widget(summary_line(log, rows.len()), summary);
    if rows.is_empty() {
        let text = format!("No events above +{:.0} dB yet.", log.min_above_db());
        frame.render_widget(Paragraph::new(text).centered().dark_gray(), list);
        return;
    }
    frame.render_widget(
        Line::from(format!(
            "  {:>10}  {:>8}  {:>13}  {:>9}  {:>10}  above floor",
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
        .map(|(index, row)| row_line(row, selected == Some(index)))
        .collect();
    frame.render_widget(Paragraph::new(lines), list);
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

fn row_line(row: &Row, selected: bool) -> Line<'static> {
    let event = &row.event;
    let mark = if row.on {
        Span::raw("● ").green()
    } else {
        Span::raw("  ")
    };
    let text = format!(
        "{:>10}  {:>8}  {:>13}  {:>9}  {:>5.1} dBFS  +{:.1} dB",
        format_time(event.start),
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
