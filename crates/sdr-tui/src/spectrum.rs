//! The spectrum screen's state and the arithmetic of drawing it: no
//! terminal here, so everything is tested with plain numbers.

use std::collections::VecDeque;

use ratatui::style::Color;
use sdr_core::SpectrumFrame;

/// Frames kept for the waterfall: more than any terminal has pixel rows.
pub const HISTORY: usize = 256;

/// How far below the noise floor the scale starts, and its height, in dB.
/// 50 dB is about what an 8-bit receiver can show above its noise; with a
/// wider scale the floor glows and real signals drown in it (seen on the
/// device with the first recording).
const BELOW_FLOOR_DB: f32 = 2.0;
const RANGE_DB: f32 = 50.0;

/// How fast the scale follows the noise floor: the share of a new frame.
const FLOOR_FOLLOW: f32 = 0.1;

#[derive(Default)]
pub struct SpectrumView {
    /// Newest first.
    frames: VecDeque<SpectrumFrame>,
    max_hold: Vec<f32>,
    floor_db: Option<f32>,
    /// Position across the width, 0 = the left edge, 1 = the right.
    pub cursor: Option<f64>,
    pub paused: bool,
    /// The visible part is 1 / 2^zoom of the received band...
    zoom: u32,
    /// ...starting this far across it, 0 = the band's left edge.
    start: f64,
}

/// The deepest zoom: 1/16 of the band, 128 of the 2048 bins.
pub const MAX_ZOOM: u32 = 4;

impl SpectrumView {
    pub fn on_frame(&mut self, frame: SpectrumFrame) {
        if self.paused {
            return;
        }
        let same_view = self.latest().is_some_and(|last| {
            last.power_db.len() == frame.power_db.len()
                && last.center_hz == frame.center_hz
                && last.sample_rate == frame.sample_rate
        });
        if !same_view {
            // Another band: the old picture says nothing about it.
            self.frames.clear();
            self.max_hold.clear();
            self.floor_db = None;
        }
        let floor = median(&frame.power_db);
        self.floor_db = Some(
            self.floor_db
                .map_or(floor, |old| old + (floor - old) * FLOOR_FOLLOW),
        );
        if self.max_hold.is_empty() {
            self.max_hold = frame.power_db.clone();
        } else {
            for (held, new) in self.max_hold.iter_mut().zip(&frame.power_db) {
                *held = held.max(*new);
            }
        }
        self.frames.push_front(frame);
        self.frames.truncate(HISTORY);
    }

    pub fn latest(&self) -> Option<&SpectrumFrame> {
        self.frames.front()
    }

    /// Newest first.
    pub fn frames(&self) -> impl Iterator<Item = &SpectrumFrame> {
        self.frames.iter()
    }

    pub fn max_hold(&self) -> &[f32] {
        &self.max_hold
    }

    pub fn reset_max_hold(&mut self) {
        self.max_hold = self
            .latest()
            .map(|frame| frame.power_db.clone())
            .unwrap_or_default();
    }

    /// The dB at the bottom and at the top of the scale: it follows the
    /// noise floor, so the picture survives a change of gain.
    pub fn range(&self) -> Option<(f32, f32)> {
        let bottom = self.floor_db? - BELOW_FLOOR_DB;
        Some((bottom, bottom + RANGE_DB))
    }

    /// How many times the visible part is narrower than the band.
    pub fn magnification(&self) -> u32 {
        1 << self.zoom
    }

    fn width(&self) -> f64 {
        1.0 / f64::from(self.magnification())
    }

    /// A position across the visible part as a position across the band.
    fn across_band(&self, across_view: f64) -> f64 {
        self.start + across_view * self.width()
    }

    /// The frequency at `across_view` of the visible part, 0 = its left edge.
    pub fn hz(&self, frame: &SpectrumFrame, across_view: f64) -> f64 {
        frame.hz_at(self.across_band(across_view))
    }

    /// The width of the visible part in hertz.
    pub fn span_hz(&self, frame: &SpectrumFrame) -> f64 {
        f64::from(frame.sample_rate) * self.width()
    }

