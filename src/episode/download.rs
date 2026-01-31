use std::path::Path;

use futures::StreamExt;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

use crate::error::DownloadError;
use crate::feed::Episode;
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

/// Download an episode to the specified output path
///
/// Streams the response body to disk, reporting progress through the reporter.
/// Returns the number of bytes downloaded on success.
pub async fn download_episode<C: HttpClient>(
    client: &C,
    episode: &Episode,
    output_path: &Path,
    context: &DownloadContext,
    reporter: &SharedProgressReporter,
) -> Result<u64, DownloadError> {
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

    // Create output file
    let mut file =
        File::create(output_path)
            .await
            .map_err(|e| DownloadError::FileCreateFailed {
                path: output_path.to_path_buf(),
                source: e,
            })?;

    // Stream body to file
    let mut bytes_downloaded: u64 = 0;
    let mut stream = response.body;

    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result.map_err(|e| DownloadError::StreamFailed {
            url: url.to_string(),
            source: e,
        })?;

        file.write_all(&chunk)
            .await
            .map_err(|e| DownloadError::FileWriteFailed {
                path: output_path.to_path_buf(),
                source: e,
            })?;

        bytes_downloaded += chunk.len() as u64;

        // Report progress
        reporter.report(ProgressEvent::DownloadProgress {
            download_id: context.download_id,
            episode_title: episode.title.clone(),
            bytes_downloaded,
            total_bytes: response.content_length,
        });
    }

    // Ensure all data is flushed to disk
    file.flush()
        .await
        .map_err(|e| DownloadError::FileWriteFailed {
            path: output_path.to_path_buf(),
            source: e,
        })?;

    // Report completion
    reporter.report(ProgressEvent::DownloadCompleted {
        download_id: context.download_id,
        episode_title: episode.title.clone(),
        bytes_downloaded,
    });

    Ok(bytes_downloaded)
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
                length: Some(1000),
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

        let bytes = download_episode(&client, &episode, &output_path, &context, &reporter)
            .await
            .unwrap();

        assert_eq!(bytes, 18); // "test audio content".len()
        assert!(output_path.exists());

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

        let result = download_episode(&client, &episode, &output_path, &context, &reporter).await;

        assert!(result.is_err());
        match result.unwrap_err() {
            DownloadError::HttpStatus { status, .. } => assert_eq!(status, 404),
            _ => panic!("Expected HttpStatus error"),
        }
    }
}
