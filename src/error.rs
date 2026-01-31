use std::path::PathBuf;
use thiserror::Error;

/// Errors that can occur when fetching or parsing RSS feeds
#[derive(Error, Debug)]
pub enum FeedError {
    #[error("Failed to fetch feed from {url}: {source}")]
    FetchFailed {
        url: String,
        #[source]
        source: reqwest::Error,
    },

    #[error("Failed to read feed file {path}: {source}")]
    FileReadFailed {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to parse RSS feed: {0}")]
    ParseFailed(#[from] rss::Error),

    #[error("Invalid feed URL: {0}")]
    InvalidUrl(#[from] url::ParseError),

    #[error("Episode '{title}' has no enclosure (audio file)")]
    MissingEnclosure { title: String },

    #[error("Failed to parse date '{date_str}': {reason}")]
    InvalidDate { date_str: String, reason: String },
}

/// Errors that can occur during episode downloads
#[derive(Error, Debug)]
pub enum DownloadError {
    #[error("HTTP request failed for {url}: {source}")]
    HttpFailed {
        url: String,
        #[source]
        source: reqwest::Error,
    },

    #[error("HTTP error {status} for {url}")]
    HttpStatus { url: String, status: u16 },

    #[error("Failed to create file {path}: {source}")]
    FileCreateFailed {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to write to file {path}: {source}")]
    FileWriteFailed {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Stream error while downloading {url}: {source}")]
    StreamFailed {
        url: String,
        #[source]
        source: reqwest::Error,
    },
}

/// Errors that can occur during metadata operations
#[derive(Error, Debug)]
pub enum MetadataError {
    #[error("Failed to read metadata file {path}: {source}")]
    ReadFailed {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to write metadata file {path}: {source}")]
    WriteFailed {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to parse metadata JSON in {path}: {source}")]
    JsonParseFailed {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("Failed to serialize metadata: {0}")]
    JsonSerializeFailed(#[from] serde_json::Error),
}

/// Errors that can occur when scanning the output directory
#[derive(Error, Debug)]
pub enum StateError {
    #[error("Output directory does not exist: {0}")]
    DirectoryNotFound(PathBuf),

    #[error("Failed to read directory {path}: {source}")]
    ReadDirectoryFailed {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to create directory {path}: {source}")]
    CreateDirectoryFailed {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Metadata error: {0}")]
    Metadata(#[from] MetadataError),
}

/// Top-level errors for sync operations
#[derive(Error, Debug)]
pub enum SyncError {
    #[error("Feed error: {0}")]
    Feed(#[from] FeedError),

    #[error("State error: {0}")]
    State(#[from] StateError),

    #[error("All downloads failed")]
    AllDownloadsFailed,
}
