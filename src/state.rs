// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use chrono::{DateTime, FixedOffset};
use url::Url;

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
    claimed_keys: HashSet<String>,
    stored_episodes: HashMap<String, StoredEpisode>,
    /// Claim keys of the stored episodes recording each GUID, as primary or
    /// additional GUID, in audio filename order
    claim_keys_by_guid: HashMap<String, Vec<String>>,
}

impl OutputState {
    fn new(
        output_dir: &Path,
        partial_files_cleaned: usize,
        stuck_partial_files: Vec<PathBuf>,
        unreadable_metadata: Vec<UnreadableMetadata>,
        claimed_keys: HashSet<String>,
        stored_episodes: HashMap<String, StoredEpisode>,
    ) -> Self {
        // Before podpull 1.2.0, colliding downloads could leave several
        // files recording one GUID, so a GUID maps to all of them. Filename
        // order keeps the result independent of the order of the scan.
        let mut by_filename: Vec<_> = stored_episodes.iter().collect();
        by_filename.sort_by(|(_, a), (_, b)| a.audio_filename.cmp(&b.audio_filename));
        let mut claim_keys_by_guid: HashMap<String, Vec<String>> = HashMap::new();
        for (claim_key, stored) in by_filename {
            for guid in stored.guid.iter().chain(&stored.additional_guids) {
                claim_keys_by_guid
                    .entry(guid.clone())
                    .or_default()
                    .push(claim_key.clone());
            }
        }

        Self {
            output_dir: output_dir.to_path_buf(),
            partial_files_cleaned,
            stuck_partial_files,
            unreadable_metadata,
            claimed_keys,
            stored_episodes,
            claim_keys_by_guid,
        }
    }

    /// State of an empty output directory
    #[cfg(test)]
    pub(crate) fn empty(output_dir: &Path) -> Self {
        Self::new(
            output_dir,
            0,
            Vec::new(),
            Vec::new(),
            HashSet::new(),
            HashMap::new(),
        )
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
        self.claim_keys_by_guid.contains_key(guid)
    }

    /// Claim keys (see [`filename_claim_key`]) of the stems of all files in
    /// the output directory, which new downloads must not reuse
    pub fn claimed_keys(&self) -> &HashSet<String> {
        &self.claimed_keys
    }

    /// The episode with readable metadata whose stem has this claim key
    pub fn stored_episode(&self, claim_key: &str) -> Option<&StoredEpisode> {
        self.stored_episodes.get(claim_key)
    }

    /// The stored episodes whose metadata records this GUID, as primary or
    /// additional GUID, in audio filename order
    pub fn stored_episodes_with_guid(&self, guid: &str) -> impl Iterator<Item = &StoredEpisode> {
        self.claim_keys_by_guid
            .get(guid)
            .into_iter()
            .flatten()
            .filter_map(|claim_key| self.stored_episodes.get(claim_key))
    }

    /// All episodes with readable metadata
    pub fn stored_episodes(&self) -> impl Iterator<Item = &StoredEpisode> {
        self.stored_episodes.values()
    }
}

/// An episode metadata file whose content could not be parsed
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UnreadableMetadata {
    pub path: PathBuf,
    /// Why the content could not be used
    pub error: String,
}

/// An episode downloaded by an earlier run, as its metadata records it
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct StoredEpisode {
    pub title: String,
    pub guid: Option<String>,
    /// Further GUIDs the feed has listed this audio under
    pub additional_guids: Vec<String>,
    /// Enclosure URL the audio was downloaded from
    pub original_url: String,
    pub pub_date: Option<DateTime<FixedOffset>>,
    /// Name of the audio file, spelled the way the directory lists it
    pub audio_filename: String,
    /// Name of the metadata file, spelled the way the directory lists it
    pub metadata_filename: String,
    /// Hash of the audio as downloaded, if the metadata records one
    pub content_hash: Option<String>,
}

impl StoredEpisode {
    /// Stem shared by the metadata file and the audio file, as listed
    pub fn stem(&self) -> &str {
        self.metadata_filename
            .strip_suffix(".json")
            .unwrap_or(&self.metadata_filename)
    }
}

/// An episode scheduled for download, with the files it is written to
///
/// Audio and metadata share one stem, so a plan cannot pair the audio of
/// one name with the metadata of another.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct PlannedDownload {
    pub episode: Episode,
    /// Name of both files inside the output directory, without extension
    pub stem: String,
    pub audio_extension: String,
    /// Whether the download replaces audio of an episode already stored
    /// under these names, as a repair does
    pub replaces_existing: bool,
    /// Stored episodes the feed entry may have replaced, in audio filename
    /// order; downloaded audio identical to one of them is recorded there
    /// instead of stored again
    pub(crate) replaced_candidates: Vec<StoredEpisode>,
    /// GUIDs besides the episode's own that its metadata keeps recording,
    /// as a repair keeps those of the episode it replaces
    pub(crate) kept_guids: Vec<String>,
}

