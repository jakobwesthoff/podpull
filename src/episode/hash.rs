// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::path::Path;

use sha2::{Digest, Sha256};

use super::lower_hex;

/// Read size for hashing stored audio; large reads keep the number of
/// round trips low on a network share
pub(super) const HASH_READ_SIZE: usize = 1024 * 1024;

/// Format a finished hash the way episode metadata records it
pub(super) fn content_hash(hasher: Sha256) -> String {
    format!("sha256:{}", lower_hex(&hasher.finalize()))
}

/// Hash a file in the format [`StagedDownload::content_hash`] uses
///
/// Reads the whole file, so on a network share it costs a full transfer.
/// After every block, `on_progress` receives the bytes hashed so far and the
/// size of the file. Blocks the calling thread while it reads.
pub(crate) fn hash_file(
    path: &Path,
    mut on_progress: impl FnMut(u64, u64),
) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let total_bytes = file.metadata()?.len();
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; HASH_READ_SIZE];
    let mut bytes_hashed = 0;
    loop {
        let read = std::io::Read::read(&mut file, &mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        bytes_hashed += read as u64;
        on_progress(bytes_hashed, total_bytes);
    }
    Ok(content_hash(hasher))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn hash_file_keeps_the_format_of_recorded_hashes() {
        // Episode metadata stores this exact spelling, so it must not
        // change: "sha256:" and the lowercase hex digest
        // (`printf abc | shasum -a 256`).
        let dir = tempdir().unwrap();
        let path = dir.path().join("episode.mp3");
        std::fs::write(&path, b"abc").unwrap();

        assert_eq!(
            hash_file(&path, |_, _| {}).unwrap(),
            "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn hash_file_reports_progress_after_every_block() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("episode.mp3");
        // Two and a half read blocks, so the last block is a partial one.
        let size = HASH_READ_SIZE as u64 * 5 / 2;
        std::fs::write(&path, vec![7u8; size as usize]).unwrap();
        let mut progress = Vec::new();

        hash_file(&path, |hashed, total| progress.push((hashed, total))).unwrap();

        let block = HASH_READ_SIZE as u64;
        assert_eq!(
            progress,
            vec![(block, size), (2 * block, size), (size, size)]
        );
    }

    #[test]
    fn hash_file_fails_for_missing_file() {
        let dir = tempdir().unwrap();

        let result = hash_file(&dir.path().join("missing.mp3"), |_, _| {});

        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::NotFound);
    }
}
