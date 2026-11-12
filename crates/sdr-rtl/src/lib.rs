//! The RTL-SDR as a [`Source`].
//!
//! The driver (`rtl-sdr-rs` on `nusb`) talks to the device through usbfs
//! and does not detach the kernel's DVB driver: where `dvb_usb_rtl28xxu`
//! is bound to the device, it has to be unbound or blacklisted first
//! (`scripts/hub-speed.sh` does it on the user's uConsole).

use rtl_sdr_rs::{AsyncReadEvent, AsyncReadHandle, DeviceId, RtlSdr, TunerGain};
use sdr_core::file::SampleFormat;
use sdr_core::{GainChange, Iq, Source, SourceError, SourceInfo};

/// The tuner's range; outside it the PLL does not lock.
pub const MIN_HZ: f64 = 24e6;
pub const MAX_HZ: f64 = 1766e6;

/// Bytes per USB transfer, a multiple of the 512-byte packet: 14 ms at
/// 2.4 MS/s. Several transfers stay queued in the driver's worker thread;
/// reading them one at a time instead lost about 10% of the samples at
/// 2.4 MS/s on the CM4 (`docs/hardware.md`).
const TRANSFER_BYTES: usize = 64 * 1024;

/// Transfers kept queued; 0 lets the driver choose (15).
const TRANSFERS: usize = 0;

#[derive(Debug, Clone, PartialEq)]
pub struct RtlConfig {
    pub sample_rate: u32,
    pub center_hz: f64,
    /// Tuner gain in dB, set to the nearest step the tuner has; `None`
    /// leaves it to the tuner's automatic gain.
    pub gain_db: Option<f32>,
}

pub struct RtlSource {
    stream: AsyncReadHandle,
    info: SourceInfo,
    tuner: String,
    /// The tuner's gain steps in tenths of a dB, ascending.
    steps: Vec<i32>,
    /// The step in force; `None` is the tuner's automatic gain.
    gain: Option<i32>,
    /// The transfer being handed out and how much of it is already gone.
    bytes: Vec<u8>,
    taken: usize,
}

impl RtlSource {
    /// Opens the first RTL-SDR and sets it up.
    pub fn open(config: &RtlConfig) -> Result<Self, SourceError> {
        let center = checked_center(config.center_hz)?;
        let mut sdr = RtlSdr::open(DeviceId::Index(0)).map_err(device_error)?;
        sdr.set_sample_rate(config.sample_rate)
            .map_err(device_error)?;
        sdr.set_center_freq(center).map_err(device_error)?;
        let mut steps = sdr.get_tuner_gains().map_err(device_error)?;
        steps.sort_unstable();
        let gain = match config.gain_db {
            None => None,
            Some(wanted) => Some(
                nearest_gain(&steps, wanted)
                    .ok_or_else(|| SourceError::Device("the tuner reports no gain steps".into()))?,
            ),
        };
        sdr.set_tuner_gain(tuner_gain(gain)).map_err(device_error)?;
        sdr.reset_buffer().map_err(device_error)?;
        let tuner = sdr.get_tuner_id().map_err(device_error)?.to_string();
        let info = SourceInfo {
            sample_rate: config.sample_rate,
            center_hz: config.center_hz,
            description: describe(&tuner, gain),
        };
        let stream = sdr
            .into_async_reader(TRANSFERS, TRANSFER_BYTES)
            .map_err(device_error)?;
        Ok(RtlSource {
            stream,
            info,
            tuner,
            steps,
            gain,
            bytes: Vec::new(),
            taken: 0,
        })
    }
}

fn tuner_gain(tenths: Option<i32>) -> TunerGain {
    tenths.map_or(TunerGain::Auto, TunerGain::Manual)
}

fn describe(tuner: &str, gain: Option<i32>) -> String {
    match gain {
        None => format!("rtl-sdr {tuner}, auto gain"),
        Some(tenths) => format!("rtl-sdr {tuner}, {:.1} dB", f64::from(tenths) / 10.0),
    }
}

impl Source for RtlSource {
    fn info(&self) -> SourceInfo {
        self.info.clone()
    }

    fn read(&mut self, buf: &mut [Iq]) -> Result<usize, SourceError> {
        // Transfers are whole packets, so even; a stray byte would otherwise
        // read as the end of the stream.
        while self.bytes.len() - self.taken < 2 {
            match self.stream.recv() {
                Some(Ok(AsyncReadEvent::Samples { data, .. })) => {
                    self.bytes = data;
                    self.taken = 0;
                }
                // A retune took effect; the samples after it follow.
                Some(Ok(_)) => {}
                Some(Err(err)) => return Err(device_error(err)),
                None => return Ok(0),
            }
        }
        let samples = buf.len().min((self.bytes.len() - self.taken) / 2);
        let bytes = &self.bytes[self.taken..self.taken + samples * 2];
        SampleFormat::Cu8.decode(bytes, &mut buf[..samples]);
        self.taken += samples * 2;
        Ok(samples)
    }

    fn tune(&mut self, center_hz: f64) -> Result<(), SourceError> {
        let center = checked_center(center_hz)?;
        self.stream
            .control_handle()
            .set_center_freq(center)
            .map_err(device_error)?;
        self.info.center_hz = center_hz;
        Ok(())
    }

    fn set_gain(&mut self, change: GainChange) -> Result<(), SourceError> {
        let gain = match change {
            GainChange::Auto => None,
            GainChange::Step(by) => stepped_gain(&self.steps, self.gain, by),
        };
        self.stream
            .control_handle()
            .set_tuner_gain(tuner_gain(gain))
            .map_err(device_error)?;
        self.gain = gain;
        self.info.description = describe(&self.tuner, gain);
        Ok(())
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

/// The gain `by` steps away from `current`, stopping at the ends. From
/// automatic gain the walk starts in the middle of the tuner's range.
fn stepped_gain(steps: &[i32], current: Option<i32>, by: i32) -> Option<i32> {
    let last = steps.len().checked_sub(1)?;
    let index = match current {
        None => return Some(steps[last / 2]),
        Some(tenths) => steps
            .iter()
            .position(|&step| step >= tenths)
            .unwrap_or(last),
    };
    let moved = (index as i64 + i64::from(by)).clamp(0, last as i64);
    Some(steps[moved as usize])
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
    fn gain_walks_the_tuners_steps_and_stops_at_the_ends() {
        assert_eq!(stepped_gain(&R820T, Some(297), 1), Some(328));
        assert_eq!(stepped_gain(&R820T, Some(297), -2), Some(254));
        assert_eq!(stepped_gain(&R820T, Some(496), 1), Some(496));
        assert_eq!(stepped_gain(&R820T, Some(0), -1), Some(0));
        // From automatic gain: the middle of the range, whichever way.
        assert_eq!(stepped_gain(&R820T, None, 1), Some(254));
        assert_eq!(stepped_gain(&R820T, None, -1), Some(254));
        assert_eq!(stepped_gain(&[], Some(100), 1), None);
    }

    #[test]
    fn the_description_names_the_tuner_and_the_gain() {
        assert_eq!(describe("r820t", Some(297)), "rtl-sdr r820t, 29.7 dB");
        assert_eq!(describe("r820t", None), "rtl-sdr r820t, auto gain");
    }

    #[test]
    fn frequencies_outside_the_tuner_are_refused() {
        assert_eq!(checked_center(433.92e6).unwrap(), 433_920_000);
        assert!(checked_center(10e6).is_err());
        assert!(checked_center(2.4e9).is_err());
    }
}
