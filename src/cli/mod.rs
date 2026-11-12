// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The terminal side of the binary: the progress display and the messages
//! it prints.

pub(crate) mod error_block;
pub(crate) mod failure;
pub(crate) mod messages;
pub(crate) mod reporter;
pub(crate) mod shown;
pub(crate) mod slow_writes;
pub(crate) mod stream_env;
#[cfg(test)]
mod virtual_terminal;

pub(crate) use stream_env::{Mark, Stream, StreamEnv};

#[cfg(test)]
use podpull::{DamageKind, DamageRemedy, DamagedAudio, FailedEpisode, FailureReason, SyncResult};

/// A sync result with the given counts, for the tests of the messages and
/// of the exit code
#[cfg(test)]
pub(crate) fn test_result(downloaded: usize, failed: usize, damaged: usize) -> SyncResult {
    SyncResult {
        downloaded,
        failed_episodes: (0..failed)
            .map(|n| FailedEpisode {
                title: format!("Episode {}", n),
                reason: FailureReason::without_subject("HTTP error 404"),
            })
            .collect(),
        damaged: (0..damaged)
            .map(|n| DamagedAudio {
                episode_title: format!("Damaged {}", n),
                audio_filename: format!("2024-01-0{}-Damaged.mp3", n + 1),
                kind: DamageKind::Mismatch,
                remedy: DamageRemedy::RepairAvailable,
            })
            .collect(),
        ..Default::default()
    }
}
