// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::collections::HashSet;
use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::{FutureExt, StreamExt};

use url::Url;

use crate::episode::{DownloadContext, get_audio_extension, hash_file, stage_download};
use crate::error::{FeedError, SyncError};
use crate::feed::{
    Podcast, fetch_feed_bytes, file_path_to_url, is_url, parse_feed, read_feed_file,
};
use crate::http::HttpClient;
use crate::metadata::{
    add_guid_to_episode_metadata, stage_episode_metadata, write_podcast_metadata,
};
use crate::progress::{ProgressEvent, SharedProgressReporter};
use crate::state::{
    CheckTarget, OutputState, PlannedDownload, StoredEpisode, archive_check_targets,
    create_sync_plan, scan_output_dir,
};

/// Options for podcast synchronization
///
/// New options can be added without breaking callers, so outside this crate
/// options start from [`Default`] and set the fields they need:
///
/// ```
/// let mut options = podpull::SyncOptions::default();
/// options.limit = Some(10);
/// ```
///
/// A struct literal is rejected, as it would break with every new field:
///
/// ```compile_fail,E0639
/// let options = podpull::SyncOptions {
///     limit: None,
///     max_concurrent: 3,
///     continue_on_error: true,
///     audio_check: podpull::AudioCheck::Collisions,
/// };
/// ```
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct SyncOptions {
    /// Maximum number of episodes to download (None = all)
    pub limit: Option<usize>,
    /// Maximum number of concurrent downloads
    pub max_concurrent: usize,
    /// Continue downloading if individual episodes fail
    pub continue_on_error: bool,
    /// Which stored audio to check against its recorded hash, and whether
    /// to download mismatched episodes again
    pub audio_check: AudioCheck,
}

/// Which stored audio a sync checks against the hash recorded when it was
/// downloaded
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum AudioCheck {
    /// Only audio a new episode's base filename collides with, which is
    /// where podpull 1.1.2 and earlier could leave damage; mismatches are
    /// reported
    #[default]
    Collisions,
    /// All stored audio, reading the whole archive; mismatches are reported
    Verify,
    /// All stored audio; mismatched episodes still in the feed are
    /// downloaded again under their existing names
    Repair,
}

impl Default for SyncOptions {
    fn default() -> Self {
        Self {
            limit: None,
            max_concurrent: 3,
            continue_on_error: true,
            audio_check: AudioCheck::Collisions,
        }
    }
}

/// Result of a sync operation
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct SyncResult {
    /// Number of episodes successfully downloaded
    pub downloaded: usize,
    /// Number of episodes skipped (already present)
    pub skipped: usize,
    /// Number of episodes that failed to download
    pub failed: usize,
    /// Details of failed episodes (title, error message)
    pub failed_episodes: Vec<(String, String)>,
    /// Number of episodes not started because an earlier download failed
    /// and `continue_on_error` is off
    pub not_started: usize,
    /// Stored audio found not to match its recorded hash and left as it is
    pub damaged: Vec<DamagedAudio>,
    /// Number of new episodes whose audio was already stored byte for byte,
    /// so only their GUID was recorded
    pub adopted: usize,
}

/// What happens with stored audio that no longer matches its recorded hash
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DamageRemedy {
    /// The episode is downloaded again under its existing names
    Repairing,
    /// The episode is still in the feed, so a run with
    /// [`AudioCheck::Repair`] downloads it again
    RepairAvailable,
    /// No episode in the feed matches the stored one, so it cannot be
    /// downloaded again
    NoFeedEpisode,
    /// The feed offers the episode in another audio format than the stored
    /// file has, so downloading it into that name would mislabel it
    EnclosureFormatChanged,
}

/// Stored audio that no longer matches its recorded hash and was left as
/// it is
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DamagedAudio {
    pub episode_title: String,
    pub audio_filename: String,
    /// Never [`DamageRemedy::Repairing`]: repaired audio is not damaged
    pub remedy: DamageRemedy,
}

impl DamagedAudio {
    pub fn new(
        episode_title: impl Into<String>,
        audio_filename: impl Into<String>,
        remedy: DamageRemedy,
    ) -> Self {
        Self {
            episode_title: episode_title.into(),
            audio_filename: audio_filename.into(),
            remedy,
        }
    }
}

/// Synchronize a podcast feed to a local directory
///
/// This is the main entry point for the library. It:
/// 1. Fetches and parses the feed
/// 2. Scans the output directory for existing downloads
/// 3. Creates a sync plan
/// 4. Downloads new episodes in parallel
/// 5. Writes metadata files
pub async fn sync_podcast<C: HttpClient>(
    client: &C,
    feed_source: &str,
    output_dir: &Path,
    options: &SyncOptions,
    reporter: SharedProgressReporter,
) -> Result<SyncResult, SyncError> {
    let podcast = load_podcast(client, feed_source, &reporter).await?;

    // Scan output directory (also cleans up any partial files from interrupted downloads)
    // Progress is reported from within scan_output_dir
    let state = scan_output_dir(output_dir, &reporter)?;

    // Report if any partial files were cleaned up
    if state.partial_files_cleaned() > 0 {
        reporter.report(ProgressEvent::PartialFilesCleanedUp {
            count: state.partial_files_cleaned(),
        });
    }

    for path in state.stuck_partial_files() {
        reporter.report(ProgressEvent::PartialFileStuck { path: path.clone() });
    }

    for unreadable in state.unreadable_metadata() {
        reporter.report(ProgressEvent::MetadataUnreadable {
            path: unreadable.path.clone(),
            error: unreadable.error.clone(),
        });
    }

    // Create sync plan (episodes are sorted by pub_date, newest first)
    let plan = create_sync_plan(podcast.episodes.clone(), &state, options.limit);
    let limited = plan.new_episodes - plan.to_download.len();

    let targets = match options.audio_check {
        AudioCheck::Collisions => plan.collisions.clone(),
        AudioCheck::Verify | AudioCheck::Repair => archive_check_targets(&state, &plan),
    };
    let verification = verify_stored_audio(
        &targets,
        state.output_dir(),
        options.audio_check == AudioCheck::Repair,
        &reporter,
    )
    .await;
    let repairs = verification.repairs.len();

    reporter.report(ProgressEvent::SyncPlanReady {
        podcast_title: podcast.title.clone(),
        total_episodes: plan.total_episodes,
        new_episodes: plan.new_episodes,
        to_download: plan.to_download.len(),
        repairs,
    });

    // An episode whose audio is repaired or found damaged no longer counts
    // as existing. A repair replaces an episode that is already present
    // rather than adding a new one, so the limit does not apply to it.
    let existing = plan
        .already_present
        .len()
        .saturating_sub(verification.affected_present_guids.len());
    let mut to_download = verification.repairs;
    to_download.extend(plan.to_download);

    // Write podcast metadata
    write_podcast_metadata(&podcast, output_dir)?;

    let totals = download_all(
        client,
        to_download,
        &state,
        &plan.feed_guids,
        &reporter,
        options,
    )
    .await;
    let downloaded = totals.downloaded;
    let failed_eps = totals.failed_episodes;
    let failed = failed_eps.len();

    reporter.report(ProgressEvent::SyncCompleted {
        downloaded_count: downloaded,
        existing_count: existing,
        limited_count: limited,
        failed_count: failed,
        not_started_count: totals.not_started,
        damaged_count: verification.damaged.len(),
        adopted_count: totals.adopted,
    });

    if downloaded == 0 && failed > 0 && !options.continue_on_error {
        return Err(SyncError::AllDownloadsFailed);
    }

    Ok(SyncResult {
        downloaded,
        skipped: existing,
        failed,
        failed_episodes: failed_eps,
        not_started: totals.not_started,
        damaged: verification.damaged,
        adopted: totals.adopted,
    })
}

/// Fetch or read the feed and parse it, reporting each phase
async fn load_podcast<C: HttpClient>(
    client: &C,
    feed_source: &str,
    reporter: &SharedProgressReporter,
) -> Result<Podcast, SyncError> {
    if is_url(feed_source) {
        reporter.report(ProgressEvent::FetchingFeed {
            url: feed_source.to_string(),
        });
        let bytes = fetch_feed_bytes(client, feed_source).await?;

        reporter.report(ProgressEvent::ParsingFeed {
            source: feed_source.to_string(),
        });
        let feed_url =
            Url::parse(feed_source).map_err(|e| SyncError::Feed(FeedError::InvalidUrl(e)))?;
        Ok(parse_feed(&bytes, feed_url)?)
    } else {
        // A local file has no fetching phase.
        reporter.report(ProgressEvent::ParsingFeed {
            source: feed_source.to_string(),
        });
        let bytes = read_feed_file(Path::new(feed_source))?;
        let feed_url = file_path_to_url(Path::new(feed_source));
        Ok(parse_feed(&bytes, feed_url)?)
    }
}

/// What became of one planned download
enum DownloadOutcome {
    Downloaded,
    /// The audio was already stored, so only its GUID was recorded
    AlreadyStored,
    Failed {
        error: String,
    },
    /// Skipped because an earlier failure stopped the run
    NotStarted,
}