impl PlannedDownload {
    /// Plan a fresh download of `episode` under `stem`
    pub fn new(
        episode: Episode,
        stem: impl Into<String>,
        audio_extension: impl Into<String>,
    ) -> Self {
        Self {
            episode,
            stem: stem.into(),
            audio_extension: audio_extension.into(),
            replaces_existing: false,
            replaced_candidates: Vec::new(),
            kept_guids: Vec::new(),
        }
    }

    /// Name of the audio file inside the output directory
    pub fn audio_filename(&self) -> String {
        format!("{}.{}", self.stem, self.audio_extension)
    }

    /// Name of the episode metadata file inside the output directory
    pub fn metadata_filename(&self) -> String {
        format!("{}.json", self.stem)
    }
}

/// Plan for synchronization, indicating what needs to be downloaded
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct SyncPlan {
    /// Episodes to download in this run, newest first, within the limit
    pub to_download: Vec<PlannedDownload>,
    /// Episodes already present in the output directory
    pub already_present: Vec<Episode>,
    /// Number of distinct episodes in the feed
    pub total_episodes: usize,
    /// Number of episodes not yet downloaded, before the limit
    pub new_episodes: usize,
    /// Stored episodes whose base filename a download in this run would
    /// have taken, each listed once
    ///
    /// Only the base name is of interest: podpull 1.1.2 and earlier wrote
    /// every episode under its base name, so only a file there can hold the
    /// bytes of two episodes downloaded at once. Names with a time or hash
    /// suffix are written with exclusive partial files.
    pub collisions: Vec<CheckTarget>,
    /// The present feed episode each stored episode was matched to, by the
    /// claim key of the stored episode's stem
    pub(crate) feed_episodes_by_stored: HashMap<String, Episode>,
}

