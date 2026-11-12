// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::path::PathBuf;
use thiserror::Error;

use crate::cause::root_text;
use crate::http::{HttpError, HttpErrorKind};
use crate::reason::{FailureReason, Subject};

/// What a person can do about an error that ended the run
pub trait Hint {
    /// A sentence that says how to fix the problem, if there is one to say
    fn hint(&self) -> Option<&'static str>;
}

// The message of an error that can end the run is what a person is told
// first, and its hint says how to fix it. Neither holds a name from the feed
// or from the disk, only a host or a status: the names are shown by the
// step the run was in (ADR 22).

/// The operation of a failed access to the output folder
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FolderOperation {
    Create,
    Read,
    Write,
}

fn host(url: &url::Url) -> &str {
    url.host_str().unwrap_or("the host")
}

fn fetch_title(url: &url::Url, kind: HttpErrorKind) -> String {
    let host = host(url);
    match kind {
        HttpErrorKind::Dns => format!("Cannot reach the feed host {host}"),
        HttpErrorKind::Refused => format!("The feed host {host} refuses connections"),
        HttpErrorKind::TimedOut => format!("The feed host {host} does not answer in time"),
        HttpErrorKind::Tls => format!("Cannot set up a secure connection to {host}"),
        HttpErrorKind::Status(status @ (404 | 410)) => {
            format!("The feed was not found on {host} (HTTP {status})")
        }
        HttpErrorKind::Status(status @ 400..=499) => {
            format!("The feed host {host} refused the request (HTTP {status})")
        }
        HttpErrorKind::Status(status @ 500..=599) => {
            format!("The feed host {host} reports an error (HTTP {status})")
        }
        HttpErrorKind::Connect => format!("Cannot connect to the feed host {host}"),
        HttpErrorKind::Status(_) | HttpErrorKind::Other => {
            format!("Cannot fetch the feed from {host}")
        }
    }
}

fn fetch_hint(kind: HttpErrorKind) -> Option<&'static str> {
    match kind {
        HttpErrorKind::Dns => {
            Some("Check the feed URL for a typo and that this machine is online.")
        }
        HttpErrorKind::Refused => {
            Some("Check the host and port in the feed URL and that the server is running.")
        }
        HttpErrorKind::TimedOut => Some("Try again later and check the network connection."),
        HttpErrorKind::Tls => Some(
            "Check the feed URL, the system clock and any proxy between this machine and the host.",
        ),
        HttpErrorKind::Status(404 | 410) => {
            Some("Check the feed URL for a typo. The feed may have moved or been removed.")
        }
        HttpErrorKind::Status(400..=499) => Some("Check the feed URL and that the feed is public."),
        HttpErrorKind::Status(500..=599) => Some("The server has a problem. Try again later."),
        HttpErrorKind::Connect => Some("Check the feed URL and the network connection."),
        HttpErrorKind::Status(_) | HttpErrorKind::Other => None,
    }
}

fn feed_file_title(kind: std::io::ErrorKind) -> &'static str {
    match kind {
        std::io::ErrorKind::NotFound => "The feed file does not exist",
        _ => "Cannot read the feed file",
    }
}

fn feed_file_hint(kind: std::io::ErrorKind) -> Option<&'static str> {
    match kind {
        std::io::ErrorKind::NotFound => {
            Some("Check the path. A feed URL starts with http:// or https://.")
        }
        std::io::ErrorKind::PermissionDenied => Some("Check the permissions of the file."),
        _ => None,
    }
}

fn folder_title(operation: FolderOperation, kind: std::io::ErrorKind) -> &'static str {
    match (kind, operation) {
        (std::io::ErrorKind::StorageFull, _) => "The disk is full",
        (_, FolderOperation::Create) => "Cannot create the output folder",
        (_, FolderOperation::Read) => "Cannot read the output folder",
        (_, FolderOperation::Write) => "Cannot write to the output folder",
    }
}

