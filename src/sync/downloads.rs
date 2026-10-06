// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::collections::HashSet;
use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::sync::Mutex;

use futures::{FutureExt, StreamExt};

use super::{FailedEpisode, SyncOptions};
use crate::episode::{DownloadContext, filename_claim_key, hash_file, stage_download};
use crate::http::HttpClient;
use crate::metadata::{EpisodeMetadata, add_guid_to_episode_metadata};
use crate::progress::{ProgressEvent, ProgressReporter};
use crate::state::{OutputState, PlannedDownload, Purpose, StoredEpisode};

/// What became of one planned download
pub(super) enum DownloadOutcome {
    Downloaded,
    /// Damaged audio of a stored episode was downloaded again
    Repaired,
    /// The audio was already stored, so only its GUID was recorded
    AlreadyStored,
    Failed {
        error: String,
    },
}

/// Results of all downloads of one sync run
pub(super) struct DownloadTotals {
    pub(super) downloaded: usize,
    pub(super) repaired: usize,
    pub(super) failed_episodes: Vec<FailedEpisode>,
    /// Episodes whose audio was already stored byte for byte
    pub(super) adopted: usize,
}

/// A download slot ID taken from the free list, returned when dropped
///
/// Slot IDs give each running download a stable progress bar. At most as
/// many downloads run as there are slots, so a free one is always available
/// when a download starts.
pub(super) struct SlotGuard<'a> {
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
pub(super) async fn download_all<C: HttpClient>(
    client: &C,
    to_download: Vec<PlannedDownload>,
    state: &OutputState,
    intact: &HashSet<String>,
    reporter: &dyn ProgressReporter,
    options: &SyncOptions,
) -> DownloadTotals {
    let total_to_download = to_download.len();
    // A limit of zero would never start a download and wait forever; it
    // means one at a time. Slots and the concurrency limit come from the
    // same number, so a starting download always finds a free slot.
    let max_concurrent = options.max_concurrent.max(1);
    let free_slots = Mutex::new((0..max_concurrent).rev().collect::<Vec<_>>());

    let outcomes: Vec<(String, DownloadOutcome)> =
        futures::stream::iter(to_download.into_iter().enumerate())
            .map(|(episode_index, planned)| {
                let free_slots = &free_slots;
                async move {
                    let title = planned.episode.title.clone();
                    let slot = SlotGuard::take(free_slots);
                    let context = DownloadContext {
                        download_id: slot.download_id,
                        episode_index,
                        total_to_download,
                    };
                    let attempt = AssertUnwindSafe(download_planned(
                        client, &planned, state, intact, &context, reporter,
                    ))
                    .catch_unwind()
                    .await;

                    let error = match attempt {
                        Ok(Ok(Placed::Downloaded))
                            if matches!(planned.purpose, Purpose::Repair { .. }) =>
                        {
                            return (title, DownloadOutcome::Repaired);
                        }
                        Ok(Ok(Placed::Downloaded)) => return (title, DownloadOutcome::Downloaded),
                        Ok(Ok(Placed::AlreadyStored)) => {
                            return (title, DownloadOutcome::AlreadyStored);
                        }
                        Ok(Err(error)) => error,
                        Err(panic) => format!("Download panicked: {}", panic_message(&*panic)),
                    };
                    // Every started download ends with an event a reporter
                    // can close its display for this slot on. The reporter
                    // may be what failed, so a second panic is contained.
                    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
                        reporter.report(ProgressEvent::DownloadFailed {
                            download_id: slot.download_id,
                            episode_title: title.clone(),
                            error: error.clone(),
                        })
                    }));
                    (title, DownloadOutcome::Failed { error })
                }
            })
            .buffer_unordered(max_concurrent)
            .collect()
            .await;

    let mut totals = DownloadTotals {
        downloaded: 0,
        repaired: 0,
        failed_episodes: Vec::new(),
        adopted: 0,
    };
    for (title, outcome) in outcomes {
        match outcome {
            DownloadOutcome::Downloaded => totals.downloaded += 1,
            DownloadOutcome::Repaired => totals.repaired += 1,
            DownloadOutcome::AlreadyStored => totals.adopted += 1,
            DownloadOutcome::Failed { error } => {
                totals.failed_episodes.push(FailedEpisode { title, error })
            }
        }
    }
    totals
}

