//! The `Source` trait: anything that yields IQ samples.

use std::fmt;
use std::io;

/// One complex baseband sample, I + jQ, full scale = 1.0.
pub type Iq = num_complex::Complex32;

/// What a source receives, for display and for turning bins into hertz.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceInfo {
    /// Samples per second.
    pub sample_rate: u32,
    /// The frequency that sits at 0 Hz of the baseband.
    pub center_hz: f64,
    /// A short human-readable name: "generator", "file capture.cu8".
    pub description: String,
}

#[derive(Debug)]
pub enum SourceError {
    Io(io::Error),
    /// The receiver refused or went away; the driver's own words.
    Device(String),
    /// The source cannot do this at all (a file cannot be retuned).
    Unsupported(&'static str),
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SourceError::Io(err) => write!(f, "I/O error: {err}"),
            SourceError::Device(message) => write!(f, "receiver: {message}"),
            SourceError::Unsupported(what) => write!(f, "not supported: {what}"),
        }
    }
}

impl std::error::Error for SourceError {}

impl From<io::Error> for SourceError {
    fn from(err: io::Error) -> Self {
        SourceError::Io(err)
    }
}

/// What to do with a receiver's gain.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GainChange {
    /// Leave it to the receiver.
    Auto,
    /// So many of the receiver's own steps up (or down, if negative).
    Step(i32),
}

/// A stream of IQ samples: the RTL-SDR, a recording, or the generator.
pub trait Source: Send {
    fn info(&self) -> SourceInfo;

    /// Fills `buf` from its start and returns how many samples were written.
    /// Fewer than `buf.len()` only at the end; 0 means the stream has ended.
    fn read(&mut self, buf: &mut [Iq]) -> Result<usize, SourceError>;

    /// Moves the centre frequency.
    fn tune(&mut self, center_hz: f64) -> Result<(), SourceError>;

    /// Changes the gain; sources without one refuse.
    fn set_gain(&mut self, _change: GainChange) -> Result<(), SourceError> {
        Err(SourceError::Unsupported("this source has no gain"))
    }

    /// Samples the receiver delivered and nobody took in time, since the
    /// start: holes in the stream. Only hardware has them.
    fn lost_samples(&self) -> u64 {
        0
    }
}

impl<S: Source + ?Sized> Source for Box<S> {
    fn info(&self) -> SourceInfo {
        (**self).info()
    }

    fn read(&mut self, buf: &mut [Iq]) -> Result<usize, SourceError> {
        (**self).read(buf)
    }

    fn tune(&mut self, center_hz: f64) -> Result<(), SourceError> {
        (**self).tune(center_hz)
    }

    fn set_gain(&mut self, change: GainChange) -> Result<(), SourceError> {
        (**self).set_gain(change)
    }

    fn lost_samples(&self) -> u64 {
        (**self).lost_samples()
    }
}
