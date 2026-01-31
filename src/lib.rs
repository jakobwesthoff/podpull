pub mod episode;
pub mod error;
pub mod feed;
pub mod http;
pub mod metadata;
pub mod progress;
pub mod state;

// Re-export main types for convenience
pub use episode::{
    download_episode, generate_filename, generate_filename_stem, get_audio_extension,
    DownloadContext,
};
pub use error::{DownloadError, FeedError, MetadataError, StateError, SyncError};
pub use feed::{fetch_feed, is_url, parse_feed, parse_feed_file, Enclosure, Episode, Podcast};
pub use http::{HttpClient, HttpResponse, ReqwestClient};
pub use metadata::{
    read_episode_metadata, read_podcast_metadata, write_episode_metadata, write_podcast_metadata,
    EpisodeMetadata, PodcastMetadata,
};
pub use progress::{NoopReporter, ProgressEvent, ProgressReporter, SharedProgressReporter};
pub use state::{create_sync_plan, scan_output_dir, OutputState, SyncPlan};
