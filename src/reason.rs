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
}

impl FailureReason {
    pub fn new(root: impl Into<String>, subject: Subject) -> Self {
        Self {
            root: root.into(),
            subject,
        }
    }

    /// A reason that is a complete sentence of its own
    pub fn without_subject(root: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            subject: Subject::None,
        }
    }
}
