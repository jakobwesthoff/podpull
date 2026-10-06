// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::episode::{
    filename_claim_key, generate_filename_stem, generate_unique_filename_stem, get_audio_extension,
};
use crate::error::{MetadataError, StateError};
use crate::feed::Episode;
use crate::metadata::read_episode_metadata;
use crate::progress::{ProgressEvent, SharedProgressReporter};

/// State of the output directory, as found by [`scan_output_dir`]
///
/// The fields are views of one directory listing and are kept consistent by
/// construction, so they are only readable from outside.
#[derive(Debug, Clone)]
pub struct OutputState {
    output_dir: PathBuf,
    partial_files_cleaned: usize,
    stuck_partial_files: Vec<PathBuf>,
    unreadable_metadata: Vec<UnreadableMetadata>,
    claimed_stems: HashSet<String>,
    stored_episodes: HashMap<String, StoredEpisode>,
    /// GUIDs recorded by `stored_episodes`, for fast lookup
    downloaded_guids: HashSet<String>,
}

impl OutputState {
    fn new(
        output_dir: &Path,
        partial_files_cleaned: usize,
        stuck_partial_files: Vec<PathBuf>,
        unreadable_metadata: Vec<UnreadableMetadata>,
        claimed_stems: HashSet<String>,
        stored_episodes: HashMap<String, StoredEpisode>,
    ) -> Self {
        let downloaded_guids = stored_episodes
            .values()
            .filter_map(|stored| stored.guid.clone())
            .collect();
        Self {
            output_dir: output_dir.to_path_buf(),
            partial_files_cleaned,
            stuck_partial_files,
            unreadable_metadata,
            claimed_stems,
            stored_episodes,
            downloaded_guids,
        }
    }

    pub fn output_dir(&self) -> &Path {
        &self.output_dir
    }

    /// Number of partial files that were cleaned up during scan
    pub fn partial_files_cleaned(&self) -> usize {
        self.partial_files_cleaned
    }

    /// Partial files the scan could not remove; a download into such a path
    /// fails until the file is gone
    pub fn stuck_partial_files(&self) -> &[PathBuf] {
        &self.stuck_partial_files
    }

    /// Episode metadata files whose content is not valid metadata
    pub fn unreadable_metadata(&self) -> &[UnreadableMetadata] {
        &self.unreadable_metadata
    }

    /// Whether readable metadata in the directory records this GUID
    pub fn is_downloaded(&self, guid: &str) -> bool {
        self.downloaded_guids.contains(guid)
    }

    /// Claim keys (see [`filename_claim_key`]) of the stems of all files in
    /// the output directory, which new downloads must not reuse
    pub fn claimed_stems(&self) -> &HashSet<String> {
        &self.claimed_stems
    }

    /// The episode with readable metadata whose stem has this claim key
    pub fn stored_episode(&self, claim_key: &str) -> Option<&StoredEpisode> {
        self.stored_episodes.get(claim_key)
    }

    /// All episodes with readable metadata
    pub fn stored_episodes(&self) -> impl Iterator<Item = &StoredEpisode> {
        self.stored_episodes.values()
    }
}

/// An episode metadata file whose content could not be parsed
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreadableMetadata {
    pub path: PathBuf,
    /// Why the content could not be used
    pub error: String,
}

/// An episode downloaded by an earlier run, as its metadata records it
#[derive(Debug, Clone)]
pub struct StoredEpisode {
    pub title: String,
    pub guid: Option<String>,
    /// Name of the audio file, spelled the way the directory lists it
    pub audio_filename: String,
    /// Name of the metadata file, spelled the way the directory lists it
    pub metadata_filename: String,
    /// Hash of the audio as downloaded, if the metadata records one
    pub content_hash: Option<String>,
}

