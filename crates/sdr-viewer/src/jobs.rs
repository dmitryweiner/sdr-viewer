//! The thread that works on the samples cut for an event — writes them to
//! the disk, makes the event's picture — so that reception never waits
//! for either.

use std::path::Path;
use std::sync::mpsc::Receiver;
use std::time::SystemTime;

use crate::reader::{Job, Update};
use crate::{fragment, save};

/// Does every job that comes and tells the interface what came of it.
/// Recordings go to `dir`; `started` is when the stream began, for a
/// source that receives as the clock runs.
///
/// Of the pictures asked for while it was busy only the last is made:
/// the selection has left the others behind.
pub fn run(
    jobs: Receiver<Job>,
    dir: Option<&Path>,
    started: Option<SystemTime>,
    mut tell: impl FnMut(Update),
) {
    while let Ok(first) = jobs.recv() {
        let mut waiting = vec![first];
        waiting.extend(jobs.try_iter());
        let wanted = waiting
            .iter()
            .rposition(|job| matches!(job, Job::Picture(_)));
        for (index, job) in waiting.into_iter().enumerate() {
            match job {
                Job::Save(cut) => tell(match dir.map(|dir| save::save(&cut, dir, started)) {
                    Some(Ok(done)) => Update::Done(done),
                    Some(Err(why)) => Update::Notice(why),
                    None => Update::Notice(
                        "nowhere to save samples: name a directory with --iq-dir".into(),
                    ),
                }),
                Job::Picture(cut) if Some(index) == wanted => {
                    let fragment = fragment::of(&cut);
                    tell(Update::Picture(cut.event, Some(fragment)));
                }
                Job::Picture(_) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::Cut;
    use sdr_core::SourceInfo;
    use sdr_detect::Event;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    /// 4096 samples of nothing at 1 MS/s around an event at `start_ms`.
    fn cut(start_ms: u64) -> Cut {
        Cut {
            samples: vec![[127, 128]; 4096],
            first_sample: 0,
            source: SourceInfo {
                sample_rate: 1_000_000,
                center_hz: 100e6,
                description: "test".into(),
            },
            event: Event {
                start: Duration::from_millis(start_ms),
                duration: Duration::from_millis(1),
                low_hz: 100.01e6,
                high_hz: 100.02e6,
                peak_hz: 100.015e6,
                peak_dbfs: -30.0,
                above_floor_db: 20.0,
            },
        }
    }

    fn told(dir: Option<&Path>, jobs: Vec<Job>) -> Vec<Update> {
        let (tx, rx) = channel();
        for job in jobs {
            tx.send(job).unwrap();
        }
        drop(tx);
        let mut told = Vec::new();
        run(rx, dir, None, |update| told.push(update));
        told
    }

    #[test]
    fn a_picture_job_is_answered_with_the_events_picture() {
        match &told(None, vec![Job::Picture(cut(1))])[..] {
            [Update::Picture(event, Some(fragment))] => {
                assert_eq!(*event, cut(1).event);
                assert_eq!(fragment.columns.len(), 2);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn of_the_pictures_that_piled_up_only_the_last_is_made() {
        let jobs = vec![
            Job::Picture(cut(1)),
            Job::Save(cut(2)),
            Job::Picture(cut(3)),
        ];
        match &told(None, jobs)[..] {
            [Update::Notice(_), Update::Picture(event, Some(_))] => {
                assert_eq!(*event, cut(3).event);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn saving_tells_what_was_saved_or_why_not() {
        let dir = std::env::temp_dir().join(format!("sdr-viewer-jobs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            told(Some(&dir), vec![Job::Save(cut(2))]),
            [Update::Done(
                "IQ saved: at-0.002s_100.015MHz (0.00 s)".into()
            )]
        );
        assert!(dir.join("at-0.002s_100.015MHz.sigmf-meta").exists());
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            told(None, vec![Job::Save(cut(2))]),
            [Update::Notice(
                "nowhere to save samples: name a directory with --iq-dir".into()
            )]
        );
        match &told(Some(Path::new("/dev/null/iq")), vec![Job::Save(cut(2))])[..] {
            [Update::Notice(why)] => assert!(why.starts_with("IQ not saved in /dev/null/iq")),
            other => panic!("{other:?}"),
        }
    }
}