/// Results of all downloads of one sync run
struct DownloadTotals {
    downloaded: usize,
    /// Failed downloads as (episode title, error message) pairs
    failed_episodes: Vec<(String, String)>,
    /// Episodes skipped because a failure stopped the run
    not_started: usize,
    /// Episodes whose audio was already stored byte for byte
    adopted: usize,
}

/// A download slot ID taken from the free list, returned when dropped
///
/// Slot IDs give each running download a stable progress bar. At most as
/// many downloads run as there are slots, so a free one is always available
/// when a download starts.
struct SlotGuard<'a> {
    free_slots: &'a Mutex<Vec<usize>>,
    download_id: usize,
}

impl<'a> SlotGuard<'a> {
    fn take(free_slots: &'a Mutex<Vec<usize>>) -> Self {
        let download_id = free_slots
            .lock()
            .expect("no code panics while holding the slot list")
            .pop()
            .expect("no more downloads run than there are slots");
        Self {
            free_slots,
            download_id,
        }
    }
}

impl Drop for SlotGuard<'_> {
    fn drop(&mut self) {
        self.free_slots
            .lock()
            .expect("no code panics while holding the slot list")
            .push(self.download_id);
    }
}

/// Download all planned episodes concurrently and write their metadata
///
/// Downloads run as concurrent futures on the calling task, at most
/// `max_concurrent` at a time and started in plan order. Keeping them off
/// spawned tasks lets the client be borrowed. A download that panics, for
/// example in a reporter, counts as a failure of its episode instead of
/// taking the whole sync down.
async fn download_all<C: HttpClient>(
    client: &C,
    to_download: Vec<PlannedDownload>,
    state: &OutputState,
    feed_guids: &HashSet<String>,
    reporter: &SharedProgressReporter,
    options: &SyncOptions,
) -> DownloadTotals {
    let total_to_download = to_download.len();
    let free_slots = Mutex::new((0..options.max_concurrent).rev().collect::<Vec<_>>());

    // Without continue_on_error, the first failure stops further downloads
    // from starting; downloads already running finish. A download starts
    // only once a running one has completed, so it sees every failure that
    // made room for it.
    let stop = AtomicBool::new(false);

    let outcomes: Vec<(String, DownloadOutcome)> =
        futures::stream::iter(to_download.into_iter().enumerate())
            .map(|(episode_index, planned)| {
                let free_slots = &free_slots;
                let stop = &stop;
                async move {
                    let title = planned.episode.title.clone();
                    if stop.load(Ordering::SeqCst) {
                        return (title, DownloadOutcome::NotStarted);
                    }

                    let slot = SlotGuard::take(free_slots);
                    let context = DownloadContext {
                        download_id: slot.download_id,
                        episode_index,
                        total_to_download,
                    };
                    let attempt = AssertUnwindSafe(download_planned(
                        client, &planned, state, feed_guids, &context, reporter,
                    ))
                    .catch_unwind()
                    .await;

                    let error = match attempt {
                        Ok(Ok(Placed::Downloaded)) => return (title, DownloadOutcome::Downloaded),
                        Ok(Ok(Placed::AlreadyStored)) => {
                            return (title, DownloadOutcome::AlreadyStored);
                        }
                        Ok(Err(error)) => {
                            reporter.report(ProgressEvent::DownloadFailed {
                                download_id: slot.download_id,
                                episode_title: title.clone(),
                                error: error.clone(),
                            });
                            error
                        }
                        Err(panic) => format!("Download panicked: {}", panic_message(&*panic)),
                    };
                    if !options.continue_on_error {
                        stop.store(true, Ordering::SeqCst);
                    }
                    (title, DownloadOutcome::Failed { error })
                }
            })
            .buffer_unordered(options.max_concurrent)
            .collect()
            .await;

    let mut totals = DownloadTotals {
        downloaded: 0,
        failed_episodes: Vec::new(),
        not_started: 0,
        adopted: 0,
    };
    for (title, outcome) in outcomes {
        match outcome {
            DownloadOutcome::Downloaded => totals.downloaded += 1,
            DownloadOutcome::AlreadyStored => totals.adopted += 1,
            DownloadOutcome::Failed { error } => totals.failed_episodes.push((title, error)),
            DownloadOutcome::NotStarted => totals.not_started += 1,
        }
    }
    totals
}

/// The message a panic was raised with, if it carried one
fn panic_message(panic: &(dyn std::any::Any + Send)) -> &str {
    panic
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("unknown cause")
}

/// Download one planned episode and put its audio and metadata in place
///
/// The metadata is written before the audio takes its final name. An
/// interruption before that point leaves only partial files, which the next
/// directory scan removes; after it, only the two renames remain. Audio left
/// without metadata would claim its name and make the next sync store the
/// episode a second time under another one.
async fn download_planned<C: HttpClient>(
    client: &C,
    planned: &PlannedDownload,
    state: &OutputState,
    feed_guids: &HashSet<String>,
    context: &DownloadContext,
    reporter: &SharedProgressReporter,
) -> Result<Placed, String> {
    let episode = &planned.episode;
    let output_dir = state.output_dir();
    let audio_filename = planned.audio_filename();
    let audio_path = output_dir.join(&audio_filename);
    let metadata_path = output_dir.join(planned.metadata_filename());

    let staged_audio = stage_download(client, episode, &audio_path, context, reporter)
        .await
        .map_err(|e| e.to_string())?;

    // An entry re-issued under a new GUID with audio stored byte for byte
    // adds that GUID to the stored episode instead of a copy. Every other
    // entry is an episode of its own, even with the same audio. A repair is
    // excluded: it is meant to replace its file.
    if !planned.replaces_existing
        && let Some(guid) = &episode.guid
        && let Some(stored) = state.stored_episode_with_content_hash(staged_audio.content_hash())
        && stored.is_replaced_by(episode, feed_guids)
        && stored_audio_still_matches(output_dir, stored, staged_audio.content_hash()).await
    {
        staged_audio.discard().await;
        add_guid_to_episode_metadata(&output_dir.join(&stored.metadata_filename), guid)
            .map_err(|e| format!("Failed to record the GUID of identical audio: {}", e))?;
        reporter.report(ProgressEvent::EpisodeAlreadyStored {
            download_id: context.download_id,
            episode_title: episode.title.clone(),
            audio_filename: stored.audio_filename.clone(),
        });
        return Ok(Placed::AlreadyStored);
    }

    let staged_metadata = match stage_episode_metadata(
        episode,
        &audio_filename,
        Some(staged_audio.content_hash().to_string()),
        &metadata_path,
    ) {
        Ok(staged_metadata) => staged_metadata,
        Err(e) => {
            staged_audio.discard().await;
            return Err(format!("Failed to write metadata: {}", e));
        }
    };

    reporter.report(ProgressEvent::Finalizing {
        download_id: context.download_id,
        episode_title: episode.title.clone(),
    });

    if let Err(e) = staged_audio.finalize().await {
        staged_metadata.discard();
        return Err(e.to_string());
    }

    if let Err(e) = staged_metadata.commit() {
        let mut error = format!("Failed to write metadata: {}", e);
        // A fresh download owns its name, so its audio goes again. A repair
        // replaced audio that the existing metadata still names; removing it
        // would leave that metadata without audio.
        if !planned.replaces_existing
            && let Err(remove_error) = tokio::fs::remove_file(&audio_path).await
        {
            error.push_str(&format!(
                "; the audio file {} could not be removed: {}",
                audio_filename, remove_error
            ));
        }
        return Err(error);
    }

    reporter.report(ProgressEvent::DownloadCompleted {
        download_id: context.download_id,
        episode_title: episode.title.clone(),
        bytes_downloaded: staged_audio.bytes_downloaded(),
    });
    Ok(Placed::Downloaded)
}

/// Where a successful download ended up
enum Placed {
    Downloaded,
    /// Identical to stored audio, which now also carries the episode's GUID
    AlreadyStored,
}

/// Whether the stored audio still holds the bytes its metadata recorded
///
/// The recorded hash describes the file as downloaded. A file changed since
/// must not take over a new download's identity, or the intact copy would
/// be thrown away.
async fn stored_audio_still_matches(
    output_dir: &Path,
    stored: &StoredEpisode,
    content_hash: &str,
) -> bool {
    let audio_path = output_dir.join(&stored.audio_filename);
    let actual_hash = tokio::task::spawn_blocking(move || hash_file(&audio_path))
        .await
        .expect("hashing a file does not panic");
    actual_hash.is_ok_and(|hash| hash == content_hash)
}

/// Outcome of checking stored audio against its recorded hashes
struct Verification {
    damaged: Vec<DamagedAudio>,
    /// Downloads that replace mismatched audio of episodes still in the feed
    repairs: Vec<PlannedDownload>,
    /// GUIDs of episodes in the feed whose stored audio was repaired or
    /// found damaged
    affected_present_guids: HashSet<String>,
}

