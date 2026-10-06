// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::MetadataError;

mod episode;
mod podcast;

pub use episode::{
    EpisodeMetadata, read_episode_metadata, stage_episode_metadata, write_episode_metadata,
};
pub use podcast::{PodcastMetadata, read_podcast_metadata, write_podcast_metadata};

/// A metadata file written to its partial file but not yet under its final
/// name
///
/// An episode metadata file cut short by an interruption would leave its
/// episode without a readable GUID while still occupying the name. Writing
/// to a partial file, which the next directory scan removes, syncing it to
/// disk and renaming it into place means the metadata file is either
/// complete or untouched. Staging also lets a sync write an episode's
/// metadata before its audio takes its name.
#[derive(Debug)]
pub struct StagedMetadata {
    partial_path: PathBuf,
    path: PathBuf,
}

impl StagedMetadata {
    /// Move the metadata from its partial file to its final name
    ///
    /// On failure the partial file is removed as well, as nothing can
    /// complete it any more.
    pub fn commit(self) -> Result<(), MetadataError> {
        std::fs::rename(&self.partial_path, &self.path).map_err(|e| {
            let _ = std::fs::remove_file(&self.partial_path);
            MetadataError::WriteFailed {
                path: self.path.clone(),
                source: e,
            }
        })
    }

    /// Abandon the metadata and remove its partial file
    ///
    /// A partial file that cannot be removed here is removed by the next
    /// directory scan.
    pub fn discard(self) {
        let _ = std::fs::remove_file(&self.partial_path);
    }
}

/// Write `contents` into the partial file next to `path` and sync it to disk
///
/// Without the sync, a power loss or a crashed file server could persist
/// the rename before the data and leave an empty file under the final name.
fn stage_metadata_file(path: &Path, contents: &[u8]) -> Result<StagedMetadata, MetadataError> {
    let partial_path = PathBuf::from(format!("{}.partial", path.display()));
    let write_failed = |e| MetadataError::WriteFailed {
        path: partial_path.clone(),
        source: e,
    };

    let mut file = std::fs::File::create(&partial_path).map_err(write_failed)?;
    file.write_all(contents).map_err(write_failed)?;
    file.sync_all().map_err(write_failed)?;

    Ok(StagedMetadata {
        partial_path,
        path: path.to_path_buf(),
    })
}
