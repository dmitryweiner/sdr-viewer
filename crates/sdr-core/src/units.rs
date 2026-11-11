//! Frequencies and rates as people type and read them.

/// Parses "433.92M", "100k", "1.2G" or a plain number of hertz.
pub fn parse_hz(text: &str) -> Option<f64> {
    let text = text.trim();
    let (number, scale) = match text.chars().last()?.to_ascii_lowercase() {
        'k' => (&text[..text.len() - 1], 1e3),
        'm' => (&text[..text.len() - 1], 1e6),
        'g' => (&text[..text.len() - 1], 1e9),
        _ => (text, 1.0),
    };
    let value: f64 = number.parse().ok()?;
    (value.is_finite() && value >= 0.0).then_some(value * scale)
}

/// "433.920 MHz", "100.000 kHz", "950 Hz" — three decimals above hertz.
pub fn format_hz(hz: f64) -> String {
    with_unit(hz, ["Hz", "kHz", "MHz", "GHz"])
}

/// "2.400 MS/s", "250.000 kS/s", "0 S/s".
pub fn format_rate(samples_per_second: f64) -> String {
    with_unit(samples_per_second, ["S/s", "kS/s", "MS/s", "GS/s"])
}

fn with_unit(value: f64, units: [&str; 4]) -> String {
    let magnitude = value.abs();
    if magnitude >= 1e9 {
        format!("{:.3} {}", value / 1e9, units[3])
    } else if magnitude >= 1e6 {
        format!("{:.3} {}", value / 1e6, units[2])
    } else if magnitude >= 1e3 {
        format!("{:.3} {}", value / 1e3, units[1])
    } else {
        format!("{value:.0} {}", units[0])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_suffixes_and_plain_numbers() {
        assert_eq!(parse_hz("433.92M"), Some(433_920_000.0));
        assert_eq!(parse_hz("100k"), Some(100_000.0));
        assert_eq!(parse_hz("100K"), Some(100_000.0));
        assert_eq!(parse_hz("1.09G"), Some(1_090_000_000.0));
        assert_eq!(parse_hz("2400000"), Some(2_400_000.0));
        assert_eq!(parse_hz(" 2.4M "), Some(2_400_000.0));
    }

    #[test]
    fn rejects_garbage_and_negative() {
        assert_eq!(parse_hz(""), None);
        assert_eq!(parse_hz("M"), None);
        assert_eq!(parse_hz("abc"), None);
        assert_eq!(parse_hz("-5M"), None);
        assert_eq!(parse_hz("1.2X"), None);
    }

    #[test]
    fn formats_with_the_largest_fitting_unit() {
        assert_eq!(format_hz(433_920_000.0), "433.920 MHz");
        assert_eq!(format_hz(1_090_000_000.0), "1.090 GHz");
        assert_eq!(format_hz(100_000.0), "100.000 kHz");
        assert_eq!(format_hz(950.0), "950 Hz");
        assert_eq!(format_hz(0.0), "0 Hz");
    }

    #[test]
    fn formats_rates() {
        assert_eq!(format_rate(2_400_000.0), "2.400 MS/s");
        assert_eq!(format_rate(250_000.0), "250.000 kS/s");
        assert_eq!(format_rate(0.0), "0 S/s");
    }
}
