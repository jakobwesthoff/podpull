// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::path::PathBuf;
use thiserror::Error;

use crate::cause::root_text;
use crate::http::HttpError;
use crate::reason::{FailureReason, Subject};

/// Errors that can occur when fetching or parsing RSS feeds
#[derive(Error, Debug)]
pub enum FeedError {
    #[error("Failed to fetch feed from {url}: {source}")]
    FetchFailed {
        url: url::Url,
        #[source]
        source: HttpError,
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
}

/// Errors that can occur during episode downloads
#[derive(Error, Debug)]
pub enum DownloadError {
    #[error("HTTP request failed for {url}: {source}")]
    HttpFailed {
        url: String,
        #[source]
        source: HttpError,
    },

    #[error("HTTP error {status} for {url}")]
    HttpStatus { url: String, status: u16 },

    #[error("Failed to create file {path}: {source}")]
    FileCreateFailed {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error(
        "Partial file {path} already exists; delete it if no other podpull run is writing to it"
    )]
    PartialFileExists { path: PathBuf },

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
        source: HttpError,
    },

    #[error("Failed to rename {partial_path} to {final_path}: {source}")]
    RenameFailed {
        partial_path: PathBuf,
        final_path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl DownloadError {
    /// Why the download failed, for the display of the episode
    ///
    /// The reason names what the failing step was about, so that a display
    /// can show it by the rules for a URL or for a path.
    pub fn reason(&self) -> FailureReason {
        match self {
            DownloadError::HttpFailed { url, source }
            | DownloadError::StreamFailed { url, source } => {
                FailureReason::new(root_text(source), Subject::Url(url.clone()))
            }
            DownloadError::HttpStatus { url, status } => {
                FailureReason::new(format!("HTTP error {status}"), Subject::Url(url.clone()))
            }
            DownloadError::FileCreateFailed { path, source }
            | DownloadError::FileWriteFailed { path, source } => {
                FailureReason::new(root_text(source), Subject::Path(path.clone()))
            }
            DownloadError::RenameFailed {
                final_path, source, ..
            } => FailureReason::new(root_text(source), Subject::Path(final_path.clone())),
            DownloadError::PartialFileExists { path } => FailureReason::new(
                "partial file already exists; delete it if no other podpull run is writing to it",
                Subject::Path(path.clone()),
            ),
        }
    }
}

/// Errors that can occur during metadata operations
#[derive(Error, Debug)]
pub enum MetadataError {
    #[error("Failed to read metadata file {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to write metadata file {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to parse metadata JSON in {path}: {source}")]
    JsonParse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("Failed to serialize metadata: {0}")]
    JsonSerialize(#[from] serde_json::Error),
}

impl MetadataError {
    /// Why the metadata could not be read or written
    pub fn reason(&self) -> FailureReason {
        match self {
            MetadataError::Read { path, source } | MetadataError::Write { path, source } => {
                FailureReason::new(root_text(source), Subject::Path(path.clone()))
            }
            MetadataError::JsonParse { path, source } => {
                FailureReason::new(root_text(source), Subject::Path(path.clone()))
            }
            MetadataError::JsonSerialize(source) => {
                FailureReason::new(root_text(source), Subject::None)
            }
        }
    }
}

/// Errors that can occur when scanning the output directory
#[derive(Error, Debug)]
pub enum StateError {
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

    #[error("Metadata error: {0}")]
    Metadata(#[from] MetadataError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::HttpErrorKind;
    use crate::reason::{FailureReason, Subject};
    use std::io::{Error as IoError, ErrorKind};

    fn reason(root: &str, subject: Subject) -> FailureReason {
        FailureReason {
            root: root.to_string(),
            subject,
        }
    }

    #[test]
    fn a_failed_request_names_its_cause_and_its_url() {
        let error = DownloadError::HttpFailed {
            url: "http://127.0.0.1:9/ep.mp3".to_string(),
            source: HttpError::new(HttpErrorKind::Refused, "Connection refused (os error 61)"),
        };

        assert_eq!(
            error.reason(),
            reason(
                "connection refused",
                Subject::Url("http://127.0.0.1:9/ep.mp3".to_string())
            )
        );
    }

    #[test]
    fn a_failed_stream_names_its_cause_and_its_url() {
        let error = DownloadError::StreamFailed {
            url: "http://127.0.0.1:9/ep.mp3".to_string(),
            source: HttpError::new(HttpErrorKind::Other, "connection reset"),
        };

        assert_eq!(
            error.reason(),
            reason(
                "connection reset",
                Subject::Url("http://127.0.0.1:9/ep.mp3".to_string())
            )
        );
    }

    #[test]
    fn an_error_status_names_the_status_and_the_url() {
        let error = DownloadError::HttpStatus {
            url: "https://example.com/ep.mp3".to_string(),
            status: 503,
        };

        assert_eq!(
            error.reason(),
            reason(
                "HTTP error 503",
                Subject::Url("https://example.com/ep.mp3".to_string())
            )
        );
    }

    #[test]
    fn a_file_that_cannot_be_created_names_the_cause_and_the_path() {
        let error = DownloadError::FileCreateFailed {
            path: PathBuf::from("/podcasts/a.mp3.partial"),
            source: IoError::from(ErrorKind::PermissionDenied),
        };

        assert_eq!(
            error.reason(),
            reason(
                "permission denied",
                Subject::Path(PathBuf::from("/podcasts/a.mp3.partial"))
            )
        );
    }

    #[test]
    fn a_file_that_cannot_be_written_names_the_cause_and_the_path() {
        let error = DownloadError::FileWriteFailed {
            path: PathBuf::from("/podcasts/a.mp3.partial"),
            source: IoError::other("No space left on device"),
        };

        assert_eq!(
            error.reason(),
            reason(
                "no space left on device",
                Subject::Path(PathBuf::from("/podcasts/a.mp3.partial"))
            )
        );
    }

    #[test]
    fn a_rename_that_fails_names_the_final_path() {
        let error = DownloadError::RenameFailed {
            partial_path: PathBuf::from("/podcasts/a.mp3.partial"),
            final_path: PathBuf::from("/podcasts/a.mp3"),
            source: IoError::from(ErrorKind::PermissionDenied),
        };

        assert_eq!(
            error.reason(),
            reason(
                "permission denied",
                Subject::Path(PathBuf::from("/podcasts/a.mp3"))
            )
        );
    }

    #[test]
    fn an_occupied_partial_file_names_the_path_and_what_to_do() {
        let error = DownloadError::PartialFileExists {
            path: PathBuf::from("/podcasts/a.mp3.partial"),
        };

        assert_eq!(
            error.reason(),
            reason(
                "partial file already exists; delete it if no other podpull run is writing to it",
                Subject::Path(PathBuf::from("/podcasts/a.mp3.partial"))
            )
        );
    }

    #[test]
    fn metadata_that_cannot_be_read_names_the_cause_and_the_path() {
        let error = MetadataError::Read {
            path: PathBuf::from("/podcasts/a.json"),
            source: IoError::from(ErrorKind::PermissionDenied),
        };

        assert_eq!(
            error.reason(),
            reason(
                "permission denied",
                Subject::Path(PathBuf::from("/podcasts/a.json"))
            )
        );
    }

    #[test]
    fn metadata_that_cannot_be_written_names_the_cause_and_the_path() {
        let error = MetadataError::Write {
            path: PathBuf::from("/podcasts/a.json"),
            source: IoError::other("disk full"),
        };

        assert_eq!(
            error.reason(),
            reason(
                "disk full",
                Subject::Path(PathBuf::from("/podcasts/a.json"))
            )
        );
    }

    #[test]
    fn metadata_that_is_not_json_names_the_parse_error_and_the_path() {
        let error = MetadataError::JsonParse {
            path: PathBuf::from("/podcasts/a.json"),
            source: serde_json::from_str::<serde_json::Value>("{ not json").unwrap_err(),
        };

        assert_eq!(
            error.reason(),
            reason(
                "key must be a string at line 1 column 3",
                Subject::Path(PathBuf::from("/podcasts/a.json"))
            )
        );
    }

    #[test]
    fn metadata_that_cannot_be_serialized_has_no_subject() {
        let error = MetadataError::JsonSerialize(
            serde_json::from_str::<serde_json::Value>("{ not json").unwrap_err(),
        );

        assert_eq!(
            error.reason(),
            reason("key must be a string at line 1 column 3", Subject::None)
        );
    }
}
