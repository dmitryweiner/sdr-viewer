//! Where the node transmits: the frequency of its slot, worked out as the
//! firmware does (`RadioInterface::applyModemConfig`, 2.7.26), since the
//! API tells the region, the preset and the slot but not the frequency.

use crate::proto::config::LoRaConfig;
use crate::proto::config::lo_ra_config::{ModemPreset, RegionCode};

/// The node's radio as the screen tells it.
#[derive(Debug, Clone, PartialEq)]
pub struct Radio {
    /// "US", "EU_868".
    pub region: String,
    /// "MediumFast", or "Custom" without a preset.
    pub preset: String,
    /// The slot it is on, from 1; none with a frequency of its own.
    pub slot: Option<u32>,
    pub frequency_mhz: Option<f64>,
}

/// The radio of `lora`; `primary_channel` is the name of the primary
/// channel, which picks the slot when none is set (empty: the preset's).
pub fn radio(lora: &LoRaConfig, primary_channel: &str) -> Radio {
    let region = RegionCode::try_from(lora.region).ok();
    let preset = ModemPreset::try_from(lora.modem_preset).unwrap_or(ModemPreset::LongFast);
    let preset_name = if lora.use_preset {
        preset_name(preset)
    } else {
        "Custom"
    };
    let mut radio = Radio {
        region: region.map_or_else(|| lora.region.to_string(), |r| r.as_str_name().into()),
        preset: preset_name.into(),
        slot: None,
        frequency_mhz: None,
    };
    let Some((start, end, wide)) = region.and_then(band) else {
        return radio;
    };
    let bw_khz = if lora.use_preset {
        preset_khz(preset, wide)
    } else {
        bandwidth_khz(lora.bandwidth)
    };
    // The firmware's arithmetic is f32; so is this, to land where it does.
    let channels = ((end - start) / (bw_khz / 1000.0)).floor() as u32;
    if channels == 0 {
        return radio;
    }
    let name = if primary_channel.is_empty() {
        preset_name
    } else {
        primary_channel
    };
    let index = match lora.channel_num {
        0 => hash(name),
        slot => slot - 1,
    } % channels;
    let mut mhz = start + bw_khz / 2000.0 + index as f32 * (bw_khz / 1000.0);
    if lora.override_frequency != 0.0 {
        mhz = lora.override_frequency;
    } else {
        radio.slot = Some(index + 1);
    }
    radio.frequency_mhz = Some(f64::from(mhz) + f64::from(lora.frequency_offset));
    radio
}

/// A region's band in MHz and whether it takes the wide presets (2.4 GHz).
fn band(region: RegionCode) -> Option<(f32, f32, bool)> {
    use RegionCode::*;
    let (start, end) = match region {
        Us | Unset => (902.0, 928.0),
        Eu433 => (433.0, 434.0),
        Eu868 => (869.4, 869.65),
        Cn => (470.0, 510.0),
        Jp => (920.5, 923.5),
        Anz => (915.0, 928.0),
        Anz433 => (433.05, 434.79),
        Ru => (868.7, 869.2),
        Kr => (920.0, 923.0),
        Tw => (920.0, 925.0),
        In => (865.0, 867.0),
        Nz865 => (864.0, 868.0),
        Th => (920.0, 925.0),
        Ua433 => (433.0, 434.7),
        Ua868 => (868.0, 868.6),
        My433 => (433.0, 435.0),
        My919 => (919.0, 924.0),
        Sg923 => (917.0, 925.0),
        Ph433 => (433.0, 434.7),
        Ph868 => (868.0, 869.4),
        Ph915 => (915.0, 918.0),
        Kz433 => (433.075, 434.775),
        Kz863 => (863.0, 868.0),
        Np865 => (865.0, 868.0),
        Br902 => (902.0, 907.5),
        Lora24 => return Some((2400.0, 2483.5, true)),
        #[allow(unreachable_patterns)]
        _ => return None,
    };
    Some((start, end, false))
}

/// A preset's bandwidth in kHz (`modemPresetToParams`).
#[allow(deprecated)]
fn preset_khz(preset: ModemPreset, wide: bool) -> f32 {
    use ModemPreset::*;
    match (preset, wide) {
        (ShortTurbo | LongTurbo, false) => 500.0,
        (ShortTurbo | LongTurbo, true) => 1625.0,
        (LongModerate | LongSlow, false) => 125.0,
        (LongModerate | LongSlow, true) => 406.25,
        (_, false) => 250.0,
        (_, true) => 812.5,
    }
}

