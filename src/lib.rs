pub mod error;
pub mod feed;
pub mod http;
pub mod progress;

// Re-export main types for convenience
pub use error::{DownloadError, FeedError, MetadataError, StateError, SyncError};
pub use feed::{fetch_feed, is_url, parse_feed, parse_feed_file, Enclosure, Episode, Podcast};
pub use http::{HttpClient, HttpResponse, ReqwestClient};
pub use progress::{NoopReporter, ProgressEvent, ProgressReporter, SharedProgressReporter};
