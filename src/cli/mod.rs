// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The terminal side of the binary: the progress display and the messages
//! it prints.

pub(crate) mod messages;
pub(crate) mod reporter;

use console::Emoji;

// Emoji with fallback for terminals without Unicode support
pub(crate) static MICROPHONE: Emoji<'_, '_> = Emoji("🎙️  ", "");
pub(crate) static GLOBE: Emoji<'_, '_> = Emoji("🌐 ", "[w] ");
pub(crate) static SCROLL: Emoji<'_, '_> = Emoji("📜 ", "[*] ");
pub(crate) static SEARCH: Emoji<'_, '_> = Emoji("🔍 ", "[~] ");
pub(crate) static HEADPHONES: Emoji<'_, '_> = Emoji("🎧 ", "[i] ");
pub(crate) static SAVING: Emoji<'_, '_> = Emoji("💾 ", "[v] ");
pub(crate) static SUCCESS: Emoji<'_, '_> = Emoji("✅ ", "[+] ");
pub(crate) static FAILURE: Emoji<'_, '_> = Emoji("❌ ", "[!] ");
pub(crate) static PARTY: Emoji<'_, '_> = Emoji("🎉 ", "[*] ");
pub(crate) static FOLDER: Emoji<'_, '_> = Emoji("📁 ", "");
pub(crate) static CROSS: Emoji<'_, '_> = Emoji("✗ ", "x ");
pub(crate) static BROOM: Emoji<'_, '_> = Emoji("🧹 ", "[c] ");
pub(crate) static WARNING: Emoji<'_, '_> = Emoji("⚠️  ", "[!] ");

#[cfg(test)]
use podpull::{DamageKind, DamageRemedy, DamagedAudio, FailedEpisode, SyncResult};

/// A sync result with the given counts, for the tests of the messages and
/// of the exit code
#[cfg(test)]
pub(crate) fn test_result(downloaded: usize, failed: usize, damaged: usize) -> SyncResult {
    SyncResult {
        downloaded,
        failed_episodes: (0..failed)
            .map(|n| FailedEpisode {
                title: format!("Episode {}", n),
                error: "HTTP error 404".to_string(),
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