/// The custom bandwidth's codes that are not the kHz themselves
/// (`bwCodeToKHz`).
fn bandwidth_khz(code: u32) -> f32 {
    match code {
        31 => 31.25,
        62 => 62.5,
        200 => 203.125,
        400 => 406.25,
        800 => 812.5,
        1600 => 1625.0,
        code => code as f32,
    }
}

/// The name the firmware shows and hashes for a preset.
#[allow(deprecated)]
fn preset_name(preset: ModemPreset) -> &'static str {
    use ModemPreset::*;
    match preset {
        ShortTurbo => "ShortTurbo",
        ShortSlow => "ShortSlow",
        ShortFast => "ShortFast",
        MediumSlow => "MediumSlow",
        MediumFast => "MediumFast",
        LongSlow => "LongSlow",
        LongFast => "LongFast",
        LongTurbo => "LongTurbo",
        LongModerate => "LongMod",
        #[allow(unreachable_patterns)]
        _ => "Invalid",
    }
}

/// The firmware's hash of a channel's name (djb2).
fn hash(name: &str) -> u32 {
    name.bytes().fold(5381_u32, |hash, byte| {
        hash.wrapping_mul(33).wrapping_add(u32::from(byte))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lora(region: RegionCode, preset: ModemPreset, slot: u32) -> LoRaConfig {
        LoRaConfig {
            use_preset: true,
            region: region as i32,
            modem_preset: preset as i32,
            channel_num: slot,
            ..LoRaConfig::default()
        }
    }

    #[test]
    fn the_users_mesh_is_on_slot_70_at_919_375() {
        let radio = radio(&lora(RegionCode::Us, ModemPreset::MediumFast, 70), "");
        assert_eq!(radio.region, "US");
        assert_eq!(radio.preset, "MediumFast");
        assert_eq!(radio.slot, Some(70));
        assert_eq!(radio.frequency_mhz, Some(919.375));
    }

    #[test]
    fn without_a_slot_the_channels_name_picks_it() {
        // The device's log before its region was set: LongFast, 104
        // channels of 250 kHz, slot 20 at 906.875 MHz.
        assert_eq!(hash("LongFast") % 104, 19);
        let unset = radio(&lora(RegionCode::Unset, ModemPreset::LongFast, 0), "");
        assert_eq!((unset.slot, unset.frequency_mhz), (Some(20), Some(906.875)));
        // Europe's 868 band holds one LongFast channel; the firmware's f32
        // puts it 24 Hz off 869.525, and so does this.
        let eu = radio(&lora(RegionCode::Eu868, ModemPreset::LongFast, 0), "");
        assert!((eu.frequency_mhz.unwrap() - 869.525).abs() < 1e-4, "{eu:?}");
        // A named primary channel moves it.
        let named = radio(&lora(RegionCode::Us, ModemPreset::LongFast, 0), "Haifa");
        assert_eq!(named.slot, Some(hash("Haifa") % 104 + 1));
    }

    #[test]
    fn an_own_frequency_and_an_offset_are_the_nodes() {
        let mut config = lora(RegionCode::Us, ModemPreset::MediumFast, 70);
        config.frequency_offset = 0.01;
        let offset = radio(&config, "").frequency_mhz.unwrap();
        assert!((offset - 919.385).abs() < 1e-6, "{offset}");
        config.override_frequency = 915.5;
        let own = radio(&config, "");
        assert_eq!(own.slot, None);
        assert!((own.frequency_mhz.unwrap() - 915.51).abs() < 1e-6);
        // Without a preset the bandwidth is the config's own code.
        let mut custom = lora(RegionCode::Us, ModemPreset::LongFast, 1);
        custom.use_preset = false;
        custom.bandwidth = 125;
        let custom = radio(&custom, "");
        assert_eq!(custom.preset, "Custom");
        assert_eq!(custom.frequency_mhz, Some(902.0625));
    }

    #[test]
    fn a_region_the_table_does_not_hold_has_no_frequency() {
        let mut config = lora(RegionCode::Us, ModemPreset::LongFast, 1);
        config.region = 9999;
        let unknown = radio(&config, "");
        assert_eq!(unknown.region, "9999");
        assert_eq!(unknown.frequency_mhz, None);
    }
}
