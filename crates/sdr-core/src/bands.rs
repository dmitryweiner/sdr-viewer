//! What a frequency is used for: the names of the bands an RTL-SDR can
//! reach, to say on the screen where one is.
//!
//! The table is ITU Region 1 (Europe, the Middle East) as commonly used,
//! by hand and from memory: a hint for the eye, not a licence. National
//! plans differ in detail, and a signal in a band need not be of the
//! service the band is named for.

/// A stretch of spectrum with a common use.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Band {
    pub low_hz: f64,
    pub high_hz: f64,
    pub name: &'static str,
}

const fn band(low_mhz: f64, high_mhz: f64, name: &'static str) -> Band {
    Band {
        low_hz: low_mhz * 1e6,
        high_hz: high_mhz * 1e6,
        name,
    }
}

/// Bands may lie inside one another: the narrowest one names a frequency.
pub const BANDS: &[Band] = &[
    band(24.89, 24.99, "amateur 12 m"),
    band(26.965, 27.405, "CB"),
    band(28.0, 29.7, "amateur 10 m"),
    band(50.0, 52.0, "amateur 6 m"),
    band(87.5, 108.0, "FM broadcast"),
    band(108.0, 117.975, "air navigation"),
    band(117.975, 137.0, "airband"),
    band(121.45, 121.55, "air emergency"),
    band(137.0, 138.0, "weather satellites"),
    band(144.0, 146.0, "amateur 2 m"),
    band(146.0, 174.0, "VHF land mobile"),
    band(156.0, 162.05, "marine VHF"),
    band(156.75, 156.85, "marine distress"),
    band(161.95, 162.05, "AIS"),
    band(174.0, 230.0, "DAB / TV"),
    band(225.0, 400.0, "military airband"),
    band(400.15, 406.0, "radiosondes"),
    band(406.0, 406.1, "emergency beacons"),
    band(406.1, 470.0, "UHF land mobile"),
    band(430.0, 440.0, "amateur 70 cm"),
    band(433.05, 434.79, "ISM 433 / LPD"),
    band(446.0, 446.2, "PMR446"),
    band(470.0, 694.0, "TV"),
    band(791.0, 862.0, "LTE 800"),
    band(863.0, 870.0, "SRD 868"),
    band(880.0, 915.0, "GSM 900 up"),
    band(915.0, 921.0, "ISM 915"),
    band(925.0, 960.0, "GSM 900 down"),
    band(960.0, 1215.0, "air navigation"),
    band(1087.0, 1093.0, "ADS-B"),
    band(1240.0, 1300.0, "amateur 23 cm"),
    band(1525.0, 1559.0, "satellite phones"),
    band(1559.0, 1610.0, "GNSS"),
    band(1616.0, 1626.5, "Iridium"),
    band(1710.0, 1785.0, "mobile 1800 up"),
    band(1805.0, 1880.0, "mobile 1800 down"),
];

/// The band `hz` is in; of several, the narrowest.
pub fn band_at(hz: f64) -> Option<&'static Band> {
    let width = |band: &&Band| band.high_hz - band.low_hz;
    BANDS
        .iter()
        .filter(|band| (band.low_hz..=band.high_hz).contains(&hz))
        .min_by(|a, b| width(a).total_cmp(&width(b)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(mhz: f64) -> Option<&'static str> {
        band_at(mhz * 1e6).map(|band| band.name)
    }

    #[test]
    fn a_frequency_is_named_by_its_band() {
        assert_eq!(name(100.0), Some("FM broadcast"));
        assert_eq!(name(118.1), Some("airband"));
        assert_eq!(name(145.5), Some("amateur 2 m"));
        assert_eq!(name(446.00625), Some("PMR446"));
        assert_eq!(name(1090.0), Some("ADS-B"));
        assert_eq!(name(919.375), Some("ISM 915"));
    }

    #[test]
    fn of_bands_inside_one_another_the_narrowest_names_it() {
        assert_eq!(name(433.92), Some("ISM 433 / LPD"));
        assert_eq!(name(432.0), Some("amateur 70 cm"));
        assert_eq!(name(445.0), Some("UHF land mobile"));
        assert_eq!(name(156.8), Some("marine distress"));
        assert_eq!(name(157.0), Some("marine VHF"));
        assert_eq!(name(162.0), Some("AIS"));
        assert_eq!(name(165.0), Some("VHF land mobile"));
        assert_eq!(name(121.5), Some("air emergency"));
        assert_eq!(name(228.0), Some("DAB / TV"));
        assert_eq!(name(300.0), Some("military airband"));
    }

    #[test]
    fn between_the_bands_there_is_no_name() {
        assert_eq!(name(10.0), None);
        assert_eq!(name(70.0), None);
        assert_eq!(name(140.0), None);
        assert_eq!(name(700.0), None);
        assert_eq!(name(2000.0), None);
    }

    #[test]
    fn the_table_is_sound() {
        for band in BANDS {
            assert!(band.low_hz < band.high_hz, "{band:?}");
            // A name is a word or two beside a frequency on an 80-column
            // screen.
            assert!((2..=18).contains(&band.name.len()), "{band:?}");
        }
    }
}
