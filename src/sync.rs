// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::Mutex;

use url::Url;

use crate::episode::{DownloadContext, download_episode};
use crate::error::{FeedError, SyncError};
use crate::feed::{fetch_feed_bytes, file_path_to_url, is_url, parse_feed, read_feed_file};
use crate::http::HttpClient;
use crate::metadata::{write_episode_metadata, write_podcast_metadata};
use crate::progress::{ProgressEvent, SharedProgressReporter};
use crate::state::{create_sync_plan, scan_output_dir};

/// Options for podcast synchronization
#[derive(Debug, Clone)]
pub struct SyncOptions {
    /// Maximum number of episodes to download (None = all)
    pub limit: Option<usize>,
    /// Maximum number of concurrent downloads
    pub max_concurrent: usize,
    /// Continue downloading if individual episodes fail
    pub continue_on_error: bool,
}

impl Default for SyncOptions {
    fn default() -> Self {
        Self {
            limit: None,
            max_concurrent: 3,
            continue_on_error: true,
        }
    }
}

/// Result of a sync operation
#[derive(Debug, Clone)]
pub struct SyncResult {
    /// Number of episodes successfully downloaded
    pub downloaded: usize,
    /// Number of episodes skipped (already present)
    pub skipped: usize,
    /// Number of episodes that failed to download
    pub failed: usize,
    /// Details of failed episodes (title, error message)
    pub failed_episodes: Vec<(String, String)>,
}

