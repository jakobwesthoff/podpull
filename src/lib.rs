pub mod error;
pub mod http;
pub mod progress;

// Re-export main types for convenience
pub use error::{DownloadError, FeedError, MetadataError, StateError, SyncError};
pub use http::{HttpClient, HttpResponse, ReqwestClient};
pub use progress::{NoopReporter, ProgressEvent, ProgressReporter, SharedProgressReporter};
