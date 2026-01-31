pub mod episode;
pub mod error;
pub mod feed;
pub mod http;
pub mod metadata;
pub mod progress;

// Re-export main types for convenience
pub use episode::{generate_filename, generate_filename_stem, get_audio_extension};
pub use error::{DownloadError, FeedError, MetadataError, StateError, SyncError};
pub use feed::{fetch_feed, is_url, parse_feed, parse_feed_file, Enclosure, Episode, Podcast};
pub use http::{HttpClient, HttpResponse, ReqwestClient};
pub use metadata::{
    read_episode_metadata, read_podcast_metadata, write_episode_metadata, write_podcast_metadata,
    EpisodeMetadata, PodcastMetadata,
};
pub use progress::{NoopReporter, ProgressEvent, ProgressReporter, SharedProgressReporter};
