// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;

use super::disk_wait::{DISK_WAIT_SLOW_AFTER, with_disk_wait_notice};
use super::partial::{FinishedPartial, PartialWriter, SyncFn};
use crate::error::DownloadError;
use crate::feed::Episode;
use crate::fs_sync::sync_file;
use crate::http::HttpClient;
use crate::partial_file::partial_path;
use crate::progress::{ProgressEvent, ProgressReporter};

/// Context for tracking a download in concurrent scenarios
#[derive(Debug, Clone)]
pub(crate) struct DownloadContext {
    /// Slot ID (0 to max_concurrent-1) for progress bar management
    pub download_id: usize,
    /// Index of this episode in the download queue
    pub episode_index: usize,
    /// Total number of episodes to download
    pub total_to_download: usize,
}

/// An episode whose audio is complete in its partial file but not yet
/// under its final name
///
/// Staging lets a caller put the episode's metadata in place before the
/// audio becomes visible, so an interruption never leaves audio without
/// metadata behind.
#[derive(Debug)]
pub(crate) struct StagedDownload {
    partial_path: PathBuf,
    final_path: PathBuf,
    bytes_downloaded: u64,
    content_hash: String,
}

impl StagedDownload {
    /// Stage a partial file whose data is on stable storage for `final_path`
    fn new(finished: FinishedPartial, final_path: &Path) -> Self {
        let (partial_path, bytes_downloaded, content_hash) = finished.into_parts();
        Self {
            partial_path,
            final_path: final_path.to_path_buf(),
            bytes_downloaded,
            content_hash,
        }
    }

    /// SHA-256 hash of the downloaded content (format: "sha256:...")
    pub(crate) fn content_hash(&self) -> &str {
        &self.content_hash
    }

    /// Move the audio from its partial file to its final name, returning
    /// the number of bytes downloaded
    ///
    /// If the rename fails, the partial file stays behind and the next
    /// directory scan reports it.
    pub(crate) async fn finalize(self) -> Result<u64, DownloadError> {
        tokio::fs::rename(&self.partial_path, &self.final_path)
            .await
            .map_err(|e| DownloadError::RenameFailed {
                partial_path: self.partial_path.clone(),
                final_path: self.final_path.clone(),
                source: e,
            })?;
        Ok(self.bytes_downloaded)
    }

    /// Abandon the download and remove its partial file
    ///
    /// A partial file that cannot be removed here is removed by the next
    /// directory scan.
    pub(crate) async fn discard(self) {
        let _ = tokio::fs::remove_file(&self.partial_path).await;
    }
}

/// How a download syncs its partial file
#[derive(Clone)]
pub(crate) struct SyncSettings {
    /// Flushes a file to stable storage
    pub(crate) sync: SyncFn,
    /// How long a sync may take before the display says that the disk is slow
    pub(crate) slow_after: Duration,
}

impl Default for SyncSettings {
    fn default() -> Self {
        Self {
            sync: Arc::new(sync_file),
            slow_after: DISK_WAIT_SLOW_AFTER,
        }
    }
}