    /// The bins of `power_db` that are in view.
    pub fn visible<'a>(&self, power_db: &'a [f32]) -> &'a [f32] {
        let bins = power_db.len();
        let count = (bins >> self.zoom).max(1).min(bins);
        let first = ((self.start * bins as f64).round() as usize).min(bins - count);
        &power_db[first..first + count]
    }

    /// Halves the visible part around the cursor, or around its middle.
    pub fn zoom_in(&mut self) {
        if self.zoom < MAX_ZOOM {
            self.set_zoom(self.zoom + 1);
        }
    }

    pub fn zoom_out(&mut self) {
        if self.zoom > 0 {
            self.set_zoom(self.zoom - 1);
        }
    }

    /// Changes the zoom keeping the cursor on its frequency.
    fn set_zoom(&mut self, zoom: u32) {
        let anchor = self.across_band(self.cursor.unwrap_or(0.5));
        self.zoom = zoom;
        let width = self.width();
        self.start = (anchor - width / 2.0).clamp(0.0, 1.0 - width);
        if self.cursor.is_some() {
            self.cursor = Some(((anchor - self.start) / width).clamp(0.0, 1.0));
        }
    }

    /// Puts the visible part, and the cursor if there is one, in the middle
    /// of the band: where a frequency just tuned to sits.
    pub fn recenter(&mut self) {
        self.start = (1.0 - self.width()) / 2.0;
        if self.cursor.is_some() {
            self.cursor = Some(0.5);
        }
    }

    /// Moves the cursor by a share of the width; it appears in the middle.
    pub fn move_cursor(&mut self, by: f64) {
        self.cursor = Some(self.cursor.map_or(0.5, |at| (at + by).clamp(0.0, 1.0)));
    }
}

fn median(values: &[f32]) -> f32 {
    let mut sorted = values.to_vec();
    let middle = sorted.len() / 2;
    let (_, median, _) = sorted.select_nth_unstable_by(middle, f32::total_cmp);
    *median
}

/// Squeezes bins into `width` columns, keeping the strongest bin of each,
/// so a narrow signal never falls between columns.
pub fn columns(power_db: &[f32], width: usize) -> Vec<f32> {
    let bins = power_db.len();
    (0..width)
        .map(|column| {
            let start = column * bins / width;
            let end = ((column + 1) * bins / width).max(start + 1).min(bins);
            power_db[start.min(bins - 1)..end]
                .iter()
                .copied()
                .fold(f32::MIN, f32::max)
        })
        .collect()
}

/// The column a cursor at `fraction` of the width sits in.
pub fn cursor_column(fraction: f64, width: usize) -> usize {
    ((fraction * width as f64) as usize).min(width.saturating_sub(1))
}

/// Where `db` sits on the scale: 0 at the bottom, 1 at the top.
pub fn level(db: f32, (bottom, top): (f32, f32)) -> f32 {
    ((db - bottom) / (top - bottom)).clamp(0.0, 1.0)
}

/// How much of the cell `row` rows above the bottom a bar of `level` fills,
/// in eighths, on a plot `rows` high.
pub fn bar_eighths(level: f32, rows: usize, row: usize) -> usize {
    let total = (level * (rows * 8) as f32).round() as usize;
    total.saturating_sub(row * 8).min(8)
}

pub const BARS: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

/// Dark blue for the floor through green and yellow to white for the top.
/// Brightness only ever grows, so stronger always reads as brighter.
const HEAT: [(u8, u8, u8); 7] = [
    (0, 0, 12),
    (10, 25, 110),
    (0, 110, 170),
    (40, 180, 80),
    (230, 200, 0),
    (255, 235, 120),
    (255, 255, 255),
];

