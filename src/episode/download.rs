// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::path::{Path, PathBuf};

use futures::StreamExt;
use sha2::{Digest, Sha256};
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

use crate::error::DownloadError;
use crate::feed::Episode;
use crate::fs_sync::sync_file;
use crate::http::HttpClient;
use crate::progress::{ProgressEvent, SharedProgressReporter};

/// Context for tracking a download in concurrent scenarios
#[derive(Debug, Clone)]
pub struct DownloadContext {
    /// Slot ID (0 to max_concurrent-1) for progress bar management
    pub download_id: usize,
    /// Index of this episode in the download queue
    pub episode_index: usize,
    /// Total number of episodes to download
    pub total_to_download: usize,
}

/// Read size for hashing stored audio; large reads keep the number of
/// round trips low on a network share
const HASH_READ_SIZE: usize = 1024 * 1024;

/// Format a finished hash the way episode metadata records it
fn content_hash(hasher: Sha256) -> String {
    format!("sha256:{:x}", hasher.finalize())
}

/// Hash a file in the format [`StagedDownload::content_hash`] uses
///
/// Reads the whole file, so on a network share it costs a full transfer.
/// Blocks the calling thread while it reads.
pub(crate) fn hash_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; HASH_READ_SIZE];
    loop {
        let read = std::io::Read::read(&mut file, &mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(content_hash(hasher))
}

/// An episode whose audio is complete in its partial file but not yet
/// under its final name
///
/// Staging lets a caller put the episode's metadata in place before the
/// audio becomes visible, so an interruption never leaves audio without
/// metadata behind.
#[derive(Debug)]
pub struct StagedDownload {
    partial_path: PathBuf,
    final_path: PathBuf,
    bytes_downloaded: u64,
    content_hash: String,
}

impl StagedDownload {
    /// SHA-256 hash of the downloaded content (format: "sha256:...")
    pub fn content_hash(&self) -> &str {
        &self.content_hash
    }

    /// Move the audio from its partial file to its final name, returning
    /// the number of bytes downloaded
    ///
    /// If the rename fails, the partial file stays behind and the next
    /// directory scan reports it.
    pub async fn finalize(self) -> Result<u64, DownloadError> {
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
    pub async fn discard(self) {
        let _ = tokio::fs::remove_file(&self.partial_path).await;
    }
}

/// Download an episode into the partial file next to `output_path`
///
/// Streams the response body to disk while computing a SHA-256 hash. The
/// audio reaches `output_path` only through [`StagedDownload::finalize`].
pub async fn stage_download<C: HttpClient>(
    client: &C,
    episode: &Episode,
    output_path: &Path,
    context: &DownloadContext,
    reporter: &SharedProgressReporter,
) -> Result<StagedDownload, DownloadError> {
    let url = episode.enclosure.url.as_str();

    // Get streaming response
    let response = client
        .get_stream(url)
        .await
        .map_err(|e| DownloadError::HttpFailed {
            url: url.to_string(),
            source: e,
        })?;

    // Check for HTTP errors
    if response.status >= 400 {
        return Err(DownloadError::HttpStatus {
            url: url.to_string(),
            status: response.status,
        });
    }

    // Report download starting
    reporter.report(ProgressEvent::DownloadStarting {
        download_id: context.download_id,
        episode_title: episode.title.clone(),
        episode_index: context.episode_index,
        total_to_download: context.total_to_download,
        content_length: response.content_length,
    });

    let partial_path = PathBuf::from(format!("{}.partial", output_path.display()));

    // Within a sync, the directory scan removes leftover partial files before
    // any download starts, so an existing one belongs to another download
    // targeting the same path. Opening exclusively turns such a conflict into
    // an error instead of two downloads interleaving their bytes in one file.
    let mut file = File::create_new(&partial_path).await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            DownloadError::PartialFileExists {
                path: partial_path.clone(),
            }
        } else {
            DownloadError::FileCreateFailed {
                path: partial_path.clone(),
                source: e,
            }
        }
    })?;

    // Initialize hasher for streaming hash computation
    let mut hasher = Sha256::new();

    // Stream body to file while computing hash
    let mut bytes_downloaded: u64 = 0;
    let mut stream = response.body;

    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result.map_err(|e| DownloadError::StreamFailed {
            url: url.to_string(),
            source: e,
        })?;

        // Update hash with chunk data
        hasher.update(&chunk);

        file.write_all(&chunk)
            .await
            .map_err(|e| DownloadError::FileWriteFailed {
                path: partial_path.clone(),
                source: e,
            })?;

        bytes_downloaded += chunk.len() as u64;

        // Report progress
        reporter.report(ProgressEvent::DownloadProgress {
            download_id: context.download_id,
            bytes_downloaded,
            total_bytes: response.content_length,
        });
    }

    // Without syncing, a power loss or a crashed file server could persist
    // the later rename before the data and leave a truncated file under the
    // final name. Flushing first hands every buffered write to the file, so
    // the synchronous sync sees all of them.
    let write_failed = |e| DownloadError::FileWriteFailed {
        path: partial_path.clone(),
        source: e,
    };
    file.flush().await.map_err(write_failed)?;
    let file = file.into_std().await;
    tokio::task::spawn_blocking(move || sync_file(&file))
        .await
        .map_err(|e| write_failed(std::io::Error::other(e)))?
        .map_err(write_failed)?;

    Ok(StagedDownload {
        partial_path,
        final_path: output_path.to_path_buf(),
        bytes_downloaded,
        content_hash: content_hash(hasher),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let reporter = NoopReporter::shared();

        let staged = stage_download(&client, &episode, &output_path, &context, &reporter)
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
        let reporter = NoopReporter::shared();

        let result = stage_download(&client, &episode, &output_path, &context, &reporter).await;

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
            &NoopReporter::shared(),
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
            &NoopReporter::shared(),
        )
        .await
        .unwrap();
        let recorded_hash = staged.content_hash().to_string();
        staged.finalize().await.unwrap();

        assert_eq!(hash_file(&output_path).unwrap(), recorded_hash);
    }

    #[test]
    fn hash_file_fails_for_missing_file() {
        let dir = tempdir().unwrap();

        let result = hash_file(&dir.path().join("missing.mp3"));

        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::NotFound);
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
            &NoopReporter::shared(),
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
            &NoopReporter::shared(),
        )
        .await
        .unwrap();
        staged.discard().await;

        assert!(!output_path.exists());
        assert!(!dir.path().join("episode.mp3.partial").exists());
    }
}
