// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::path::PathBuf;
use std::sync::Arc;

use crate::damage::{DamageKind, DamageRemedy};

/// Events emitted during podcast synchronization for progress reporting
#[derive(Debug, Clone)]
pub enum ProgressEvent {
    /// Feed is being fetched from URL (network request)
    FetchingFeed { url: String },

    /// Feed is being parsed (XML processing)
    ParsingFeed {
        /// Source being parsed (URL or file path)
        source: String,
    },

    /// Output directory is being scanned for existing episodes
    ScanningDirectory {
        /// Number of files scanned so far
        files_scanned: usize,
        /// Total number of files to scan
        total_files: usize,
    },

    /// Sync plan is ready (feed parsed, directory scanned, plan created)
    SyncPlanReady {
        podcast_title: String,
        total_episodes: usize,
        /// All episodes not yet downloaded
        new_episodes: usize,
        /// New episodes to download after limit applied (may equal new_episodes)
        to_download: usize,
        /// Episodes downloaded again to replace damaged audio
        repairs: usize,
    },

    /// A download is starting
    DownloadStarting {
        /// Identifies the download slot (0 to max_concurrent-1)
        download_id: usize,
        episode_title: String,
        /// Index of this episode in the download queue
        episode_index: usize,
        /// Total number of episodes to download
        total_to_download: usize,
        /// Expected content length in bytes, if known
        content_length: Option<u64>,
    },

    /// Download progress update
    DownloadProgress {
        /// Identifies the download slot
        download_id: usize,
        bytes_downloaded: u64,
        total_bytes: Option<u64>,
    },

    /// A download completed successfully
    DownloadCompleted {
        /// Identifies the download slot
        download_id: usize,
        episode_title: String,
        bytes_downloaded: u64,
    },

    /// A download failed
    DownloadFailed {
        /// Identifies the download slot
        download_id: usize,
        episode_title: String,
        error: String,
    },

    /// Partial files were cleaned up during directory scan
    PartialFilesCleanedUp { count: usize },

    /// A new episode's audio turned out to be stored already, byte for
    /// byte; its GUID was added to the stored episode instead of a copy
    EpisodeAlreadyStored {
        /// Identifies the download slot
        download_id: usize,
        episode_title: String,
        /// The stored audio file the episode matched
        audio_filename: String,
    },

    /// A partial file left by an interrupted download could not be removed
    PartialFileStuck { path: PathBuf },

    /// An episode metadata file in the output directory holds no valid
    /// metadata
    MetadataUnreadable { path: PathBuf, error: String },

    /// An existing audio file is about to be hashed to check it against the
    /// hash recorded when it was downloaded
    VerifyingStoredAudio { audio_filename: String },

    /// An existing audio file could not be read to check it
    StoredAudioUnverifiable {
        audio_filename: String,
        error: String,
    },

    /// An existing audio file no longer matches the hash recorded when it
    /// was downloaded, or the directory no longer lists it
    StoredAudioDamaged {
        episode_title: String,
        audio_filename: String,
        kind: DamageKind,
        remedy: DamageRemedy,
    },

    /// Sync operation completed; its counts are in the `SyncResult`
    SyncCompleted,
}

/// Trait for reporting progress events during synchronization.
///
/// Implementations can use this to display progress bars, log messages,
/// or collect statistics.
pub trait ProgressReporter: Send + Sync {
    /// Report a progress event
    fn report(&self, event: ProgressEvent);
}

/// A shared reference to a progress reporter
pub type SharedProgressReporter = Arc<dyn ProgressReporter>;

/// A no-op progress reporter that silently ignores all events.
/// Useful for tests or quiet mode.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopReporter;

impl ProgressReporter for NoopReporter {
    fn report(&self, _event: ProgressEvent) {
        // Intentionally empty
    }
}

impl NoopReporter {
    /// Create a new NoopReporter wrapped in an Arc
    pub fn shared() -> SharedProgressReporter {
        Arc::new(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_reporter_handles_all_events() {
        let reporter = NoopReporter;

        reporter.report(ProgressEvent::FetchingFeed {
            url: "https://example.com/feed.xml".to_string(),
        });

        reporter.report(ProgressEvent::ParsingFeed {
            source: "https://example.com/feed.xml".to_string(),
        });

        reporter.report(ProgressEvent::ScanningDirectory {
            files_scanned: 5,
            total_files: 10,
        });

        reporter.report(ProgressEvent::SyncPlanReady {
            podcast_title: "Test Podcast".to_string(),
            total_episodes: 10,
            new_episodes: 5,
            to_download: 3,
            repairs: 0,
        });

        reporter.report(ProgressEvent::DownloadStarting {
            download_id: 0,
            episode_title: "Episode 1".to_string(),
            episode_index: 0,
            total_to_download: 5,
            content_length: Some(1024),
        });

        reporter.report(ProgressEvent::DownloadProgress {
            download_id: 0,
            bytes_downloaded: 512,
            total_bytes: Some(1024),
        });

        reporter.report(ProgressEvent::DownloadCompleted {
            download_id: 0,
            episode_title: "Episode 1".to_string(),
            bytes_downloaded: 1024,
        });

        reporter.report(ProgressEvent::DownloadFailed {
            download_id: 1,
            episode_title: "Episode 2".to_string(),
            error: "Connection timeout".to_string(),
        });

        reporter.report(ProgressEvent::PartialFilesCleanedUp { count: 2 });

        reporter.report(ProgressEvent::PartialFileStuck {
            path: PathBuf::from("/podcasts/2024-01-15-Episode.mp3.partial"),
        });

        reporter.report(ProgressEvent::MetadataUnreadable {
            path: PathBuf::from("/podcasts/2024-01-15-Episode.json"),
            error: "EOF while parsing".to_string(),
        });

        reporter.report(ProgressEvent::StoredAudioDamaged {
            episode_title: "Episode 1".to_string(),
            audio_filename: "2024-01-15-Episode 1.mp3".to_string(),
            kind: DamageKind::Mismatch,
            remedy: DamageRemedy::RepairAvailable,
        });

        reporter.report(ProgressEvent::VerifyingStoredAudio {
            audio_filename: "2024-01-15-Episode 1.mp3".to_string(),
        });

        reporter.report(ProgressEvent::StoredAudioUnverifiable {
            audio_filename: "2024-01-15-Episode 1.mp3".to_string(),
            error: "Permission denied".to_string(),
        });

        reporter.report(ProgressEvent::SyncCompleted);

        reporter.report(ProgressEvent::EpisodeAlreadyStored {
            download_id: 0,
            episode_title: "Episode 1".to_string(),
            audio_filename: "2024-01-15-Episode 1.mp3".to_string(),
        });
    }
}
