// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The episodes whose write to disk turned slow during a run
//!
//! The library reports a slow write only as a progress event. The lists at
//! the end of the run are the one place a log file sees it, since the live
//! warning is drawn above the progress bars only, so the binary keeps the
//! slow writes it was told about until the run ends.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use podpull::{ProgressEvent, ProgressReporter};

/// An episode whose write to disk took longer than `threshold`
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SlowWrite {
    pub(crate) episode_title: String,
    /// How long the wait had lasted when it was reported as slow
    pub(crate) threshold: Duration,
}

/// Passes every event on to `reporter` and keeps the slow writes among them
pub(crate) struct SlowWriteRecorder<'a> {
    reporter: &'a dyn ProgressReporter,
    /// Keyed by the episode's place in the download queue, so each download
    /// is listed once and the list follows the queue
    slow: Mutex<BTreeMap<usize, SlowWrite>>,
}

impl<'a> SlowWriteRecorder<'a> {
    pub(crate) fn new(reporter: &'a dyn ProgressReporter) -> Self {
        Self {
            reporter,
            slow: Mutex::new(BTreeMap::new()),
        }
    }

    /// The slow writes of the run, in the order of the download queue
    pub(crate) fn into_slow_writes(self) -> Vec<SlowWrite> {
        self.slow
            .into_inner()
            .expect("no code panics while holding the slow writes")
            .into_values()
            .collect()
    }
}

impl ProgressReporter for SlowWriteRecorder<'_> {
    fn report(&self, event: ProgressEvent) {
        if let ProgressEvent::DownloadSyncSlow {
            episode_title,
            episode_index,
            threshold,
            ..
        } = &event
        {
            self.slow
                .lock()
                .expect("no code panics while holding the slow writes")
                .entry(*episode_index)
                .or_insert_with(|| SlowWrite {
                    episode_title: episode_title.clone(),
                    threshold: *threshold,
                });
        }
        self.reporter.report(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Keeps the events it is handed
    #[derive(Default)]
    struct Recording(Mutex<Vec<ProgressEvent>>);

    impl ProgressReporter for Recording {
        fn report(&self, event: ProgressEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    fn slow(episode_index: usize, title: &str, threshold_secs: u64) -> ProgressEvent {
        ProgressEvent::DownloadSyncSlow {
            download_id: 0,
            episode_title: title.to_string(),
            episode_index,
            total_to_download: 3,
            threshold: Duration::from_secs(threshold_secs),
        }
    }

    fn write(title: &str, threshold_secs: u64) -> SlowWrite {
        SlowWrite {
            episode_title: title.to_string(),
            threshold: Duration::from_secs(threshold_secs),
        }
    }

    #[test]
    fn every_event_reaches_the_reporter() {
        let inner = Recording::default();
        let recorder = SlowWriteRecorder::new(&inner);

        recorder.report(ProgressEvent::SyncCompleted);
        recorder.report(slow(0, "Sega Nomad", 30));

        let events = inner.0.lock().unwrap();
        assert!(matches!(
            events.as_slice(),
            [
                ProgressEvent::SyncCompleted,
                ProgressEvent::DownloadSyncSlow { .. }
            ]
        ));
    }

    #[test]
    fn a_slow_write_is_kept_with_its_title_and_threshold() {
        let recorder = SlowWriteRecorder::new(&podpull::NoopReporter);

        recorder.report(slow(0, "Sega Nomad", 30));

        assert_eq!(recorder.into_slow_writes(), [write("Sega Nomad", 30)]);
    }

    #[test]
    fn a_download_slow_more_than_once_is_kept_once() {
        let recorder = SlowWriteRecorder::new(&podpull::NoopReporter);

        // A slow share can hold up the syncs during the download and the
        // one of the metadata, each reported on its own.
        recorder.report(slow(0, "Sega Nomad", 30));
        recorder.report(slow(0, "Sega Nomad", 30));

        assert_eq!(recorder.into_slow_writes(), [write("Sega Nomad", 30)]);
    }

    #[test]
    fn slow_writes_follow_the_download_queue() {
        let recorder = SlowWriteRecorder::new(&podpull::NoopReporter);

        // Concurrent downloads report in the order their waits turn slow.
        recorder.report(slow(2, "Atari Lynx", 30));
        recorder.report(slow(0, "Sega Nomad", 30));

        assert_eq!(
            recorder.into_slow_writes(),
            [write("Sega Nomad", 30), write("Atari Lynx", 30)]
        );
    }

    #[test]
    fn other_events_are_no_slow_writes() {
        let recorder = SlowWriteRecorder::new(&podpull::NoopReporter);

        recorder.report(ProgressEvent::DownloadSyncing {
            download_id: 0,
            episode_title: "Sega Nomad".to_string(),
            episode_index: 0,
            total_to_download: 1,
        });

        assert_eq!(recorder.into_slow_writes(), []);
    }
}
