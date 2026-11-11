//! Playing back raw IQ recordings, as `rtl_sdr` and SigMF write them.

use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use crate::source::{Iq, Source, SourceError, SourceInfo};

/// Interleaved I/Q sample layouts; the names are the usual file extensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    /// Unsigned 8 bit, centred at 127.5: what `rtl_sdr` writes.
    Cu8,
    /// Signed 8 bit.
    Cs8,
    /// Signed 16 bit, little endian.
    Cs16,
    /// 32-bit float, little endian.
    Cf32,
}

impl SampleFormat {
    pub const ALL: [SampleFormat; 4] = [
        SampleFormat::Cu8,
        SampleFormat::Cs8,
        SampleFormat::Cs16,
        SampleFormat::Cf32,
    ];

    pub fn name(self) -> &'static str {
        match self {
            SampleFormat::Cu8 => "cu8",
            SampleFormat::Cs8 => "cs8",
            SampleFormat::Cs16 => "cs16",
            SampleFormat::Cf32 => "cf32",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        let name = name.to_ascii_lowercase();
        SampleFormat::ALL
            .into_iter()
            .find(|format| format.name() == name)
    }

    /// From the extension: `capture.cu8`, `x.cf32`; `.raw` and `.bin` are
    /// taken as cu8, since that is what `rtl_sdr` produces.
    pub fn from_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        match extension.as_str() {
            "raw" | "bin" => Some(SampleFormat::Cu8),
            other => SampleFormat::from_name(other),
        }
    }

    /// Bytes per complex sample.
    pub fn sample_bytes(self) -> usize {
        match self {
            SampleFormat::Cu8 | SampleFormat::Cs8 => 2,
            SampleFormat::Cs16 => 4,
            SampleFormat::Cf32 => 8,
        }
    }

    /// Decodes whole samples from `bytes` into `out`; both must match in length.
    pub fn decode(self, bytes: &[u8], out: &mut [Iq]) {
        assert_eq!(bytes.len(), out.len() * self.sample_bytes());
        let pairs = bytes.chunks_exact(self.sample_bytes()).zip(out.iter_mut());
        match self {
            SampleFormat::Cu8 => pairs.for_each(|(b, s)| {
                *s = Iq::new(
                    (f32::from(b[0]) - 127.5) / 127.5,
                    (f32::from(b[1]) - 127.5) / 127.5,
                );
            }),
            SampleFormat::Cs8 => pairs.for_each(|(b, s)| {
                let [i, q] = [b[0], b[1]].map(|v| f32::from(i8::from_le_bytes([v])) / 128.0);
                *s = Iq::new(i, q);
            }),
            SampleFormat::Cs16 => pairs.for_each(|(b, s)| {
                let i = f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0;
                let q = f32::from(i16::from_le_bytes([b[2], b[3]])) / 32768.0;
                *s = Iq::new(i, q);
            }),
            SampleFormat::Cf32 => pairs.for_each(|(b, s)| {
                let i = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                let q = f32::from_le_bytes([b[4], b[5], b[6], b[7]]);
                *s = Iq::new(i, q);
            }),
        }
    }
}

/// A recording as a source. At the end it stops, or starts over if looping.
pub struct FileSource<R> {
    reader: R,
    format: SampleFormat,
    info: SourceInfo,
    looping: bool,
    bytes: Vec<u8>,
}

impl<R: Read + Seek + Send> FileSource<R> {
    pub fn new(reader: R, format: SampleFormat, info: SourceInfo, looping: bool) -> Self {
        FileSource {
            reader,
            format,
            info,
            looping,
            bytes: Vec::new(),
        }
    }
}

impl<R: Read + Seek + Send> Source for FileSource<R> {
    fn info(&self) -> SourceInfo {
        self.info.clone()
    }

    fn read(&mut self, buf: &mut [Iq]) -> Result<usize, SourceError> {
        let sample_bytes = self.format.sample_bytes();
        self.bytes.resize(buf.len() * sample_bytes, 0);
        let mut filled = 0;
        // Bytes since the last rewind: a file without one whole sample must
        // end the stream rather than rewind forever.
        let mut since_rewind = 0;
        while filled < self.bytes.len() {
            let got = match self.reader.read(&mut self.bytes[filled..]) {
                Ok(got) => got,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => return Err(err.into()),
            };
            if got > 0 {
                filled += got;
                since_rewind += got;
                continue;
            }
            // The end of the file: a partial sample there is dropped.
            filled -= filled % sample_bytes;
            if !self.looping || since_rewind < sample_bytes {
                break;
            }
            self.reader.seek(SeekFrom::Start(0))?;
            since_rewind = 0;
        }
        let samples = filled / sample_bytes;
        self.format
            .decode(&self.bytes[..samples * sample_bytes], &mut buf[..samples]);
        Ok(samples)
    }

