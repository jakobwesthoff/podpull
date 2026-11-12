// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The partial file an episode's audio is written to
//!
//! [`PartialWriter`] owns the file while it is written, hashes what goes in
//! and syncs it to stable storage in steps while the writes go on. Its end
//! is a chain of types: [`PartialWriter::finish`] flushes,
//! [`FlushedPartial::synced`] waits for the step sync still running and
//! syncs the rest, and only the [`FinishedPartial`] that returns can be
//! turned into a staged download, so all the data is on stable storage
//! before the file can be renamed.

use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio::task::JoinHandle;

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
    /// A duplicate of the descriptor of `file`, which the blocking threads
    /// sync while `file` goes on being written
    sync_handle: Arc<std::fs::File>,
    sync: SyncFn,
    /// Bytes written since the last step sync started that make the next one
    /// due
    step_bytes: u64,
    unsynced_bytes: u64,
    /// The step sync running on a blocking thread, if any. At most one runs
    /// at a time.
    step_sync: Option<JoinHandle<io::Result<()>>>,
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
    pub(crate) async fn create(
        path: PathBuf,
        sync: SyncFn,
        step_bytes: u64,
    ) -> Result<Self, DownloadError> {
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
        let sync_handle = match file.try_clone().await {
            Ok(duplicate) => duplicate.into_std().await,
            Err(e) => {
                // The file was created by this call and nothing was written to
                // it, so it goes again instead of being left for the next
                // directory scan to remove.
                drop(file);
                let _ = tokio::fs::remove_file(&path).await;
                return Err(DownloadError::FileCreateFailed { path, source: e });
            }
        };
        Ok(Self {
            path,
            file,
            sync_handle: Arc::new(sync_handle),
            sync,
            step_bytes,
            unsynced_bytes: 0,
            step_sync: None,
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
        self.unsynced_bytes += chunk.len() as u64;
        Ok(())
    }

    /// Bytes written so far
    pub(crate) fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    /// Whether enough bytes were written since the last step sync started
    /// to make the next one due
    pub(crate) fn sync_due(&self) -> bool {
        self.unsynced_bytes >= self.step_bytes
    }

    /// Whether a step sync is still running on its blocking thread
    pub(crate) fn step_sync_running(&self) -> bool {
        self.step_sync
            .as_ref()
            .is_some_and(|sync| !sync.is_finished())
    }

    /// Wait for the step sync that is running, if any
    ///
    /// Fails if that sync failed; the failure is reported once.
    pub(crate) async fn await_step_sync(&mut self) -> Result<(), DownloadError> {
        match self.step_sync.take() {
            Some(sync) => settled(sync, &self.path).await,
            None => Ok(()),
        }
    }

    /// Start syncing what was written so far on a blocking thread and go on
    ///
    /// The writer goes on while the sync runs, so the caller keeps reading
    /// from its source. Only one sync runs at a time: a previous one is
    /// waited for first. The writes are flushed before the sync starts, as
    /// the sync sees only what the file has received.
    pub(crate) async fn start_step_sync(&mut self) -> Result<(), DownloadError> {
        self.await_step_sync().await?;
        self.file.flush().await.map_err(|e| self.write_failed(e))?;
        let (sync, sync_handle) = (self.sync.clone(), self.sync_handle.clone());
        self.step_sync = Some(tokio::task::spawn_blocking(move || sync(&sync_handle)));
        self.unsynced_bytes = 0;
        Ok(())
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
            sync_handle: self.sync_handle,
            sync: self.sync,
            step_sync: self.step_sync,
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
    sync_handle: Arc<std::fs::File>,
    sync: SyncFn,
    step_sync: Option<JoinHandle<io::Result<()>>>,
    hasher: Sha256,
    bytes_written: u64,
}

impl FlushedPartial {
    /// Sync the file to stable storage
    ///
    /// Waits for the step sync still running, then syncs the file again for
    /// the bytes written after it. Blocks a thread of the blocking pool
    /// until the file system confirms the data, which on a network share
    /// can take minutes.
    pub(crate) async fn synced(self) -> Result<FinishedPartial, DownloadError> {
        if let Some(step_sync) = self.step_sync {
            settled(step_sync, &self.path).await?;
        }
        let sync = self.sync;
        let sync_handle = self.sync_handle;
        settled(
            tokio::task::spawn_blocking(move || sync(&sync_handle)),
            &self.path,
        )
        .await?;
        Ok(FinishedPartial {
            path: self.path,
            bytes_written: self.bytes_written,
            content_hash: content_hash(self.hasher),
        })
    }
}

/// Wait for a sync on a blocking thread, reporting its failure as a failed
/// write of the file at `path`
async fn settled(
    sync: JoinHandle<io::Result<()>>,
    path: &std::path::Path,
) -> Result<(), DownloadError> {
    sync.await
        .map_err(io::Error::other)
        .and_then(|synced| synced)
        .map_err(|source| DownloadError::FileWriteFailed {
            path: path.to_path_buf(),
            source,
        })
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
        let mut writer =
            PartialWriter::create(dir.path().join("a.mp3.partial"), no_sync(), u64::MAX)
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
        let mut writer =
            PartialWriter::create(dir.path().join("a.mp3.partial"), no_sync(), u64::MAX)
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
        let _first = PartialWriter::create(path.clone(), no_sync(), u64::MAX)
            .await
            .unwrap();

        let second = PartialWriter::create(path.clone(), no_sync(), u64::MAX).await;

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
        let mut writer = PartialWriter::create(
            path.clone(),
            recording_sync(path, lengths.clone()),
            u64::MAX,
        )
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
        let mut writer = PartialWriter::create(path.clone(), failing, u64::MAX)
            .await
            .unwrap();
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

    #[tokio::test]
    async fn a_step_sync_is_due_after_the_step_and_not_again_until_the_next_one() {
        let dir = tempdir().unwrap();
        let mut writer = PartialWriter::create(dir.path().join("a.mp3.partial"), no_sync(), 25)
            .await
            .unwrap();
        let mut due = Vec::new();

        for chunk in [30, 30, 30, 10] {
            writer.write(&vec![b'a'; chunk]).await.unwrap();
            due.push(writer.sync_due());
            if writer.sync_due() {
                writer.start_step_sync().await.unwrap();
            }
        }

        assert_eq!(due, [true, true, true, false]);
    }

    #[tokio::test]
    async fn a_step_sync_sees_every_byte_written_before_it_started() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("a.mp3.partial");
        let lengths = Arc::new(Mutex::new(Vec::new()));
        let mut writer =
            PartialWriter::create(path.clone(), recording_sync(path, lengths.clone()), 1)
                .await
                .unwrap();
        writer.write(b"abc").await.unwrap();

        writer.start_step_sync().await.unwrap();
        writer.await_step_sync().await.unwrap();

        assert_eq!(*lengths.lock().unwrap(), vec![3]);
    }

    #[tokio::test]
    async fn the_final_sync_starts_only_after_the_step_sync_that_is_running_ended() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("a.mp3.partial");
        let (release, released) = std::sync::mpsc::channel::<()>();
        let released = Mutex::new(released);
        let (started_tx, started) = std::sync::mpsc::channel::<()>();
        let started_tx = Mutex::new(started_tx);
        let log = Arc::new(Mutex::new(Vec::new()));
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let sync: SyncFn = {
            let log = log.clone();
            Arc::new(move |_| {
                if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    log.lock().unwrap().push("step start");
                    started_tx.lock().unwrap().send(()).unwrap();
                    released.lock().unwrap().recv().map_err(io::Error::other)?;
                    log.lock().unwrap().push("step end");
                } else {
                    log.lock().unwrap().push("final start");
                }
                Ok(())
            })
        };
        let mut writer = PartialWriter::create(path, sync, 1).await.unwrap();
        writer.write(b"abc").await.unwrap();
        writer.start_step_sync().await.unwrap();
        started.recv().unwrap();
        let mut synced = Box::pin(writer.finish().await.unwrap().synced());

        assert!(futures::poll!(&mut synced).is_pending());
        assert_eq!(
            *log.lock().unwrap(),
            vec!["step start"],
            "the final sync must not start while the step sync is held"
        );
        release.send(()).unwrap();
        synced.await.unwrap();

        assert_eq!(
            *log.lock().unwrap(),
            vec!["step start", "step end", "final start"]
        );
    }

    #[tokio::test]
    async fn a_failed_last_step_sync_fails_the_final_sync() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("a.mp3.partial");
        let calls = std::sync::atomic::AtomicUsize::new(0);
        // Only the step sync fails; the sync after the last byte would pass.
        let sync: SyncFn = Arc::new(move |_| {
            if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                Err(io::Error::from_raw_os_error(libc::EIO))
            } else {
                Ok(())
            }
        });
        let mut writer = PartialWriter::create(path.clone(), sync, 1).await.unwrap();
        writer.write(b"abc").await.unwrap();
        writer.start_step_sync().await.unwrap();

        let result = writer.finish().await.unwrap().synced().await;

        match result.err().expect("the step sync failed") {
            DownloadError::FileWriteFailed {
                path: failed,
                source,
            } => {
                assert_eq!(failed, path);
                assert_eq!(source.raw_os_error(), Some(libc::EIO));
            }
            other => panic!("expected FileWriteFailed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_failed_step_sync_is_reported_once_by_the_next_wait() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("a.mp3.partial");
        let failing: SyncFn = Arc::new(|_| Err(io::Error::from_raw_os_error(libc::EIO)));
        let mut writer = PartialWriter::create(path.clone(), failing, 1)
            .await
            .unwrap();
        writer.write(b"abc").await.unwrap();
        writer.start_step_sync().await.unwrap();

        let first = writer.await_step_sync().await;
        let second = writer.await_step_sync().await;

        match first.expect_err("the step sync failed") {
            DownloadError::FileWriteFailed {
                path: failed,
                source,
            } => {
                assert_eq!(failed, path);
                assert_eq!(source.raw_os_error(), Some(libc::EIO));
            }
            other => panic!("expected FileWriteFailed, got {other:?}"),
        }
        assert!(second.is_ok());
    }
}
