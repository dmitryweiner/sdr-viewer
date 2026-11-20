//! The framing of the API's stream (TCP, and the serial line): 0x94 0xC3,
//! the length in two bytes, high first, then that many bytes of one
//! protobuf message.

pub const START1: u8 = 0x94;
pub const START2: u8 = 0xC3;
/// The longest message the firmware sends or takes.
pub const MAX_LEN: usize = 512;

/// One message in its frame.
pub fn encode(message: &[u8]) -> Vec<u8> {
    let len = u16::try_from(message.len()).unwrap_or(u16::MAX);
    let mut frame = vec![START1, START2];
    frame.extend(len.to_be_bytes());
    frame.extend(message);
    frame
}

/// Takes the stream's bytes as they come and gives back whole messages.
/// What is not a frame — a serial line's debug text, a frame longer than
/// the firmware sends — is skipped up to the next start.
#[derive(Debug, Default)]
pub struct Deframer {
    buf: Vec<u8>,
}

impl Deframer {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        self.buf.extend(bytes);
        let mut messages = Vec::new();
        loop {
            // Up to the next start byte.
            let start = self.buf.iter().position(|&b| b == START1);
            self.buf.drain(..start.unwrap_or(self.buf.len()));
            if self.buf.len() < 4 {
                // A lone start byte not followed by the second is no frame.
                if self.buf.len() >= 2 && self.buf[1] != START2 {
                    self.buf.remove(0);
                    continue;
                }
                return messages;
            }
            let len = usize::from(u16::from_be_bytes([self.buf[2], self.buf[3]]));
            if self.buf[1] != START2 || len > MAX_LEN {
                self.buf.remove(0);
                continue;
            }
            if self.buf.len() < 4 + len {
                return messages;
            }
            messages.push(self.buf[4..4 + len].to_vec());
            self.buf.drain(..4 + len);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_is_framed_with_its_length_high_byte_first() {
        assert_eq!(encode(&[7; 300])[..4], [0x94, 0xC3, 0x01, 0x2C]);
        assert_eq!(encode(&[7; 300]).len(), 304);
        assert_eq!(encode(&[]), [0x94, 0xC3, 0, 0]);
    }

    #[test]
    fn messages_come_out_whole_however_the_bytes_arrive() {
        let mut stream = encode(b"one");
        stream.extend(encode(b""));
        stream.extend(encode(b"three"));
        let mut deframer = Deframer::default();
        assert_eq!(deframer.push(&stream), [&b"one"[..], b"", b"three"]);
        let mut deframer = Deframer::default();
        let mut got = Vec::new();
        for byte in &stream {
            got.extend(deframer.push(&[*byte]));
        }
        assert_eq!(got, [&b"one"[..], b"", b"three"]);
    }

    #[test]
    fn what_is_not_a_frame_is_skipped_to_the_next_start() {
        let mut stream = b"INFO | booting\r\n".to_vec();
        // A start byte that does not begin a frame, then one that does.
        stream.extend([0x94, 0x94]);
        stream.extend(&encode(b"ok")[1..]);
        // A length the firmware never sends: the header is no frame.
        stream.extend([0x94, 0xC3, 0x02, 0x01]);
        stream.extend(encode(b"after"));
        let mut deframer = Deframer::default();
        assert_eq!(deframer.push(&stream), [&b"ok"[..], b"after"]);
    }
}