/// A stored episode whose audio is checked against its recorded hash,
/// with the feed episode it belongs to
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct CheckTarget {
    pub stored: StoredEpisode,
    /// The feed episode the stored episode was matched to, if any
    pub feed_episode: Option<Episode>,
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
    let claimed_keys = existing_files
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
                        additional_guids: metadata.additional_guids,
                        original_url: metadata.original_url,
                        pub_date: metadata
                            .pub_date
                            .and_then(|date| DateTime::parse_from_rfc3339(&date).ok()),
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
        claimed_keys,
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
pub fn create_sync_plan(
    mut episodes: Vec<Episode>,
    state: &OutputState,
    limit: Option<usize>,
) -> SyncPlan {
    sort_newest_first(&mut episodes);

    // The GUID identifies an episode, so a feed listing one twice still
    // holds one episode; the newest listing is kept. Episodes without a
    // GUID have nothing to compare and all stay.
    let mut seen_guids = HashSet::new();
    episodes.retain(|episode| match &episode.guid {
        Some(guid) => seen_guids.insert(guid.clone()),
        None => true,
    });

    let total_episodes = episodes.len();
    let feed_guids: HashSet<String> = episodes
        .iter()
        .filter_map(|episode| episode.guid.clone())
        .collect();
    let mut to_download = Vec::new();
    let mut already_present = Vec::new();

    // Each present episode is matched to the stored episodes it is, so that
    // checking stored audio knows where to download it from again. A stored
    // episode matched by several feed entries keeps the newest.
    let mut feed_episodes_by_stored = HashMap::new();
    for episode in episodes {
        let mut matched: Vec<&StoredEpisode> = match &episode.guid {
            Some(guid) => state.stored_episodes_with_guid(guid).collect(),
            None => Vec::new(),
        };
        if matched.is_empty() {
            matched = guidless_reissue_of(&episode, state);
        }

        if matched.is_empty() {
            to_download.push(episode);
        } else {
            for stored in matched {
                feed_episodes_by_stored
                    .entry(filename_claim_key(stored.stem()))
                    .or_insert_with(|| episode.clone());
            }
            already_present.push(episode);
        }
    }

    let new_episodes = to_download.len();
    if let Some(limit) = limit {
        to_download.truncate(limit);
    }

    // A stored episode under a planned download's base name is what the
    // collision check looks at, once per stored episode.
    let mut collisions: Vec<CheckTarget> = Vec::new();
    for episode in &to_download {
        let base_key = filename_claim_key(&generate_filename_stem(episode));
        if let Some(stored) = state.stored_episode(&base_key)
            && !collisions
                .iter()
                .any(|collision| collision.stored.metadata_filename == stored.metadata_filename)
        {
            collisions.push(check_target(stored, &feed_episodes_by_stored));
        }
    }

    // The plan decides where every download goes, so that the paths of all
    // downloads are known before any of them starts. Names are handed out in
    // download order and each one is claimed right away, so episodes that
    // share title and day within this run get distinct names as well.
    let mut claimed_keys = state.claimed_keys().clone();
    let to_download = to_download
        .into_iter()
        .map(|episode| {
            let stem = generate_unique_filename_stem(&episode, &claimed_keys);
            claimed_keys.insert(filename_claim_key(&stem));
            PlannedDownload {
                replaced_candidates: replaced_candidates(&episode, state, &feed_guids),
                ..PlannedDownload::new(episode.clone(), stem, get_audio_extension(&episode))
            }
        })
        .collect();

    SyncPlan {
        to_download,
        already_present,
        total_episodes,
        new_episodes,
        collisions,
        feed_episodes_by_stored,
    }
}

/// Stored episodes that `episode` may have replaced in the feed, in audio
/// filename order
///
/// Only those with a recorded hash qualify, as identical audio is what
/// confirms the replacement. The match records the episode's GUID, so an
/// episode without one has nothing to record and no candidates.
fn replaced_candidates(
    episode: &Episode,
    state: &OutputState,
    feed_guids: &HashSet<String>,
) -> Vec<StoredEpisode> {
    if episode.guid.is_none() {
        return Vec::new();
    }
    let mut candidates: Vec<_> = state
        .stored_episodes()
        .filter(|stored| {
            stored.content_hash.is_some() && is_replaced_by(stored, episode, feed_guids)
        })
        .cloned()
        .collect();
    candidates.sort_by(|a, b| a.audio_filename.cmp(&b.audio_filename));
    candidates
}

/// Whether `episode` is `stored` re-issued under a new GUID
///
/// A feed entry that replaced the stored one makes all of its GUIDs
/// disappear from the feed. Title and exact publication time must stay the
/// same, so that another entry with the same audio, such as a rerun listed
/// after the original dropped out of a feed that keeps only its newest
/// episodes, is not taken for it.
fn is_replaced_by(stored: &StoredEpisode, episode: &Episode, feed_guids: &HashSet<String>) -> bool {
    let still_listed = stored
        .guid
        .iter()
        .chain(&stored.additional_guids)
        .any(|guid| feed_guids.contains(guid));
    !still_listed
        && stored.title == episode.title
        && stored.pub_date.is_some()
        && stored.pub_date == episode.pub_date
}

/// The stored episodes that `episode` is, if it lacks a GUID and is stored
/// under an enclosure URL that differs only before its file name
///
/// Without a GUID the enclosure URL identifies an episode, and private feeds
/// put an access token into that URL. Every new token would otherwise make
/// the whole archive count as new and store it a second time. The match
/// requires title, exact publication time and the URL's file name to agree,
/// because a wrong match skips a distinct episode.
fn guidless_reissue_of<'a>(episode: &Episode, state: &'a OutputState) -> Vec<&'a StoredEpisode> {
    let url = &episode.enclosure.url;
    if !identity_is_url(episode.guid.as_deref(), url) {
        return Vec::new();
    }
    let (Some(pub_date), Some(file_name)) = (episode.pub_date, url_file_name(url)) else {
        return Vec::new();
    };

    let mut matched: Vec<_> = state
        .stored_episodes()
        .filter(|stored| {
            let Ok(stored_url) = Url::parse(&stored.original_url) else {
                return false;
            };
            stored.title == episode.title
                && stored.pub_date == Some(pub_date)
                && identity_is_url(stored.guid.as_deref(), &stored_url)
                && url_file_name(&stored_url) == Some(file_name)
        })
        .collect();
    matched.sort_by(|a, b| a.audio_filename.cmp(&b.audio_filename));
    matched
}

/// Whether a GUID is the enclosure URL, as parsing makes it for an item
/// without a GUID
///
/// Comparing parsed URLs tolerates the differences between the raw string
/// in the feed and the URL as parsed, such as escaping.
fn identity_is_url(guid: Option<&str>, url: &Url) -> bool {
    guid.and_then(|guid| Url::parse(guid).ok())
        .is_some_and(|guid_url| guid_url == *url)
}

fn url_file_name(url: &Url) -> Option<&str> {
    url.path_segments()?
        .next_back()
        .filter(|name| !name.is_empty())
}

/// Every stored episode, as targets for checking the whole archive
///
/// Stored episodes no present feed episode matched are targets as well,
/// without a feed episode. Ordered by audio filename, so a check runs in the
/// same order each time.
pub(crate) fn archive_check_targets(state: &OutputState, plan: &SyncPlan) -> Vec<CheckTarget> {
    let mut targets: Vec<_> = state
        .stored_episodes()
        .map(|stored| check_target(stored, &plan.feed_episodes_by_stored))
        .collect();
    targets.sort_by(|a, b| a.stored.audio_filename.cmp(&b.stored.audio_filename));
    targets
}

