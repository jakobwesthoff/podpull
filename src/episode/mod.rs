// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

mod download;
mod filename;

pub(crate) use download::hash_file;
pub use download::{DownloadContext, stage_download};
pub use filename::{
    filename_claim_key, generate_filename_stem, generate_unique_filename_stem, get_audio_extension,
};

/// Lowercase hex digits of `bytes`, as stored content hashes and hashed
/// filename suffixes spell a SHA-256 digest
pub(crate) fn lower_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
