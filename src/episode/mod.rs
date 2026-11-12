// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

mod disk_wait;
mod download;
mod filename;
mod hash;
mod partial;
#[cfg(test)]
mod test_support;

pub(crate) use disk_wait::{DISK_WAIT_SLOW_AFTER, WaitNotice, with_disk_wait_notice};
pub(crate) use download::{DownloadContext, SyncSettings, stage_download};
pub use filename::{
    filename_claim_key, generate_filename_stem, generate_unique_filename_stem, get_audio_extension,
};
pub(crate) use hash::hash_file;

/// Lowercase hex digits of `bytes`, as stored content hashes and hashed
/// filename suffixes spell a SHA-256 digest
pub(crate) fn lower_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
