// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Helpers the tests of the download path share

use std::time::Duration;

use super::DownloadContext;
use crate::progress::{ProgressEvent, ProgressReporter};

pub(crate) fn context() -> DownloadContext {
    DownloadContext {
        download_id: 0,
        episode_index: 0,
        total_to_download: 1,
    }
}

/// Records every reported event and runs `on_slow` when a wait is
/// reported as slow
pub(crate) struct GateReporter {
    events: std::sync::Mutex<Vec<ProgressEvent>>,
    on_slow: Box<dyn Fn() + Send + Sync>,
}

impl GateReporter {
    pub(crate) fn new(on_slow: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            events: Default::default(),
            on_slow: Box::new(on_slow),
        }
    }

    /// Bytes downloaded as of the last progress event before the first wait
    /// event (`DownloadSyncing` or `DownloadSyncSlow`), or `None` if no wait was
    /// reported
    pub(crate) fn bytes_before_first_wait(&self) -> Option<u64> {
        let mut bytes = 0;
        for event in self.events.lock().unwrap().iter() {
            match event {
                ProgressEvent::DownloadProgress {
                    bytes_downloaded, ..
                } => bytes = *bytes_downloaded,
                ProgressEvent::DownloadSyncing { .. } | ProgressEvent::DownloadSyncSlow { .. } => {
                    return Some(bytes);
                }
                _ => {}
            }
        }
        None
    }

    /// Names of the wait events reported so far, in order
    pub(crate) fn waits(&self) -> Vec<&'static str> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                ProgressEvent::DownloadSyncing { .. } => Some("syncing"),
                ProgressEvent::DownloadSyncSlow { .. } => Some("slow"),
                ProgressEvent::DownloadSynced { .. } => Some("synced"),
                _ => None,
            })
            .collect()
    }
}

impl ProgressReporter for GateReporter {
    fn report(&self, event: ProgressEvent) {
        let slow = matches!(event, ProgressEvent::DownloadSyncSlow { .. });
        self.events.lock().unwrap().push(event);
        if slow {
            (self.on_slow)();
        }
    }
}

/// A gate a blocking closure waits on until the test opens it
pub(crate) fn gate() -> (
    impl Fn() + Send + Sync + 'static,
    std::sync::mpsc::Receiver<()>,
) {
    let (sender, receiver) = std::sync::mpsc::channel();
    let sender = std::sync::Mutex::new(sender);
    (
        move || {
            let _ = sender.lock().unwrap().send(());
        },
        receiver,
    )
}

/// Bound for tests that wait on a gate, so a gate that never opens fails
/// the test instead of hanging it
pub(crate) const GATE_TIMEOUT: Duration = Duration::from_secs(10);