/// An episode scheduled for download, with the files it is written to
#[derive(Debug, Clone)]
pub struct PlannedDownload {
    pub episode: Episode,
    /// Name of the audio file inside the output directory
    pub audio_filename: String,
    /// Name of the episode metadata file inside the output directory
    pub metadata_filename: String,
    /// Key in [`OutputState::stored_episodes`] of the episode that already
    /// occupies this episode's base filename, if any
    pub collides_with: Option<String>,
    /// Whether the download replaces audio of an episode already stored
    /// under these names, as a repair does
    pub replaces_existing: bool,
}

/// Plan for synchronization, indicating what needs to be downloaded
#[derive(Debug, Clone)]
pub struct SyncPlan {
    /// Episodes that need to be downloaded, newest first
    pub to_download: Vec<PlannedDownload>,
    /// Episodes already present in the output directory
    pub already_present: Vec<Episode>,
    /// Total number of episodes in the feed
    pub total_episodes: usize,
}

/// Scan the output directory to detect existing downloads
///
/// Reads all .json metadata files to extract GUIDs of already-downloaded episodes.
/// Also cleans up any `.partial` files from interrupted downloads.
pub fn scan_output_dir(
    output_dir: &Path,
    reporter: &SharedProgressReporter,
) -> Result<OutputState, StateError> {
    let mut existing_files = HashSet::new();
    let mut partial_files_cleaned = 0;

    if !output_dir.exists() {
        // Create the directory if it doesn't exist
        std::fs::create_dir_all(output_dir).map_err(|e| StateError::CreateDirectoryFailed {
            path: output_dir.to_path_buf(),
            source: e,
        })?;

        reporter.report(ProgressEvent::ScanningDirectory {
            files_scanned: 0,
            total_files: 0,
        });

        return Ok(OutputState::new(
            output_dir,
            0,
            Vec::new(),
            Vec::new(),
            HashSet::new(),
            HashMap::new(),
        ));
    }

    // Collect entries first (single network traversal)
    let entries: Vec<_> = std::fs::read_dir(output_dir)
        .map_err(|e| StateError::ReadDirectoryFailed {
            path: output_dir.to_path_buf(),
            source: e,
        })?
        .collect();

    // Categorize entries - this is fast (just filename checks, no I/O)
    let mut partial_files = Vec::new();
    let mut json_files = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|e| StateError::ReadDirectoryFailed {
            path: output_dir.to_path_buf(),
            source: e,
        })?;

        let path = entry.path();
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        if filename.ends_with(".partial") {
            partial_files.push(path);
        } else {
            existing_files.insert(filename.clone());

            if filename.ends_with(".json") && filename != "podcast.json" {
                json_files.push(path);
            }
        }
    }

    // Clean up partial files (fast local operation)
    let mut stuck_partial_files = Vec::new();
    for path in partial_files {
        match std::fs::remove_file(&path) {
            Ok(()) => partial_files_cleaned += 1,
            Err(_) => stuck_partial_files.push(path),
        }
    }

    // Every file occupies its stem, whether or not metadata says which
    // episode it belongs to. Audio without readable metadata may be the only
    // copy of an episode that has since left the feed, so a new download
    // must not take its name. The cost is a second copy when that audio
    // turns out to be a leftover of the very episode being downloaded.
    let claimed_stems = existing_files
        .iter()
        .filter_map(|name| Path::new(name).file_stem())
        .map(|stem| filename_claim_key(&stem.to_string_lossy()))
        .collect();

    // Process JSON metadata files with progress (this is the slow part on network shares)
    let total_json_files = json_files.len();

    reporter.report(ProgressEvent::ScanningDirectory {
        files_scanned: 0,
        total_files: total_json_files,
    });

    // An unreadable metadata file hides which episode it belongs to, so that
    // episode counts as not downloaded. It is collected for reporting rather
    // than dropped silently, because the user is the only one who can tell
    // what the file was.
    let mut unreadable_metadata = Vec::new();
    let mut stored_episodes = HashMap::new();

    for (index, path) in json_files.into_iter().enumerate() {
        match read_episode_metadata(&path) {
            Ok(metadata) => {
                // The metadata records the audio name podpull asked for, but
                // a share may list it in another Unicode normalization. The
                // audio was written under the same stem as the metadata, so
                // the listed metadata stem plus the recorded extension is
                // how the directory spells the audio name.
                let stem = path
                    .file_stem()
                    .expect("metadata paths come from names ending in .json")
                    .to_string_lossy()
                    .into_owned();
                let audio_filename = match Path::new(&metadata.audio_filename).extension() {
                    Some(ext) => format!("{}.{}", stem, ext.to_string_lossy()),
                    None => stem.clone(),
                };
                stored_episodes.insert(
                    filename_claim_key(&stem),
                    StoredEpisode {
                        title: metadata.title,
                        guid: metadata.guid,
                        audio_filename,
                        metadata_filename: format!("{}.json", stem),
                        content_hash: metadata.content_hash,
                    },
                );
            }
            // Content that is not valid metadata stays broken on every run,
            // so it is reported and its episode downloaded again. An I/O
            // error, by contrast, may be a passing network failure; treating
            // it the same would leave a second copy once the file reads fine
            // again, so the scan stops and the next run retries.
            Err(MetadataError::ReadFailed { source, .. })
                if source.kind() != std::io::ErrorKind::InvalidData =>
            {
                return Err(StateError::Metadata(MetadataError::ReadFailed {
                    path,
                    source,
                }));
            }
            Err(error) => unreadable_metadata.push(UnreadableMetadata {
                path,
                error: error.to_string(),
            }),
        }

        reporter.report(ProgressEvent::ScanningDirectory {
            files_scanned: index + 1,
            total_files: total_json_files,
        });
    }

    Ok(OutputState::new(
        output_dir,
        partial_files_cleaned,
        stuck_partial_files,
        unreadable_metadata,
        claimed_stems,
        stored_episodes,
    ))
}

