// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::collections::HashSet;
use std::path::Path;

use crate::damage::{DamageKind, DamageRemedy, DamagedAudio};
use crate::episode::{get_audio_extension, hash_file};
use crate::progress::{ProgressEvent, SharedProgressReporter};
use crate::state::{CheckTarget, PlannedDownload};

/// Outcome of checking stored audio against its recorded hashes
pub(super) struct Verification {
    pub(super) damaged: Vec<DamagedAudio>,
    /// Downloads that replace mismatched audio of episodes still in the feed
    pub(super) repairs: Vec<PlannedDownload>,
    /// GUIDs of episodes in the feed whose stored audio was repaired or
    /// found damaged
    pub(super) affected_present_guids: HashSet<String>,
    /// Audio filenames of stored audio found to match its recorded hash
    pub(super) intact: HashSet<String>,
    /// Stored audio that could not be read, as (audio filename, error)
    pub(super) unverifiable: Vec<(String, String)>,
}

/// Check stored audio against the hashes recorded when it was downloaded
///
/// A file that no longer matches may hold bytes of two episodes, as podpull
/// 1.1.2 and earlier downloaded episodes sharing a filename into one file at
/// the same time, or may have decayed on disk. Tags edited by the user
/// cause a mismatch as well, so a mismatch is only repaired when `repair`
/// is set: the episode is then downloaded again under its existing names,
/// which requires it to still be in the feed in the same audio format.
pub(super) async fn verify_stored_audio(
    targets: &[CheckTarget],
    output_dir: &Path,
    repair: bool,
    reporter: &SharedProgressReporter,
) -> Verification {
    let mut verification = Verification {
        damaged: Vec::new(),
        repairs: Vec::new(),
        affected_present_guids: HashSet::new(),
        intact: HashSet::new(),
        unverifiable: Vec::new(),
    };

    for target in targets {
        let stored = &target.stored;

        // Missing audio is told by the directory listing. An error reading a
        // listed file, such as a dropped network connection, says nothing
        // about the file, so it is reported as unverifiable instead.
        let kind = if !stored.audio_listed {
            DamageKind::Missing
        } else {
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

            match actual_hash {
                Ok(hash) if &hash == recorded_hash => {
                    verification.intact.insert(stored.audio_filename.clone());
                    continue;
                }
                Ok(_) => DamageKind::Mismatch,
                Err(e) => {
                    reporter.report(ProgressEvent::StoredAudioUnverifiable {
                        audio_filename: stored.audio_filename.clone(),
                        error: e.to_string(),
                    });
                    verification
                        .unverifiable
                        .push((stored.audio_filename.clone(), e.to_string()));
                    continue;
                }
            }
        };

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
        let episode_title = stored.title.clone();
        let audio_filename = stored.audio_filename.clone();
        reporter.report(match kind {
            DamageKind::Missing => ProgressEvent::StoredAudioMissing {
                episode_title,
                audio_filename,
                remedy,
            },
            _ => ProgressEvent::StoredAudioMismatch {
                episode_title,
                audio_filename,
                remedy,
            },
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
                // The repaired episode stays the one all of its GUIDs named,
                // such as a GUID the feed listed it under before.
                let kept_guids = stored
                    .guid
                    .iter()
                    .chain(&stored.additional_guids)
                    .filter(|guid| episode.guid.as_ref() != Some(*guid))
                    .cloned()
                    .collect();
                verification.repairs.push(PlannedDownload {
                    replaces_existing: true,
                    kept_guids,
                    // The stored spelling keeps the download on the very
                    // file it replaces.
                    ..PlannedDownload::new(episode.clone(), stored.stem(), stored_extension)
                })
            }
            _ => verification.damaged.push(DamagedAudio::new(
                stored.title.clone(),
                stored.audio_filename.clone(),
                kind,
                remedy,
            )),
        }
    }

    verification
}
