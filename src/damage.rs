// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

/// What happens with stored audio that is damaged or missing
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DamageRemedy {
    /// The episode is downloaded again under its existing names
    Repairing,
    /// The episode is still in the feed, so a run with
    /// [`AudioCheck::Repair`](crate::AudioCheck::Repair) downloads it again
    RepairAvailable,
    /// No episode in the feed matches the stored one, so it cannot be
    /// downloaded again
    NoFeedEpisode,
    /// The feed offers the episode in another audio format than the stored
    /// file has, so downloading it into that name would mislabel it
    EnclosureFormatChanged,
}

/// How stored audio differs from what its metadata records
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DamageKind {
    /// The audio file no longer matches the hash recorded when it was
    /// downloaded
    Mismatch,
    /// The directory does not list the audio file the metadata names
    Missing,
}

/// Stored audio that is damaged or missing and was left as it is
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DamagedAudio {
    pub episode_title: String,
    pub audio_filename: String,
    pub kind: DamageKind,
    /// Never [`DamageRemedy::Repairing`]: repaired audio is not damaged
    pub remedy: DamageRemedy,
}

impl DamagedAudio {
    pub fn new(
        episode_title: impl Into<String>,
        audio_filename: impl Into<String>,
        kind: DamageKind,
        remedy: DamageRemedy,
    ) -> Self {
        Self {
            episode_title: episode_title.into(),
            audio_filename: audio_filename.into(),
            kind,
            remedy,
        }
    }
}