/// Pair a stored episode with the present feed episode the plan matched it to
fn check_target(
    stored: &StoredEpisode,
    feed_episodes_by_stored: &HashMap<String, Episode>,
) -> CheckTarget {
    let feed_episode = feed_episodes_by_stored
        .get(&filename_claim_key(stored.stem()))
        .cloned();
    CheckTarget {
        stored: stored.clone(),
        feed_episode,
    }
}

/// Sort episodes by publication date, newest first
///
/// Episodes without a publication date go last, keeping their order.
fn sort_newest_first(episodes: &mut [Episode]) {
    episodes.sort_by(|a, b| match (&b.pub_date, &a.pub_date) {
        (Some(b_date), Some(a_date)) => b_date.cmp(a_date),
        (Some(_), None) => std::cmp::Ordering::Greater, // b has date, a doesn't => b comes first
        (None, Some(_)) => std::cmp::Ordering::Less,    // a has date, b doesn't => a comes first
        (None, None) => std::cmp::Ordering::Equal,
    });
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
            additional_guids: Vec::new(),
            original_url: "https://example.com/ep.mp3".to_string(),
            pub_date: None,
            audio_filename: format!("{}.mp3", stem),
            metadata_filename: format!("{}.json", stem),
            content_hash: None,
        }
    }

    const TOKEN_A_URL: &str = "https://example.com/media/token-a/episode-uuid.mp3";
    const TOKEN_B_URL: &str = "https://example.com/media/token-b/episode-uuid.mp3";
    const EPISODE_TIME: &str = "Thu, 19 Dec 2024 10:25:22 +0000";

    /// An episode of a feed without GUIDs, identified by its enclosure URL,
    /// as parsing does for a missing GUID
    fn guidless(url: &str, title: &str, time: &str) -> Episode {
        Episode {
            guid: Some(url.to_string()),
            enclosure: Enclosure {
                url: Url::parse(url).unwrap(),
                length: None,
                mime_type: None,
            },
            ..make_episode_with_date(title, None, make_time(time))
        }
    }

    /// The stored counterpart of [`guidless`]
    fn stored_guidless(url: &str, title: &str, time: &str) -> StoredEpisode {
        StoredEpisode {
            guid: Some(url.to_string()),
            original_url: url.to_string(),
            pub_date: make_time(time),
            ..stored("2024-12-19-Sega Nomad", title, url)
        }
    }

    fn plan_for_rotated_token(stored: StoredEpisode, episode: Episode) -> SyncPlan {
        create_sync_plan(vec![episode], &state_with_stored(vec![stored]), None)
    }

    /// State of an output directory holding exactly these stored episodes,
    /// each claiming the stem of its metadata file
    fn state_with_stored(stored: Vec<StoredEpisode>) -> OutputState {
        let stored: HashMap<_, _> = stored
            .into_iter()
            .map(|episode| {
                let stem = episode.stem();
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
        assert!(state.claimed_keys().is_empty());
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
                .claimed_keys()
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
        assert_eq!(state.claimed_keys(), &expected);
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
        assert_eq!(stored.original_url, "https://example.com/ep.mp3");
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
    fn scan_counts_additional_guids_as_downloaded() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("2024-01-15-Episode.json");
        write_episode_metadata(
            &make_episode("Episode", Some("old-guid")),
            "2024-01-15-Episode.mp3",
            None,
            &path,
        )
        .unwrap();
        crate::metadata::add_guid_to_episode_metadata(&path, "new-guid").unwrap();

        let state = scan_output_dir(dir.path(), &NoopReporter::shared()).unwrap();

        assert!(state.is_downloaded("old-guid"));
        assert!(state.is_downloaded("new-guid"));
    }

    #[test]
    fn state_finds_every_stored_episode_recording_a_guid() {
        let adopted = StoredEpisode {
            additional_guids: vec!["guid-shared".to_string()],
            ..stored("2024-01-01-Adopted", "Episode", "guid-adopted")
        };
        // Before podpull 1.2.0, colliding downloads could leave two files
        // recording one GUID.
        let state = state_with_stored(vec![
            adopted,
            stored("2024-01-02-Copy", "Episode", "guid-shared"),
            stored("2024-01-03-Other", "Other", "guid-other"),
        ]);

        let mut found: Vec<_> = state
            .stored_episodes_with_guid("guid-shared")
            .map(|stored| stored.audio_filename.as_str())
            .collect();
        found.sort();

        assert_eq!(found, vec!["2024-01-01-Adopted.mp3", "2024-01-02-Copy.mp3"]);
        assert_eq!(state.stored_episodes_with_guid("guid-gone").count(), 0);
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
        assert!(state.claimed_keys().contains("podcast"));
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

        let plan = create_sync_plan(episodes, &state, None);

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

        let plan = create_sync_plan(episodes, &state, None);

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

        let plan = create_sync_plan(episodes, &state, None);

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

        let plan = create_sync_plan(vec![episode], &state_with_guids(&[]), None);

        assert_eq!(
            plan.to_download[0].audio_filename(),
            "2024-01-16-Audio Book.m4a"
        );
        assert_eq!(plan.to_download[0].stem, "2024-01-16-Audio Book");
        assert_eq!(plan.to_download[0].audio_extension, "m4a");
        assert_eq!(
            plan.to_download[0].metadata_filename(),
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

        let plan = create_sync_plan(vec![older, newer], &state_with_guids(&[]), None);

        let names: Vec<_> = plan
            .to_download
            .iter()
            .map(|planned| (planned.audio_filename(), planned.metadata_filename()))
            .collect();
        let names: Vec<_> = names
            .iter()
            .map(|(audio, metadata)| (audio.as_str(), metadata.as_str()))
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
            .claimed_keys
            .insert(filename_claim_key("2024-12-19-Sega Nomad"));
        let episode = make_episode_with_date(
            "Sega Nomad",
            Some("guid-1"),
            make_time("Thu, 19 Dec 2024 10:25:22 +0000"),
        );

        let plan = create_sync_plan(vec![episode], &state, None);

        assert_eq!(
            plan.to_download[0].audio_filename(),
            "2024-12-19-102522-Sega Nomad.mp3"
        );
    }

    fn sega_nomad(guid: &str, time: &str) -> Episode {
        make_episode_with_date("Sega Nomad", Some(guid), make_time(time))
    }

    #[test]
    fn sync_plan_lists_stored_episodes_that_downloads_collide_with() {
        let state = state_with_stored(vec![stored(
            "2024-12-19-Sega Nomad",
            "Sega Nomad",
            "guid-stored",
        )]);
        let stored_episode = sega_nomad("guid-stored", "Thu, 19 Dec 2024 11:00:00 +0000");
        let newer = sega_nomad("guid-newer", "Thu, 19 Dec 2024 10:45:35 +0000");
        let older = sega_nomad("guid-older", "Thu, 19 Dec 2024 10:25:22 +0000");
        let unrelated = make_episode_with_date(
            "Other",
            Some("guid-other"),
            make_time("Thu, 19 Dec 2024 08:00:00 +0000"),
        );

        let plan = create_sync_plan(vec![older, newer, unrelated, stored_episode], &state, None);

        // Two downloads collide with the same stored episode; it is listed
        // once, together with the feed episode it belongs to.
        assert_eq!(plan.collisions.len(), 1);
        let collision = &plan.collisions[0];
        assert_eq!(
            collision.stored.metadata_filename,
            "2024-12-19-Sega Nomad.json"
        );
        assert_eq!(
            collision
                .feed_episode
                .as_ref()
                .and_then(|e| e.guid.as_deref()),
            Some("guid-stored")
        );
    }

    #[test]
    fn sync_plan_leaves_feed_episode_empty_for_stored_episodes_gone_from_feed() {
        let state = state_with_stored(vec![stored(
            "2024-12-19-Sega Nomad",
            "Sega Nomad",
            "guid-gone",
        )]);
        let newer = sega_nomad("guid-newer", "Thu, 19 Dec 2024 10:45:35 +0000");

        let plan = create_sync_plan(vec![newer], &state, None);

        assert!(plan.collisions[0].feed_episode.is_none());
    }

    #[test]
    fn sync_plan_lists_no_collisions_within_the_run() {
        let older = sega_nomad("guid-older", "Thu, 19 Dec 2024 10:25:22 +0000");
        let newer = sega_nomad("guid-newer", "Thu, 19 Dec 2024 10:45:35 +0000");

        let plan = create_sync_plan(vec![older, newer], &state_with_guids(&[]), None);

        assert!(plan.collisions.is_empty());
    }

    #[test]
    fn sync_plan_applies_the_limit_to_new_episodes() {
        let episodes = vec![
            make_episode_with_date("Old", Some("guid-1"), Some(make_date(2024, 1, 1))),
            make_episode_with_date("New", Some("guid-2"), Some(make_date(2024, 1, 2))),
        ];

        let plan = create_sync_plan(episodes, &state_with_guids(&[]), Some(1));

        assert_eq!(plan.new_episodes, 2);
        assert_eq!(plan.to_download.len(), 1);
        assert_eq!(plan.to_download[0].episode.title, "New");
    }

    #[test]
    fn sync_plan_lists_only_collisions_of_downloads_within_the_limit() {
        let state = state_with_stored(vec![stored(
            "2024-12-19-Sega Nomad",
            "Sega Nomad",
            "guid-stored",
        )]);
        let unrelated = make_episode_with_date(
            "Other",
            Some("guid-other"),
            make_time("Fri, 20 Dec 2024 08:00:00 +0000"),
        );
        let older = sega_nomad("guid-older", "Thu, 19 Dec 2024 10:25:22 +0000");

        let plan = create_sync_plan(vec![older, unrelated], &state, Some(1));

        assert!(plan.collisions.is_empty());
    }

    #[test]
    fn sync_plan_downloads_a_repeated_guid_once() {
        let older = make_episode_with_date(
            "Sega Nomad",
            Some("guid-1"),
            make_time("Thu, 19 Dec 2024 10:25:22 +0000"),
        );
        let newer = make_episode_with_date(
            "Sega Nomad",
            Some("guid-1"),
            make_time("Thu, 19 Dec 2024 10:45:35 +0000"),
        );

        let plan = create_sync_plan(vec![older, newer], &state_with_guids(&[]), None);

        // The GUID is podpull's identity of an episode, so a feed listing it
        // twice still holds one episode; the newer listing wins.
        assert_eq!(plan.total_episodes, 1);
        assert_eq!(plan.to_download.len(), 1);
        assert_eq!(
            plan.to_download[0].episode.pub_date,
            make_time("Thu, 19 Dec 2024 10:45:35 +0000")
        );
    }

    #[test]
    fn sync_plan_counts_a_repeated_present_guid_once() {
        let episodes = vec![
            make_episode("Ep 1", Some("guid-1")),
            make_episode("Ep 1", Some("guid-1")),
        ];

        let plan = create_sync_plan(episodes, &state_with_guids(&["guid-1"]), None);

        assert_eq!(plan.already_present.len(), 1);
        assert_eq!(plan.total_episodes, 1);
    }

    #[test]
    fn sync_plan_keeps_episodes_without_guid_apart() {
        let episodes = vec![make_episode("Ep 1", None), make_episode("Ep 2", None)];

        let plan = create_sync_plan(episodes, &state_with_guids(&[]), None);

        assert_eq!(plan.to_download.len(), 2);
    }

    #[test]
    fn archive_check_targets_cover_every_stored_episode() {
        let state = state_with_stored(vec![
            stored("2024-01-02-B", "B", "guid-b"),
            stored("2024-01-01-A", "A", "guid-gone"),
        ]);
        let present = make_episode_with_date("B", Some("guid-b"), Some(make_date(2024, 1, 2)));
        let plan = create_sync_plan(vec![present], &state, None);

        let targets = archive_check_targets(&state, &plan);

        let summary: Vec<_> = targets
            .iter()
            .map(|target| {
                (
                    target.stored.audio_filename.as_str(),
                    target
                        .feed_episode
                        .as_ref()
                        .map(|episode| episode.title.as_str()),
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![("2024-01-01-A.mp3", None), ("2024-01-02-B.mp3", Some("B"))]
        );
    }

    /// Audio filename and feed episode title of every archive check target
    fn archive_pairs(state: &OutputState, plan: &SyncPlan) -> Vec<(String, Option<String>)> {
        archive_check_targets(state, plan)
            .into_iter()
            .map(|target| {
                (
                    target.stored.audio_filename,
                    target.feed_episode.map(|episode| episode.title),
                )
            })
            .collect()
    }

    #[test]
    fn archive_check_targets_pair_episode_listed_under_an_additional_guid() {
        let adopted = StoredEpisode {
            additional_guids: vec!["guid-new".to_string()],
            ..stored("2024-01-01-A", "A", "guid-old")
        };
        let state = state_with_stored(vec![adopted]);
        let present = make_episode_with_date("A", Some("guid-new"), Some(make_date(2024, 1, 1)));
        let plan = create_sync_plan(vec![present], &state, None);

        assert_eq!(
            archive_pairs(&state, &plan),
            vec![("2024-01-01-A.mp3".to_string(), Some("A".to_string()))]
        );
    }

    #[test]
    fn archive_check_targets_pair_every_stored_copy_of_a_guid() {
        let state = state_with_stored(vec![
            stored("2024-01-01-A", "A", "guid-a"),
            stored("2024-01-01-A Copy", "A", "guid-a"),
        ]);
        let present = make_episode_with_date("A", Some("guid-a"), Some(make_date(2024, 1, 1)));
        let plan = create_sync_plan(vec![present], &state, None);

        assert_eq!(
            archive_pairs(&state, &plan),
            vec![
                ("2024-01-01-A Copy.mp3".to_string(), Some("A".to_string())),
                ("2024-01-01-A.mp3".to_string(), Some("A".to_string())),
            ]
        );
    }

    #[test]
    fn archive_check_targets_pair_guidless_episode_after_url_token_change() {
        let state = state_with_stored(vec![stored_guidless(
            TOKEN_A_URL,
            "Sega Nomad",
            EPISODE_TIME,
        )]);
        let plan = create_sync_plan(
            vec![guidless(TOKEN_B_URL, "Sega Nomad", EPISODE_TIME)],
            &state,
            None,
        );

        let targets = archive_check_targets(&state, &plan);

        assert_eq!(
            targets[0]
                .feed_episode
                .as_ref()
                .map(|episode| episode.enclosure.url.as_str()),
            Some(TOKEN_B_URL)
        );
    }

    #[test]
    fn sync_plan_pairs_colliding_stored_episode_listed_under_an_additional_guid() {
        let adopted = StoredEpisode {
            additional_guids: vec!["guid-stored-new".to_string()],
            ..stored("2024-12-19-Sega Nomad", "Sega Nomad", "guid-stored-old")
        };
        let state = state_with_stored(vec![adopted]);
        let present = sega_nomad("guid-stored-new", "Thu, 19 Dec 2024 11:00:00 +0000");
        let newer = sega_nomad("guid-newer", "Thu, 19 Dec 2024 10:45:35 +0000");

        let plan = create_sync_plan(vec![present, newer], &state, None);

        assert_eq!(
            plan.collisions[0]
                .feed_episode
                .as_ref()
                .and_then(|episode| episode.guid.as_deref()),
            Some("guid-stored-new")
        );
    }

    #[test]
    fn sync_plan_lists_stored_episodes_a_download_may_have_replaced() {
        let with_time = |stem: &str, title: &str, guid: &str| StoredEpisode {
            pub_date: make_time(EPISODE_TIME),
            content_hash: Some("sha256:nomad".to_string()),
            ..stored(stem, title, guid)
        };
        let state = state_with_stored(vec![
            with_time("2024-12-19-Sega Nomad", "Sega Nomad", "guid-gone"),
            with_time("2024-12-19-Sega Nomad Copy", "Sega Nomad", "guid-gone"),
            with_time("2024-12-19-Sega Nomad Listed", "Sega Nomad", "guid-listed"),
            with_time("2024-12-19-Best of", "Best of", "guid-gone-too"),
        ]);
        let unhashed = StoredEpisode {
            content_hash: None,
            ..with_time("2024-12-19-Sega Nomad Unhashed", "Sega Nomad", "guid-gone")
        };
        let state = state_with_stored(
            state
                .stored_episodes()
                .cloned()
                .chain(std::iter::once(unhashed))
                .collect(),
        );
        let reissued = sega_nomad("guid-new", EPISODE_TIME);
        let listed = sega_nomad("guid-listed", EPISODE_TIME);

        let plan = create_sync_plan(vec![reissued, listed], &state, None);

        // Only stored episodes that left the feed with title and time
        // unchanged and recorded a hash qualify, every copy of them in name
        // order.
        let candidates: Vec<_> = plan.to_download[0]
            .replaced_candidates
            .iter()
            .map(|stored| stored.audio_filename.as_str())
            .collect();
        assert_eq!(
            candidates,
            vec![
                "2024-12-19-Sega Nomad Copy.mp3",
                "2024-12-19-Sega Nomad.mp3"
            ]
        );
    }

    #[test]
    fn sync_plan_skips_guidless_episode_whose_url_token_changed() {
        let plan = plan_for_rotated_token(
            stored_guidless(TOKEN_A_URL, "Sega Nomad", EPISODE_TIME),
            guidless(TOKEN_B_URL, "Sega Nomad", EPISODE_TIME),
        );

        assert!(plan.to_download.is_empty());
        assert_eq!(plan.already_present.len(), 1);
    }

    #[test]
    fn sync_plan_downloads_guidless_episode_with_another_file_name() {
        let plan = plan_for_rotated_token(
            stored_guidless(TOKEN_A_URL, "Sega Nomad", EPISODE_TIME),
            guidless(
                "https://example.com/media/token-b/other-uuid.mp3",
                "Sega Nomad",
                EPISODE_TIME,
            ),
        );

        assert_eq!(plan.to_download.len(), 1);
    }

    #[test]
    fn sync_plan_downloads_guidless_episode_published_at_another_time() {
        let plan = plan_for_rotated_token(
            stored_guidless(TOKEN_A_URL, "Sega Nomad", EPISODE_TIME),
            guidless(TOKEN_B_URL, "Sega Nomad", "Thu, 19 Dec 2024 10:25:23 +0000"),
        );

        assert_eq!(plan.to_download.len(), 1);
    }

    #[test]
    fn sync_plan_downloads_guidless_episode_with_another_title() {
        let plan = plan_for_rotated_token(
            stored_guidless(TOKEN_A_URL, "Sega Nomad", EPISODE_TIME),
            guidless(TOKEN_B_URL, "Sega Nomad Part 2", EPISODE_TIME),
        );

        assert_eq!(plan.to_download.len(), 1);
    }

    #[test]
    fn sync_plan_downloads_episode_with_real_guid_despite_matching_guidless_one() {
        let mut episode = guidless(TOKEN_B_URL, "Sega Nomad", EPISODE_TIME);
        episode.guid = Some("real-guid".to_string());

        let plan = plan_for_rotated_token(
            stored_guidless(TOKEN_A_URL, "Sega Nomad", EPISODE_TIME),
            episode,
        );

        assert_eq!(plan.to_download.len(), 1);
    }

    #[test]
    fn sync_plan_does_not_match_stored_episode_with_real_guid() {
        let stored = StoredEpisode {
            guid: Some("real-guid".to_string()),
            ..stored_guidless(TOKEN_A_URL, "Sega Nomad", EPISODE_TIME)
        };

        let plan =
            plan_for_rotated_token(stored, guidless(TOKEN_B_URL, "Sega Nomad", EPISODE_TIME));

        assert_eq!(plan.to_download.len(), 1);
    }

    #[test]
    fn sync_plan_does_not_match_guidless_episodes_without_publication_date() {
        let stored = StoredEpisode {
            pub_date: None,
            ..stored_guidless(TOKEN_A_URL, "Sega Nomad", EPISODE_TIME)
        };
        let mut episode = guidless(TOKEN_B_URL, "Sega Nomad", EPISODE_TIME);
        episode.pub_date = None;

        let plan = plan_for_rotated_token(stored, episode);

        assert_eq!(plan.to_download.len(), 1);
    }

    fn feed_guids(guids: &[&str]) -> HashSet<String> {
        guids.iter().map(|guid| guid.to_string()).collect()
    }

    fn stored_nomad() -> StoredEpisode {
        StoredEpisode {
            pub_date: make_time(EPISODE_TIME),
            ..stored("2024-12-19-Sega Nomad", "Sega Nomad", "old-guid")
        }
    }

    #[test]
    fn stored_episode_is_replaced_by_entry_taking_its_place() {
        let reissued =
            make_episode_with_date("Sega Nomad", Some("new-guid"), make_time(EPISODE_TIME));

        assert!(is_replaced_by(
            &stored_nomad(),
            &reissued,
            &feed_guids(&["new-guid"])
        ));
    }

    #[test]
    fn stored_episode_still_in_feed_is_not_replaced() {
        let reissued =
            make_episode_with_date("Sega Nomad", Some("new-guid"), make_time(EPISODE_TIME));

        assert!(!is_replaced_by(
            &stored_nomad(),
            &reissued,
            &feed_guids(&["new-guid", "old-guid"])
        ));
    }

    #[test]
    fn stored_episode_is_not_replaced_by_entry_still_naming_an_additional_guid() {
        let stored = StoredEpisode {
            additional_guids: vec!["alias-guid".to_string()],
            ..stored_nomad()
        };
        let reissued =
            make_episode_with_date("Sega Nomad", Some("new-guid"), make_time(EPISODE_TIME));

        assert!(!is_replaced_by(
            &stored,
            &reissued,
            &feed_guids(&["new-guid", "alias-guid"])
        ));
    }

    #[test]
    fn stored_episode_is_not_replaced_by_entry_with_other_title_or_time() {
        let retitled = make_episode_with_date("Best of", Some("new-guid"), make_time(EPISODE_TIME));
        let moved = make_episode_with_date(
            "Sega Nomad",
            Some("new-guid"),
            make_time("Fri, 20 Dec 2024 10:25:22 +0000"),
        );
        let guids = feed_guids(&["new-guid"]);

        assert!(!is_replaced_by(&stored_nomad(), &retitled, &guids));
        assert!(!is_replaced_by(&stored_nomad(), &moved, &guids));
    }

    #[test]
    fn stored_episode_without_publication_time_is_never_replaced() {
        let stored = StoredEpisode {
            pub_date: None,
            ..stored_nomad()
        };
        let undated = make_episode_with_date("Sega Nomad", Some("new-guid"), None);

        assert!(!is_replaced_by(
            &stored,
            &undated,
            &feed_guids(&["new-guid"])
        ));
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
        assert!(state.claimed_keys().contains("episode3"));
        // Partial files claim no name
        assert!(!state.claimed_keys().contains("episode1.mp3"));
        assert!(!state.claimed_keys().contains("episode2.mp3"));
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

        let plan = create_sync_plan(episodes, &state, None);

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

        let plan = create_sync_plan(episodes, &state, None);

        // Episode with date should be first, undated ones at the end
        assert_eq!(plan.to_download.len(), 3);
        assert_eq!(plan.to_download[0].episode.title, "With Date");
        // Undated episodes preserve relative order
        assert_eq!(plan.to_download[1].episode.title, "No Date 1");
        assert_eq!(plan.to_download[2].episode.title, "No Date 2");
    }
}
