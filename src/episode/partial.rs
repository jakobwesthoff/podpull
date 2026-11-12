// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The partial file an episode's audio is written to
//!
//! [`PartialWriter`] owns the file while it is written and hashes what goes
//! in. Its end is a chain of types: [`PartialWriter::finish`] flushes,
//! [`FlushedPartial::synced`] syncs, and only the [`FinishedPartial`] that
//! returns can be turned into a staged download, so the data is on stable
//! storage before the file can be renamed.

use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

use super::hash::content_hash;
use crate::error::DownloadError;

/// Flushes a file's data to stable storage
///
/// Injected so tests can hold a sync until they release it. Production
/// passes [`crate::fs_sync::sync_file`].
pub(crate) type SyncFn = Arc<dyn Fn(&std::fs::File) -> io::Result<()> + Send + Sync>;

/// A partial file being written
pub(crate) struct PartialWriter {
    path: PathBuf,
    file: File,
    sync: SyncFn,
    hasher: Sha256,
    bytes_written: u64,
}

impl PartialWriter {
    /// Create the partial file at `path`
    ///
    /// Within a sync, the directory scan removes leftover partial files
    /// before any download starts, so an existing one belongs to another
    /// download targeting the same path. Opening exclusively turns such a
    /// conflict into an error instead of two downloads interleaving their
    /// bytes in one file.
    pub(crate) async fn create(path: PathBuf, sync: SyncFn) -> Result<Self, DownloadError> {
        let file = File::create_new(&path).await.map_err(|e| {
            if e.kind() == io::ErrorKind::AlreadyExists {
                DownloadError::PartialFileExists { path: path.clone() }
            } else {
                DownloadError::FileCreateFailed {
                    path: path.clone(),
                    source: e,
                }
            }
        })?;
        Ok(Self {
            path,
            file,
            sync,
            hasher: Sha256::new(),
            bytes_written: 0,
        })
    }

    /// Append `chunk` to the file and to the hash
    pub(crate) async fn write(&mut self, chunk: &[u8]) -> Result<(), DownloadError> {
        self.hasher.update(chunk);
        self.file
            .write_all(chunk)
            .await
            .map_err(|e| self.write_failed(e))?;
        self.bytes_written += chunk.len() as u64;
        Ok(())
    }

    /// Bytes written so far
    pub(crate) fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    /// Hand every buffered write to the file
    ///
    /// The sync that follows runs on another thread and sees only what the
    /// file has received. A write that failed in the background is reported
    /// here, so this call stays between the last write and the sync.
    pub(crate) async fn finish(mut self) -> Result<FlushedPartial, DownloadError> {
        self.file.flush().await.map_err(|e| self.write_failed(e))?;
        Ok(FlushedPartial {
            path: self.path,
            file: self.file,
            sync: self.sync,
            hasher: self.hasher,
            bytes_written: self.bytes_written,
        })
    }

    fn write_failed(&self, source: io::Error) -> DownloadError {
        DownloadError::FileWriteFailed {
            path: self.path.clone(),
            source,
        }
    }
}

/// A partial file whose writes all reached the file but are not yet synced
pub(crate) struct FlushedPartial {
    path: PathBuf,
    file: File,
    sync: SyncFn,
    hasher: Sha256,
    bytes_written: u64,
}

impl FlushedPartial {
    /// Sync the file to stable storage
    ///
    /// Blocks a thread of the blocking pool until the file system confirms
    /// the data, which on a network share can take minutes.
    pub(crate) async fn synced(self) -> Result<FinishedPartial, DownloadError> {
        let write_failed = |source| DownloadError::FileWriteFailed {
            path: self.path.clone(),
            source,
        };
        let file = self.file.into_std().await;
        let sync = self.sync;
        tokio::task::spawn_blocking(move || sync(&file))
            .await
            .map_err(|e| write_failed(io::Error::other(e)))?
            .map_err(write_failed)?;
        Ok(FinishedPartial {
            path: self.path,
            bytes_written: self.bytes_written,
            content_hash: content_hash(self.hasher),
        })
    }
}

