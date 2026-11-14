//! Drawing the density screen: frequency across, level up, and the
//! brighter a point, the more of the time the signal was at that level.
//! Two pixels per character cell, like the waterfall.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use sdr_core::SpectrumFrame;
use sdr_core::units::format_hz;
use sdr_detect::DensityMap;

use crate::density::{brightness, busy, pixels};
use crate::spectrum::{SpectrumView, cursor_column, heat};
use crate::spectrum_ui::{BAND, GUTTER, draw_axis, draw_band, draw_input, draw_scale, other_band};

/// The band, the zoom, the cursor and the scale are the spectrum
/// screen's: the two show the same piece of the air.
pub(crate) fn draw(
    frame: &mut Frame,
    view: &SpectrumView,
    map: Option<&DensityMap>,
    input: Option<&str>,
    area: Rect,
) {
    let latest = view.latest().zip(view.range());
    let map = map.filter(|map| latest.is_some_and(|(frame, _)| frame.power_db.len() == map.bins));
    let (Some((latest, range)), Some(map)) = (latest, map) else {
        frame.render_widget(
            Paragraph::new("Waiting for samples…")
                .centered()
                .dark_gray(),
            area,
        );
        return;
    };
    let [header, body, axis] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);
    let plot = |rect: Rect| Rect {
        x: rect.x + GUTTER,
        width: rect.width - GUTTER,
        ..rect
    };
    let width = usize::from(plot(body).width);
    let cursor = view.cursor.map(|at| cursor_column(at, width));

    match input {
        Some(typed) => draw_input(frame, typed, plot(header)),
        None => {
            draw_band(frame, view, latest, plot(header));
            draw_header(frame, view, map, latest, cursor, width, plot(header));
        }
    }
    draw_scale(frame, range, body);
    draw_map(frame, view, map, range, cursor, plot(body));
    draw_axis(frame, view, latest, cursor, plot(axis));
}

fn draw_header(
    frame: &mut Frame,
    view: &SpectrumView,
    map: &DensityMap,
    latest: &SpectrumFrame,
    cursor: Option<usize>,
    width: usize,
    area: Rect,
) {
    let mut spans = Vec::new();
    if let Some(column) = cursor {
        let hz = view.hz(latest, (column as f64 + 0.5) / width as f64);
        spans.push(Span::raw("cursor ").dark_gray());
        spans.push(Span::raw(format_hz(hz)).bold());
        if let Some(band) = other_band(view, latest, hz) {
            spans.push(Span::raw(format!("  {band}")).fg(BAND));
        }
        if let Some(threshold) = view.threshold() {
            // The bins of the cursor's column, as `columns` divides them.
            let bins = view.visible_bins(map.bins);
            let start = bins.start + column * bins.len() / width;
            let end = (bins.start + (column + 1) * bins.len() / width)
                .max(start + 1)
                .min(bins.end);
            let share = busy(map, start..end, &threshold[start..end]);
            spans.push(Span::raw(format!("  busy {}", percent(share))));
            spans.push(Span::raw(" of the time  ").dark_gray());
        } else {
            spans.push(Span::raw("  "));
        }
    }
    let half_life = map.config.half_life.as_secs_f64();
    spans.push(Span::raw(format!("half-life {half_life:.0} s")).dark_gray());
    if view.magnification() > 1 {
        spans.push(Span::raw(format!("  ×{}", view.magnification())).cyan());
    }
    if view.paused {
        spans.push(Span::raw("  paused").yellow());
    }
    frame.render_widget(Line::from(spans), area);
}

/// A share of time with as many digits as it is worth.
pub(crate) fn percent(share: f32) -> String {
    let percent = share * 100.0;
    if percent >= 10.0 || percent == 0.0 {
        format!("{percent:.0}%")
    } else if percent >= 0.1 {
        format!("{percent:.1}%")
    } else {
        "<0.1%".into()
    }
}

/// A cell is "▀": its foreground is the upper of two levels, its
/// background the lower one. Where the signal never was, nothing is drawn.
fn draw_map(
    frame: &mut Frame,
    view: &SpectrumView,
    map: &DensityMap,
    range: (f32, f32),
    cursor: Option<usize>,
    area: Rect,
) {
    let width = usize::from(area.width);
    let rows = usize::from(area.height) * 2;
    let pixels = pixels(map, view.visible_bins(map.bins), width, rows, range);
    let color = |column: usize, row: usize| brightness(pixels[row * width + column]).map(heat);
    let buffer = frame.buffer_mut();
    for y in 0..usize::from(area.height) {
        let upper_row = rows - 1 - 2 * y;
        for column in 0..width {
            let cell = &mut buffer[(area.x + column as u16, area.y + y as u16)];
            match (color(column, upper_row), color(column, upper_row - 1)) {
                (None, None) if cursor == Some(column) => {
                    cell.set_symbol("│")
                        .set_style(Style::new().fg(Color::DarkGray));
                }
                (None, None) => {}
                (Some(upper), lower) => {
                    cell.set_symbol("▀")
                        .set_style(Style::new().fg(upper).bg(lower.unwrap_or(Color::Reset)));
                }
                (None, Some(lower)) => {
                    cell.set_symbol("▄").set_style(Style::new().fg(lower));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_share_of_time_is_told_with_the_digits_it_is_worth() {
        assert_eq!(percent(1.0), "100%");
        assert_eq!(percent(0.123), "12%");
        assert_eq!(percent(0.0234), "2.3%");
        assert_eq!(percent(0.001), "0.1%");
        assert_eq!(percent(0.0004), "<0.1%");
        assert_eq!(percent(0.0), "0%");
    }
}