/// Check stored audio against the hashes recorded when it was downloaded
///
/// A file that no longer matches may hold bytes of two episodes, as podpull
/// 1.1.2 and earlier downloaded episodes sharing a filename into one file at
/// the same time, or may have decayed on disk. Tags edited by the user
/// cause a mismatch as well, so a mismatch is only repaired when `repair`
/// is set: the episode is then downloaded again under its existing names,
/// which requires it to still be in the feed in the same audio format.
async fn verify_stored_audio(
    targets: &[CheckTarget],
    output_dir: &Path,
    repair: bool,
    reporter: &SharedProgressReporter,
) -> Verification {
    let mut verification = Verification {
        damaged: Vec::new(),
        repairs: Vec::new(),
        affected_present_guids: HashSet::new(),
    };

    for target in targets {
        let stored = &target.stored;
        let Some(recorded_hash) = &stored.content_hash else {
            continue;
        };

        // Hashing reads the whole file, which takes a while on a network
        // share.
        reporter.report(ProgressEvent::VerifyingStoredAudio {
            audio_filename: stored.audio_filename.clone(),
        });
        let audio_path = output_dir.join(&stored.audio_filename);
        let actual_hash = tokio::task::spawn_blocking(move || hash_file(&audio_path))
            .await
            .expect("hashing a file does not panic");

        let actual_hash = match actual_hash {
            Ok(hash) => hash,
            // A missing file has nothing left to verify.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                reporter.report(ProgressEvent::StoredAudioUnverifiable {
                    audio_filename: stored.audio_filename.clone(),
                    error: e.to_string(),
                });
                continue;
            }
        };
        if &actual_hash == recorded_hash {
            continue;
        }

        let stored_extension = Path::new(&stored.audio_filename)
            .extension()
            .map(|ext| ext.to_string_lossy().into_owned())
            .unwrap_or_default();
        let remedy = match &target.feed_episode {
            None => DamageRemedy::NoFeedEpisode,
            Some(episode)
                if !get_audio_extension(episode).eq_ignore_ascii_case(&stored_extension) =>
            {
                DamageRemedy::EnclosureFormatChanged
            }
            Some(_) if repair => DamageRemedy::Repairing,
            Some(_) => DamageRemedy::RepairAvailable,
        };
        reporter.report(ProgressEvent::StoredAudioMismatch {
            episode_title: stored.title.clone(),
            audio_filename: stored.audio_filename.clone(),
            remedy,
        });

        if let Some(guid) = target
            .feed_episode
            .as_ref()
            .and_then(|episode| episode.guid.clone())
        {
            verification.affected_present_guids.insert(guid);
        }

        match (&target.feed_episode, remedy) {
            (Some(episode), DamageRemedy::Repairing) => {
                verification.repairs.push(PlannedDownload {
                    episode: episode.clone(),
                    stem: stored
                        .metadata_filename
                        .trim_end_matches(".json")
                        .to_string(),
                    // The stored spelling keeps the download on the very
                    // file it replaces.
                    audio_extension: stored_extension,
                    replaces_existing: true,
                })
            }
            _ => verification.damaged.push(DamagedAudio {
                episode_title: stored.title.clone(),
                audio_filename: stored.audio_filename.clone(),
                remedy,
            }),
        }
    }

    verification
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::write_episode_metadata;
    use std::path::PathBuf;
    use std::sync::Arc;

    use crate::http::{ByteStream, HttpResponse};
    use crate::progress::{NoopReporter, ProgressReporter};
    use async_trait::async_trait;
    use bytes::Bytes;
    use tempfile::tempdir;

    /// Collects every reported event so tests can assert on warnings
    #[derive(Default)]
    struct RecordingReporter {
        events: std::sync::Mutex<Vec<ProgressEvent>>,
    }

    impl ProgressReporter for RecordingReporter {
        fn report(&self, event: ProgressEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    impl RecordingReporter {
        fn events(&self) -> Vec<ProgressEvent> {
            self.events.lock().unwrap().clone()
        }
    }

    #[derive(Clone)]
    struct MockHttpClient {
        feed_xml: String,
        audio_data: Vec<u8>,
    }

    #[async_trait]
    impl HttpClient for MockHttpClient {
        async fn get_bytes(&self, url: &str) -> Result<Bytes, reqwest::Error> {
            if url.ends_with(".xml") || url.contains("feed") {
                Ok(Bytes::from(self.feed_xml.clone()))
            } else {
                Ok(Bytes::from(self.audio_data.clone()))
            }
        }

        async fn get_stream(&self, _url: &str) -> Result<HttpResponse, reqwest::Error> {
            let data = self.audio_data.clone();
            let len = data.len() as u64;

            let stream: ByteStream =
                Box::pin(futures::stream::once(async move { Ok(Bytes::from(data)) }));

            Ok(HttpResponse {
                status: 200,
                content_length: Some(len),
                body: stream,
            })
        }
    }

    const SAMPLE_FEED: &str = r#"<?xml version="1.0"?>
<rss version="2.0">
  <channel>
    <title>Test Podcast</title>
    <description>A test podcast</description>
    <item>
      <title>Episode 1</title>
      <guid>ep1-guid</guid>
      <enclosure url="https://example.com/ep1.mp3" type="audio/mpeg"/>
    </item>
    <item>
      <title>Episode 2</title>
      <guid>ep2-guid</guid>
      <enclosure url="https://example.com/ep2.mp3" type="audio/mpeg"/>
    </item>
  </channel>
</rss>"#;

    #[tokio::test]
    async fn sync_downloads_all_episodes() {
        let dir = tempdir().unwrap();

        let client = MockHttpClient {
            feed_xml: SAMPLE_FEED.to_string(),
            audio_data: b"fake audio".to_vec(),
        };

        let result = sync_podcast(
            &client,
            "https://example.com/feed.xml",
            dir.path(),
            &SyncOptions::default(),
            NoopReporter::shared(),
        )
        .await
        .unwrap();

        assert_eq!(result.downloaded, 2);
        assert_eq!(result.skipped, 0);
        assert_eq!(result.failed, 0);

        // Check files exist
        assert!(dir.path().join("podcast.json").exists());
    }

    #[tokio::test]
    async fn sync_respects_limit() {
        let dir = tempdir().unwrap();

        let client = MockHttpClient {
            feed_xml: SAMPLE_FEED.to_string(),
            audio_data: b"fake audio".to_vec(),
        };

        let options = SyncOptions {
            limit: Some(1),
            ..Default::default()
        };

        let result = sync_podcast(
            &client,
            "https://example.com/feed.xml",
            dir.path(),
            &options,
            NoopReporter::shared(),
        )
        .await
        .unwrap();

        assert_eq!(result.downloaded, 1);
    }

    #[tokio::test]
    async fn sync_skips_existing_episodes() {
        let dir = tempdir().unwrap();

        let client = MockHttpClient {
            feed_xml: SAMPLE_FEED.to_string(),
            audio_data: b"fake audio".to_vec(),
        };

        // First sync
        sync_podcast(
            &client,
            "https://example.com/feed.xml",
            dir.path(),
            &SyncOptions::default(),
            NoopReporter::shared(),
        )
        .await
        .unwrap();

        // Second sync should skip all
        let result = sync_podcast(
            &client,
            "https://example.com/feed.xml",
            dir.path(),
            &SyncOptions::default(),
            NoopReporter::shared(),
        )
        .await
        .unwrap();

        assert_eq!(result.downloaded, 0);
        assert_eq!(result.skipped, 2);
    }

    #[tokio::test]
    async fn sync_reports_unreadable_episode_metadata() {
        let dir = tempdir().unwrap();
        let truncated = dir.path().join("2024-01-15-Episode 9.json");
        std::fs::write(&truncated, b"{\"title\": \"Epis").unwrap();

        let client = MockHttpClient {
            feed_xml: SAMPLE_FEED.to_string(),
            audio_data: b"fake audio".to_vec(),
        };
        let reporter = Arc::new(RecordingReporter::default());

        sync_podcast(
            &client,
            "https://example.com/feed.xml",
            dir.path(),
            &SyncOptions::default(),
            reporter.clone(),
        )
        .await
        .unwrap();

        let reported: Vec<_> = reporter
            .events()
            .into_iter()
            .filter_map(|event| match event {
                ProgressEvent::MetadataUnreadable { path, .. } => Some(path),
                _ => None,
            })
            .collect();
        assert_eq!(reported, vec![truncated]);
    }

    // =========================================================
    // Filename collisions
    // =========================================================

    /// One item of a generated test feed
    struct FeedItem {
        title: &'static str,
        pub_date: &'static str,
        guid: &'static str,
    }

    // Two distinct episodes (different GUIDs and enclosures) sharing title and
    // publication day, as found in a real feed where the publisher duplicated
    // an entry. Both map to the same base filename.
    const NOMAD_REUPLOAD: FeedItem = FeedItem {
        title: "SFT Bits: Sega Nomad",
        pub_date: "Thu, 19 Dec 2024 10:45:35 GMT",
        guid: "nomad-reupload",
    };
    const NOMAD_ORIGINAL: FeedItem = FeedItem {
        title: "SFT Bits: Sega Nomad",
        pub_date: "Thu, 19 Dec 2024 10:25:22 GMT",
        guid: "nomad-original",
    };

    fn feed_xml(items: &[FeedItem]) -> String {
        let items: String = items
            .iter()
            .map(|item| {
                format!(
                    r#"
    <item>
      <title>{}</title>
      <pubDate>{}</pubDate>
      <guid>{}</guid>
      <enclosure url="https://example.com/{}.mp3" type="audio/mpeg"/>
    </item>"#,
                    item.title, item.pub_date, item.guid, item.guid
                )
            })
            .collect();

        format!(
            r#"<?xml version="1.0"?>
<rss version="2.0">
  <channel>
    <title>Test Podcast</title>
    <description>A test podcast</description>{}
  </channel>
</rss>"#,
            items
        )
    }

    /// Serves different audio for every enclosure URL, as distinct
    /// episodes have, so identical-audio detection does not merge them
    struct DistinctAudioClient(MockHttpClient);

    #[async_trait]
    impl HttpClient for DistinctAudioClient {
        async fn get_bytes(&self, url: &str) -> Result<Bytes, reqwest::Error> {
            self.0.get_bytes(url).await
        }

        async fn get_stream(&self, url: &str) -> Result<HttpResponse, reqwest::Error> {
            let data = Bytes::from(format!("audio of {}", url));
            let len = data.len() as u64;
            let stream: ByteStream = Box::pin(futures::stream::once(async move { Ok(data) }));
            Ok(HttpResponse {
                status: 200,
                content_length: Some(len),
                body: stream,
            })
        }
    }

    fn client_for(items: &[FeedItem]) -> MockHttpClient {
        MockHttpClient {
            feed_xml: feed_xml(items),
            audio_data: b"fake audio".to_vec(),
        }
    }

    async fn sync_with<C: HttpClient>(dir: &Path, client: &C, options: &SyncOptions) -> SyncResult {
        sync_podcast(
            client,
            "https://example.com/feed.xml",
            dir,
            options,
            NoopReporter::shared(),
        )
        .await
        .unwrap()
    }

    /// Store an episode the way an earlier sync run would have left it:
    /// audio and metadata under `stem`, with the hash of `audio` recorded.
    fn store_episode(dir: &Path, stem: &str, item: &FeedItem, audio: &[u8]) {
        store_episode_as(dir, stem, "mp3", item, audio);
    }

    /// Like [`store_episode`], with the audio under extension `ext`
    fn store_episode_as(dir: &Path, stem: &str, ext: &str, item: &FeedItem, audio: &[u8]) {
        use sha2::{Digest, Sha256};

        let episode = crate::feed::Episode {
            title: item.title.to_string(),
            description: None,
            pub_date: chrono::DateTime::parse_from_rfc2822(item.pub_date).ok(),
            guid: Some(item.guid.to_string()),
            enclosure: crate::feed::Enclosure {
                url: url::Url::parse(&format!("https://example.com/{}.mp3", item.guid)).unwrap(),
                length: None,
                mime_type: None,
            },
            duration: None,
            episode_number: None,
            season_number: None,
        };
        let audio_filename = format!("{}.{}", stem, ext);
        std::fs::write(dir.join(&audio_filename), audio).unwrap();
        write_episode_metadata(
            &episode,
            &audio_filename,
            Some(format!("sha256:{:x}", Sha256::digest(audio))),
            &dir.join(format!("{}.json", stem)),
        )
        .unwrap();
    }

    /// Map every GUID recorded in the readable episode metadata of `dir` to
    /// its audio filename, asserting that the audio file exists.
    fn recorded_episodes(dir: &Path) -> std::collections::HashMap<String, String> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.extension().is_some_and(|ext| ext == "json")
                    && path.file_name().is_some_and(|name| name != "podcast.json")
            })
            .filter_map(|path| crate::metadata::read_episode_metadata(&path).ok())
            .map(|metadata| {
                assert!(dir.join(&metadata.audio_filename).exists());
                (metadata.guid.unwrap(), metadata.audio_filename)
            })
            .collect()
    }

    #[tokio::test]
    async fn sync_keeps_colliding_episodes_apart_within_one_run() {
        let dir = tempdir().unwrap();
        let client = client_for(&[NOMAD_REUPLOAD, NOMAD_ORIGINAL]);

        let result = sync_with(dir.path(), &client, &SyncOptions::default()).await;

        assert_eq!(result.downloaded, 2);
        assert_eq!(result.failed, 0);
        let recorded = recorded_episodes(dir.path());
        assert_eq!(
            recorded["nomad-reupload"],
            "2024-12-19-SFT Bits Sega Nomad.mp3"
        );
        assert_eq!(
            recorded["nomad-original"],
            "2024-12-19-102522-SFT Bits Sega Nomad.mp3"
        );

        // A second run recognises both episodes instead of fetching whichever
        // one lost the shared filename.
        let result = sync_with(dir.path(), &client, &SyncOptions::default()).await;

        assert_eq!(result.downloaded, 0);
        assert_eq!(result.skipped, 2);
    }

    #[tokio::test]
    async fn sync_does_not_overwrite_colliding_episode_from_earlier_run() {
        let dir = tempdir().unwrap();
        let client = DistinctAudioClient(client_for(&[NOMAD_REUPLOAD, NOMAD_ORIGINAL]));

        // The first run only fetches the newer episode, so the older one
        // meets an already occupied filename on the second run.
        let limited = SyncOptions {
            limit: Some(1),
            ..Default::default()
        };
        sync_with(dir.path(), &client, &limited).await;
        let result = sync_with(dir.path(), &client, &SyncOptions::default()).await;

        assert_eq!(result.downloaded, 1);
        assert_eq!(result.skipped, 1);
        let recorded = recorded_episodes(dir.path());
        assert_eq!(
            recorded["nomad-reupload"],
            "2024-12-19-SFT Bits Sega Nomad.mp3"
        );
        assert_eq!(
            recorded["nomad-original"],
            "2024-12-19-102522-SFT Bits Sega Nomad.mp3"
        );
    }

    #[tokio::test]
    async fn sync_respects_names_stored_in_decomposed_unicode() {
        let dir = tempdir().unwrap();
        let earlier = FeedItem {
            title: "Neuzugänge #4",
            pub_date: "Fri, 27 Dec 2019 10:12:15 GMT",
            guid: "neuzugaenge-earlier",
        };
        let later = FeedItem {
            title: "Neuzugänge #4",
            pub_date: "Fri, 27 Dec 2019 09:29:26 GMT",
            guid: "neuzugaenge-later",
        };

        // Network shares mounted from macOS list "ä" decomposed into "a" and
        // a combining diaeresis, while feed titles use the composed form.
        let decomposed_stem = "2019-12-27-Neuzuga\u{0308}nge #4";
        store_episode(dir.path(), decomposed_stem, &earlier, b"earlier audio");

        let client = client_for(&[earlier, later]);
        let result = sync_with(dir.path(), &client, &SyncOptions::default()).await;

        assert_eq!(result.downloaded, 1);
        assert_eq!(
            std::fs::read(dir.path().join(format!("{}.mp3", decomposed_stem))).unwrap(),
            b"earlier audio"
        );
        assert_eq!(
            recorded_episodes(dir.path())["neuzugaenge-later"],
            "2019-12-27-092926-Neuzugänge #4.mp3"
        );
    }

    #[tokio::test]
    async fn sync_treats_names_differing_only_in_case_as_colliding() {
        let dir = tempdir().unwrap();
        let lowercase = FeedItem {
            title: "SFT Bits: Sega nomad",
            pub_date: "Thu, 19 Dec 2024 10:45:35 GMT",
            guid: "nomad-lowercase",
        };
        store_episode(
            dir.path(),
            "2024-12-19-SFT Bits Sega nomad",
            &lowercase,
            b"lowercase audio",
        );

        let client = client_for(&[lowercase, NOMAD_ORIGINAL]);
        let result = sync_with(dir.path(), &client, &SyncOptions::default()).await;

        assert_eq!(result.downloaded, 1);
        assert_eq!(
            std::fs::read(dir.path().join("2024-12-19-SFT Bits Sega nomad.mp3")).unwrap(),
            b"lowercase audio"
        );
        assert_eq!(
            recorded_episodes(dir.path())["nomad-original"],
            "2024-12-19-102522-SFT Bits Sega Nomad.mp3"
        );
    }

    #[tokio::test]
    async fn sync_does_not_overwrite_audio_without_metadata() {
        let dir = tempdir().unwrap();
        let orphan = dir.path().join("2024-12-19-SFT Bits Sega Nomad.mp3");
        std::fs::write(&orphan, b"audio of an unknown episode").unwrap();

        let client = client_for(&[NOMAD_ORIGINAL]);
        let result = sync_with(dir.path(), &client, &SyncOptions::default()).await;

        assert_eq!(result.downloaded, 1);
        assert_eq!(
            std::fs::read(&orphan).unwrap(),
            b"audio of an unknown episode"
        );
        assert_eq!(
            recorded_episodes(dir.path())["nomad-original"],
            "2024-12-19-102522-SFT Bits Sega Nomad.mp3"
        );
    }

    #[tokio::test]
    async fn sync_does_not_overwrite_episode_with_unreadable_metadata() {
        let dir = tempdir().unwrap();
        let audio = dir.path().join("2024-12-19-SFT Bits Sega Nomad.mp3");
        std::fs::write(&audio, b"audio of an unknown episode").unwrap();
        std::fs::write(
            dir.path().join("2024-12-19-SFT Bits Sega Nomad.json"),
            b"{\"title\": \"SFT",
        )
        .unwrap();

        let client = client_for(&[NOMAD_ORIGINAL]);
        let result = sync_with(dir.path(), &client, &SyncOptions::default()).await;

        assert_eq!(result.downloaded, 1);
        assert_eq!(
            std::fs::read(&audio).unwrap(),
            b"audio of an unknown episode"
        );
        assert_eq!(
            recorded_episodes(dir.path())["nomad-original"],
            "2024-12-19-102522-SFT Bits Sega Nomad.mp3"
        );
    }

    #[tokio::test]
    async fn sync_keeps_earlier_download_when_feed_changes_guid() {
        let dir = tempdir().unwrap();
        let before = FeedItem {
            guid: "nomad-before-migration",
            ..NOMAD_ORIGINAL
        };
        let after = FeedItem {
            guid: "nomad-after-migration",
            ..NOMAD_ORIGINAL
        };
        store_episode(
            dir.path(),
            "2024-12-19-SFT Bits Sega Nomad",
            &before,
            b"audio before migration",
        );

        // Title and publication time alone cannot prove that the new GUID
        // names the same episode, so the earlier file must survive.
        let result = sync_with(dir.path(), &client_for(&[after]), &SyncOptions::default()).await;

        assert_eq!(result.downloaded, 1);
        assert_eq!(
            std::fs::read(dir.path().join("2024-12-19-SFT Bits Sega Nomad.mp3")).unwrap(),
            b"audio before migration"
        );
        let recorded = recorded_episodes(dir.path());
        assert_eq!(
            recorded["nomad-before-migration"],
            "2024-12-19-SFT Bits Sega Nomad.mp3"
        );
        assert_eq!(
            recorded["nomad-after-migration"],
            "2024-12-19-102522-SFT Bits Sega Nomad.mp3"
        );
    }

    // =========================================================
    // Verification of audio a collision points at
    // =========================================================

    const NOMAD_STEM: &str = "2024-12-19-SFT Bits Sega Nomad";

    async fn sync_recording(dir: &Path, items: &[FeedItem]) -> (SyncResult, Vec<ProgressEvent>) {
        let reporter = Arc::new(RecordingReporter::default());
        let result = sync_podcast(
            &client_for(items),
            "https://example.com/feed.xml",
            dir,
            &SyncOptions::default(),
            reporter.clone(),
        )
        .await
        .unwrap();
        (result, reporter.events())
    }

    fn mismatch_events(events: &[ProgressEvent]) -> Vec<(String, String, DamageRemedy)> {
        events
            .iter()
            .filter_map(|event| match event {
                ProgressEvent::StoredAudioMismatch {
                    episode_title,
                    audio_filename,
                    remedy,
                } => Some((episode_title.clone(), audio_filename.clone(), *remedy)),
                _ => None,
            })
            .collect()
    }

    fn verifying_events(events: &[ProgressEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|event| match event {
                ProgressEvent::VerifyingStoredAudio { audio_filename } => {
                    Some(audio_filename.clone())
                }
                _ => None,
            })
            .collect()
    }

    fn damage(remedy: DamageRemedy) -> DamagedAudio {
        DamagedAudio {
            episode_title: "SFT Bits: Sega Nomad".to_string(),
            audio_filename: format!("{}.mp3", NOMAD_STEM),
            remedy,
        }
    }

    /// Store NOMAD_REUPLOAD under the base name with audio that no longer
    /// matches its recorded hash
    fn store_damaged_nomad(dir: &Path) -> PathBuf {
        store_episode(dir, NOMAD_STEM, &NOMAD_REUPLOAD, b"clean audio");
        let audio = dir.join(format!("{}.mp3", NOMAD_STEM));
        // Two concurrent downloads into one file, as podpull 1.1.2 and
        // earlier did for colliding episodes, leave bytes that match the
        // hash recorded by neither download.
        std::fs::write(&audio, b"interleaved audio").unwrap();
        audio
    }

    #[tokio::test]
    async fn sync_reports_damaged_audio_of_colliding_episode() {
        let dir = tempdir().unwrap();
        let audio = store_damaged_nomad(dir.path());

        let (result, events) = sync_recording(dir.path(), &[NOMAD_REUPLOAD, NOMAD_ORIGINAL]).await;

        assert_eq!(result.downloaded, 1);
        assert_eq!(result.failed, 0);
        assert_eq!(result.skipped, 0);
        assert_eq!(result.damaged, vec![damage(DamageRemedy::RepairAvailable)]);
        assert_eq!(
            mismatch_events(&events),
            vec![(
                "SFT Bits: Sega Nomad".to_string(),
                format!("{}.mp3", NOMAD_STEM),
                DamageRemedy::RepairAvailable
            )]
        );
        // Without --repair the file stays: it may hold edits made by the user.
        assert_eq!(std::fs::read(&audio).unwrap(), b"interleaved audio");
    }

    #[tokio::test]
    async fn sync_accepts_intact_audio_of_colliding_episode() {
        let dir = tempdir().unwrap();
        store_episode(dir.path(), NOMAD_STEM, &NOMAD_REUPLOAD, b"clean audio");

        let (result, events) = sync_recording(dir.path(), &[NOMAD_REUPLOAD, NOMAD_ORIGINAL]).await;

        assert!(result.damaged.is_empty());
        assert!(mismatch_events(&events).is_empty());
        assert_eq!(
            verifying_events(&events),
            vec![format!("{}.mp3", NOMAD_STEM)]
        );
    }

    #[tokio::test]
    async fn sync_verifies_audio_once_when_several_episodes_collide_with_it() {
        let dir = tempdir().unwrap();
        store_damaged_nomad(dir.path());
        let third = FeedItem {
            pub_date: "Thu, 19 Dec 2024 09:00:00 GMT",
            guid: "nomad-third",
            ..NOMAD_ORIGINAL
        };

        let (result, events) =
            sync_recording(dir.path(), &[NOMAD_REUPLOAD, NOMAD_ORIGINAL, third]).await;

        assert_eq!(result.downloaded, 2);
        assert_eq!(result.damaged.len(), 1);
        assert_eq!(verifying_events(&events).len(), 1);
    }

    #[tokio::test]
    async fn sync_names_no_remedy_for_damaged_audio_without_guid() {
        let dir = tempdir().unwrap();
        store_damaged_nomad(dir.path());
        let metadata_path = dir.path().join(format!("{}.json", NOMAD_STEM));
        let mut metadata = crate::metadata::read_episode_metadata(&metadata_path).unwrap();
        metadata.guid = None;
        std::fs::write(&metadata_path, serde_json::to_string(&metadata).unwrap()).unwrap();

        let (result, _) = sync_recording(dir.path(), &[NOMAD_ORIGINAL]).await;

        assert_eq!(result.damaged, vec![damage(DamageRemedy::NoFeedEpisode)]);
    }

    #[tokio::test]
    async fn sync_skips_verification_without_recorded_hash() {
        let dir = tempdir().unwrap();
        store_episode(dir.path(), NOMAD_STEM, &NOMAD_REUPLOAD, b"clean audio");
        let metadata_path = dir.path().join(format!("{}.json", NOMAD_STEM));
        let mut metadata = crate::metadata::read_episode_metadata(&metadata_path).unwrap();
        metadata.content_hash = None;
        std::fs::write(&metadata_path, serde_json::to_string(&metadata).unwrap()).unwrap();
        std::fs::write(dir.path().join(format!("{}.mp3", NOMAD_STEM)), b"edited").unwrap();

        let (result, events) = sync_recording(dir.path(), &[NOMAD_REUPLOAD, NOMAD_ORIGINAL]).await;

        assert!(result.damaged.is_empty());
        assert!(verifying_events(&events).is_empty());
    }

    #[tokio::test]
    async fn sync_skips_verification_when_audio_is_missing() {
        let dir = tempdir().unwrap();
        store_episode(dir.path(), NOMAD_STEM, &NOMAD_REUPLOAD, b"clean audio");
        std::fs::remove_file(dir.path().join(format!("{}.mp3", NOMAD_STEM))).unwrap();

        let (result, events) = sync_recording(dir.path(), &[NOMAD_REUPLOAD, NOMAD_ORIGINAL]).await;

        assert!(result.damaged.is_empty());
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, ProgressEvent::StoredAudioUnverifiable { .. }))
        );
    }

    #[tokio::test]
    async fn sync_warns_when_audio_cannot_be_read_for_verification() {
        let dir = tempdir().unwrap();
        store_episode(dir.path(), NOMAD_STEM, &NOMAD_REUPLOAD, b"clean audio");
        let audio = dir.path().join(format!("{}.mp3", NOMAD_STEM));
        std::fs::remove_file(&audio).unwrap();
        std::fs::create_dir(&audio).unwrap();

        let (result, events) = sync_recording(dir.path(), &[NOMAD_REUPLOAD, NOMAD_ORIGINAL]).await;

        // The check could not run, which says nothing about the file.
        assert!(result.damaged.is_empty());
        assert!(events.iter().any(|event| matches!(
            event,
            ProgressEvent::StoredAudioUnverifiable { audio_filename, .. }
                if *audio_filename == format!("{}.mp3", NOMAD_STEM)
        )));
    }

    // =========================================================
    // Failures during the download loop
    // =========================================================

    #[tokio::test]
    async fn sync_counts_failed_metadata_write_as_failure() {
        let dir = tempdir().unwrap();
        // The directory scan cannot remove a directory, so it keeps blocking
        // the partial file the metadata is written through.
        std::fs::create_dir(dir.path().join(format!("{}.json.partial", NOMAD_STEM))).unwrap();

        let client = client_for(&[NOMAD_ORIGINAL]);
        let result = sync_with(dir.path(), &client, &SyncOptions::default()).await;

        assert_eq!(result.downloaded, 0);
        assert_eq!(result.failed, 1);
        assert_eq!(result.failed_episodes[0].0, "SFT Bits: Sega Nomad");
        // Audio without metadata would claim its name and make the next sync
        // store the episode a second time under another one.
        assert!(!dir.path().join(format!("{}.mp3", NOMAD_STEM)).exists());
        assert!(
            !dir.path()
                .join(format!("{}.mp3.partial", NOMAD_STEM))
                .exists()
        );
    }

    #[tokio::test]
    async fn sync_reports_partial_files_it_cannot_remove() {
        let dir = tempdir().unwrap();
        let stuck = dir.path().join(format!("{}.mp3.partial", NOMAD_STEM));
        std::fs::create_dir(&stuck).unwrap();

        let (result, events) = sync_recording(dir.path(), &[NOMAD_ORIGINAL]).await;

        assert_eq!(result.failed, 1);
        assert!(result.failed_episodes[0].1.contains("already exists"));
        assert!(events.iter().any(|event| matches!(
            event,
            ProgressEvent::PartialFileStuck { path } if *path == stuck
        )));
    }

    #[tokio::test]
    async fn sync_counts_occupied_partial_file_as_failure() {
        let dir = tempdir().unwrap();
        std::fs::create_dir(dir.path().join(format!("{}.mp3.partial", NOMAD_STEM))).unwrap();

        let client = client_for(&[NOMAD_ORIGINAL]);
        let result = sync_with(dir.path(), &client, &SyncOptions::default()).await;

        assert_eq!(result.downloaded, 0);
        assert_eq!(result.failed, 1);
        assert!(!dir.path().join(format!("{}.mp3", NOMAD_STEM)).exists());
    }

    #[tokio::test]
    async fn sync_fails_when_every_download_fails_without_continue_on_error() {
        let dir = tempdir().unwrap();
        std::fs::create_dir(dir.path().join(format!("{}.mp3.partial", NOMAD_STEM))).unwrap();

        let result = sync_podcast(
            &client_for(&[NOMAD_ORIGINAL]),
            "https://example.com/feed.xml",
            dir.path(),
            &SyncOptions {
                continue_on_error: false,
                ..Default::default()
            },
            NoopReporter::shared(),
        )
        .await;

        assert!(matches!(result, Err(SyncError::AllDownloadsFailed)));
    }

    // =========================================================
    // Opt-in repair of damaged audio
    // =========================================================

    async fn sync_repairing(
        dir: &Path,
        items: &[FeedItem],
        limit: Option<usize>,
    ) -> (SyncResult, Vec<ProgressEvent>) {
        let reporter = Arc::new(RecordingReporter::default());
        let options = SyncOptions {
            limit,
            audio_check: AudioCheck::Repair,
            ..Default::default()
        };
        let result = sync_podcast(
            &client_for(items),
            "https://example.com/feed.xml",
            dir,
            &options,
            reporter.clone(),
        )
        .await
        .unwrap();
        (result, reporter.events())
    }

    #[tokio::test]
    async fn sync_repairs_damaged_audio_in_place_when_enabled() {
        let dir = tempdir().unwrap();
        let audio = store_damaged_nomad(dir.path());

        let (result, events) =
            sync_repairing(dir.path(), &[NOMAD_REUPLOAD, NOMAD_ORIGINAL], None).await;

        assert_eq!(result.downloaded, 2);
        assert_eq!(result.skipped, 0);
        assert_eq!(result.failed, 0);
        assert!(result.damaged.is_empty());
        assert_eq!(std::fs::read(&audio).unwrap(), b"fake audio");
        let metadata = crate::metadata::read_episode_metadata(
            &dir.path().join(format!("{}.json", NOMAD_STEM)),
        )
        .unwrap();
        assert_eq!(metadata.guid.as_deref(), Some("nomad-reupload"));
        assert_eq!(metadata.content_hash, hash_file(&audio).ok());
        assert_eq!(
            mismatch_events(&events),
            vec![(
                "SFT Bits: Sega Nomad".to_string(),
                format!("{}.mp3", NOMAD_STEM),
                DamageRemedy::Repairing
            )]
        );
    }

    #[tokio::test]
    async fn sync_repairs_damaged_audio_beyond_the_limit() {
        let dir = tempdir().unwrap();
        store_damaged_nomad(dir.path());
        let older = FeedItem {
            title: "Older Episode",
            pub_date: "Mon, 01 Jan 2024 12:00:00 GMT",
            guid: "older",
        };

        // The limit admits only the newest new episode; the repair of the
        // episode it collides with comes on top.
        let (result, events) = sync_repairing(
            dir.path(),
            &[NOMAD_REUPLOAD, NOMAD_ORIGINAL, older],
            Some(1),
        )
        .await;

        assert_eq!(result.downloaded, 2);
        assert_eq!(result.failed, 0);
        assert!(!recorded_episodes(dir.path()).contains_key("older"));
        // The repair is reported apart from the new episodes, so the limit
        // stays visible.
        assert!(events.iter().any(|event| matches!(
            event,
            ProgressEvent::SyncPlanReady {
                new_episodes: 2,
                to_download: 1,
                repairs: 1,
                ..
            }
        )));
    }

    #[tokio::test]
    async fn sync_reports_damage_it_cannot_repair() {
        let dir = tempdir().unwrap();
        // The damaged episode has left the feed, so there is nothing to
        // download it from.
        let audio = store_damaged_nomad(dir.path());

        let (result, _) = sync_repairing(dir.path(), &[NOMAD_ORIGINAL], None).await;

        assert_eq!(result.downloaded, 1);
        assert_eq!(result.failed, 0);
        assert_eq!(result.damaged, vec![damage(DamageRemedy::NoFeedEpisode)]);
        assert_eq!(std::fs::read(&audio).unwrap(), b"interleaved audio");
    }

    #[tokio::test]
    async fn sync_does_not_repair_audio_whose_feed_format_changed() {
        let dir = tempdir().unwrap();
        store_episode_as(
            dir.path(),
            NOMAD_STEM,
            "m4a",
            &NOMAD_REUPLOAD,
            b"clean audio",
        );
        let audio = dir.path().join(format!("{}.m4a", NOMAD_STEM));
        std::fs::write(&audio, b"interleaved audio").unwrap();

        // The feed now offers the episode as mp3; downloading it into the
        // m4a name would mislabel the audio.
        let (result, _) = sync_repairing(dir.path(), &[NOMAD_REUPLOAD, NOMAD_ORIGINAL], None).await;

        assert_eq!(
            result.damaged,
            vec![DamagedAudio {
                audio_filename: format!("{}.m4a", NOMAD_STEM),
                ..damage(DamageRemedy::EnclosureFormatChanged)
            }]
        );
        assert_eq!(std::fs::read(&audio).unwrap(), b"interleaved audio");
    }

    #[tokio::test]
    async fn sync_repairs_audio_whose_extension_differs_only_in_case() {
        let dir = tempdir().unwrap();
        store_episode_as(
            dir.path(),
            NOMAD_STEM,
            "MP3",
            &NOMAD_REUPLOAD,
            b"clean audio",
        );
        let audio = dir.path().join(format!("{}.MP3", NOMAD_STEM));
        std::fs::write(&audio, b"interleaved audio").unwrap();

        let (result, _) = sync_repairing(dir.path(), &[NOMAD_REUPLOAD, NOMAD_ORIGINAL], None).await;

        assert!(result.damaged.is_empty());
        assert_eq!(std::fs::read(&audio).unwrap(), b"fake audio");
    }

    #[tokio::test]
    async fn sync_repairs_every_damaged_copy_of_an_episode() {
        let dir = tempdir().unwrap();
        // A copy of the episode under another name, with its metadata
        // unchanged, as a user might make by hand.
        let copy_stem = "2024-12-19-SFT Bits Sega Nomad Copy";
        store_damaged_nomad(dir.path());
        store_episode(dir.path(), copy_stem, &NOMAD_REUPLOAD, b"clean audio");
        std::fs::write(dir.path().join(format!("{}.mp3", copy_stem)), b"damaged").unwrap();
        let copy_collider = FeedItem {
            title: "SFT Bits: Sega Nomad Copy",
            pub_date: "Thu, 19 Dec 2024 08:00:00 GMT",
            guid: "nomad-copy-collider",
        };

        let (result, _) = sync_repairing(
            dir.path(),
            &[NOMAD_REUPLOAD, NOMAD_ORIGINAL, copy_collider],
            None,
        )
        .await;

        assert_eq!(result.downloaded, 4);
        assert_eq!(result.skipped, 0);
        assert!(result.damaged.is_empty());
    }

    /// Panics while a download reports its completion, as a faulty
    /// reporter implementation would
    struct PanickingReporter;

    impl ProgressReporter for PanickingReporter {
        fn report(&self, event: ProgressEvent) {
            if matches!(event, ProgressEvent::DownloadCompleted { .. }) {
                panic!("reporter failure");
            }
        }
    }

    #[tokio::test]
    async fn sync_counts_panicking_downloads_as_failed() {
        let dir = tempdir().unwrap();
        let client = MockHttpClient {
            feed_xml: SAMPLE_FEED.to_string(),
            audio_data: b"fake audio".to_vec(),
        };
        let options = SyncOptions {
            max_concurrent: 1,
            ..Default::default()
        };

        // With a single slot, a download that never hands its slot back
        // would block the second episode forever.
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            sync_podcast(
                &client,
                "https://example.com/feed.xml",
                dir.path(),
                &options,
                Arc::new(PanickingReporter),
            ),
        )
        .await
        .expect("sync finishes although its downloads panic")
        .unwrap();

        assert_eq!(result.downloaded, 0);
        assert_eq!(result.failed, 2);
        let mut titles: Vec<_> = result
            .failed_episodes
            .iter()
            .map(|(title, _)| title.as_str())
            .collect();
        titles.sort();
        assert_eq!(titles, vec!["Episode 1", "Episode 2"]);
    }

    #[tokio::test]
    async fn sync_stops_starting_downloads_after_a_failure_without_continue_on_error() {
        let dir = tempdir().unwrap();
        let items = [
            FeedItem {
                title: "Newest",
                pub_date: "Wed, 03 Jan 2024 12:00:00 GMT",
                guid: "newest",
            },
            FeedItem {
                title: "Middle",
                pub_date: "Tue, 02 Jan 2024 12:00:00 GMT",
                guid: "middle",
            },
            FeedItem {
                title: "Oldest",
                pub_date: "Mon, 01 Jan 2024 12:00:00 GMT",
                guid: "oldest",
            },
        ];
        // The directory scan cannot remove a directory, so it blocks the
        // partial file of the middle episode.
        std::fs::create_dir(dir.path().join("2024-01-02-Middle.mp3.partial")).unwrap();
        let reporter = Arc::new(RecordingReporter::default());

        let result = sync_podcast(
            &client_for(&items),
            "https://example.com/feed.xml",
            dir.path(),
            &SyncOptions {
                max_concurrent: 1,
                continue_on_error: false,
                ..Default::default()
            },
            reporter.clone(),
        )
        .await
        .unwrap();

        assert_eq!(result.downloaded, 1);
        assert_eq!(result.failed, 1);
        assert_eq!(result.not_started, 1);
        assert!(!dir.path().join("2024-01-01-Oldest.mp3").exists());
        assert!(reporter.events().iter().any(|event| matches!(
            event,
            ProgressEvent::SyncCompleted {
                not_started_count: 1,
                ..
            }
        )));
    }

    // =========================================================
    // Finalizing a single download
    // =========================================================

    fn planned_nomad(replaces_existing: bool) -> PlannedDownload {
        let feed = crate::feed::parse_feed(
            feed_xml(&[NOMAD_ORIGINAL]).as_bytes(),
            url::Url::parse("https://example.com/feed.xml").unwrap(),
        )
        .unwrap();
        let planned = PlannedDownload::new(feed.episodes[0].clone(), NOMAD_STEM, "mp3");
        PlannedDownload {
            replaces_existing,
            ..planned
        }
    }

    async fn download_one(dir: &Path, planned: &PlannedDownload) -> Result<(), String> {
        let context = DownloadContext {
            download_id: 0,
            episode_index: 0,
            total_to_download: 1,
        };
        download_planned(
            &client_for(&[NOMAD_ORIGINAL]),
            planned,
            &OutputState::empty(dir),
            &HashSet::new(),
            &context,
            &NoopReporter::shared(),
        )
        .await
        .map(|_| ())
    }

    /// Make renaming onto the metadata path fail: a non-empty directory
    /// cannot be replaced by a file.
    fn block_metadata_path(dir: &Path) {
        let metadata_path = dir.join(format!("{}.json", NOMAD_STEM));
        std::fs::create_dir(&metadata_path).unwrap();
        std::fs::write(metadata_path.join("occupant"), b"").unwrap();
    }

    #[tokio::test]
    async fn download_planned_writes_audio_and_metadata() {
        let dir = tempdir().unwrap();

        download_one(dir.path(), &planned_nomad(false))
            .await
            .unwrap();

        assert_eq!(
            recorded_episodes(dir.path())["nomad-original"],
            format!("{}.mp3", NOMAD_STEM)
        );
    }

    #[tokio::test]
    async fn download_planned_removes_fresh_audio_when_metadata_cannot_be_placed() {
        let dir = tempdir().unwrap();
        block_metadata_path(dir.path());

        let error = download_one(dir.path(), &planned_nomad(false))
            .await
            .unwrap_err();

        assert!(error.starts_with("Failed to write metadata"));
        assert!(!dir.path().join(format!("{}.mp3", NOMAD_STEM)).exists());
        assert!(
            !dir.path()
                .join(format!("{}.json.partial", NOMAD_STEM))
                .exists()
        );
    }

    #[tokio::test]
    async fn download_planned_keeps_repaired_audio_when_metadata_cannot_be_placed() {
        let dir = tempdir().unwrap();
        block_metadata_path(dir.path());

        // A repair replaced audio that the existing metadata still names, so
        // removing it would leave that metadata without its audio.
        let error = download_one(dir.path(), &planned_nomad(true))
            .await
            .unwrap_err();

        assert!(error.starts_with("Failed to write metadata"));
        assert_eq!(
            std::fs::read(dir.path().join(format!("{}.mp3", NOMAD_STEM))).unwrap(),
            b"fake audio"
        );
    }

    #[tokio::test]
    async fn download_planned_discards_metadata_when_audio_cannot_be_placed() {
        let dir = tempdir().unwrap();
        let audio_path = dir.path().join(format!("{}.mp3", NOMAD_STEM));
        std::fs::create_dir(&audio_path).unwrap();
        std::fs::write(audio_path.join("occupant"), b"").unwrap();

        download_one(dir.path(), &planned_nomad(false))
            .await
            .unwrap_err();

        assert!(!dir.path().join(format!("{}.json", NOMAD_STEM)).exists());
        assert!(
            !dir.path()
                .join(format!("{}.json.partial", NOMAD_STEM))
                .exists()
        );
    }

    // =========================================================
    // Checking the whole archive
    // =========================================================

    async fn sync_checking(
        dir: &Path,
        items: &[FeedItem],
        audio_check: AudioCheck,
    ) -> (SyncResult, Vec<ProgressEvent>) {
        let reporter = Arc::new(RecordingReporter::default());
        let result = sync_podcast(
            &client_for(items),
            "https://example.com/feed.xml",
            dir,
            &SyncOptions {
                audio_check,
                ..Default::default()
            },
            reporter.clone(),
        )
        .await
        .unwrap();
        (result, reporter.events())
    }

    #[tokio::test]
    async fn sync_checks_only_colliding_audio_by_default() {
        let dir = tempdir().unwrap();
        store_damaged_nomad(dir.path());

        let (result, events) =
            sync_checking(dir.path(), &[NOMAD_REUPLOAD], AudioCheck::Collisions).await;

        assert!(result.damaged.is_empty());
        assert!(verifying_events(&events).is_empty());
    }

    #[tokio::test]
    async fn sync_verify_reports_damaged_audio_without_a_collision() {
        let dir = tempdir().unwrap();
        let audio = store_damaged_nomad(dir.path());

        let (result, events) =
            sync_checking(dir.path(), &[NOMAD_REUPLOAD], AudioCheck::Verify).await;

        assert_eq!(result.downloaded, 0);
        assert_eq!(result.damaged, vec![damage(DamageRemedy::RepairAvailable)]);
        assert_eq!(
            verifying_events(&events),
            vec![format!("{}.mp3", NOMAD_STEM)]
        );
        assert_eq!(std::fs::read(&audio).unwrap(), b"interleaved audio");
    }

    #[tokio::test]
    async fn sync_repair_fixes_damaged_audio_without_a_collision() {
        let dir = tempdir().unwrap();
        let audio = store_damaged_nomad(dir.path());

        let (result, _) = sync_checking(dir.path(), &[NOMAD_REUPLOAD], AudioCheck::Repair).await;

        assert_eq!(result.downloaded, 1);
        assert!(result.damaged.is_empty());
        assert_eq!(std::fs::read(&audio).unwrap(), b"fake audio");
    }

    #[tokio::test]
    async fn sync_repair_fixes_damage_reported_by_an_earlier_run() {
        let dir = tempdir().unwrap();
        let audio = store_damaged_nomad(dir.path());
        let feed = [NOMAD_REUPLOAD, NOMAD_ORIGINAL];

        // The first run downloads the colliding episode and reports the
        // damage; afterwards nothing collides any more.
        let (first, _) = sync_checking(dir.path(), &feed, AudioCheck::Collisions).await;
        assert_eq!(first.damaged.len(), 1);

        let (second, _) = sync_checking(dir.path(), &feed, AudioCheck::Repair).await;

        assert_eq!(second.downloaded, 1);
        assert!(second.damaged.is_empty());
        assert_eq!(std::fs::read(&audio).unwrap(), b"fake audio");
    }

    #[tokio::test]
    async fn sync_does_not_store_guidless_episode_again_after_url_token_change() {
        let dir = tempdir().unwrap();
        let feed = |token: &str| {
            format!(
                r#"<?xml version="1.0"?>
<rss version="2.0">
  <channel>
    <title>Test Podcast</title>
    <description>A test podcast</description>
    <item>
      <title>Members Only</title>
      <pubDate>Thu, 19 Dec 2024 10:25:22 GMT</pubDate>
      <enclosure url="https://example.com/media/{}/episode-uuid.mp3" type="audio/mpeg"/>
    </item>
  </channel>
</rss>"#,
                token
            )
        };
        let sync_feed = |feed_xml: String| {
            let dir = dir.path().to_path_buf();
            async move {
                let client = MockHttpClient {
                    feed_xml,
                    audio_data: b"fake audio".to_vec(),
                };
                sync_with(&dir, &client, &SyncOptions::default()).await
            }
        };

        sync_feed(feed("token-a")).await;
        let result = sync_feed(feed("token-b")).await;

        assert_eq!(result.downloaded, 0);
        assert_eq!(result.skipped, 1);
        let audio_files = std::fs::read_dir(dir.path())
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .path()
                    .extension()
                    .is_some_and(|ext| ext == "mp3")
            })
            .count();
        assert_eq!(audio_files, 1);
    }

    /// A client that borrows its responses, so it is neither Clone nor
    /// 'static
    struct BorrowingClient<'a> {
        inner: &'a MockHttpClient,
    }

    #[async_trait]
    impl HttpClient for BorrowingClient<'_> {
        async fn get_bytes(&self, url: &str) -> Result<Bytes, reqwest::Error> {
            self.inner.get_bytes(url).await
        }

        async fn get_stream(&self, url: &str) -> Result<HttpResponse, reqwest::Error> {
            self.inner.get_stream(url).await
        }
    }

    #[tokio::test]
    async fn sync_accepts_a_client_that_is_neither_clone_nor_static() {
        let dir = tempdir().unwrap();
        let mock = MockHttpClient {
            feed_xml: SAMPLE_FEED.to_string(),
            audio_data: b"fake audio".to_vec(),
        };

        let result = sync_podcast(
            &BorrowingClient { inner: &mock },
            "https://example.com/feed.xml",
            dir.path(),
            &SyncOptions::default(),
            NoopReporter::shared(),
        )
        .await
        .unwrap();

        assert_eq!(result.downloaded, 2);
    }

    #[test]
    fn sync_future_can_be_spawned() {
        fn assert_send<T: Send>(_: &T) {}
        let mock = MockHttpClient {
            feed_xml: SAMPLE_FEED.to_string(),
            audio_data: Vec::new(),
        };

        let options = SyncOptions::default();

        // Library users run a sync on a spawned task, which needs a Send
        // future; the future is only built here, never polled.
        let sync = sync_podcast(
            &mock,
            "https://example.com/feed.xml",
            Path::new("/nonexistent"),
            &options,
            NoopReporter::shared(),
        );
        assert_send(&sync);
    }

    // =========================================================
    // Re-issued episodes with identical audio
    // =========================================================

    const REISSUED: FeedItem = FeedItem {
        guid: "nomad-reissued",
        ..NOMAD_ORIGINAL
    };

    fn audio_files(dir: &Path) -> Vec<String> {
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".mp3"))
            .collect();
        files.sort();
        files
    }

    #[tokio::test]
    async fn sync_records_new_guid_for_identical_audio_instead_of_a_copy() {
        let dir = tempdir().unwrap();
        // The mock serves "fake audio" for every enclosure, so the re-issued
        // episode downloads byte for byte what is stored.
        store_episode(dir.path(), NOMAD_STEM, &NOMAD_ORIGINAL, b"fake audio");

        let (result, events) = sync_recording(dir.path(), &[REISSUED]).await;

        assert_eq!(result.downloaded, 0);
        assert_eq!(result.adopted, 1);
        assert_eq!(audio_files(dir.path()), vec![format!("{}.mp3", NOMAD_STEM)]);
        let metadata = crate::metadata::read_episode_metadata(
            &dir.path().join(format!("{}.json", NOMAD_STEM)),
        )
        .unwrap();
        assert_eq!(metadata.guid.as_deref(), Some("nomad-original"));
        assert_eq!(
            metadata.additional_guids,
            vec!["nomad-reissued".to_string()]
        );
        assert!(events.iter().any(|event| matches!(
            event,
            ProgressEvent::EpisodeAlreadyStored { audio_filename, .. }
                if *audio_filename == format!("{}.mp3", NOMAD_STEM)
        )));

        let (result, _) = sync_recording(dir.path(), &[REISSUED]).await;

        assert_eq!(result.downloaded, 0);
        assert_eq!(result.adopted, 0);
        assert_eq!(result.skipped, 1);
    }

    #[tokio::test]
    async fn sync_keeps_identical_audio_of_two_listed_entries_apart() {
        let dir = tempdir().unwrap();
        store_episode(dir.path(), NOMAD_STEM, &NOMAD_ORIGINAL, b"fake audio");

        // Both entries are in the feed at the same time, so they are two
        // episodes even though their audio is the same.
        let (first, _) = sync_recording(dir.path(), &[NOMAD_ORIGINAL, REISSUED]).await;
        let (second, _) = sync_recording(dir.path(), &[NOMAD_ORIGINAL, REISSUED]).await;

        assert_eq!(first.downloaded, 1);
        assert_eq!(first.adopted, 0);
        assert_eq!(audio_files(dir.path()).len(), 2);
        assert_eq!(second.downloaded, 0);
        assert_eq!(second.skipped, 2);
    }

    #[tokio::test]
    async fn sync_keeps_identical_audio_of_a_retitled_entry_apart() {
        let dir = tempdir().unwrap();
        store_episode(dir.path(), NOMAD_STEM, &NOMAD_ORIGINAL, b"fake audio");
        let rerun = FeedItem {
            title: "Best of: Sega Nomad",
            ..REISSUED
        };

        let (result, _) = sync_recording(dir.path(), &[rerun]).await;

        assert_eq!(result.downloaded, 1);
        assert_eq!(result.adopted, 0);
    }

    #[tokio::test]
    async fn sync_keeps_the_download_when_stored_audio_changed_since_its_hash() {
        let dir = tempdir().unwrap();
        // The recorded hash matches the new download, but the file on disk
        // no longer holds those bytes.
        store_episode(dir.path(), NOMAD_STEM, &NOMAD_ORIGINAL, b"fake audio");
        std::fs::write(dir.path().join(format!("{}.mp3", NOMAD_STEM)), b"damaged").unwrap();

        let (result, _) = sync_recording(dir.path(), &[REISSUED]).await;

        assert_eq!(result.adopted, 0);
        assert_eq!(result.downloaded, 1);
        assert_eq!(audio_files(dir.path()).len(), 2);
    }

    #[tokio::test]
    async fn sync_fails_an_identical_download_whose_guid_cannot_be_recorded() {
        let dir = tempdir().unwrap();
        store_episode(dir.path(), NOMAD_STEM, &NOMAD_ORIGINAL, b"fake audio");
        // A directory blocks the partial file the metadata is rewritten
        // through; the scan cannot remove it.
        std::fs::create_dir(dir.path().join(format!("{}.json.partial", NOMAD_STEM))).unwrap();

        let (result, _) = sync_recording(dir.path(), &[REISSUED]).await;

        assert_eq!(result.failed, 1);
        assert_eq!(result.adopted, 0);
        assert_eq!(audio_files(dir.path()), vec![format!("{}.mp3", NOMAD_STEM)]);
    }
}