    fn tune(&mut self, _center_hz: f64) -> Result<(), SourceError> {
        Err(SourceError::Unsupported("a recording cannot be retuned"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn info() -> SourceInfo {
        SourceInfo {
            sample_rate: 1000,
            center_hz: 1e6,
            description: "test".into(),
        }
    }

    fn read_all(source: &mut impl Source, chunk: usize) -> Vec<Iq> {
        let mut all = Vec::new();
        let mut buf = vec![Iq::new(0.0, 0.0); chunk];
        loop {
            let n = source.read(&mut buf).unwrap();
            if n == 0 {
                return all;
            }
            all.extend_from_slice(&buf[..n]);
        }
    }

    #[test]
    fn knows_formats_by_name_and_extension() {
        assert_eq!(SampleFormat::from_name("cf32"), Some(SampleFormat::Cf32));
        assert_eq!(SampleFormat::from_name("CS16"), Some(SampleFormat::Cs16));
        assert_eq!(SampleFormat::from_name("wav"), None);
        assert_eq!(
            SampleFormat::from_path(Path::new("a/b.cu8")),
            Some(SampleFormat::Cu8)
        );
        assert_eq!(
            SampleFormat::from_path(Path::new("x.CS8")),
            Some(SampleFormat::Cs8)
        );
        assert_eq!(
            SampleFormat::from_path(Path::new("x.raw")),
            Some(SampleFormat::Cu8)
        );
        assert_eq!(
            SampleFormat::from_path(Path::new("x.bin")),
            Some(SampleFormat::Cu8)
        );
        assert_eq!(SampleFormat::from_path(Path::new("x.txt")), None);
        assert_eq!(SampleFormat::from_path(Path::new("noext")), None);
    }

    #[test]
    fn decodes_every_format_to_full_scale() {
        let mut out = [Iq::new(0.0, 0.0); 2];

        SampleFormat::Cu8.decode(&[255, 0, 127, 128], &mut out);
        assert_eq!(out[0], Iq::new(1.0, -1.0));
        assert!(out[1].re < 0.0 && out[1].re > -0.01 && out[1].im > 0.0 && out[1].im < 0.01);

        SampleFormat::Cs8.decode(&[127, 0x80, 0, 64], &mut out);
        assert_eq!(out, [Iq::new(127.0 / 128.0, -1.0), Iq::new(0.0, 0.5)]);

        let cs16: Vec<u8> = [16384i16, -32768, 0, 1]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        SampleFormat::Cs16.decode(&cs16, &mut out);
        assert_eq!(out, [Iq::new(0.5, -1.0), Iq::new(0.0, 1.0 / 32768.0)]);

        let cf32: Vec<u8> = [0.25f32, -0.5, 1.0, 0.0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        SampleFormat::Cf32.decode(&cf32, &mut out);
        assert_eq!(out, [Iq::new(0.25, -0.5), Iq::new(1.0, 0.0)]);
    }

    #[test]
    fn plays_the_file_to_its_end_and_drops_a_partial_sample() {
        // Five cs8 samples and one stray byte.
        let bytes: Vec<u8> = (0..11).collect();
        let mut source = FileSource::new(Cursor::new(bytes), SampleFormat::Cs8, info(), false);
        let samples = read_all(&mut source, 2);
        assert_eq!(samples.len(), 5);
        assert_eq!(samples[4], Iq::new(8.0 / 128.0, 9.0 / 128.0));
        assert_eq!(source.read(&mut [Iq::new(0.0, 0.0); 4]).unwrap(), 0);
    }

    #[test]
    fn looping_starts_over_at_the_end() {
        let bytes = vec![1u8, 2, 3, 4, 5, 6];
        let mut source = FileSource::new(Cursor::new(bytes), SampleFormat::Cs8, info(), true);
        let mut buf = [Iq::new(0.0, 0.0); 7];
        assert_eq!(source.read(&mut buf).unwrap(), 7);
        let first = Iq::new(1.0 / 128.0, 2.0 / 128.0);
        assert_eq!(buf[0], first);
        assert_eq!(buf[3], first);
        assert_eq!(buf[6], first);
    }

    #[test]
    fn an_empty_looping_file_ends_instead_of_spinning() {
        let mut source = FileSource::new(Cursor::new(vec![]), SampleFormat::Cu8, info(), true);
        assert_eq!(source.read(&mut [Iq::new(0.0, 0.0); 4]).unwrap(), 0);
    }

    /// A reader that hands out at most three bytes per call, as pipes do.
    struct Trickle(Cursor<Vec<u8>>);

    impl Read for Trickle {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = buf.len().min(3);
            self.0.read(&mut buf[..n])
        }
    }

    impl Seek for Trickle {
        fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
            self.0.seek(pos)
        }
    }

    #[test]
    fn short_reads_still_fill_whole_samples() {
        let bytes: Vec<u8> = [0.5f32, 0.25, -0.5, -0.25]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let reader = Trickle(Cursor::new(bytes));
        let mut source = FileSource::new(reader, SampleFormat::Cf32, info(), false);
        let samples = read_all(&mut source, 8);
        assert_eq!(samples, [Iq::new(0.5, 0.25), Iq::new(-0.5, -0.25)]);
    }

    #[test]
    fn cannot_be_retuned() {
        let mut source = FileSource::new(Cursor::new(vec![]), SampleFormat::Cu8, info(), false);
        assert!(matches!(source.tune(1e6), Err(SourceError::Unsupported(_))));
    }
}