/// A partial file with all its data on stable storage
///
/// Only [`FlushedPartial::synced`] makes one.
pub(crate) struct FinishedPartial {
    path: PathBuf,
    bytes_written: u64,
    content_hash: String,
}

impl FinishedPartial {
    /// The path of the partial file, the bytes in it and their hash
    pub(super) fn into_parts(self) -> (PathBuf, u64, String) {
        (self.path, self.bytes_written, self.content_hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::tempdir;

    /// A sync that records how many bytes the file held when it ran
    fn recording_sync(path: PathBuf, lengths: Arc<Mutex<Vec<u64>>>) -> SyncFn {
        Arc::new(move |_| {
            lengths
                .lock()
                .unwrap()
                .push(std::fs::metadata(&path).unwrap().len());
            Ok(())
        })
    }

    fn no_sync() -> SyncFn {
        Arc::new(|_| Ok(()))
    }

    #[tokio::test]
    async fn the_hash_covers_every_chunk_written() {
        let dir = tempdir().unwrap();
        let mut writer = PartialWriter::create(dir.path().join("a.mp3.partial"), no_sync())
            .await
            .unwrap();

        writer.write(b"a").await.unwrap();
        writer.write(b"bc").await.unwrap();
        let (_, _, hash) = writer
            .finish()
            .await
            .unwrap()
            .synced()
            .await
            .unwrap()
            .into_parts();

        // `printf abc | shasum -a 256`
        assert_eq!(
            hash,
            "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[tokio::test]
    async fn bytes_written_counts_across_writes() {
        let dir = tempdir().unwrap();
        let mut writer = PartialWriter::create(dir.path().join("a.mp3.partial"), no_sync())
            .await
            .unwrap();

        writer.write(b"abc").await.unwrap();
        writer.write(b"de").await.unwrap();

        assert_eq!(writer.bytes_written(), 5);
        let (_, bytes, _) = writer
            .finish()
            .await
            .unwrap()
            .synced()
            .await
            .unwrap()
            .into_parts();
        assert_eq!(bytes, 5);
    }

    #[tokio::test]
    async fn a_second_writer_for_the_same_path_is_refused() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("a.mp3.partial");
        let _first = PartialWriter::create(path.clone(), no_sync())
            .await
            .unwrap();

        let second = PartialWriter::create(path.clone(), no_sync()).await;

        match second.err().expect("the second create must fail") {
            DownloadError::PartialFileExists { path: refused } => assert_eq!(refused, path),
            other => panic!("expected PartialFileExists, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn the_sync_runs_once_after_the_last_write_and_sees_every_byte() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("a.mp3.partial");
        let lengths = Arc::new(Mutex::new(Vec::new()));
        let mut writer = PartialWriter::create(path.clone(), recording_sync(path, lengths.clone()))
            .await
            .unwrap();
        writer.write(b"abc").await.unwrap();
        writer.write(b"de").await.unwrap();

        let flushed = writer.finish().await.unwrap();
        assert!(
            lengths.lock().unwrap().is_empty(),
            "finishing the writes does not sync"
        );
        flushed.synced().await.unwrap();

        assert_eq!(*lengths.lock().unwrap(), vec![5]);
    }

    #[tokio::test]
    async fn a_failing_sync_is_a_write_failure_and_keeps_the_partial_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("a.mp3.partial");
        let failing: SyncFn = Arc::new(|_| Err(io::Error::from_raw_os_error(libc::EIO)));
        let mut writer = PartialWriter::create(path.clone(), failing).await.unwrap();
        writer.write(b"abc").await.unwrap();

        let result = writer.finish().await.unwrap().synced().await;

        match result.err().expect("the sync fails") {
            DownloadError::FileWriteFailed {
                path: failed,
                source,
            } => {
                assert_eq!(failed, path);
                assert_eq!(source.raw_os_error(), Some(libc::EIO));
            }
            other => panic!("expected FileWriteFailed, got {other:?}"),
        }
        assert!(path.exists());
    }
}