fn folder_hint(kind: std::io::ErrorKind) -> Option<&'static str> {
    match kind {
        std::io::ErrorKind::StorageFull => {
            Some("Free some space on the disk and run podpull again.")
        }
        std::io::ErrorKind::PermissionDenied => Some("Check the permissions of the folder."),
        _ => None,
    }
}

fn metadata_read_hint(kind: std::io::ErrorKind) -> Option<&'static str> {
    use std::io::ErrorKind::*;
    match kind {
        PermissionDenied => Some("Check the permissions of the file."),
        TimedOut | ConnectionReset | ConnectionAborted | NotConnected | BrokenPipe => {
            Some("If the folder is on a network share, check the connection and run podpull again.")
        }
        _ => None,
    }
}

impl Hint for FeedError {
    fn hint(&self) -> Option<&'static str> {
        match self {
            FeedError::FetchFailed { source, .. } => fetch_hint(source.kind()),
            FeedError::FileReadFailed { source, .. } => feed_file_hint(source.kind()),
            FeedError::ParseFailed(_) => {
                Some("Check that the URL serves an RSS feed and not a web page.")
            }
            FeedError::InvalidUrl(_) => {
                Some("Give a URL that starts with http:// or https://, or the path of an RSS file.")
            }
            FeedError::MissingEnclosure { .. } => None,
        }
    }
}

impl Hint for MetadataError {
    fn hint(&self) -> Option<&'static str> {
        match self {
            MetadataError::Read { source, .. } => metadata_read_hint(source.kind()),
            MetadataError::Write { source, .. } => folder_hint(source.kind()),
            MetadataError::JsonParse { .. } | MetadataError::JsonSerialize(_) => None,
        }
    }
}

impl Hint for StateError {
    fn hint(&self) -> Option<&'static str> {
        match self {
            StateError::ReadDirectoryFailed { source, .. }
            | StateError::CreateDirectoryFailed { source, .. } => folder_hint(source.kind()),
            StateError::Metadata(error) => error.hint(),
        }
    }
}

impl Hint for SyncError {
    fn hint(&self) -> Option<&'static str> {
        match self {
            SyncError::Feed(error) => error.hint(),
            SyncError::State(error) => error.hint(),
            SyncError::Metadata(error) => error.hint(),
        }
    }
}

/// Errors that can occur when fetching or parsing RSS feeds
#[derive(Error, Debug)]
pub enum FeedError {
    #[error("{}", fetch_title(.url, .source.kind()))]
    FetchFailed {
        url: url::Url,
        #[source]
        source: HttpError,
    },