/// The message a panic was raised with, if it carried one
pub(super) fn panic_message(panic: &(dyn std::any::Any + Send)) -> &str {
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
pub(super) async fn download_planned<C: HttpClient>(
    client: &C,
    planned: &PlannedDownload,
    state: &OutputState,
    intact: &HashSet<String>,
    context: &DownloadContext,
    reporter: &dyn ProgressReporter,
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
    // entry is an episode of its own, even with the same audio.
    if let Some(guid) = &episode.guid
        && let Some(stored) =
            replaced_with_identical_audio(output_dir, planned, staged_audio.content_hash(), intact)
                .await
    {
        staged_audio.discard().await;
        let stored_metadata_path = output_dir.join(&stored.metadata_filename);
        let guid = guid.clone();
        blocking(move || add_guid_to_episode_metadata(&stored_metadata_path, &guid))
            .await?
            .map_err(|e| format!("Failed to record the GUID of identical audio: {}", e))?;
        reporter.report(ProgressEvent::EpisodeAlreadyStored {
            download_id: context.download_id,
            episode_title: episode.title.clone(),
            audio_filename: stored.audio_filename.clone(),
        });
        return Ok(Placed::AlreadyStored);
    }

    let staged_metadata = {
        let mut metadata = EpisodeMetadata::from_episode(
            episode,
            &audio_filename,
            Some(staged_audio.content_hash().to_string()),
        );
        if let Purpose::Repair { kept_guids } = &planned.purpose {
            metadata.additional_guids = kept_guids.clone();
        }
        blocking(move || metadata.stage(&metadata_path)).await?
    };
    let staged_metadata = match staged_metadata {
        Ok(staged_metadata) => staged_metadata,
        Err(e) => {
            staged_audio.discard().await;
            return Err(format!("Failed to write metadata: {}", e));
        }
    };

    let bytes_downloaded = match staged_audio.finalize().await {
        Ok(bytes_downloaded) => bytes_downloaded,
        Err(e) => {
            blocking(move || staged_metadata.discard()).await?;
            return Err(e.to_string());
        }
    };

    if let Err(e) = blocking(move || staged_metadata.commit()).await? {
        let mut error = format!("Failed to write metadata: {}", e);
        // A fresh download owns its name, so its audio goes again. A repair
        // replaced audio that the existing metadata still names; removing it
        // would leave that metadata without audio.
        if matches!(planned.purpose, Purpose::NewEpisode { .. })
            && let Err(remove_error) = remove_audio(output_dir, &audio_filename).await
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
        bytes_downloaded,
    });
    Ok(Placed::Downloaded)
}

/// Run blocking file system work outside the task that drives all
/// downloads
///
/// The downloads share one task, so a slow write to a network share done
/// directly in one of them would hold up every other download.
pub(super) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| format!("File system work failed: {}", e))
}

/// Remove an audio file podpull just put in place
///
/// The file is first removed under the name podpull gave it. A network share
/// mounted on macOS can fail to find a name with "ä" composed although it
/// stores exactly that file, so a missing file is looked up under the
/// spelling the directory lists.
pub(super) async fn remove_audio(output_dir: &Path, audio_filename: &str) -> std::io::Result<()> {
    let output_dir = output_dir.to_path_buf();
    let audio_filename = audio_filename.to_string();
    let removal = tokio::task::spawn_blocking(move || {
        match std::fs::remove_file(output_dir.join(&audio_filename)) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                match listed_name(&output_dir, &audio_filename)? {
                    Some(listed) => std::fs::remove_file(output_dir.join(listed)),
                    None => Err(e),
                }
            }
            outcome => outcome,
        }
    });
    removal.await.map_err(std::io::Error::other)?
}

/// The name under which `dir` lists the file podpull calls `filename`,
/// compared by claim key
pub(super) fn listed_name(
    dir: &Path,
    filename: &str,
) -> std::io::Result<Option<std::ffi::OsString>> {
    let key = filename_claim_key(filename);
    for entry in std::fs::read_dir(dir)? {
        let name = entry?.file_name();
        if filename_claim_key(&name.to_string_lossy()) == key {
            return Ok(Some(name));
        }
    }
    Ok(None)
}

/// Where a successful download ended up
pub(super) enum Placed {
    Downloaded,
    /// Identical to stored audio, which now also carries the episode's GUID
    AlreadyStored,
}

/// The first stored episode the planned download may have replaced whose
/// audio is identical to the download's
///
/// Audio in `intact` was found to match its recorded hash earlier in this
/// run and is not read a second time.
pub(super) async fn replaced_with_identical_audio<'a>(
    output_dir: &Path,
    planned: &'a PlannedDownload,
    content_hash: &str,
    intact: &HashSet<String>,
) -> Option<&'a StoredEpisode> {
    let Purpose::NewEpisode {
        replaced_candidates,
    } = &planned.purpose
    else {
        return None;
    };
    for stored in replaced_candidates {
        if stored.content_hash.as_deref() == Some(content_hash)
            && (intact.contains(&stored.audio_filename)
                || stored_audio_still_matches(output_dir, stored, content_hash).await)
        {
            return Some(stored);
        }
    }
    None
}

/// Whether the stored audio still holds the bytes its metadata recorded
///
/// The recorded hash describes the file as downloaded. A file changed since
/// must not take over a new download's identity, or the intact copy would
/// be thrown away.
pub(super) async fn stored_audio_still_matches(
    output_dir: &Path,
    stored: &StoredEpisode,
    content_hash: &str,
) -> bool {
    let audio_path = output_dir.join(&stored.audio_filename);
    let actual_hash = tokio::task::spawn_blocking(move || hash_file(&audio_path, |_, _| {}))
        .await
        .expect("hashing a file does not panic");
    actual_hash.is_ok_and(|hash| hash == content_hash)
}
