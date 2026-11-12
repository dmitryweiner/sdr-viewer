//! The spectrum screen without a terminal: read the start of a stream as
//! fast as it comes, print the screen as text and list the strongest
//! signals and the detector's events. For looking at a recording from a script or over SSH.

use std::fmt::Write;

use sdr_core::units::format_hz;
use sdr_core::{Iq, Source, SpectrumFrame};
use sdr_detect::Event;
use sdr_tui::{App, SourceState, ui};

use crate::pipeline::{FFT_SIZE, Output, Pipeline};

/// A peak must stand this far above the noise floor to be listed.
const MIN_ABOVE_FLOOR_DB: f32 = 10.0;
const MAX_PEAKS: usize = 8;
const MAX_EVENTS: usize = 40;

pub fn run(
    mut source: impl Source,
    (width, height): (u16, u16),
    secs: f64,
) -> Result<String, String> {
    let info = source.info();
    let mut app = App::new(info.clone());
    let mut pipeline = Pipeline::new(info.sample_rate);
    let mut events = Vec::new();
    let wanted = (secs * f64::from(info.sample_rate)) as u64;
    let mut buf = vec![Iq::new(0.0, 0.0); 16 * 1024];
    let mut seen = 0u64;
    let mut frames = 0;
    while seen < wanted {
        let n = source.read(&mut buf).map_err(|err| err.to_string())?;
        if n == 0 {
            break;
        }
        seen += n as u64;
        pipeline.push(&buf[..n], &info, |output| match output {
            Output::Frame(frame) => {
                app.spectrum.on_frame(frame);
                frames += 1;
            }
            Output::Event(event) => events.push(event),
        });
    }
    app.status.state = SourceState::Ended;

    let mut text = ui::render_text(&app, width, height);
    let seconds = seen as f64 / f64::from(info.sample_rate);
    let _ = writeln!(text, "\n{frames} frames from {seconds:.1} s.");
    let (Some(latest), Some((bottom, _))) = (app.spectrum.latest(), app.spectrum.range()) else {
        return Ok(text);
    };
    let latest = latest.clone();
    // The scale starts 2 dB under the floor.
    let floor = bottom + 2.0;
    let peaks = peaks(
        app.spectrum.max_hold(),
        floor + MIN_ABOVE_FLOOR_DB,
        FFT_SIZE / 100,
    );
    let _ = writeln!(
        text,
        "Noise floor {floor:.1} dBFS per bin. Max hold peaks above floor + 10 dB:"
    );
    if peaks.is_empty() {
        let _ = writeln!(text, "  none");
    }
    for (bin, db) in peaks.into_iter().take(MAX_PEAKS) {
        let hz = frame_hz(&latest, bin);
        let _ = writeln!(
            text,
            "  {:>13}  {db:6.1} dBFS  +{:.1} dB",
            format_hz(hz),
            db - floor
        );
    }
    write_events(&mut text, &events, &pipeline.open_events());
    Ok(text)
}

/// The detector's events in the order they began; those still going on at
/// the end have a `+` after their duration.
fn write_events(text: &mut String, ended: &[Event], open: &[Event]) {
    let _ = writeln!(
        text,
        "Events: {} ended, {} still on at the end.",
        ended.len(),
        open.len()
    );
    let mut rows: Vec<(&Event, char)> = ended
        .iter()
        .map(|event| (event, ' '))
        .chain(open.iter().map(|event| (event, '+')))
        .collect();
    rows.sort_by_key(|(event, _)| event.start);
    if !rows.is_empty() {
        let _ = writeln!(
            text,
            "  {:>9}  {:>9}   {:>13}  {:>11}  {:>11}  above floor",
            "start", "duration", "centre", "width", "peak"
        );
    }
    for (event, mark) in rows.iter().take(MAX_EVENTS) {
        let _ = writeln!(
            text,
            "  {:>7.3} s  {:>7.3} s{mark}  {:>13}  {:>11}  {:>6.1} dBFS  +{:.1} dB",
            event.start.as_secs_f64(),
            event.duration.as_secs_f64(),
            format_hz(event.center_hz()),
            format_hz(event.width_hz()),
            event.peak_dbfs,
            event.above_floor_db
        );
    }
    if rows.len() > MAX_EVENTS {
        let _ = writeln!(text, "  … and {} more", rows.len() - MAX_EVENTS);
    }
}

