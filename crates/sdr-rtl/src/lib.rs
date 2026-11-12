//! The RTL-SDR as a [`Source`].
//!
//! The driver (`rtl-sdr-rs` on `nusb`) talks to the device through usbfs
//! and does not detach the kernel's DVB driver: where `dvb_usb_rtl28xxu`
//! is bound to the device, it has to be unbound or blacklisted first
//! (`scripts/hub-speed.sh` does it on the user's uConsole).

use rtl_sdr_rs::{DeviceId, RtlSdr, TunerGain};
use sdr_core::file::SampleFormat;
use sdr_core::{Iq, Source, SourceError, SourceInfo};

/// The tuner's range; outside it the PLL does not lock.
pub const MIN_HZ: f64 = 24e6;
pub const MAX_HZ: f64 = 1766e6;

/// Bulk transfers must be whole USB packets.
const PACKET_BYTES: usize = 512;

#[derive(Debug, Clone, PartialEq)]
pub struct RtlConfig {
    pub sample_rate: u32,
    pub center_hz: f64,
    /// Tuner gain in dB, set to the nearest step the tuner has; `None`
    /// leaves it to the tuner's automatic gain.
    pub gain_db: Option<f32>,
}

pub struct RtlSource {
    sdr: RtlSdr,
    info: SourceInfo,
    bytes: Vec<u8>,
}

impl RtlSource {
    /// Opens the first RTL-SDR and sets it up.
    pub fn open(config: &RtlConfig) -> Result<Self, SourceError> {
        let center = checked_center(config.center_hz)?;
        let mut sdr = RtlSdr::open(DeviceId::Index(0)).map_err(device_error)?;
        sdr.set_sample_rate(config.sample_rate)
            .map_err(device_error)?;
        sdr.set_center_freq(center).map_err(device_error)?;
        let (gain, gain_text) = match config.gain_db {
            None => (TunerGain::Auto, "auto gain".to_string()),
            Some(wanted) => {
                let steps = sdr.get_tuner_gains().map_err(device_error)?;
                let tenths = nearest_gain(&steps, wanted)
                    .ok_or_else(|| SourceError::Device("the tuner reports no gain steps".into()))?;
                (
                    TunerGain::Manual(tenths),
                    format!("{:.1} dB", f64::from(tenths) / 10.0),
                )
            }
        };
        sdr.set_tuner_gain(gain).map_err(device_error)?;
        sdr.reset_buffer().map_err(device_error)?;
        let tuner = sdr.get_tuner_id().map_err(device_error)?.to_string();
        let info = SourceInfo {
            sample_rate: config.sample_rate,
            center_hz: config.center_hz,
            description: format!("rtl-sdr {tuner}, {gain_text}"),
        };
        Ok(RtlSource {
            sdr,
            info,
            bytes: Vec::new(),
        })
    }
}

impl Source for RtlSource {
    fn info(&self) -> SourceInfo {
        self.info.clone()
    }

    fn read(&mut self, buf: &mut [Iq]) -> Result<usize, SourceError> {
        let bytes = transfer_bytes(buf.len()).ok_or(SourceError::Unsupported(
            "reading fewer than 256 samples from an RTL-SDR",
        ))?;
        self.bytes.resize(bytes, 0);
        let got = self.sdr.read_sync(&mut self.bytes).map_err(device_error)?;
        let samples = got / 2;
        SampleFormat::Cu8.decode(&self.bytes[..samples * 2], &mut buf[..samples]);
        Ok(samples)
    }

    fn tune(&mut self, center_hz: f64) -> Result<(), SourceError> {
        self.sdr
            .set_center_freq(checked_center(center_hz)?)
            .map_err(device_error)?;
        self.info.center_hz = center_hz;
        Ok(())
    }
}

impl Drop for RtlSource {
    fn drop(&mut self) {
        let _ = self.sdr.close();
    }
}

fn device_error(err: rtl_sdr_rs::error::RtlsdrError) -> SourceError {
    SourceError::Device(err.to_string())
}

fn checked_center(center_hz: f64) -> Result<u32, SourceError> {
    if (MIN_HZ..=MAX_HZ).contains(&center_hz) {
        Ok(center_hz.round() as u32)
    } else {
        Err(SourceError::Unsupported(
            "a frequency outside the tuner's 24–1766 MHz",
        ))
    }
}

/// The largest whole number of USB packets that fits `samples`; `None` if
/// not even one does.
fn transfer_bytes(samples: usize) -> Option<usize> {
    let bytes = samples * 2 / PACKET_BYTES * PACKET_BYTES;
    (bytes > 0).then_some(bytes)
}

/// The tuner's step (in tenths of a dB, as the driver counts) nearest to
/// `wanted_db`.
fn nearest_gain(steps: &[i32], wanted_db: f32) -> Option<i32> {
    let wanted = (wanted_db * 10.0).round() as i32;
    steps
        .iter()
        .copied()
        .min_by_key(|step| (step - wanted).abs())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The R820T's steps, as `rtl_test` listed them on the device.
    const R820T: [i32; 29] = [
        0, 9, 14, 27, 37, 77, 87, 125, 144, 157, 166, 197, 207, 229, 254, 280, 297, 328, 338, 364,
        372, 386, 402, 421, 434, 439, 445, 480, 496,
    ];

    #[test]
    fn gain_goes_to_the_nearest_step_the_tuner_has() {
        assert_eq!(nearest_gain(&R820T, 30.0), Some(297));
        assert_eq!(nearest_gain(&R820T, 0.0), Some(0));
        assert_eq!(nearest_gain(&R820T, 100.0), Some(496));
        assert_eq!(nearest_gain(&R820T, -5.0), Some(0));
        assert_eq!(nearest_gain(&R820T, 20.2), Some(197));
        assert_eq!(nearest_gain(&[], 20.0), None);
    }

    #[test]
    fn transfers_are_whole_packets_no_larger_than_the_buffer() {
        assert_eq!(transfer_bytes(16 * 1024), Some(32768));
        assert_eq!(transfer_bytes(1000), Some(1536));
        assert_eq!(transfer_bytes(256), Some(512));
        assert_eq!(transfer_bytes(255), None);
    }

    #[test]
    fn frequencies_outside_the_tuner_are_refused() {
        assert_eq!(checked_center(433.92e6).unwrap(), 433_920_000);
        assert!(checked_center(10e6).is_err());
        assert!(checked_center(2.4e9).is_err());
    }
}
