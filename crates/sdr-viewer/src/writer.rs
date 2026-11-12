//! The thread that puts events into the database, so that neither
//! reception nor the interface waits for the disk.

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use sdr_detect::Event;

/// Events are gathered for this long and written together: one flush to
/// the SD card a second instead of one per event.
pub const WRITE_EVERY: Duration = Duration::from_secs(1);

/// Writes what comes over `events` with `write`, a batch at most every
/// `WRITE_EVERY`, until the sender is gone; what is waiting then is still
/// written. Stops at the first failure and returns its message.
pub fn run(
    events: Receiver<Vec<Event>>,
    mut write: impl FnMut(&[Event]) -> Result<(), String>,
) -> Result<(), String> {
    while let Ok(mut batch) = events.recv() {
        let deadline = Instant::now() + WRITE_EVERY;
        let over = loop {
            match events.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(more) => batch.extend(more),
                Err(RecvTimeoutError::Timeout) => break false,
                Err(RecvTimeoutError::Disconnected) => break true,
            }
        };
        if !batch.is_empty() {
            write(&batch)?;
        }
        if over {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    fn event(start_ms: u64) -> Event {
        Event {
            start: Duration::from_millis(start_ms),
            duration: Duration::from_millis(40),
            low_hz: 433.42e6,
            high_hz: 433.43e6,
            peak_hz: 433.425e6,
            peak_dbfs: -57.5,
            above_floor_db: 13.0,
        }
    }

    #[test]
    fn writes_what_was_waiting_as_one_batch_when_the_sender_is_gone() {
        let (tx, rx) = channel();
        tx.send(vec![event(1), event(2)]).unwrap();
        tx.send(Vec::new()).unwrap();
        tx.send(vec![event(3)]).unwrap();
        drop(tx);
        let mut written = Vec::new();
        let result = run(rx, |batch| {
            written.push(batch.to_vec());
            Ok(())
        });
        assert_eq!(result, Ok(()));
        assert_eq!(written, [vec![event(1), event(2), event(3)]]);
    }

    #[test]
    fn nothing_to_write_writes_nothing() {
        let (tx, rx) = channel();
        tx.send(Vec::new()).unwrap();
        drop(tx);
        let mut writes = 0;
        let result = run(rx, |_| {
            writes += 1;
            Ok(())
        });
        assert_eq!((result, writes), (Ok(()), 0));
    }

    #[test]
    fn stops_at_the_first_failure_with_its_message() {
        let (tx, rx) = channel();
        tx.send(vec![event(1)]).unwrap();
        drop(tx);
        let result = run(rx, |_| Err("disk full".into()));
        assert_eq!(result, Err("disk full".into()));
    }
}
