// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The failures that end a run and what a person is told about them

use std::error::Error;
use std::io::Write;
use std::path::{Path, PathBuf};

use podpull::{FeedError, Hint, MetadataError, StateError, SyncError, root_text};

use super::error_block::{self, Block};
use super::shown::{self, Safe};
use super::stream_env::{Stream, StreamEnv};

/// The file the podcast metadata is written to, inside the output folder
const PODCAST_METADATA_FILENAME: &str = "podcast.json";

/// What ended a run
#[derive(Debug, thiserror::Error)]
pub(crate) enum Failure {
    /// The sync failed. The message and the hint are those of its error.
    #[error(transparent)]
    Sync(#[from] SyncError),

    /// A stream the run writes to failed
    #[error("podpull cannot write its output")]
    Output {
        stream: Stream,
        #[source]
        source: std::io::Error,
    },
}

impl Hint for Failure {
    fn hint(&self) -> Option<&'static str> {
        match self {
            Failure::Sync(error) => error.hint(),
            Failure::Output { .. } => None,
        }
    }
}

/// What podpull was doing when the run ended
///
/// An error does not know it all: the feed argument is the user's, and the
/// path of the podcast metadata is the folder's. The step is decided here,
/// where both are known.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    FetchingFeed(url::Url),
    ReadingFeedFile(PathBuf),
    ParsingFeed(String),
    ReadingFeedUrl(String),
    CreatingFolder(PathBuf),
    ScanningFolder(PathBuf),
    ReadingMetadata(PathBuf),
    WritingPodcastMetadata(PathBuf),
    WritingOutput(Stream),
    /// The sync of a feed, for a failure that has no step of its own
    Syncing(String),
}

impl Step {
    /// The rest of the sentence "Podpull was ...", without capital and full
    /// stop, with every name escaped
    fn phrase(&self) -> Safe {
        let text = match self {
            Step::FetchingFeed(url) => format!("fetching the feed {}", shown::url(url.as_str())),
            Step::ReadingFeedFile(path) => format!("reading the feed file {}", shown::path(path)),
            Step::ParsingFeed(feed) => format!("parsing the feed {}", shown::feed_source(feed)),
            Step::ReadingFeedUrl(feed) => format!("reading the feed URL {}", shown::text(feed)),
            Step::CreatingFolder(path) => {
                format!("creating the output folder {}", shown::path(path))
            }
            Step::ScanningFolder(path) => {
                format!("scanning the output folder {}", shown::path(path))
            }
            Step::ReadingMetadata(path) => {
                format!("reading the episode metadata {}", shown::path(path))
            }
            Step::WritingPodcastMetadata(path) => {
                format!("writing the podcast metadata {}", shown::path(path))
            }
            Step::WritingOutput(stream) => {
                format!("writing the end-of-run output to {stream}")
            }
            Step::Syncing(feed) => format!("syncing the feed {}", shown::feed_source(feed)),
        };
        shown::text(&text)
    }
}

fn metadata_step(error: &MetadataError, output_dir: &Path) -> Step {
    match error {
        MetadataError::Read { path, .. } | MetadataError::JsonParse { path, .. } => {
            Step::ReadingMetadata(path.clone())
        }
        // The partial file the write goes through is an internal detail.
        MetadataError::Write { .. } | MetadataError::JsonSerialize(_) => {
            Step::WritingPodcastMetadata(output_dir.join(PODCAST_METADATA_FILENAME))
        }
    }
}

/// What podpull was doing when `failure` ended the run of `feed` into
/// `output_dir`
fn step_of(failure: &Failure, feed: &str, output_dir: &Path) -> Step {
    match failure {
        Failure::Output { stream, .. } => Step::WritingOutput(*stream),
        Failure::Sync(SyncError::Feed(error)) => match error {
            FeedError::FetchFailed { url, .. } => Step::FetchingFeed(url.clone()),
            FeedError::FileReadFailed { path, .. } => Step::ReadingFeedFile(path.clone()),
            FeedError::ParseFailed(_) => Step::ParsingFeed(feed.to_string()),
            FeedError::InvalidUrl(_) => Step::ReadingFeedUrl(feed.to_string()),
            FeedError::MissingEnclosure { .. } => Step::Syncing(feed.to_string()),
        },
        Failure::Sync(SyncError::State(error)) => match error {
            StateError::CreateDirectoryFailed { path, .. } => Step::CreatingFolder(path.clone()),
            StateError::ReadDirectoryFailed { path, .. } => Step::ScanningFolder(path.clone()),
            StateError::Metadata(error) => metadata_step(error, output_dir),
        },
        Failure::Sync(SyncError::Metadata(error)) => metadata_step(error, output_dir),
    }
}

/// The block that tells a person about `failure`
///
/// The title and the fix are the message and the hint of the error, the cause
/// is its root, and the step is the one `step_of` finds. Everything passes
/// through `shown` on its way, as the text of the error holds no name but
/// the error behind it may.
fn block_for(failure: &Failure, feed: &str, output_dir: &Path) -> Block {
    Block {
        title: shown::text(&failure.to_string()),
        // An error without a cause has nothing to add to its title.
        cause: failure.source().map(|_| shown::text(&root_text(failure))),
        fix: failure.hint().map(shown::text),
        doing: step_of(failure, feed, output_dir).phrase(),
    }
}

