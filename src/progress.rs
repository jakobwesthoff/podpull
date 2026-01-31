use std::sync::Arc;

/// Events emitted during podcast synchronization for progress reporting
#[derive(Debug, Clone)]
pub enum ProgressEvent {
    /// Feed is being fetched from URL
    FetchingFeed { url: String },

    /// Feed has been parsed successfully
    FeedParsed {
        podcast_title: String,
        total_episodes: usize,
        new_episodes: usize,
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
        episode_title: String,
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

    /// Download is being finalized (renamed from .partial)
    Finalizing {
        /// Identifies the download slot
        download_id: usize,
        episode_title: String,
    },

    /// Hashing completed for a download
    HashingCompleted {
        /// Identifies the download slot
        download_id: usize,
        episode_title: String,
        hash: String,
    },

    /// Partial files were cleaned up during directory scan
    PartialFilesCleanedUp { count: usize },

    /// Sync operation completed
    SyncCompleted {
        downloaded_count: usize,
        skipped_count: usize,
        failed_count: usize,
    },
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

        reporter.report(ProgressEvent::FeedParsed {
            podcast_title: "Test Podcast".to_string(),
            total_episodes: 10,
            new_episodes: 5,
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
            episode_title: "Episode 1".to_string(),
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

        reporter.report(ProgressEvent::Finalizing {
            download_id: 0,
            episode_title: "Episode 1".to_string(),
        });

        reporter.report(ProgressEvent::HashingCompleted {
            download_id: 0,
            episode_title: "Episode 1".to_string(),
            hash: "sha256:abc123".to_string(),
        });

        reporter.report(ProgressEvent::PartialFilesCleanedUp { count: 2 });

        reporter.report(ProgressEvent::SyncCompleted {
            downloaded_count: 4,
            skipped_count: 5,
            failed_count: 1,
        });
    }
}
