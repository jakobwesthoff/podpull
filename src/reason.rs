// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::path::PathBuf;

/// What a failed step was about
///
/// The kind of subject tells a display how to show it: a URL is shown as
/// parsed, a path as a name from the file system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subject {
    Url(String),
    Path(PathBuf),
    None,
}

/// Why a step failed
///
/// `root` is the text of the cause. `subject` is what the failing step was
/// about, kept apart from the text so that each is shown by its own rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailureReason {
    pub root: String,
    pub subject: Subject,
    /// The audio file the failure left behind, when it could not be removed
    pub leftover_audio: Option<LeftoverAudio>,
}

/// An audio file that stayed after a failure because removing it failed too
///
/// Kept apart from the root so that a display shows the file name by the
/// rule for names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeftoverAudio {
    pub audio_filename: String,
    /// Why it could not be removed
    pub reason: String,
}

impl FailureReason {
    pub fn new(root: impl Into<String>, subject: Subject) -> Self {
        Self {
            root: root.into(),
            subject,
            leftover_audio: None,
        }
    }

    /// The same reason, with an audio file that could not be removed
    pub fn with_leftover_audio(
        mut self,
        audio_filename: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        self.leftover_audio = Some(LeftoverAudio {
            audio_filename: audio_filename.into(),
            reason: reason.into(),
        });
        self
    }

    /// A reason that is a complete sentence of its own
    pub fn without_subject(root: impl Into<String>) -> Self {
        Self::new(root, Subject::None)
    }
}