/// Write the report of `failure` to `out`
pub(crate) fn report(
    out: &mut impl Write,
    failure: &Failure,
    feed: &str,
    output_dir: &Path,
    env: &StreamEnv,
    columns: usize,
) -> std::io::Result<()> {
    for line in error_block::lines(&block_for(failure, feed, output_dir), env, columns) {
        writeln!(out, "{line}")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use podpull::{HttpError, HttpErrorKind};
    use std::io::{Error as IoError, ErrorKind};

    const FEED: &str = "https://feeds.example.com/podcast.xml";

    fn output_dir() -> PathBuf {
        PathBuf::from("/podcasts/show")
    }

    fn phrase_of(error: impl Into<SyncError>, feed: &str) -> String {
        step_of(&Failure::Sync(error.into()), feed, &output_dir())
            .phrase()
            .to_string()
    }

    #[test]
    fn a_failed_fetch_was_fetching_the_feed() {
        let error = FeedError::FetchFailed {
            url: url::Url::parse(FEED).unwrap(),
            source: HttpError::new(HttpErrorKind::Refused, "refused"),
        };

        assert_eq!(phrase_of(error, FEED), format!("fetching the feed {FEED}"));
    }

    #[test]
    fn a_feed_file_that_cannot_be_read_was_being_read() {
        let error = FeedError::FileReadFailed {
            path: PathBuf::from("feeds/show.xml"),
            source: IoError::from(ErrorKind::NotFound),
        };

        assert_eq!(
            phrase_of(error, "feeds/show.xml"),
            "reading the feed file feeds/show.xml"
        );
    }

    #[test]
    fn a_feed_that_cannot_be_parsed_was_being_parsed_as_the_user_named_it() {
        assert_eq!(
            phrase_of(FeedError::ParseFailed(rss::Error::Eof), FEED),
            format!("parsing the feed {FEED}")
        );
        assert_eq!(
            phrase_of(FeedError::ParseFailed(rss::Error::Eof), "feeds/show.xml"),
            "parsing the feed feeds/show.xml"
        );
    }

    #[test]
    fn a_feed_url_that_is_none_keeps_its_percent_in_the_step() {
        assert_eq!(
            phrase_of(
                FeedError::InvalidUrl(url::ParseError::InvalidDomainCharacter),
                "https://exa mple.com/%41"
            ),
            "reading the feed URL https://exa mple.com/%41"
        );
    }

    #[test]
    fn an_entry_without_an_enclosure_gets_the_step_of_the_sync() {
        let error = FeedError::MissingEnclosure {
            title: "Episode".to_string(),
        };

        assert_eq!(phrase_of(error, FEED), format!("syncing the feed {FEED}"));
    }

    #[test]
    fn a_folder_that_cannot_be_created_was_being_created() {
        let error = StateError::CreateDirectoryFailed {
            path: PathBuf::from("/podcasts/show"),
            source: IoError::from(ErrorKind::PermissionDenied),
        };

        assert_eq!(
            phrase_of(error, FEED),
            "creating the output folder /podcasts/show"
        );
    }

    #[test]
    fn a_folder_that_cannot_be_read_was_being_scanned() {
        let error = StateError::ReadDirectoryFailed {
            path: PathBuf::from("/podcasts/show"),
            source: IoError::from(ErrorKind::PermissionDenied),
        };

        assert_eq!(
            phrase_of(error, FEED),
            "scanning the output folder /podcasts/show"
        );
    }

    #[test]
    fn metadata_that_cannot_be_read_was_being_read_in_the_scan_or_in_the_sync() {
        let read = || MetadataError::Read {
            path: PathBuf::from("/podcasts/show/a.json"),
            source: IoError::from(ErrorKind::PermissionDenied),
        };

        assert_eq!(
            phrase_of(StateError::Metadata(read()), FEED),
            "reading the episode metadata /podcasts/show/a.json"
        );
        assert_eq!(
            phrase_of(read(), FEED),
            "reading the episode metadata /podcasts/show/a.json"
        );
    }

    #[test]
    fn metadata_that_is_not_json_was_being_read() {
        let error = MetadataError::JsonParse {
            path: PathBuf::from("/podcasts/show/a.json"),
            source: serde_json::from_str::<serde_json::Value>("{ nope").unwrap_err(),
        };

        assert_eq!(
            phrase_of(error, FEED),
            "reading the episode metadata /podcasts/show/a.json"
        );
    }

    #[test]
    fn podcast_metadata_that_cannot_be_written_names_podcast_json_and_not_its_partial_file() {
        let error = MetadataError::Write {
            path: PathBuf::from("/podcasts/show/podcast.json.partial"),
            source: IoError::from(ErrorKind::PermissionDenied),
        };

        assert_eq!(
            phrase_of(error, FEED),
            "writing the podcast metadata /podcasts/show/podcast.json"
        );
    }

    #[test]
    fn podcast_metadata_that_cannot_be_serialized_names_podcast_json() {
        let error = MetadataError::JsonSerialize(
            serde_json::from_str::<serde_json::Value>("{ nope").unwrap_err(),
        );

        assert_eq!(
            phrase_of(error, FEED),
            "writing the podcast metadata /podcasts/show/podcast.json"
        );
    }

    #[test]
    fn output_that_cannot_be_written_names_the_stream() {
        let failure = Failure::Output {
            stream: Stream::Stdout,
            source: IoError::from(ErrorKind::StorageFull),
        };

        assert_eq!(
            step_of(&failure, FEED, &output_dir()).phrase().to_string(),
            "writing the end-of-run output to stdout"
        );
    }

    #[test]
    fn a_step_shows_a_hostile_path_escaped() {
        let error = StateError::CreateDirectoryFailed {
            path: PathBuf::from("/podcasts/a\u{202E}%\u{1b}"),
            source: IoError::from(ErrorKind::PermissionDenied),
        };

        let phrase = phrase_of(error, FEED);

        assert_eq!(
            phrase,
            "creating the output folder /podcasts/a%E2%80%AE%25%1B"
        );
    }

    // =========================================================
    // The block a person is shown
    // =========================================================

    fn pipe() -> StreamEnv {
        StreamEnv {
            color: false,
            utf8: true,
            rich: false,
        }
    }

    fn terminal() -> StreamEnv {
        StreamEnv {
            color: false,
            utf8: true,
            rich: true,
        }
    }

    fn refused() -> Failure {
        Failure::Sync(SyncError::Feed(FeedError::FetchFailed {
            url: url::Url::parse(FEED).unwrap(),
            source: HttpError::new(HttpErrorKind::Refused, "Connection refused (os error 61)"),
        }))
    }

    fn report_of(failure: &Failure, env: &StreamEnv) -> Vec<String> {
        let mut out = Vec::new();
        report(&mut out, failure, FEED, &output_dir(), env, 80).unwrap();
        String::from_utf8(out)
            .unwrap()
            .lines()
            .map(String::from)
            .collect()
    }

    #[test]
    fn the_block_of_a_failed_fetch_has_its_title_cause_fix_and_step() {
        let lines = report_of(&refused(), &terminal());

        assert_eq!(
            lines,
            vec![
                "▌ ✖ ERROR".to_string(),
                "▌ The feed host feeds.example.com refuses connections".to_string(),
                "▌".to_string(),
                "▌ Cause".to_string(),
                "▌   connection refused".to_string(),
                "▌".to_string(),
                "▌ How to fix it".to_string(),
                "▌   Check the host and port in the feed URL and that the server is running."
                    .to_string(),
                "▌".to_string(),
                "▌ What podpull was doing".to_string(),
                format!("▌   Fetching the feed {FEED}."),
            ]
        );
    }

    #[test]
    fn the_block_of_a_failure_inside_two_wrappers_still_has_its_fix() {
        let failure = Failure::Sync(SyncError::State(StateError::Metadata(
            MetadataError::Read {
                path: PathBuf::from("/podcasts/show/a.json"),
                source: IoError::from(ErrorKind::PermissionDenied),
            },
        )));

        let lines = report_of(&failure, &terminal());

        assert!(lines.contains(&"▌ Cannot read an episode metadata file".to_string()));
        assert!(lines.contains(&"▌   Check the permissions of the file.".to_string()));
    }

    #[test]
    fn the_pipe_form_of_a_failed_fetch_has_no_fix_line() {
        let lines = report_of(&refused(), &pipe());

        assert_eq!(
            lines,
            vec![
                "Error: The feed host feeds.example.com refuses connections".to_string(),
                "  cause: connection refused".to_string(),
                format!("  while fetching the feed {FEED}"),
            ]
        );
    }

    #[test]
    fn a_failure_to_write_output_has_no_hint() {
        let failure = Failure::Output {
            stream: Stream::Stderr,
            source: IoError::from(ErrorKind::StorageFull),
        };

        assert_eq!(failure.hint(), None);
        assert_eq!(
            report_of(&failure, &pipe()),
            vec![
                "Error: podpull cannot write its output".to_string(),
                "  cause: no storage space".to_string(),
                "  while writing the end-of-run output to stderr".to_string(),
            ]
        );
    }

    #[test]
    fn an_error_without_a_cause_has_no_cause_line() {
        let failure = Failure::Sync(SyncError::Feed(FeedError::MissingEnclosure {
            title: "Episode".to_string(),
        }));

        let lines = report_of(&failure, &pipe());

        assert!(
            !lines.iter().any(|line| line.contains("cause:")),
            "{lines:?}"
        );
    }

    #[test]
    fn a_cause_from_outside_is_shown_escaped() {
        let failure = Failure::Output {
            stream: Stream::Stdout,
            source: IoError::other("bad\u{1b}[2J \u{202E}"),
        };

        let lines = report_of(&failure, &pipe());

        assert_eq!(lines[1], "  cause: bad%1B[2J %E2%80%AE");
    }
}