    #[error("{}", feed_file_title(.source.kind()))]
    FileReadFailed {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("The feed is not a valid RSS feed")]
    ParseFailed(#[from] rss::Error),

    #[error("The feed URL is not valid")]
    InvalidUrl(#[from] url::ParseError),

    #[error("Episode '{title}' has no enclosure (audio file)")]
    MissingEnclosure { title: String },
}

/// Errors that can occur during episode downloads
#[derive(Error, Debug)]
pub enum DownloadError {
    #[error("HTTP request failed for {url}")]
    HttpFailed {
        url: String,
        #[source]
        source: HttpError,
    },

    #[error("HTTP error {status} for {url}")]
    HttpStatus { url: String, status: u16 },

    #[error("Failed to create file {path}")]
    FileCreateFailed {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error(
        "Partial file {path} already exists; delete it if no other podpull run is writing to it"
    )]
    PartialFileExists { path: PathBuf },

    #[error("Failed to write to file {path}")]
    FileWriteFailed {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Stream error while downloading {url}")]
    StreamFailed {
        url: String,
        #[source]
        source: HttpError,
    },

    #[error("Failed to rename {partial_path} to {final_path}")]
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
    #[error("Cannot read an episode metadata file")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("{}", folder_title(FolderOperation::Write, .source.kind()))]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("An episode metadata file is not valid")]
    JsonParse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("Cannot write the podcast metadata")]
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
    #[error("{}", folder_title(FolderOperation::Read, .source.kind()))]
    ReadDirectoryFailed {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("{}", folder_title(FolderOperation::Create, .source.kind()))]
    CreateDirectoryFailed {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error(transparent)]
    Metadata(#[from] MetadataError),
}

/// Top-level errors for sync operations
#[derive(Error, Debug)]
pub enum SyncError {
    #[error(transparent)]
    Feed(#[from] FeedError),

    #[error(transparent)]
    State(#[from] StateError),

    #[error(transparent)]
    Metadata(#[from] MetadataError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::HttpErrorKind;
    use crate::reason::{FailureReason, Subject};
    use std::io::{Error as IoError, ErrorKind};

    fn reason(root: &str, subject: Subject) -> FailureReason {
        FailureReason::new(root, subject)
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

    // =========================================================
    // What the person is told when an error ends the run
    // =========================================================

    use std::error::Error as _;

    fn feed_url() -> url::Url {
        url::Url::parse("https://feeds.example.com/podcast.xml").unwrap()
    }

    fn fetch(kind: HttpErrorKind) -> SyncError {
        FeedError::FetchFailed {
            url: feed_url(),
            source: HttpError::new(kind, "cause"),
        }
        .into()
    }

    fn feed_file(kind: ErrorKind) -> SyncError {
        FeedError::FileReadFailed {
            path: PathBuf::from("feeds/show.xml"),
            source: IoError::from(kind),
        }
        .into()
    }

    fn folder(operation: &str, kind: ErrorKind) -> SyncError {
        let path = PathBuf::from("/podcasts");
        let source = IoError::from(kind);
        match operation {
            "create" => StateError::CreateDirectoryFailed { path, source }.into(),
            "read" => StateError::ReadDirectoryFailed { path, source }.into(),
            _ => MetadataError::Write { path, source }.into(),
        }
    }

    fn metadata_read(kind: ErrorKind) -> SyncError {
        StateError::Metadata(MetadataError::Read {
            path: PathBuf::from("/podcasts/a.json"),
            source: IoError::from(kind),
        })
        .into()
    }

    /// A test that the error shows `title` and offers `hint`
    macro_rules! row {
        ($name:ident, $error:expr, $title:expr, $hint:expr) => {
            #[test]
            fn $name() {
                let error: SyncError = $error;

                assert_eq!(error.to_string(), $title);
                assert_eq!(error.hint(), $hint);
            }
        };
    }

    row!(
        an_unknown_feed_host,
        fetch(HttpErrorKind::Dns),
        "Cannot reach the feed host feeds.example.com",
        Some("Check the feed URL for a typo and that this machine is online.")
    );
    row!(
        a_feed_host_that_refuses,
        fetch(HttpErrorKind::Refused),
        "The feed host feeds.example.com refuses connections",
        Some("Check the host and port in the feed URL and that the server is running.")
    );
    row!(
        a_feed_host_that_does_not_answer,
        fetch(HttpErrorKind::TimedOut),
        "The feed host feeds.example.com does not answer in time",
        Some("Try again later and check the network connection.")
    );
    row!(
        a_feed_host_with_a_broken_secure_connection,
        fetch(HttpErrorKind::Tls),
        "Cannot set up a secure connection to feeds.example.com",
        Some(
            "Check the feed URL, the system clock and any proxy between this machine and the host."
        )
    );
    row!(
        a_feed_that_was_not_found,
        fetch(HttpErrorKind::Status(404)),
        "The feed was not found on feeds.example.com (HTTP 404)",
        Some("Check the feed URL for a typo. The feed may have moved or been removed.")
    );
    row!(
        a_feed_that_is_gone,
        fetch(HttpErrorKind::Status(410)),
        "The feed was not found on feeds.example.com (HTTP 410)",
        Some("Check the feed URL for a typo. The feed may have moved or been removed.")
    );
    row!(
        a_feed_that_is_refused,
        fetch(HttpErrorKind::Status(403)),
        "The feed host feeds.example.com refused the request (HTTP 403)",
        Some("Check the feed URL and that the feed is public.")
    );
    row!(
        a_feed_host_with_a_server_error,
        fetch(HttpErrorKind::Status(503)),
        "The feed host feeds.example.com reports an error (HTTP 503)",
        Some("The server has a problem. Try again later.")
    );
    row!(
        a_status_that_is_no_error_status,
        fetch(HttpErrorKind::Status(302)),
        "Cannot fetch the feed from feeds.example.com",
        None
    );
    row!(
        a_feed_host_that_cannot_be_connected_to,
        fetch(HttpErrorKind::Connect),
        "Cannot connect to the feed host feeds.example.com",
        Some("Check the feed URL and the network connection.")
    );
    row!(
        a_feed_fetch_that_fails_otherwise,
        fetch(HttpErrorKind::Other),
        "Cannot fetch the feed from feeds.example.com",
        None
    );
    row!(
        a_feed_file_that_does_not_exist,
        feed_file(ErrorKind::NotFound),
        "The feed file does not exist",
        Some("Check the path. A feed URL starts with http:// or https://.")
    );
    row!(
        a_feed_file_that_cannot_be_read_for_lack_of_permission,
        feed_file(ErrorKind::PermissionDenied),
        "Cannot read the feed file",
        Some("Check the permissions of the file.")
    );
    row!(
        a_feed_file_that_cannot_be_read_otherwise,
        feed_file(ErrorKind::InvalidData),
        "Cannot read the feed file",
        None
    );
    row!(
        a_feed_that_is_not_rss,
        FeedError::ParseFailed(rss::Error::Eof).into(),
        "The feed is not a valid RSS feed",
        Some("Check that the URL serves an RSS feed and not a web page.")
    );
    row!(
        a_feed_url_that_is_none,
        FeedError::InvalidUrl(url::ParseError::EmptyHost).into(),
        "The feed URL is not valid",
        Some("Give a URL that starts with http:// or https://, or the path of an RSS file.")
    );
    row!(
        a_folder_that_cannot_be_created_for_lack_of_space,
        folder("create", ErrorKind::StorageFull),
        "The disk is full",
        Some("Free some space on the disk and run podpull again.")
    );
    row!(
        a_folder_that_cannot_be_created_for_lack_of_permission,
        folder("create", ErrorKind::PermissionDenied),
        "Cannot create the output folder",
        Some("Check the permissions of the folder.")
    );
    row!(
        a_folder_that_cannot_be_created_otherwise,
        folder("create", ErrorKind::NotADirectory),
        "Cannot create the output folder",
        None
    );
    row!(
        a_folder_that_cannot_be_read_for_lack_of_space,
        folder("read", ErrorKind::StorageFull),
        "The disk is full",
        Some("Free some space on the disk and run podpull again.")
    );
    row!(
        a_folder_that_cannot_be_read_for_lack_of_permission,
        folder("read", ErrorKind::PermissionDenied),
        "Cannot read the output folder",
        Some("Check the permissions of the folder.")
    );
    row!(
        a_folder_that_cannot_be_read_otherwise,
        folder("read", ErrorKind::Other),
        "Cannot read the output folder",
        None
    );
    row!(
        podcast_metadata_that_cannot_be_written_for_lack_of_space,
        folder("write", ErrorKind::StorageFull),
        "The disk is full",
        Some("Free some space on the disk and run podpull again.")
    );
    row!(
        podcast_metadata_that_cannot_be_written_for_lack_of_permission,
        folder("write", ErrorKind::PermissionDenied),
        "Cannot write to the output folder",
        Some("Check the permissions of the folder.")
    );
    row!(
        podcast_metadata_that_cannot_be_written_otherwise,
        folder("write", ErrorKind::Other),
        "Cannot write to the output folder",
        None
    );
    row!(
        an_episode_metadata_file_that_cannot_be_read_for_lack_of_permission,
        metadata_read(ErrorKind::PermissionDenied),
        "Cannot read an episode metadata file",
        Some("Check the permissions of the file.")
    );
    row!(
        an_episode_metadata_file_that_cannot_be_read_over_a_dropped_connection,
        metadata_read(ErrorKind::ConnectionReset),
        "Cannot read an episode metadata file",
        Some("If the folder is on a network share, check the connection and run podpull again.")
    );
    row!(
        an_episode_metadata_file_that_cannot_be_read_in_time,
        metadata_read(ErrorKind::TimedOut),
        "Cannot read an episode metadata file",
        Some("If the folder is on a network share, check the connection and run podpull again.")
    );
    row!(
        an_episode_metadata_file_that_cannot_be_read_otherwise,
        metadata_read(ErrorKind::IsADirectory),
        "Cannot read an episode metadata file",
        None
    );
    row!(
        podcast_metadata_that_cannot_be_serialized,
        MetadataError::JsonSerialize(
            serde_json::from_str::<serde_json::Value>("{ nope").unwrap_err()
        )
        .into(),
        "Cannot write the podcast metadata",
        None
    );
    row!(
        an_episode_metadata_file_that_is_not_json,
        MetadataError::JsonParse {
            path: PathBuf::from("/podcasts/a.json"),
            source: serde_json::from_str::<serde_json::Value>("{ nope").unwrap_err(),
        }
        .into(),
        "An episode metadata file is not valid",
        None
    );

    #[test]
    fn a_wrapper_shows_the_message_and_the_hint_of_what_it_wraps() {
        let inner = StateError::Metadata(MetadataError::Read {
            path: PathBuf::from("/podcasts/a.json"),
            source: IoError::from(ErrorKind::PermissionDenied),
        });
        let two_levels = SyncError::State(StateError::Metadata(MetadataError::Read {
            path: PathBuf::from("/podcasts/a.json"),
            source: IoError::from(ErrorKind::PermissionDenied),
        }));

        assert_eq!(two_levels.to_string(), inner.to_string());
        assert_eq!(two_levels.hint(), inner.hint());
        assert!(two_levels.hint().is_some());
    }

    #[test]
    fn a_wrapper_passes_the_cause_of_what_it_wraps_on() {
        let error = fetch(HttpErrorKind::Refused);

        let cause = error.source().expect("the fetch error has a cause");

        assert_eq!(cause.to_string(), "cause");
    }

    #[test]
    fn the_message_of_an_error_that_ends_the_run_holds_no_name_from_outside() {
        let hostile = "a\u{202E}%\u{1b}.xml";
        let errors: Vec<SyncError> = vec![
            FeedError::FileReadFailed {
                path: PathBuf::from(hostile),
                source: IoError::from(ErrorKind::NotFound),
            }
            .into(),
            StateError::CreateDirectoryFailed {
                path: PathBuf::from(hostile),
                source: IoError::from(ErrorKind::PermissionDenied),
            }
            .into(),
            StateError::ReadDirectoryFailed {
                path: PathBuf::from(hostile),
                source: IoError::from(ErrorKind::PermissionDenied),
            }
            .into(),
            MetadataError::Write {
                path: PathBuf::from(hostile),
                source: IoError::from(ErrorKind::PermissionDenied),
            }
            .into(),
            metadata_read_at(hostile),
            MetadataError::JsonParse {
                path: PathBuf::from(hostile),
                source: serde_json::from_str::<serde_json::Value>("{ nope").unwrap_err(),
            }
            .into(),
        ];

        for error in errors {
            let message = error.to_string();
            assert!(
                !message.contains('\u{202E}')
                    && !message.contains('\u{1b}')
                    && !message.contains('%'),
                "{message:?}"
            );
        }
    }

    fn metadata_read_at(path: &str) -> SyncError {
        StateError::Metadata(MetadataError::Read {
            path: PathBuf::from(path),
            source: IoError::from(ErrorKind::PermissionDenied),
        })
        .into()
    }
}