/// Create a sync plan by comparing episodes against the output state
///
/// Determines which episodes need to be downloaded based on:
/// 1. GUID matching (if episode has a GUID that matches a downloaded one, skip)
/// 2. If no GUID match, episode will be downloaded
///
/// Episodes are sorted by publication date (newest first). Episodes without
/// a publication date are placed at the end, preserving their relative order.
pub fn create_sync_plan(episodes: Vec<Episode>, state: &OutputState) -> SyncPlan {
    let total_episodes = episodes.len();
    let mut to_download = Vec::new();
    let mut already_present = Vec::new();

    for episode in episodes {
        let is_downloaded = episode
            .guid
            .as_ref()
            .is_some_and(|guid| state.is_downloaded(guid));

        if is_downloaded {
            already_present.push(episode);
        } else {
            to_download.push(episode);
        }
    }

    // Sort episodes by publication date (newest first)
    // Episodes without pub_date are placed at the end
    to_download.sort_by(|a, b| match (&b.pub_date, &a.pub_date) {
        (Some(b_date), Some(a_date)) => b_date.cmp(a_date),
        (Some(_), None) => std::cmp::Ordering::Greater, // b has date, a doesn't => b comes first
        (None, Some(_)) => std::cmp::Ordering::Less,    // a has date, b doesn't => a comes first
        (None, None) => std::cmp::Ordering::Equal,
    });

    // The plan decides where every download goes, so that the paths of all
    // downloads are known before any of them starts. Names are handed out in
    // download order and each one is claimed right away, so episodes that
    // share title and day within this run get distinct names as well.
    let mut claimed_stems = state.claimed_stems().clone();
    let to_download = to_download
        .into_iter()
        .map(|episode| {
            // An earlier download under the base name is what a collision
            // points at; it gets checked for damage before the sync starts.
            let base_key = filename_claim_key(&generate_filename_stem(&episode));
            let collides_with = state
                .stored_episode(&base_key)
                .is_some()
                .then_some(base_key);

            let stem = generate_unique_filename_stem(&episode, &claimed_stems);
            claimed_stems.insert(filename_claim_key(&stem));
            PlannedDownload {
                collides_with,
                replaces_existing: false,
                audio_filename: format!("{}.{}", stem, get_audio_extension(&episode)),
                metadata_filename: format!("{}.json", stem),
                episode,
            }
        })
        .collect();

    SyncPlan {
        to_download,
        already_present,
        total_episodes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed::Enclosure;
    use crate::metadata::write_episode_metadata;
    use crate::progress::NoopReporter;
    use chrono::{DateTime, FixedOffset, TimeZone, Utc};
    use tempfile::tempdir;
    use url::Url;

    fn make_episode(title: &str, guid: Option<&str>) -> Episode {
        Episode {
            title: title.to_string(),
            description: None,
            pub_date: None,
            guid: guid.map(String::from),
            enclosure: Enclosure {
                url: Url::parse("https://example.com/ep.mp3").unwrap(),
                length: None,
                mime_type: None,
            },
            duration: None,
            episode_number: None,
            season_number: None,
        }
    }

    fn make_episode_with_date(
        title: &str,
        guid: Option<&str>,
        pub_date: Option<DateTime<FixedOffset>>,
    ) -> Episode {
        Episode {
            title: title.to_string(),
            description: None,
            pub_date,
            guid: guid.map(String::from),
            enclosure: Enclosure {
                url: Url::parse("https://example.com/ep.mp3").unwrap(),
                length: None,
                mime_type: None,
            },
            duration: None,
            episode_number: None,
            season_number: None,
        }
    }

    fn stored(stem: &str, title: &str, guid: &str) -> StoredEpisode {
        StoredEpisode {
            title: title.to_string(),
            guid: Some(guid.to_string()),
            audio_filename: format!("{}.mp3", stem),
            metadata_filename: format!("{}.json", stem),
            content_hash: None,
        }
    }

    /// State of an output directory holding exactly these stored episodes,
    /// each claiming the stem of its metadata file
    fn state_with_stored(stored: Vec<StoredEpisode>) -> OutputState {
        let stored: HashMap<_, _> = stored
            .into_iter()
            .map(|episode| {
                let stem = episode.metadata_filename.trim_end_matches(".json");
                (filename_claim_key(stem), episode)
            })
            .collect();
        let claimed = stored.keys().cloned().collect();
        OutputState::new(
            Path::new("/tmp"),
            0,
            Vec::new(),
            Vec::new(),
            claimed,
            stored,
        )
    }

    /// State of an output directory in which exactly `guids` were downloaded
    fn state_with_guids(guids: &[&str]) -> OutputState {
        state_with_stored(
            guids
                .iter()
                .map(|guid| stored(&format!("stored-{}", guid), "Stored", guid))
                .collect(),
        )
    }

    fn make_time(rfc2822: &str) -> Option<DateTime<FixedOffset>> {
        Some(DateTime::parse_from_rfc2822(rfc2822).unwrap())
    }

    fn make_date(year: i32, month: u32, day: u32) -> DateTime<FixedOffset> {
        Utc.with_ymd_and_hms(year, month, day, 12, 0, 0)
            .unwrap()
            .with_timezone(&FixedOffset::east_opt(0).unwrap())
    }

    #[test]
    fn scan_empty_dir_returns_empty_state() {
        let dir = tempdir().unwrap();
        let reporter = NoopReporter::shared();
        let state = scan_output_dir(dir.path(), &reporter).unwrap();

        assert!(state.stored_episodes().next().is_none());
        assert!(state.claimed_stems().is_empty());
        assert_eq!(state.partial_files_cleaned(), 0);
    }

    #[test]
    fn scan_creates_nonexistent_dir() {
        let dir = tempdir().unwrap();
        let output_dir = dir.path().join("new_podcast");
        let reporter = NoopReporter::shared();

        assert!(!output_dir.exists());
        let state = scan_output_dir(&output_dir, &reporter).unwrap();
        assert!(output_dir.exists());
        assert!(state.stored_episodes().next().is_none());
    }

    #[test]
    fn scan_finds_downloaded_episodes() {
        let dir = tempdir().unwrap();
        let episode = make_episode("Test Episode", Some("test-guid-123"));

        // Write episode metadata
        let meta_path = dir.path().join("2024-01-15-test-episode.json");
        write_episode_metadata(&episode, "2024-01-15-test-episode.mp3", None, &meta_path).unwrap();

        let reporter = NoopReporter::shared();
        let state = scan_output_dir(dir.path(), &reporter).unwrap();

        assert!(state.is_downloaded("test-guid-123"));
        assert!(
            state
                .claimed_stems()
                .contains(&filename_claim_key("2024-01-15-test-episode"))
        );
    }

    #[test]
    fn scan_records_unreadable_episode_metadata() {
        let dir = tempdir().unwrap();
        let episode = make_episode("Readable", Some("readable-guid"));
        write_episode_metadata(
            &episode,
            "readable.mp3",
            None,
            &dir.path().join("readable.json"),
        )
        .unwrap();
        std::fs::write(dir.path().join("truncated.json"), b"{\"title\": \"Trunc").unwrap();

        let state = scan_output_dir(dir.path(), &NoopReporter::shared()).unwrap();

        let unreadable: Vec<_> = state
            .unreadable_metadata
            .iter()
            .map(|unreadable| &unreadable.path)
            .collect();
        assert_eq!(unreadable, vec![&dir.path().join("truncated.json")]);
        assert!(state.unreadable_metadata()[0].error.contains("EOF"));
        assert!(state.is_downloaded("readable-guid"));
    }

    #[test]
    fn scan_records_metadata_that_is_not_utf8_as_unreadable() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("binary.json"), [0xff, 0xfe, 0x00]).unwrap();

        let state = scan_output_dir(dir.path(), &NoopReporter::shared()).unwrap();

        assert_eq!(
            state.unreadable_metadata()[0].path,
            dir.path().join("binary.json")
        );
    }

    #[test]
    fn scan_fails_when_metadata_cannot_be_read_from_disk() {
        let dir = tempdir().unwrap();
        // Reading a directory fails with an I/O error, as a dropped network
        // connection would, rather than yielding broken content.
        std::fs::create_dir(dir.path().join("2024-01-15-Episode.json")).unwrap();

        let result = scan_output_dir(dir.path(), &NoopReporter::shared());

        assert!(matches!(
            result,
            Err(StateError::Metadata(MetadataError::ReadFailed { .. }))
        ));
    }

    #[test]
    fn scan_claims_the_stem_of_every_existing_file() {
        let dir = tempdir().unwrap();
        let episode = make_episode("Readable", Some("readable-guid"));
        write_episode_metadata(
            &episode,
            "2024-01-15-Readable.mp3",
            None,
            &dir.path().join("2024-01-15-Readable.json"),
        )
        .unwrap();
        std::fs::write(dir.path().join("2024-01-15-Readable.mp3"), b"audio").unwrap();
        std::fs::write(dir.path().join("2024-01-16-Orphan.m4a"), b"audio").unwrap();
        std::fs::write(dir.path().join("2024-01-17-Broken.json"), b"{").unwrap();
        std::fs::write(dir.path().join("2019-12-27-Neuzuga\u{0308}nge #4.mp3"), b"").unwrap();
        std::fs::write(dir.path().join("2024-01-18-Cut.mp3.partial"), b"").unwrap();

        let state = scan_output_dir(dir.path(), &NoopReporter::shared()).unwrap();

        let expected: HashSet<String> = [
            "2024-01-15-Readable",
            "2024-01-16-Orphan",
            "2024-01-17-Broken",
            "2019-12-27-Neuzug\u{00e4}nge #4",
        ]
        .iter()
        .map(|stem| filename_claim_key(stem))
        .collect();
        assert_eq!(state.claimed_stems(), &expected);
    }

    #[test]
    fn scan_indexes_stored_episodes_by_claim_key_of_their_stem() {
        let dir = tempdir().unwrap();
        // The metadata records the composed name podpull asked for, while
        // the directory lists the decomposed one the share stored.
        let listed_stem = "2019-12-27-Neuzuga\u{0308}nge #4";
        write_episode_metadata(
            &make_episode("Neuzug\u{00e4}nge #4", Some("guid-1")),
            "2019-12-27-Neuzug\u{00e4}nge #4.mp3",
            Some("sha256:abc".to_string()),
            &dir.path().join(format!("{}.json", listed_stem)),
        )
        .unwrap();

        let state = scan_output_dir(dir.path(), &NoopReporter::shared()).unwrap();

        let stored = state
            .stored_episode(&filename_claim_key("2019-12-27-Neuzug\u{00e4}nge #4"))
            .unwrap();
        assert_eq!(stored.title, "Neuzug\u{00e4}nge #4");
        assert_eq!(stored.audio_filename, format!("{}.mp3", listed_stem));
        assert_eq!(stored.metadata_filename, format!("{}.json", listed_stem));
        assert_eq!(stored.guid.as_deref(), Some("guid-1"));
        assert_eq!(stored.content_hash.as_deref(), Some("sha256:abc"));
    }

    #[test]
    fn scan_indexes_stored_episode_whose_audio_has_no_extension() {
        let dir = tempdir().unwrap();
        write_episode_metadata(
            &make_episode("Bare", Some("guid-1")),
            "2024-01-15-Bare",
            None,
            &dir.path().join("2024-01-15-Bare.json"),
        )
        .unwrap();

        let state = scan_output_dir(dir.path(), &NoopReporter::shared()).unwrap();

        assert_eq!(
            state
                .stored_episode(&filename_claim_key("2024-01-15-Bare"))
                .unwrap()
                .audio_filename,
            "2024-01-15-Bare"
        );
    }

    #[test]
    fn scan_records_partial_files_it_cannot_remove() {
        let dir = tempdir().unwrap();
        // A directory cannot be removed like a file.
        let stuck = dir.path().join("2024-01-15-Episode.mp3.partial");
        std::fs::create_dir(&stuck).unwrap();
        std::fs::write(dir.path().join("2024-01-16-Episode.mp3.partial"), b"").unwrap();

        let state = scan_output_dir(dir.path(), &NoopReporter::shared()).unwrap();

        assert_eq!(state.partial_files_cleaned(), 1);
        assert_eq!(state.stuck_partial_files, vec![stuck]);
    }

    #[test]
    fn scan_ignores_podcast_json() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("podcast.json"),
            r#"{"title": "Test", "feed_url": "http://example.com", "updated_at": "2024-01-01"}"#,
        )
        .unwrap();

        let reporter = NoopReporter::shared();
        let state = scan_output_dir(dir.path(), &reporter).unwrap();

        // podcast.json claims its stem but is no episode
        assert!(state.claimed_stems().contains("podcast"));
        assert!(state.stored_episodes().next().is_none());
        assert_eq!(state.stored_episodes().count(), 0);
    }

    #[test]
    fn sync_plan_identifies_new_episodes() {
        let state = state_with_guids(&[]);

        let episodes = vec![
            make_episode("Ep 1", Some("guid-1")),
            make_episode("Ep 2", Some("guid-2")),
        ];

        let plan = create_sync_plan(episodes, &state);

        assert_eq!(plan.to_download.len(), 2);
        assert_eq!(plan.already_present.len(), 0);
        assert_eq!(plan.total_episodes, 2);
    }

    #[test]
    fn sync_plan_skips_downloaded_episodes() {
        let state = state_with_guids(&["guid-1"]);

        let episodes = vec![
            make_episode("Ep 1", Some("guid-1")),
            make_episode("Ep 2", Some("guid-2")),
        ];

        let plan = create_sync_plan(episodes, &state);

        assert_eq!(plan.to_download.len(), 1);
        assert_eq!(plan.to_download[0].episode.title, "Ep 2");
        assert_eq!(plan.already_present.len(), 1);
        assert_eq!(plan.already_present[0].title, "Ep 1");
    }

    #[test]
    fn sync_plan_downloads_episodes_without_guid() {
        let state = state_with_guids(&["guid-1"]);

        let episodes = vec![
            make_episode("Ep 1", Some("guid-1")),
            make_episode("Ep 2", None), // No GUID, should be downloaded
        ];

        let plan = create_sync_plan(episodes, &state);

        assert_eq!(plan.to_download.len(), 1);
        assert_eq!(plan.to_download[0].episode.title, "Ep 2");
    }

    #[test]
    fn sync_plan_assigns_audio_and_metadata_filenames() {
        let episode = Episode {
            enclosure: Enclosure {
                url: Url::parse("https://example.com/book.m4a").unwrap(),
                length: None,
                mime_type: None,
            },
            ..make_episode_with_date("Audio Book", Some("guid-1"), Some(make_date(2024, 1, 16)))
        };

        let plan = create_sync_plan(vec![episode], &state_with_guids(&[]));

        assert_eq!(
            plan.to_download[0].audio_filename,
            "2024-01-16-Audio Book.m4a"
        );
        assert_eq!(
            plan.to_download[0].metadata_filename,
            "2024-01-16-Audio Book.json"
        );
    }

    #[test]
    fn sync_plan_gives_colliding_episodes_distinct_stems_newest_first() {
        let older = make_episode_with_date(
            "Sega Nomad",
            Some("guid-older"),
            make_time("Thu, 19 Dec 2024 10:25:22 +0000"),
        );
        let newer = make_episode_with_date(
            "Sega Nomad",
            Some("guid-newer"),
            make_time("Thu, 19 Dec 2024 10:45:35 +0000"),
        );

        let plan = create_sync_plan(vec![older, newer], &state_with_guids(&[]));

        let names: Vec<_> = plan
            .to_download
            .iter()
            .map(|planned| {
                (
                    planned.audio_filename.as_str(),
                    planned.metadata_filename.as_str(),
                )
            })
            .collect();
        assert_eq!(
            names,
            vec![
                ("2024-12-19-Sega Nomad.mp3", "2024-12-19-Sega Nomad.json"),
                (
                    "2024-12-19-102522-Sega Nomad.mp3",
                    "2024-12-19-102522-Sega Nomad.json"
                ),
            ]
        );
    }

    #[test]
    fn sync_plan_avoids_stems_claimed_on_disk() {
        let mut state = state_with_guids(&[]);
        state
            .claimed_stems
            .insert(filename_claim_key("2024-12-19-Sega Nomad"));
        let episode = make_episode_with_date(
            "Sega Nomad",
            Some("guid-1"),
            make_time("Thu, 19 Dec 2024 10:25:22 +0000"),
        );

        let plan = create_sync_plan(vec![episode], &state);

        assert_eq!(
            plan.to_download[0].audio_filename,
            "2024-12-19-102522-Sega Nomad.mp3"
        );
    }

    #[test]
    fn sync_plan_points_disk_collisions_at_the_stored_episode() {
        let state = state_with_stored(vec![stored(
            "2024-12-19-Sega Nomad",
            "Sega Nomad",
            "guid-stored",
        )]);
        let key = filename_claim_key("2024-12-19-Sega Nomad");
        let newer = make_episode_with_date(
            "Sega Nomad",
            Some("guid-newer"),
            make_time("Thu, 19 Dec 2024 10:45:35 +0000"),
        );
        let older = make_episode_with_date(
            "Sega Nomad",
            Some("guid-older"),
            make_time("Thu, 19 Dec 2024 10:25:22 +0000"),
        );
        let unrelated = make_episode_with_date(
            "Other",
            Some("guid-other"),
            make_time("Thu, 19 Dec 2024 08:00:00 +0000"),
        );

        let plan = create_sync_plan(vec![older, newer, unrelated], &state);

        let collisions: Vec<_> = plan
            .to_download
            .iter()
            .map(|planned| planned.collides_with.as_deref())
            .collect();
        assert_eq!(
            collisions,
            vec![Some(key.as_str()), Some(key.as_str()), None]
        );
    }

    #[test]
    fn sync_plan_does_not_point_collisions_within_the_run_at_stored_episodes() {
        let older = make_episode_with_date(
            "Sega Nomad",
            Some("guid-older"),
            make_time("Thu, 19 Dec 2024 10:25:22 +0000"),
        );
        let newer = make_episode_with_date(
            "Sega Nomad",
            Some("guid-newer"),
            make_time("Thu, 19 Dec 2024 10:45:35 +0000"),
        );

        let plan = create_sync_plan(vec![older, newer], &state_with_guids(&[]));

        assert!(
            plan.to_download
                .iter()
                .all(|planned| planned.collides_with.is_none())
        );
    }

    #[test]
    fn scan_cleans_up_partial_files() {
        let dir = tempdir().unwrap();

        // Create some partial files
        std::fs::write(dir.path().join("episode1.mp3.partial"), b"partial data 1").unwrap();
        std::fs::write(dir.path().join("episode2.mp3.partial"), b"partial data 2").unwrap();
        // Create a normal file
        std::fs::write(dir.path().join("episode3.mp3"), b"complete audio").unwrap();

        let reporter = NoopReporter::shared();
        let state = scan_output_dir(dir.path(), &reporter).unwrap();

        // Partial files should have been cleaned up
        assert_eq!(state.partial_files_cleaned(), 2);
        assert!(!dir.path().join("episode1.mp3.partial").exists());
        assert!(!dir.path().join("episode2.mp3.partial").exists());
        // Normal file should still exist
        assert!(dir.path().join("episode3.mp3").exists());
        assert!(state.claimed_stems().contains("episode3"));
        // Partial files claim no name
        assert!(!state.claimed_stems().contains("episode1.mp3"));
        assert!(!state.claimed_stems().contains("episode2.mp3"));
    }

    #[test]
    fn sync_plan_sorts_episodes_by_pub_date_newest_first() {
        let state = state_with_guids(&[]);

        // Create episodes in random order
        let episodes = vec![
            make_episode_with_date("Old Episode", Some("guid-1"), Some(make_date(2024, 1, 1))),
            make_episode_with_date(
                "Newest Episode",
                Some("guid-2"),
                Some(make_date(2024, 3, 15)),
            ),
            make_episode_with_date(
                "Middle Episode",
                Some("guid-3"),
                Some(make_date(2024, 2, 10)),
            ),
        ];

        let plan = create_sync_plan(episodes, &state);

        // Should be sorted newest first
        assert_eq!(plan.to_download.len(), 3);
        assert_eq!(plan.to_download[0].episode.title, "Newest Episode");
        assert_eq!(plan.to_download[1].episode.title, "Middle Episode");
        assert_eq!(plan.to_download[2].episode.title, "Old Episode");
    }

    #[test]
    fn sync_plan_places_episodes_without_date_at_end() {
        let state = state_with_guids(&[]);

        let episodes = vec![
            make_episode_with_date("No Date 1", Some("guid-1"), None),
            make_episode_with_date("With Date", Some("guid-2"), Some(make_date(2024, 1, 15))),
            make_episode_with_date("No Date 2", Some("guid-3"), None),
        ];

        let plan = create_sync_plan(episodes, &state);

        // Episode with date should be first, undated ones at the end
        assert_eq!(plan.to_download.len(), 3);
        assert_eq!(plan.to_download[0].episode.title, "With Date");
        // Undated episodes preserve relative order
        assert_eq!(plan.to_download[1].episode.title, "No Date 1");
        assert_eq!(plan.to_download[2].episode.title, "No Date 2");
    }
}