pub fn heat(level: f32) -> Color {
    let position = level.clamp(0.0, 1.0) * (HEAT.len() - 1) as f32;
    let index = (position as usize).min(HEAT.len() - 2);
    let share = position - index as f32;
    let (from, to) = (HEAT[index], HEAT[index + 1]);
    let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * share).round() as u8;
    Color::Rgb(mix(from.0, to.0), mix(from.1, to.1), mix(from.2, to.2))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn frame(power_db: Vec<f32>) -> SpectrumFrame {
        SpectrumFrame {
            time: Duration::ZERO,
            center_hz: 100e6,
            sample_rate: 1_000_000,
            power_db,
        }
    }

    #[test]
    fn keeps_frames_newest_first_and_only_so_many() {
        let mut view = SpectrumView::default();
        for i in 0..HISTORY + 10 {
            view.on_frame(frame(vec![i as f32; 4]));
        }
        assert_eq!(view.frames().count(), HISTORY);
        assert_eq!(view.latest().unwrap().power_db[0], (HISTORY + 9) as f32);
        assert_eq!(view.frames().last().unwrap().power_db[0], 10.0);
    }

    #[test]
    fn max_hold_remembers_the_strongest_of_each_bin_until_reset() {
        let mut view = SpectrumView::default();
        view.on_frame(frame(vec![-50.0, -90.0, -90.0]));
        view.on_frame(frame(vec![-90.0, -40.0, -90.0]));
        assert_eq!(view.max_hold(), [-50.0, -40.0, -90.0]);
        view.reset_max_hold();
        assert_eq!(view.max_hold(), [-90.0, -40.0, -90.0]);
    }

    #[test]
    fn the_scale_sits_on_the_noise_floor_and_follows_it_slowly() {
        let mut view = SpectrumView::default();
        assert_eq!(view.range(), None);
        view.on_frame(frame(vec![-90.0, -90.0, -90.0, -90.0, -20.0]));
        assert_eq!(view.range(), Some((-92.0, -42.0)));
        view.on_frame(frame(vec![-80.0; 5]));
        assert_eq!(view.range(), Some((-91.0, -41.0)));
    }

    #[test]
    fn another_band_starts_the_picture_afresh() {
        let mut view = SpectrumView::default();
        view.on_frame(frame(vec![-20.0; 4]));
        let mut retuned = frame(vec![-90.0; 4]);
        retuned.center_hz = 200e6;
        view.on_frame(retuned);
        assert_eq!(view.frames().count(), 1);
        assert_eq!(view.max_hold(), [-90.0; 4]);
        assert_eq!(view.range(), Some((-92.0, -42.0)));
    }

    #[test]
    fn a_paused_view_ignores_new_frames() {
        let mut view = SpectrumView::default();
        view.on_frame(frame(vec![-50.0; 4]));
        view.paused = true;
        view.on_frame(frame(vec![-10.0; 4]));
        assert_eq!(view.frames().count(), 1);
        assert_eq!(view.max_hold(), [-50.0; 4]);
    }

    #[test]
    fn the_cursor_appears_in_the_middle_and_stops_at_the_edges() {
        let mut view = SpectrumView::default();
        view.move_cursor(-0.1);
        assert_eq!(view.cursor, Some(0.5));
        view.move_cursor(0.25);
        assert_eq!(view.cursor, Some(0.75));
        view.move_cursor(0.5);
        assert_eq!(view.cursor, Some(1.0));
        view.move_cursor(-3.0);
        assert_eq!(view.cursor, Some(0.0));
        assert_eq!(cursor_column(0.0, 80), 0);
        assert_eq!(cursor_column(0.5, 80), 40);
        assert_eq!(cursor_column(1.0, 80), 79);
    }

    #[test]
    fn zoom_halves_the_view_around_its_middle() {
        let mut view = SpectrumView::default();
        let frame = frame(vec![0.0; 1000]);
        let power: Vec<f32> = (0..1000).map(|i| i as f32).collect();
        assert_eq!(view.visible(&power).len(), 1000);
        assert_eq!(view.span_hz(&frame), 1e6);
        view.zoom_in();
        assert_eq!(view.magnification(), 2);
        assert_eq!(view.visible(&power), &power[250..750]);
        assert_eq!(view.hz(&frame, 0.0), 99.75e6);
        assert_eq!(view.hz(&frame, 1.0), 100.25e6);
        assert_eq!(view.span_hz(&frame), 0.5e6);
        view.zoom_out();
        view.zoom_out();
        assert_eq!(view.magnification(), 1);
        assert_eq!(view.visible(&power).len(), 1000);
    }

    #[test]
    fn zoom_keeps_the_cursor_on_its_frequency() {
        let mut view = SpectrumView::default();
        let frame = frame(vec![0.0; 1000]);
        view.cursor = Some(0.75);
        let at = view.hz(&frame, 0.75);
        view.zoom_in();
        // The cursor's frequency becomes the middle of the view.
        assert_eq!(view.cursor, Some(0.5));
        assert_eq!(view.hz(&frame, 0.5), at);
        view.zoom_in();
        view.zoom_out();
        view.zoom_out();
        assert_eq!(view.hz(&frame, view.cursor.unwrap()), at);
    }

    #[test]
    fn zoom_stops_at_the_band_edges_and_at_its_limit() {
        let mut view = SpectrumView::default();
        let frame = frame(vec![0.0; 1000]);
        view.cursor = Some(1.0);
        view.zoom_in();
        // The view cannot leave the band, so the cursor stays at its edge.
        assert_eq!(view.hz(&frame, 1.0), 100.5e6);
        assert_eq!(view.cursor, Some(1.0));
        for _ in 0..10 {
            view.zoom_in();
        }
        assert_eq!(view.magnification(), 1 << MAX_ZOOM);
    }

    #[test]
    fn recentering_shows_the_middle_of_the_band() {
        let mut view = SpectrumView::default();
        let frame = frame(vec![0.0; 1000]);
        view.cursor = Some(0.1);
        view.zoom_in();
        view.recenter();
        assert_eq!(view.hz(&frame, 0.5), 100e6);
        assert_eq!(view.cursor, Some(0.5));
        assert_eq!(view.magnification(), 2);
    }

    #[test]
    fn columns_keep_the_strongest_bin() {
        let mut power = vec![-90.0; 1000];
        power[333] = -10.0;
        let cols = columns(&power, 100);
        assert_eq!(cols.len(), 100);
        assert_eq!(cols[33], -10.0);
        assert!(cols.iter().enumerate().all(|(i, &v)| i == 33 || v == -90.0));
    }

    #[test]
    fn more_columns_than_bins_repeat_bins() {
        assert_eq!(columns(&[1.0, 2.0], 4), [1.0, 1.0, 2.0, 2.0]);
        assert_eq!(columns(&[1.0, 2.0, 3.0], 3), [1.0, 2.0, 3.0]);
    }

    #[test]
    fn level_is_clamped_to_the_scale() {
        let range = (-100.0, -30.0);
        assert_eq!(level(-100.0, range), 0.0);
        assert_eq!(level(-65.0, range), 0.5);
        assert_eq!(level(-30.0, range), 1.0);
        assert_eq!(level(-120.0, range), 0.0);
        assert_eq!(level(0.0, range), 1.0);
    }

    #[test]
    fn a_bar_fills_cells_from_the_bottom_in_eighths() {
        // Half of 4 rows: two full cells, two empty.
        let fills: Vec<usize> = (0..4).map(|row| bar_eighths(0.5, 4, row)).collect();
        assert_eq!(fills, [8, 8, 0, 0]);
        // 1.25 rows of 4.
        let fills: Vec<usize> = (0..4).map(|row| bar_eighths(1.25 / 4.0, 4, row)).collect();
        assert_eq!(fills, [8, 2, 0, 0]);
        assert_eq!(bar_eighths(1.0, 4, 3), 8);
        assert_eq!(bar_eighths(0.0, 4, 0), 0);
    }

    #[test]
    fn heat_runs_from_dark_to_white_getting_brighter() {
        assert_eq!(heat(0.0), Color::Rgb(0, 0, 12));
        assert_eq!(heat(1.0), Color::Rgb(255, 255, 255));
        assert_eq!(heat(-1.0), heat(0.0));
        assert_eq!(heat(2.0), heat(1.0));
        let luma = |color| match color {
            Color::Rgb(r, g, b) => 0.3 * f32::from(r) + 0.6 * f32::from(g) + 0.1 * f32::from(b),
            _ => panic!("not rgb"),
        };
        let steps: Vec<f32> = (0..=20).map(|i| luma(heat(i as f32 / 20.0))).collect();
        assert!(steps.windows(2).all(|pair| pair[1] > pair[0]), "{steps:?}");
    }
}