fn frame_hz(frame: &SpectrumFrame, bin: usize) -> f64 {
    frame.bin_hz(bin)
}

/// Bins above `threshold` that are the strongest within `apart` bins on
/// either side, strongest first.
fn peaks(power_db: &[f32], threshold: f32, apart: usize) -> Vec<(usize, f32)> {
    let mut found: Vec<(usize, f32)> = power_db
        .iter()
        .copied()
        .enumerate()
        .filter(|&(bin, db)| {
            let from = bin.saturating_sub(apart);
            let to = (bin + apart + 1).min(power_db.len());
            db > threshold && power_db[from..to].iter().all(|&other| other <= db)
        })
        .collect();
    found.sort_by(|a, b| b.1.total_cmp(&a.1));
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdr_core::generator::{Generator, GeneratorConfig};

    #[test]
    fn peaks_are_local_maxima_above_the_threshold_strongest_first() {
        let mut power = vec![-90.0; 100];
        power[10] = -40.0;
        power[11] = -45.0; // the skirt of the peak at 10
        power[50] = -20.0;
        power[80] = -85.0; // below the threshold
        assert_eq!(peaks(&power, -80.0, 3), [(50, -20.0), (10, -40.0)]);
    }

    #[test]
    fn a_snapshot_of_the_demo_scene_shows_and_lists_both_signals() {
        let generator = Generator::new(GeneratorConfig::demo());
        let text = run(generator, (100, 30), 3.0).unwrap();
        assert_eq!(text.lines().take(30).count(), 30);
        assert!(
            text.contains("peak 434.200 MHz") || text.contains("peak 433.500 MHz"),
            "{text}"
        );
        assert!(text.contains("29 frames from 3.0 s."), "{text}");
        // The burst sits on a bin; the carrier falls between two and reads
        // 0.9 dB low, as a Hann window does.
        let listed: Vec<&str> = text
            .lines()
            .take_while(|line| !line.starts_with("Events:"))
            .filter(|line| line.contains("dBFS  +"))
            .collect();
        assert_eq!(listed.len(), 2, "{text}");
        assert!(
            listed[0].contains("434.200 MHz") && listed[0].contains("-25.0 dBFS"),
            "{text}"
        );
        assert!(
            listed[1].contains("433.500 MHz") && listed[1].contains("-30.9 dBFS"),
            "{text}"
        );
    }

    #[test]
    fn a_snapshot_lists_the_events_in_the_order_they_began() {
        let generator = Generator::new(GeneratorConfig::demo());
        let text = run(generator, (100, 30), 3.0).unwrap();
        let from = text.find("Events:").expect(&text);
        let lines: Vec<&str> = text[from..].lines().collect();
        assert_eq!(lines[0], "Events: 2 ended, 1 still on at the end.");
        assert_eq!(lines.len(), 5, "{text}");
        // The burst: 0.3 s every 2.04 s.
        assert!(lines[2].contains("0.000 s    0.307 s "), "{text}");
        assert!(lines[2].contains("434.200 MHz"), "{text}");
        // The carrier, on from the start to the end.
        assert!(lines[3].contains("0.000 s    3.000 s+ "), "{text}");
        assert!(lines[3].contains("433.500 MHz"), "{text}");
        assert!(lines[4].contains("2.038 s    0.307 s "), "{text}");
        assert!(lines[4].contains("-25.0 dBFS  +66"), "{text}");
    }
}