/// Synchronize a podcast feed to a local directory
///
/// This is the main entry point for the library. It:
/// 1. Fetches and parses the feed
/// 2. Scans the output directory for existing downloads
/// 3. Creates a sync plan
/// 4. Downloads new episodes in parallel
/// 5. Writes metadata files
pub async fn sync_podcast<C: HttpClient + Clone + 'static>(
    client: &C,
    feed_source: &str,
    output_dir: &Path,
    options: &SyncOptions,
    reporter: SharedProgressReporter,
) -> Result<SyncResult, SyncError> {
    // Fetch and parse feed with granular progress reporting
    let podcast = if is_url(feed_source) {
        // For URLs: report fetching, then parsing
        reporter.report(ProgressEvent::FetchingFeed {
            url: feed_source.to_string(),
        });

        let bytes = fetch_feed_bytes(client, feed_source).await?;

        reporter.report(ProgressEvent::ParsingFeed {
            source: feed_source.to_string(),
        });

        let feed_url =
            Url::parse(feed_source).map_err(|e| SyncError::Feed(FeedError::InvalidUrl(e)))?;
        parse_feed(&bytes, feed_url)?
    } else {
        // For local files: skip "Fetching" and go straight to parsing
        reporter.report(ProgressEvent::ParsingFeed {
            source: feed_source.to_string(),
        });

        let bytes = read_feed_file(Path::new(feed_source))?;
        let feed_url = file_path_to_url(Path::new(feed_source));
        parse_feed(&bytes, feed_url)?
    };

    // Scan output directory (also cleans up any partial files from interrupted downloads)
    // Progress is reported from within scan_output_dir
    let state = scan_output_dir(output_dir, &reporter)?;

    // Report if any partial files were cleaned up
    if state.partial_files_cleaned > 0 {
        reporter.report(ProgressEvent::PartialFilesCleanedUp {
            count: state.partial_files_cleaned,
        });
    }

    for path in &state.unreadable_metadata {
        reporter.report(ProgressEvent::MetadataUnreadable { path: path.clone() });
    }

    // Create sync plan (episodes are sorted by pub_date, newest first)
    let plan = create_sync_plan(podcast.episodes.clone(), &state);

    // Track new episodes count before applying limit
    let new_episodes_count = plan.to_download.len();

    // Apply limit if specified
    let to_download: Vec<_> = if let Some(limit) = options.limit {
        plan.to_download.into_iter().take(limit).collect()
    } else {
        plan.to_download
    };

    let total_to_download = to_download.len();
    let existing = plan.already_present.len();
    let limited = new_episodes_count.saturating_sub(total_to_download);

    reporter.report(ProgressEvent::SyncPlanReady {
        podcast_title: podcast.title.clone(),
        total_episodes: plan.total_episodes,
        new_episodes: new_episodes_count,
        to_download: total_to_download,
    });

    // Write podcast metadata
    write_podcast_metadata(&podcast, output_dir)?;

    if to_download.is_empty() {
        reporter.report(ProgressEvent::SyncCompleted {
            downloaded_count: 0,
            existing_count: existing,
            limited_count: limited,
            failed_count: 0,
        });

        return Ok(SyncResult {
            downloaded: 0,
            skipped: existing,
            failed: 0,
            failed_episodes: vec![],
        });
    }

    // Download episodes in parallel using a slot pool
    // The slot pool serves dual purpose: limits concurrency AND provides stable slot IDs
    let (slot_tx, slot_rx) = tokio::sync::mpsc::channel(options.max_concurrent);
    for slot in 0..options.max_concurrent {
        slot_tx.send(slot).await.unwrap();
    }
    let slot_rx = Arc::new(Mutex::new(slot_rx));

    let downloaded_count = Arc::new(AtomicUsize::new(0));
    let failed_count = Arc::new(AtomicUsize::new(0));
    let failed_episodes = Arc::new(Mutex::new(Vec::new()));

    let output_dir = output_dir.to_path_buf();
    let client = client.clone();

    let mut handles = Vec::new();

    for (episode_index, planned) in to_download.into_iter().enumerate() {
        // Acquire a slot from the pool BEFORE spawning (blocks until one is free)
        // This ensures episodes are started in order
        let download_id = slot_rx.lock().await.recv().await.unwrap();

        let slot_tx = slot_tx.clone();
        let client = client.clone();
        let output_dir = output_dir.clone();
        let reporter = reporter.clone();
        let downloaded_count = downloaded_count.clone();
        let failed_count = failed_count.clone();
        let failed_episodes = failed_episodes.clone();
        let continue_on_error = options.continue_on_error;

        let handle = tokio::spawn(async move {
            let context = DownloadContext {
                download_id,
                episode_index,
                total_to_download,
            };

            let episode = planned.episode;
            let audio_path = output_dir.join(&planned.audio_filename);
            let metadata_path = output_dir.join(&planned.metadata_filename);

            let result =
                download_episode(&client, &episode, &audio_path, &context, &reporter).await;

            let return_result = match result {
                Ok(download_result) => {
                    // Write episode metadata with content hash
                    if let Err(e) = write_episode_metadata(
                        &episode,
                        &planned.audio_filename,
                        Some(download_result.content_hash),
                        &metadata_path,
                    ) {
                        reporter.report(ProgressEvent::DownloadFailed {
                            download_id,
                            episode_title: episode.title.clone(),
                            error: format!("Failed to write metadata: {}", e),
                        });
                        failed_count.fetch_add(1, Ordering::SeqCst);
                        failed_episodes
                            .lock()
                            .await
                            .push((episode.title.clone(), e.to_string()));
                    } else {
                        downloaded_count.fetch_add(1, Ordering::SeqCst);
                    }
                    Ok(())
                }
                Err(e) => {
                    reporter.report(ProgressEvent::DownloadFailed {
                        download_id,
                        episode_title: episode.title.clone(),
                        error: e.to_string(),
                    });
                    failed_count.fetch_add(1, Ordering::SeqCst);
                    failed_episodes
                        .lock()
                        .await
                        .push((episode.title.clone(), e.to_string()));

                    if !continue_on_error { Err(e) } else { Ok(()) }
                }
            };

            // Return slot to the pool when done
            let _ = slot_tx.send(download_id).await;

            return_result
        });

        handles.push(handle);
    }

    // Wait for all downloads to complete
    for handle in handles {
        let _ = handle.await;
    }

    let downloaded = downloaded_count.load(Ordering::SeqCst);
    let failed = failed_count.load(Ordering::SeqCst);
    let failed_eps = failed_episodes.lock().await.clone();

    reporter.report(ProgressEvent::SyncCompleted {
        downloaded_count: downloaded,
        existing_count: existing,
        limited_count: limited,
        failed_count: failed,
    });

    if downloaded == 0 && failed > 0 && !options.continue_on_error {
        return Err(SyncError::AllDownloadsFailed);
    }

    Ok(SyncResult {
        downloaded,
        skipped: existing,
        failed,
        failed_episodes: failed_eps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::http::{ByteStream, HttpResponse};
    use crate::progress::{NoopReporter, ProgressReporter};
    use async_trait::async_trait;
    use bytes::Bytes;
    use tempfile::tempdir;

    /// Collects every reported event so tests can assert on warnings
    #[derive(Default)]
    struct RecordingReporter {
        events: std::sync::Mutex<Vec<ProgressEvent>>,
    }

    impl ProgressReporter for RecordingReporter {
        fn report(&self, event: ProgressEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    impl RecordingReporter {
        fn events(&self) -> Vec<ProgressEvent> {
            self.events.lock().unwrap().clone()
        }
    }

    #[derive(Clone)]
    struct MockHttpClient {
        feed_xml: String,
        audio_data: Vec<u8>,
    }

    #[async_trait]
    impl HttpClient for MockHttpClient {
        async fn get_bytes(&self, url: &str) -> Result<Bytes, reqwest::Error> {
            if url.ends_with(".xml") || url.contains("feed") {
                Ok(Bytes::from(self.feed_xml.clone()))
            } else {
                Ok(Bytes::from(self.audio_data.clone()))
            }
        }

        async fn get_stream(&self, _url: &str) -> Result<HttpResponse, reqwest::Error> {
            let data = self.audio_data.clone();
            let len = data.len() as u64;

            let stream: ByteStream =
                Box::pin(futures::stream::once(async move { Ok(Bytes::from(data)) }));

            Ok(HttpResponse {
                status: 200,
                content_length: Some(len),
                body: stream,
            })
        }
    }

    const SAMPLE_FEED: &str = r#"<?xml version="1.0"?>
<rss version="2.0">
  <channel>
    <title>Test Podcast</title>
    <description>A test podcast</description>
    <item>
      <title>Episode 1</title>
      <guid>ep1-guid</guid>
      <enclosure url="https://example.com/ep1.mp3" type="audio/mpeg"/>
    </item>
    <item>
      <title>Episode 2</title>
      <guid>ep2-guid</guid>
      <enclosure url="https://example.com/ep2.mp3" type="audio/mpeg"/>
    </item>
  </channel>
</rss>"#;

    #[tokio::test]
    async fn sync_downloads_all_episodes() {
        let dir = tempdir().unwrap();

        let client = MockHttpClient {
            feed_xml: SAMPLE_FEED.to_string(),
            audio_data: b"fake audio".to_vec(),
        };

        let result = sync_podcast(
            &client,
            "https://example.com/feed.xml",
            dir.path(),
            &SyncOptions::default(),
            NoopReporter::shared(),
        )
        .await
        .unwrap();

        assert_eq!(result.downloaded, 2);
        assert_eq!(result.skipped, 0);
        assert_eq!(result.failed, 0);

        // Check files exist
        assert!(dir.path().join("podcast.json").exists());
    }

    #[tokio::test]
    async fn sync_respects_limit() {
        let dir = tempdir().unwrap();

        let client = MockHttpClient {
            feed_xml: SAMPLE_FEED.to_string(),
            audio_data: b"fake audio".to_vec(),
        };

        let options = SyncOptions {
            limit: Some(1),
            ..Default::default()
        };

        let result = sync_podcast(
            &client,
            "https://example.com/feed.xml",
            dir.path(),
            &options,
            NoopReporter::shared(),
        )
        .await
        .unwrap();

        assert_eq!(result.downloaded, 1);
    }

    #[tokio::test]
    async fn sync_skips_existing_episodes() {
        let dir = tempdir().unwrap();

        let client = MockHttpClient {
            feed_xml: SAMPLE_FEED.to_string(),
            audio_data: b"fake audio".to_vec(),
        };

        // First sync
        sync_podcast(
            &client,
            "https://example.com/feed.xml",
            dir.path(),
            &SyncOptions::default(),
            NoopReporter::shared(),
        )
        .await
        .unwrap();

        // Second sync should skip all
        let result = sync_podcast(
            &client,
            "https://example.com/feed.xml",
            dir.path(),
            &SyncOptions::default(),
            NoopReporter::shared(),
        )
        .await
        .unwrap();

        assert_eq!(result.downloaded, 0);
        assert_eq!(result.skipped, 2);
    }

    #[tokio::test]
    async fn sync_reports_unreadable_episode_metadata() {
        let dir = tempdir().unwrap();
        let truncated = dir.path().join("2024-01-15-Episode 9.json");
        std::fs::write(&truncated, b"{\"title\": \"Epis").unwrap();

        let client = MockHttpClient {
            feed_xml: SAMPLE_FEED.to_string(),
            audio_data: b"fake audio".to_vec(),
        };
        let reporter = Arc::new(RecordingReporter::default());

        sync_podcast(
            &client,
            "https://example.com/feed.xml",
            dir.path(),
            &SyncOptions::default(),
            reporter.clone(),
        )
        .await
        .unwrap();

        let reported: Vec<_> = reporter
            .events()
            .into_iter()
            .filter_map(|event| match event {
                ProgressEvent::MetadataUnreadable { path, .. } => Some(path),
                _ => None,
            })
            .collect();
        assert_eq!(reported, vec![truncated]);
    }

    // =========================================================
    // Filename collisions
    // =========================================================

    /// One item of a generated test feed
    struct FeedItem {
        title: &'static str,
        pub_date: &'static str,
        guid: &'static str,
    }

    // Two distinct episodes (different GUIDs and enclosures) sharing title and
    // publication day, as found in a real feed where the publisher duplicated
    // an entry. Both map to the same base filename.
    const NOMAD_REUPLOAD: FeedItem = FeedItem {
        title: "SFT Bits: Sega Nomad",
        pub_date: "Thu, 19 Dec 2024 10:45:35 GMT",
        guid: "nomad-reupload",
    };
    const NOMAD_ORIGINAL: FeedItem = FeedItem {
        title: "SFT Bits: Sega Nomad",
        pub_date: "Thu, 19 Dec 2024 10:25:22 GMT",
        guid: "nomad-original",
    };

    fn feed_xml(items: &[FeedItem]) -> String {
        let items: String = items
            .iter()
            .map(|item| {
                format!(
                    r#"
    <item>
      <title>{}</title>
      <pubDate>{}</pubDate>
      <guid>{}</guid>
      <enclosure url="https://example.com/{}.mp3" type="audio/mpeg"/>
    </item>"#,
                    item.title, item.pub_date, item.guid, item.guid
                )
            })
            .collect();

        format!(
            r#"<?xml version="1.0"?>
<rss version="2.0">
  <channel>
    <title>Test Podcast</title>
    <description>A test podcast</description>{}
  </channel>
</rss>"#,
            items
        )
    }

    fn client_for(items: &[FeedItem]) -> MockHttpClient {
        MockHttpClient {
            feed_xml: feed_xml(items),
            audio_data: b"fake audio".to_vec(),
        }
    }

    async fn sync_with(dir: &Path, client: &MockHttpClient, options: &SyncOptions) -> SyncResult {
        sync_podcast(
            client,
            "https://example.com/feed.xml",
            dir,
            options,
            NoopReporter::shared(),
        )
        .await
        .unwrap()
    }

    /// Store an episode the way an earlier sync run would have left it:
    /// audio and metadata under `stem`, with the hash of `audio` recorded.
    fn store_episode(dir: &Path, stem: &str, item: &FeedItem, audio: &[u8]) {
        use sha2::{Digest, Sha256};

        let episode = crate::feed::Episode {
            title: item.title.to_string(),
            description: None,
            pub_date: chrono::DateTime::parse_from_rfc2822(item.pub_date).ok(),
            guid: Some(item.guid.to_string()),
            enclosure: crate::feed::Enclosure {
                url: url::Url::parse(&format!("https://example.com/{}.mp3", item.guid)).unwrap(),
                length: None,
                mime_type: None,
            },
            duration: None,
            episode_number: None,
            season_number: None,
        };
        let audio_filename = format!("{}.mp3", stem);
        std::fs::write(dir.join(&audio_filename), audio).unwrap();
        write_episode_metadata(
            &episode,
            &audio_filename,
            Some(format!("sha256:{:x}", Sha256::digest(audio))),
            &dir.join(format!("{}.json", stem)),
        )
        .unwrap();
    }

    /// Map every GUID recorded in the readable episode metadata of `dir` to
    /// its audio filename, asserting that the audio file exists.
    fn recorded_episodes(dir: &Path) -> std::collections::HashMap<String, String> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.extension().is_some_and(|ext| ext == "json")
                    && path.file_name().is_some_and(|name| name != "podcast.json")
            })
            .filter_map(|path| crate::metadata::read_episode_metadata(&path).ok())
            .map(|metadata| {
                assert!(dir.join(&metadata.audio_filename).exists());
                (metadata.guid.unwrap(), metadata.audio_filename)
            })
            .collect()
    }

    #[tokio::test]
    async fn sync_keeps_colliding_episodes_apart_within_one_run() {
        let dir = tempdir().unwrap();
        let client = client_for(&[NOMAD_REUPLOAD, NOMAD_ORIGINAL]);

        let result = sync_with(dir.path(), &client, &SyncOptions::default()).await;

        assert_eq!(result.downloaded, 2);
        assert_eq!(result.failed, 0);
        let recorded = recorded_episodes(dir.path());
        assert_eq!(
            recorded["nomad-reupload"],
            "2024-12-19-SFT Bits Sega Nomad.mp3"
        );
        assert_eq!(
            recorded["nomad-original"],
            "2024-12-19-102522-SFT Bits Sega Nomad.mp3"
        );

        // A second run recognises both episodes instead of fetching whichever
        // one lost the shared filename.
        let result = sync_with(dir.path(), &client, &SyncOptions::default()).await;

        assert_eq!(result.downloaded, 0);
        assert_eq!(result.skipped, 2);
    }

    #[tokio::test]
    async fn sync_does_not_overwrite_colliding_episode_from_earlier_run() {
        let dir = tempdir().unwrap();
        let client = client_for(&[NOMAD_REUPLOAD, NOMAD_ORIGINAL]);

        // The first run only fetches the newer episode, so the older one
        // meets an already occupied filename on the second run.
        let limited = SyncOptions {
            limit: Some(1),
            ..Default::default()
        };
        sync_with(dir.path(), &client, &limited).await;
        let result = sync_with(dir.path(), &client, &SyncOptions::default()).await;

        assert_eq!(result.downloaded, 1);
        assert_eq!(result.skipped, 1);
        let recorded = recorded_episodes(dir.path());
        assert_eq!(
            recorded["nomad-reupload"],
            "2024-12-19-SFT Bits Sega Nomad.mp3"
        );
        assert_eq!(
            recorded["nomad-original"],
            "2024-12-19-102522-SFT Bits Sega Nomad.mp3"
        );
    }

    #[tokio::test]
    async fn sync_respects_names_stored_in_decomposed_unicode() {
        let dir = tempdir().unwrap();
        let earlier = FeedItem {
            title: "Neuzugänge #4",
            pub_date: "Fri, 27 Dec 2019 10:12:15 GMT",
            guid: "neuzugaenge-earlier",
        };
        let later = FeedItem {
            title: "Neuzugänge #4",
            pub_date: "Fri, 27 Dec 2019 09:29:26 GMT",
            guid: "neuzugaenge-later",
        };

        // Network shares mounted from macOS list "ä" decomposed into "a" and
        // a combining diaeresis, while feed titles use the composed form.
        let decomposed_stem = "2019-12-27-Neuzuga\u{0308}nge #4";
        store_episode(dir.path(), decomposed_stem, &earlier, b"earlier audio");

        let client = client_for(&[earlier, later]);
        let result = sync_with(dir.path(), &client, &SyncOptions::default()).await;

        assert_eq!(result.downloaded, 1);
        assert_eq!(
            std::fs::read(dir.path().join(format!("{}.mp3", decomposed_stem))).unwrap(),
            b"earlier audio"
        );
        assert_eq!(
            recorded_episodes(dir.path())["neuzugaenge-later"],
            "2019-12-27-092926-Neuzugänge #4.mp3"
        );
    }

    #[tokio::test]
    async fn sync_treats_names_differing_only_in_case_as_colliding() {
        let dir = tempdir().unwrap();
        let lowercase = FeedItem {
            title: "SFT Bits: Sega nomad",
            pub_date: "Thu, 19 Dec 2024 10:45:35 GMT",
            guid: "nomad-lowercase",
        };
        store_episode(
            dir.path(),
            "2024-12-19-SFT Bits Sega nomad",
            &lowercase,
            b"lowercase audio",
        );

        let client = client_for(&[lowercase, NOMAD_ORIGINAL]);
        let result = sync_with(dir.path(), &client, &SyncOptions::default()).await;

        assert_eq!(result.downloaded, 1);
        assert_eq!(
            std::fs::read(dir.path().join("2024-12-19-SFT Bits Sega nomad.mp3")).unwrap(),
            b"lowercase audio"
        );
        assert_eq!(
            recorded_episodes(dir.path())["nomad-original"],
            "2024-12-19-102522-SFT Bits Sega Nomad.mp3"
        );
    }

    #[tokio::test]
    async fn sync_does_not_overwrite_audio_without_metadata() {
        let dir = tempdir().unwrap();
        let orphan = dir.path().join("2024-12-19-SFT Bits Sega Nomad.mp3");
        std::fs::write(&orphan, b"audio of an unknown episode").unwrap();

        let client = client_for(&[NOMAD_ORIGINAL]);
        let result = sync_with(dir.path(), &client, &SyncOptions::default()).await;

        assert_eq!(result.downloaded, 1);
        assert_eq!(
            std::fs::read(&orphan).unwrap(),
            b"audio of an unknown episode"
        );
        assert_eq!(
            recorded_episodes(dir.path())["nomad-original"],
            "2024-12-19-102522-SFT Bits Sega Nomad.mp3"
        );
    }

    #[tokio::test]
    async fn sync_does_not_overwrite_episode_with_unreadable_metadata() {
        let dir = tempdir().unwrap();
        let audio = dir.path().join("2024-12-19-SFT Bits Sega Nomad.mp3");
        std::fs::write(&audio, b"audio of an unknown episode").unwrap();
        std::fs::write(
            dir.path().join("2024-12-19-SFT Bits Sega Nomad.json"),
            b"{\"title\": \"SFT",
        )
        .unwrap();

        let client = client_for(&[NOMAD_ORIGINAL]);
        let result = sync_with(dir.path(), &client, &SyncOptions::default()).await;

        assert_eq!(result.downloaded, 1);
        assert_eq!(
            std::fs::read(&audio).unwrap(),
            b"audio of an unknown episode"
        );
        assert_eq!(
            recorded_episodes(dir.path())["nomad-original"],
            "2024-12-19-102522-SFT Bits Sega Nomad.mp3"
        );
    }
}