/// Download an episode into the partial file next to `output_path`
///
/// Streams the response body to disk while computing a SHA-256 hash. The
/// audio reaches `output_path` only through [`StagedDownload::finalize`],
/// after the partial file was synced to stable storage with `settings`.
pub(crate) async fn stage_download<C: HttpClient>(
    client: &C,
    episode: &Episode,
    output_path: &Path,
    context: &DownloadContext,
    reporter: &dyn ProgressReporter,
    settings: &SyncSettings,
) -> Result<StagedDownload, DownloadError> {
    let url = episode.enclosure.url.as_str();

    let response = client
        .get_stream(url)
        .await
        .map_err(|e| DownloadError::HttpFailed {
            url: url.to_string(),
            source: e,
        })?;

    if response.status >= 400 {
        return Err(DownloadError::HttpStatus {
            url: url.to_string(),
            status: response.status,
        });
    }

    reporter.report(ProgressEvent::DownloadStarting {
        download_id: context.download_id,
        episode_title: episode.title.clone(),
        episode_index: context.episode_index,
        total_to_download: context.total_to_download,
        content_length: response.content_length,
    });

    let mut writer =
        PartialWriter::create(partial_path(output_path), settings.sync.clone()).await?;

    let mut stream = response.body;
    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result.map_err(|e| DownloadError::StreamFailed {
            url: url.to_string(),
            source: e,
        })?;

        writer.write(&chunk).await?;

        reporter.report(ProgressEvent::DownloadProgress {
            download_id: context.download_id,
            bytes_downloaded: writer.bytes_written(),
            total_bytes: response.content_length,
        });
    }

    // Without syncing, a power loss or a crashed file server could persist
    // the later rename before the data and leave a truncated file under the
    // final name.
    let flushed = writer.finish().await?;
    let finished = with_disk_wait_notice(
        flushed.synced(),
        context,
        &episode.title,
        reporter,
        settings.slow_after,
    )
    .await?;

    Ok(StagedDownload::new(finished, output_path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::episode::hash::hash_file;
    use crate::episode::test_support::{GATE_TIMEOUT, GateReporter, gate};
    use crate::feed::Enclosure;
    use crate::http::{ByteStream, HttpResponse};
    use crate::progress::NoopReporter;
    use async_trait::async_trait;
    use bytes::Bytes;

    use tempfile::tempdir;
    use url::Url;

    struct MockHttpClient {
        response_data: Vec<u8>,
        status: u16,
    }

    #[async_trait]
    impl HttpClient for MockHttpClient {
        async fn get_bytes(&self, _url: &str) -> Result<Bytes, reqwest::Error> {
            Ok(Bytes::from(self.response_data.clone()))
        }

        async fn get_stream(&self, _url: &str) -> Result<HttpResponse, reqwest::Error> {
            let data = self.response_data.clone();
            let len = data.len() as u64;

            let stream: ByteStream =
                Box::pin(futures::stream::once(async move { Ok(Bytes::from(data)) }));

            Ok(HttpResponse {
                status: self.status,
                content_length: Some(len),
                body: stream,
            })
        }
    }

    fn make_episode() -> Episode {
        Episode {
            title: "Test Episode".to_string(),
            description: None,
            pub_date: None,
            guid: Some("test-guid".to_string()),
            enclosure: Enclosure {
                url: Url::parse("https://example.com/episode.mp3").unwrap(),
                mime_type: Some("audio/mpeg".to_string()),
            },
            duration: None,
            episode_number: None,
            season_number: None,
        }
    }

    #[tokio::test]
    async fn download_writes_file() {
        let dir = tempdir().unwrap();
        let output_path = dir.path().join("episode.mp3");

        let client = MockHttpClient {
            response_data: b"test audio content".to_vec(),
            status: 200,
        };

        let episode = make_episode();
        let context = DownloadContext {
            download_id: 0,
            episode_index: 0,
            total_to_download: 1,
        };
        let reporter = NoopReporter;

        let staged = stage_download(
            &client,
            &episode,
            &output_path,
            &context,
            &reporter,
            &SyncSettings::default(),
        )
        .await
        .unwrap();
        assert!(staged.content_hash().starts_with("sha256:"));
        let bytes_downloaded = staged.finalize().await.unwrap();

        assert_eq!(bytes_downloaded, 18); // "test audio content".len()
        assert!(output_path.exists());
        // Verify no .partial file remains
        assert!(!dir.path().join("episode.mp3.partial").exists());

        let content = std::fs::read(&output_path).unwrap();
        assert_eq!(content, b"test audio content");
    }

    #[tokio::test]
    async fn download_fails_on_http_error() {
        let dir = tempdir().unwrap();
        let output_path = dir.path().join("episode.mp3");

        let client = MockHttpClient {
            response_data: b"Not Found".to_vec(),
            status: 404,
        };

        let episode = make_episode();
        let context = DownloadContext {
            download_id: 0,
            episode_index: 0,
            total_to_download: 1,
        };
        let reporter = NoopReporter;

        let result = stage_download(
            &client,
            &episode,
            &output_path,
            &context,
            &reporter,
            &SyncSettings::default(),
        )
        .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            DownloadError::HttpStatus { status, .. } => assert_eq!(status, 404),
            _ => panic!("Expected HttpStatus error"),
        }
    }

    #[tokio::test]
    async fn download_refuses_to_share_a_partial_file() {
        let dir = tempdir().unwrap();
        let output_path = dir.path().join("episode.mp3");
        let partial_path = dir.path().join("episode.mp3.partial");

        // Another download already writes to this partial file. Truncating
        // and writing into it would interleave the bytes of both downloads.
        std::fs::write(&partial_path, b"bytes of another download").unwrap();

        let client = MockHttpClient {
            response_data: b"test audio content".to_vec(),
            status: 200,
        };
        let context = DownloadContext {
            download_id: 0,
            episode_index: 0,
            total_to_download: 1,
        };

        let result = stage_download(
            &client,
            &make_episode(),
            &output_path,
            &context,
            &NoopReporter,
            &SyncSettings::default(),
        )
        .await;

        match result.unwrap_err() {
            DownloadError::PartialFileExists { path } => assert_eq!(path, partial_path),
            other => panic!("Expected PartialFileExists, got {other:?}"),
        }
        assert_eq!(
            std::fs::read(&partial_path).unwrap(),
            b"bytes of another download"
        );
        assert!(!output_path.exists());
    }

    #[tokio::test]
    async fn hash_file_matches_hash_recorded_by_download() {
        let dir = tempdir().unwrap();
        let output_path = dir.path().join("episode.mp3");
        let client = MockHttpClient {
            response_data: b"test audio content".to_vec(),
            status: 200,
        };
        let context = DownloadContext {
            download_id: 0,
            episode_index: 0,
            total_to_download: 1,
        };

        let staged = stage_download(
            &client,
            &make_episode(),
            &output_path,
            &context,
            &NoopReporter,
            &SyncSettings::default(),
        )
        .await
        .unwrap();
        let recorded_hash = staged.content_hash().to_string();
        staged.finalize().await.unwrap();

        assert_eq!(hash_file(&output_path, |_, _| {}).unwrap(), recorded_hash);
    }

    fn context() -> DownloadContext {
        DownloadContext {
            download_id: 0,
            episode_index: 0,
            total_to_download: 1,
        }
    }

    fn ok_client() -> MockHttpClient {
        MockHttpClient {
            response_data: b"test audio content".to_vec(),
            status: 200,
        }
    }

    #[tokio::test]
    async fn staged_download_stays_partial_until_finalized() {
        let dir = tempdir().unwrap();
        let output_path = dir.path().join("episode.mp3");

        let staged = stage_download(
            &ok_client(),
            &make_episode(),
            &output_path,
            &context(),
            &NoopReporter,
            &SyncSettings::default(),
        )
        .await
        .unwrap();

        assert!(!output_path.exists());
        assert!(dir.path().join("episode.mp3.partial").exists());

        let bytes_downloaded = staged.finalize().await.unwrap();

        assert_eq!(bytes_downloaded, 18);

        assert_eq!(std::fs::read(&output_path).unwrap(), b"test audio content");
        assert!(!dir.path().join("episode.mp3.partial").exists());
    }

    #[tokio::test]
    async fn discarded_download_leaves_nothing_behind() {
        let dir = tempdir().unwrap();
        let output_path = dir.path().join("episode.mp3");

        let staged = stage_download(
            &ok_client(),
            &make_episode(),
            &output_path,
            &context(),
            &NoopReporter,
            &SyncSettings::default(),
        )
        .await
        .unwrap();
        staged.discard().await;

        assert!(!output_path.exists());
        assert!(!dir.path().join("episode.mp3.partial").exists());
    }

    #[tokio::test]
    async fn the_audio_sync_wait_is_reported_while_the_file_is_still_partial() {
        let dir = tempdir().unwrap();
        let output_path = dir.path().join("episode.mp3");
        let partial_path = dir.path().join("episode.mp3.partial");
        let (open_gate, closed_gate) = gate();
        let closed_gate = std::sync::Mutex::new(closed_gate);
        let seen_while_slow = std::sync::Arc::new(std::sync::Mutex::new(None));
        let reporter = {
            let (output_path, partial_path) = (output_path.clone(), partial_path.clone());
            let seen_while_slow = seen_while_slow.clone();
            GateReporter::new(move || {
                let partial_len = std::fs::metadata(&partial_path).map(|m| m.len()).ok();
                *seen_while_slow.lock().unwrap() = Some((partial_len, output_path.exists()));
                open_gate();
            })
        };

        let staged = tokio::time::timeout(
            GATE_TIMEOUT,
            stage_download(
                &ok_client(),
                &make_episode(),
                &output_path,
                &context(),
                &reporter,
                &SyncSettings {
                    sync: Arc::new(move |_| {
                        closed_gate
                            .lock()
                            .unwrap()
                            .recv()
                            .map_err(std::io::Error::other)
                    }),
                    slow_after: Duration::ZERO,
                },
            ),
        )
        .await
        .expect("no DownloadSyncSlow opened the gate")
        .unwrap();

        assert_eq!(
            *seen_while_slow.lock().unwrap(),
            Some((Some(18), false)),
            "the partial file holds every byte and the final name is not taken"
        );
        assert_eq!(reporter.waits(), ["syncing", "slow", "synced"]);
        staged.finalize().await.unwrap();
        assert_eq!(std::fs::read(&output_path).unwrap(), b"test audio content");
    }

    #[tokio::test]
    async fn a_failing_sync_fails_the_download_and_keeps_the_partial_file() {
        let dir = tempdir().unwrap();
        let output_path = dir.path().join("episode.mp3");

        let result = stage_download(
            &ok_client(),
            &make_episode(),
            &output_path,
            &context(),
            &NoopReporter,
            &SyncSettings {
                sync: Arc::new(|_| Err(std::io::Error::from_raw_os_error(libc::EIO))),
                slow_after: DISK_WAIT_SLOW_AFTER,
            },
        )
        .await;

        match result.unwrap_err() {
            DownloadError::FileWriteFailed { path, source } => {
                assert_eq!(path, dir.path().join("episode.mp3.partial"));
                assert_eq!(source.raw_os_error(), Some(libc::EIO));
            }
            other => panic!("expected FileWriteFailed, got {other:?}"),
        }
        assert!(dir.path().join("episode.mp3.partial").exists());
    }
}
