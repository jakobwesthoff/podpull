// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

mod downloads;
mod verify;

use std::path::{Path, PathBuf};

use url::Url;

use crate::damage::DamagedAudio;
use crate::error::{FeedError, SyncError};
use crate::feed::{
    Podcast, fetch_feed_bytes, file_path_to_url, is_url, parse_feed, read_feed_file,
};
use crate::http::HttpClient;
use crate::metadata::write_podcast_metadata;
use crate::progress::{ProgressEvent, ProgressReporter};
use crate::state::{UnreadableMetadata, archive_check_targets, create_sync_plan, scan_output_dir};
use downloads::download_all;
use verify::verify_stored_audio;

/// Options for podcast synchronization
#[derive(Debug, Clone)]
pub struct SyncOptions {
    /// Maximum number of episodes to download (None = all)
    pub limit: Option<usize>,
    /// Maximum number of concurrent downloads; 0 counts as 1
    pub max_concurrent: usize,
    /// Which stored audio to check against its recorded hash, and whether
    /// to download mismatched episodes again
    pub audio_check: AudioCheck,
}

/// Which stored audio a sync checks against the hash recorded when it was
/// downloaded
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCheck {
    /// Only audio a new episode's base filename collides with, which is
    /// where podpull 1.1.2 and earlier could leave damage; mismatches are
    /// reported
    Collisions,
    /// All stored audio, reading the whole archive; mismatches are reported
    Verify,
    /// All stored audio; mismatched episodes still in the feed are
    /// downloaded again under their existing names
    Repair,
}

/// Options the tests start from; the CLI sets every option itself
#[cfg(test)]
impl Default for SyncOptions {
    fn default() -> Self {
        Self {
            limit: None,
            max_concurrent: 3,
            audio_check: AudioCheck::Collisions,
        }
    }
}

/// Result of a sync operation
#[derive(Debug, Clone, Default)]
pub struct SyncResult {
    /// Number of new episodes successfully downloaded
    pub downloaded: usize,
    /// Number of episodes whose damaged audio was downloaded again
    pub repaired: usize,
    /// Number of episodes already present in the output directory
    pub existing: usize,
    /// Number of new episodes the limit held back
    pub limited: usize,
    /// Episodes whose download failed
    pub failed_episodes: Vec<FailedEpisode>,
    /// Stored audio found damaged or missing and left as it is
    pub damaged: Vec<DamagedAudio>,
    /// Number of new episodes whose audio was already stored byte for byte,
    /// so only their GUID was recorded
    pub adopted: usize,
    /// Partial files the scan could not remove; their episodes cannot be
    /// downloaded until they are deleted
    pub stuck_partial_files: Vec<PathBuf>,
    /// Episode metadata files whose content is not valid metadata
    pub unreadable_metadata: Vec<UnreadableMetadata>,
    /// Stored audio that could not be read to check it
    pub unverifiable_audio: Vec<UnverifiableAudio>,
}

/// An episode whose download failed
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedEpisode {
    pub title: String,
    pub error: String,
}

/// Stored audio that could not be read to check it against its recorded
/// hash
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnverifiableAudio {
    pub audio_filename: String,
    pub error: String,
}

/// Synchronize a podcast feed to a local directory
///
/// This is the main entry point for the library. It:
/// 1. Fetches or reads the feed and parses it
/// 2. Scans the output directory for existing downloads
/// 3. Creates a sync plan
/// 4. Checks stored audio against its recorded hash, as
///    [`SyncOptions::audio_check`] selects, and plans repairs
/// 5. Writes the podcast metadata
/// 6. Downloads repairs and new episodes concurrently, each with its
///    metadata
pub async fn sync_podcast<C: HttpClient>(
    client: &C,
    feed_source: &str,
    output_dir: &Path,
    options: &SyncOptions,
    reporter: &dyn ProgressReporter,
) -> Result<SyncResult, SyncError> {
    let podcast = load_podcast(client, feed_source, reporter).await?;

    // Scan output directory (also cleans up any partial files from interrupted downloads)
    // Progress is reported from within scan_output_dir
    let state = scan_output_dir(output_dir, reporter)?;

    // Report if any partial files were cleaned up
    if state.partial_files_cleaned() > 0 {
        reporter.report(ProgressEvent::PartialFilesCleanedUp {
            count: state.partial_files_cleaned(),
        });
    }

    for path in state.stuck_partial_files() {
        reporter.report(ProgressEvent::PartialFileStuck { path: path.clone() });
    }

    for unreadable in state.unreadable_metadata() {
        reporter.report(ProgressEvent::MetadataUnreadable {
            path: unreadable.path.clone(),
            error: unreadable.error.clone(),
        });
    }

    // Create sync plan (episodes are sorted by pub_date, newest first)
    let plan = create_sync_plan(podcast.episodes.clone(), &state, options.limit);
    let limited = plan.new_episodes - plan.to_download.len();

    let targets = match options.audio_check {
        AudioCheck::Collisions => plan.collisions.clone(),
        AudioCheck::Verify | AudioCheck::Repair => archive_check_targets(&state, &plan),
    };
    let verification = verify_stored_audio(
        &targets,
        state.output_dir(),
        options.audio_check == AudioCheck::Repair,
        reporter,
    )
    .await;
    let repairs = verification.repairs.len();

    reporter.report(ProgressEvent::SyncPlanReady {
        podcast_title: podcast.title.clone(),
        total_episodes: plan.total_episodes,
        new_episodes: plan.new_episodes,
        to_download: plan.to_download.len(),
        repairs,
    });

    // An episode whose audio is repaired or found damaged no longer counts
    // as existing. A repair replaces an episode that is already present
    // rather than adding a new one, so the limit does not apply to it.
    let existing = plan
        .already_present
        .len()
        .saturating_sub(verification.affected_present_guids.len());
    let mut to_download = verification.repairs;
    to_download.extend(plan.to_download);

    // Write podcast metadata
    write_podcast_metadata(&podcast, output_dir)?;

    let totals = download_all(
        client,
        to_download,
        &state,
        &verification.intact,
        reporter,
        options,
    )
    .await;
    let downloaded = totals.downloaded;
    let failed_episodes = totals.failed_episodes;

    reporter.report(ProgressEvent::SyncCompleted);

    Ok(SyncResult {
        downloaded,
        repaired: totals.repaired,
        existing,
        limited,
        failed_episodes,
        damaged: verification.damaged,
        adopted: totals.adopted,
        stuck_partial_files: state.stuck_partial_files().to_vec(),
        unreadable_metadata: state.unreadable_metadata().to_vec(),
        unverifiable_audio: verification.unverifiable,
    })
}

/// Fetch or read the feed and parse it, reporting each phase
async fn load_podcast<C: HttpClient>(
    client: &C,
    feed_source: &str,
    reporter: &dyn ProgressReporter,
) -> Result<Podcast, SyncError> {
    if is_url(feed_source) {
        reporter.report(ProgressEvent::FetchingFeed {
            url: feed_source.to_string(),
        });
        let bytes = fetch_feed_bytes(client, feed_source).await?;

        reporter.report(ProgressEvent::ParsingFeed {
            source: feed_source.to_string(),
        });
        let feed_url =
            Url::parse(feed_source).map_err(|e| SyncError::Feed(FeedError::InvalidUrl(e)))?;
        Ok(parse_feed(&bytes, feed_url)?)
    } else {
        // A local file has no fetching phase.
        reporter.report(ProgressEvent::ParsingFeed {
            source: feed_source.to_string(),
        });
        let bytes = read_feed_file(Path::new(feed_source))?;
        let feed_url = file_path_to_url(Path::new(feed_source));
        Ok(parse_feed(&bytes, feed_url)?)
    }
}

#[cfg(test)]
mod tests {
    use super::downloads::{download_planned, listed_name, replaced_with_identical_audio};
    use super::*;
    use crate::damage::{DamageKind, DamageRemedy};
    use crate::episode::{DownloadContext, hash_file};
    use crate::metadata::write_episode_metadata;
    use crate::state::{OutputState, PlannedDownload, Purpose};
    use std::collections::HashSet;
    use std::path::PathBuf;

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
            &NoopReporter,
        )
        .await
        .unwrap();

        assert_eq!(result.downloaded, 2);
        assert_eq!(result.existing, 0);
        assert_eq!(result.failed_episodes.len(), 0);

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
            &NoopReporter,
        )
        .await
        .unwrap();

        assert_eq!(result.downloaded, 1);
        assert_eq!(result.limited, 1);
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
            &NoopReporter,
        )
        .await
        .unwrap();

        // Second sync should skip all
        let result = sync_podcast(
            &client,
            "https://example.com/feed.xml",
            dir.path(),
            &SyncOptions::default(),
            &NoopReporter,
        )
        .await
        .unwrap();

        assert_eq!(result.downloaded, 0);
        assert_eq!(result.existing, 2);
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
        let reporter = RecordingReporter::default();

        let result = sync_podcast(
            &client,
            "https://example.com/feed.xml",
            dir.path(),
            &SyncOptions::default(),
            &reporter,
        )
        .await
        .unwrap();

        assert_eq!(
            result
                .unreadable_metadata
                .iter()
                .map(|unreadable| &unreadable.path)
                .collect::<Vec<_>>(),
            vec![&truncated]
        );
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

    /// Serves different audio for every enclosure URL, as distinct
    /// episodes have, so identical-audio detection does not merge them
    struct DistinctAudioClient(MockHttpClient);

    #[async_trait]
    impl HttpClient for DistinctAudioClient {
        async fn get_bytes(&self, url: &str) -> Result<Bytes, reqwest::Error> {
            self.0.get_bytes(url).await
        }

        async fn get_stream(&self, url: &str) -> Result<HttpResponse, reqwest::Error> {
            let data = Bytes::from(format!("audio of {}", url));
            let len = data.len() as u64;
            let stream: ByteStream = Box::pin(futures::stream::once(async move { Ok(data) }));
            Ok(HttpResponse {
                status: 200,
                content_length: Some(len),
                body: stream,
            })
        }
    }

    fn client_for(items: &[FeedItem]) -> MockHttpClient {
        MockHttpClient {
            feed_xml: feed_xml(items),
            audio_data: b"fake audio".to_vec(),
        }
    }

    async fn sync_with<C: HttpClient>(dir: &Path, client: &C, options: &SyncOptions) -> SyncResult {
        sync_podcast(
            client,
            "https://example.com/feed.xml",
            dir,
            options,
            &NoopReporter,
        )
        .await
        .unwrap()
    }

    /// Store an episode the way an earlier sync run would have left it:
    /// audio and metadata under `stem`, with the hash of `audio` recorded.
    fn store_episode(dir: &Path, stem: &str, item: &FeedItem, audio: &[u8]) {
        store_episode_as(dir, stem, "mp3", item, audio);
    }

    /// Like [`store_episode`], with the audio under extension `ext`
    fn store_episode_as(dir: &Path, stem: &str, ext: &str, item: &FeedItem, audio: &[u8]) {
        use sha2::{Digest, Sha256};

        let episode = crate::feed::Episode {
            title: item.title.to_string(),
            description: None,
            pub_date: chrono::DateTime::parse_from_rfc2822(item.pub_date).ok(),
            guid: Some(item.guid.to_string()),
            enclosure: crate::feed::Enclosure {
                url: url::Url::parse(&format!("https://example.com/{}.mp3", item.guid)).unwrap(),
                mime_type: None,
            },
            duration: None,
            episode_number: None,
            season_number: None,
        };
        let audio_filename = format!("{}.{}", stem, ext);
        std::fs::write(dir.join(&audio_filename), audio).unwrap();
        write_episode_metadata(
            &episode,
            &audio_filename,
            Some(format!(
                "sha256:{}",
                crate::episode::lower_hex(&Sha256::digest(audio))
            )),
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
        assert_eq!(result.failed_episodes.len(), 0);
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
        assert_eq!(result.existing, 2);
    }

    #[tokio::test]
    async fn sync_does_not_overwrite_colliding_episode_from_earlier_run() {
        let dir = tempdir().unwrap();
        let client = DistinctAudioClient(client_for(&[NOMAD_REUPLOAD, NOMAD_ORIGINAL]));

        // The first run only fetches the newer episode, so the older one
        // meets an already occupied filename on the second run.
        let limited = SyncOptions {
            limit: Some(1),
            ..Default::default()
        };
        sync_with(dir.path(), &client, &limited).await;
        let result = sync_with(dir.path(), &client, &SyncOptions::default()).await;

        assert_eq!(result.downloaded, 1);
        assert_eq!(result.existing, 1);
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

    #[tokio::test]
    async fn sync_keeps_earlier_download_when_feed_changes_guid() {
        let dir = tempdir().unwrap();
        let before = FeedItem {
            guid: "nomad-before-migration",
            ..NOMAD_ORIGINAL
        };
        let after = FeedItem {
            guid: "nomad-after-migration",
            ..NOMAD_ORIGINAL
        };
        store_episode(
            dir.path(),
            "2024-12-19-SFT Bits Sega Nomad",
            &before,
            b"audio before migration",
        );

        // Title and publication time alone cannot prove that the new GUID
        // names the same episode, so the earlier file must survive.
        let result = sync_with(dir.path(), &client_for(&[after]), &SyncOptions::default()).await;

        assert_eq!(result.downloaded, 1);
        assert_eq!(
            std::fs::read(dir.path().join("2024-12-19-SFT Bits Sega Nomad.mp3")).unwrap(),
            b"audio before migration"
        );
        let recorded = recorded_episodes(dir.path());
        assert_eq!(
            recorded["nomad-before-migration"],
            "2024-12-19-SFT Bits Sega Nomad.mp3"
        );
        assert_eq!(
            recorded["nomad-after-migration"],
            "2024-12-19-102522-SFT Bits Sega Nomad.mp3"
        );
    }

    // =========================================================
    // Verification of audio a collision points at
    // =========================================================

    const NOMAD_STEM: &str = "2024-12-19-SFT Bits Sega Nomad";

    async fn sync_recording(dir: &Path, items: &[FeedItem]) -> (SyncResult, Vec<ProgressEvent>) {
        let reporter = RecordingReporter::default();
        let result = sync_podcast(
            &client_for(items),
            "https://example.com/feed.xml",
            dir,
            &SyncOptions::default(),
            &reporter,
        )
        .await
        .unwrap();
        (result, reporter.events())
    }

    fn mismatch_events(events: &[ProgressEvent]) -> Vec<(String, String, DamageRemedy)> {
        events
            .iter()
            .filter_map(|event| match event {
                ProgressEvent::StoredAudioDamaged {
                    episode_title,
                    audio_filename,
                    kind: DamageKind::Mismatch,
                    remedy,
                } => Some((episode_title.clone(), audio_filename.clone(), *remedy)),
                _ => None,
            })
            .collect()
    }

    fn verifying_events(events: &[ProgressEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|event| match event {
                ProgressEvent::VerifyingStoredAudio { audio_filename, .. } => {
                    Some(audio_filename.clone())
                }
                _ => None,
            })
            .collect()
    }

    fn damage(remedy: DamageRemedy) -> DamagedAudio {
        DamagedAudio {
            episode_title: "SFT Bits: Sega Nomad".to_string(),
            audio_filename: format!("{}.mp3", NOMAD_STEM),
            kind: DamageKind::Mismatch,
            remedy,
        }
    }

    fn missing(remedy: DamageRemedy) -> DamagedAudio {
        DamagedAudio {
            kind: DamageKind::Missing,
            ..damage(remedy)
        }
    }

    /// Store NOMAD_REUPLOAD under the base name with audio that no longer
    /// matches its recorded hash
    fn store_damaged_nomad(dir: &Path) -> PathBuf {
        store_episode(dir, NOMAD_STEM, &NOMAD_REUPLOAD, b"clean audio");
        let audio = dir.join(format!("{}.mp3", NOMAD_STEM));
        // Two concurrent downloads into one file, as podpull 1.1.2 and
        // earlier did for colliding episodes, leave bytes that match the
        // hash recorded by neither download.
        std::fs::write(&audio, b"interleaved audio").unwrap();
        audio
    }

    #[tokio::test]
    async fn sync_reports_damaged_audio_of_colliding_episode() {
        let dir = tempdir().unwrap();
        let audio = store_damaged_nomad(dir.path());

        let (result, events) = sync_recording(dir.path(), &[NOMAD_REUPLOAD, NOMAD_ORIGINAL]).await;

        assert_eq!(result.downloaded, 1);
        assert_eq!(result.failed_episodes.len(), 0);
        assert_eq!(result.existing, 0);
        assert_eq!(result.damaged, vec![damage(DamageRemedy::RepairAvailable)]);
        assert_eq!(
            mismatch_events(&events),
            vec![(
                "SFT Bits: Sega Nomad".to_string(),
                format!("{}.mp3", NOMAD_STEM),
                DamageRemedy::RepairAvailable
            )]
        );
        // Without --repair the file stays: it may hold edits made by the user.
        assert_eq!(std::fs::read(&audio).unwrap(), b"interleaved audio");
    }

    #[tokio::test]
    async fn sync_accepts_intact_audio_of_colliding_episode() {
        let dir = tempdir().unwrap();
        store_episode(dir.path(), NOMAD_STEM, &NOMAD_REUPLOAD, b"clean audio");

        let (result, events) = sync_recording(dir.path(), &[NOMAD_REUPLOAD, NOMAD_ORIGINAL]).await;

        assert!(result.damaged.is_empty());
        assert!(mismatch_events(&events).is_empty());
        assert_eq!(
            verifying_events(&events),
            vec![format!("{}.mp3", NOMAD_STEM)]
        );
    }

    #[tokio::test]
    async fn sync_verifies_audio_once_when_several_episodes_collide_with_it() {
        let dir = tempdir().unwrap();
        store_damaged_nomad(dir.path());
        let third = FeedItem {
            pub_date: "Thu, 19 Dec 2024 09:00:00 GMT",
            guid: "nomad-third",
            ..NOMAD_ORIGINAL
        };

        let (result, events) =
            sync_recording(dir.path(), &[NOMAD_REUPLOAD, NOMAD_ORIGINAL, third]).await;

        assert_eq!(result.downloaded, 2);
        assert_eq!(result.damaged.len(), 1);
        assert_eq!(verifying_events(&events).len(), 1);
    }

    #[tokio::test]
    async fn sync_names_no_remedy_for_damaged_audio_without_guid() {
        let dir = tempdir().unwrap();
        store_damaged_nomad(dir.path());
        let metadata_path = dir.path().join(format!("{}.json", NOMAD_STEM));
        let mut metadata = crate::metadata::read_episode_metadata(&metadata_path).unwrap();
        metadata.guid = None;
        std::fs::write(&metadata_path, serde_json::to_string(&metadata).unwrap()).unwrap();

        let (result, _) = sync_recording(dir.path(), &[NOMAD_ORIGINAL]).await;

        assert_eq!(result.damaged, vec![damage(DamageRemedy::NoFeedEpisode)]);
    }

    #[tokio::test]
    async fn sync_skips_verification_without_recorded_hash() {
        let dir = tempdir().unwrap();
        store_episode(dir.path(), NOMAD_STEM, &NOMAD_REUPLOAD, b"clean audio");
        let metadata_path = dir.path().join(format!("{}.json", NOMAD_STEM));
        let mut metadata = crate::metadata::read_episode_metadata(&metadata_path).unwrap();
        metadata.content_hash = None;
        std::fs::write(&metadata_path, serde_json::to_string(&metadata).unwrap()).unwrap();
        std::fs::write(dir.path().join(format!("{}.mp3", NOMAD_STEM)), b"edited").unwrap();

        let (result, events) = sync_recording(dir.path(), &[NOMAD_REUPLOAD, NOMAD_ORIGINAL]).await;

        assert!(result.damaged.is_empty());
        assert!(verifying_events(&events).is_empty());
    }

    #[tokio::test]
    async fn sync_reports_missing_audio_of_colliding_episode() {
        let dir = tempdir().unwrap();
        store_episode(dir.path(), NOMAD_STEM, &NOMAD_REUPLOAD, b"clean audio");
        std::fs::remove_file(dir.path().join(format!("{}.mp3", NOMAD_STEM))).unwrap();

        let (result, events) = sync_recording(dir.path(), &[NOMAD_REUPLOAD, NOMAD_ORIGINAL]).await;

        assert_eq!(result.damaged, vec![missing(DamageRemedy::RepairAvailable)]);
        assert!(events.iter().any(|event| matches!(
            event,
            ProgressEvent::StoredAudioDamaged {
                kind: DamageKind::Missing,
                remedy: DamageRemedy::RepairAvailable,
                ..
            }
        )));
        // The listing already shows the file is gone; there is nothing to
        // read.
        assert!(verifying_events(&events).is_empty());
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, ProgressEvent::StoredAudioUnverifiable { .. }))
        );
    }

    #[tokio::test]
    async fn sync_verify_reports_missing_audio_without_recorded_hash() {
        let dir = tempdir().unwrap();
        store_episode(dir.path(), NOMAD_STEM, &NOMAD_REUPLOAD, b"clean audio");
        let metadata_path = dir.path().join(format!("{}.json", NOMAD_STEM));
        let mut metadata = crate::metadata::read_episode_metadata(&metadata_path).unwrap();
        metadata.content_hash = None;
        metadata.stage(&metadata_path).unwrap().commit().unwrap();
        std::fs::remove_file(dir.path().join(format!("{}.mp3", NOMAD_STEM))).unwrap();

        let (result, _) = sync_checking(dir.path(), &[NOMAD_REUPLOAD], AudioCheck::Verify).await;

        assert_eq!(result.damaged, vec![missing(DamageRemedy::RepairAvailable)]);
    }

    #[tokio::test]
    async fn sync_repair_downloads_missing_audio_again() {
        let dir = tempdir().unwrap();
        store_episode(dir.path(), NOMAD_STEM, &NOMAD_REUPLOAD, b"clean audio");
        let audio = dir.path().join(format!("{}.mp3", NOMAD_STEM));
        std::fs::remove_file(&audio).unwrap();

        let (result, _) = sync_checking(dir.path(), &[NOMAD_REUPLOAD], AudioCheck::Repair).await;

        assert_eq!(result.repaired, 1);
        assert!(result.damaged.is_empty());
        assert_eq!(std::fs::read(&audio).unwrap(), b"fake audio");
    }

    #[tokio::test]
    async fn sync_warns_when_audio_cannot_be_read_for_verification() {
        let dir = tempdir().unwrap();
        store_episode(dir.path(), NOMAD_STEM, &NOMAD_REUPLOAD, b"clean audio");
        let audio = dir.path().join(format!("{}.mp3", NOMAD_STEM));
        std::fs::remove_file(&audio).unwrap();
        std::fs::create_dir(&audio).unwrap();

        let (result, events) = sync_recording(dir.path(), &[NOMAD_REUPLOAD, NOMAD_ORIGINAL]).await;

        // The check could not run, which says nothing about the file.
        assert!(result.damaged.is_empty());
        assert_eq!(result.unverifiable_audio.len(), 1);
        assert_eq!(
            result.unverifiable_audio[0].audio_filename,
            format!("{}.mp3", NOMAD_STEM)
        );
        assert!(events.iter().any(|event| matches!(
            event,
            ProgressEvent::StoredAudioUnverifiable { audio_filename, .. }
                if *audio_filename == format!("{}.mp3", NOMAD_STEM)
        )));
    }

    // =========================================================
    // Failures during the download loop
    // =========================================================

    #[tokio::test]
    async fn sync_counts_failed_metadata_write_as_failure() {
        let dir = tempdir().unwrap();
        // The directory scan cannot remove a directory, so it keeps blocking
        // the partial file the metadata is written through.
        std::fs::create_dir(dir.path().join(format!("{}.json.partial", NOMAD_STEM))).unwrap();

        let client = client_for(&[NOMAD_ORIGINAL]);
        let result = sync_with(dir.path(), &client, &SyncOptions::default()).await;

        assert_eq!(result.downloaded, 0);
        assert_eq!(result.failed_episodes.len(), 1);
        assert_eq!(result.failed_episodes[0].title, "SFT Bits: Sega Nomad");
        // Audio without metadata would claim its name and make the next sync
        // store the episode a second time under another one.
        assert!(!dir.path().join(format!("{}.mp3", NOMAD_STEM)).exists());
        assert!(
            !dir.path()
                .join(format!("{}.mp3.partial", NOMAD_STEM))
                .exists()
        );
    }

    #[tokio::test]
    async fn sync_reports_partial_files_it_cannot_remove() {
        let dir = tempdir().unwrap();
        let stuck = dir.path().join(format!("{}.mp3.partial", NOMAD_STEM));
        std::fs::create_dir(&stuck).unwrap();

        let (result, events) = sync_recording(dir.path(), &[NOMAD_ORIGINAL]).await;

        assert_eq!(result.failed_episodes.len(), 1);
        assert!(result.failed_episodes[0].error.contains("already exists"));
        assert_eq!(result.stuck_partial_files, vec![stuck.clone()]);
        assert!(events.iter().any(|event| matches!(
            event,
            ProgressEvent::PartialFileStuck { path } if *path == stuck
        )));
    }

    #[tokio::test]
    async fn sync_counts_occupied_partial_file_as_failure() {
        let dir = tempdir().unwrap();
        std::fs::create_dir(dir.path().join(format!("{}.mp3.partial", NOMAD_STEM))).unwrap();

        let client = client_for(&[NOMAD_ORIGINAL]);
        let result = sync_with(dir.path(), &client, &SyncOptions::default()).await;

        assert_eq!(result.downloaded, 0);
        assert_eq!(result.failed_episodes.len(), 1);
        assert!(!dir.path().join(format!("{}.mp3", NOMAD_STEM)).exists());
    }

    // =========================================================
    // Opt-in repair of damaged audio
    // =========================================================

    async fn sync_repairing(
        dir: &Path,
        items: &[FeedItem],
        limit: Option<usize>,
    ) -> (SyncResult, Vec<ProgressEvent>) {
        let reporter = RecordingReporter::default();
        let options = SyncOptions {
            limit,
            audio_check: AudioCheck::Repair,
            ..Default::default()
        };
        let result = sync_podcast(
            &client_for(items),
            "https://example.com/feed.xml",
            dir,
            &options,
            &reporter,
        )
        .await
        .unwrap();
        (result, reporter.events())
    }

    #[tokio::test]
    async fn sync_repairs_damaged_audio_in_place_when_enabled() {
        let dir = tempdir().unwrap();
        let audio = store_damaged_nomad(dir.path());

        let (result, events) =
            sync_repairing(dir.path(), &[NOMAD_REUPLOAD, NOMAD_ORIGINAL], None).await;

        assert_eq!(result.downloaded, 1);
        assert_eq!(result.repaired, 1);
        assert_eq!(result.existing, 0);
        assert_eq!(result.failed_episodes.len(), 0);
        assert!(result.damaged.is_empty());
        assert!(
            events
                .iter()
                .any(|event| matches!(event, ProgressEvent::SyncCompleted))
        );
        assert_eq!(std::fs::read(&audio).unwrap(), b"fake audio");
        let metadata = crate::metadata::read_episode_metadata(
            &dir.path().join(format!("{}.json", NOMAD_STEM)),
        )
        .unwrap();
        assert_eq!(metadata.guid.as_deref(), Some("nomad-reupload"));
        assert!(metadata.additional_guids.is_empty());
        assert_eq!(metadata.content_hash, hash_file(&audio, |_, _| {}).ok());
        assert_eq!(
            mismatch_events(&events),
            vec![(
                "SFT Bits: Sega Nomad".to_string(),
                format!("{}.mp3", NOMAD_STEM),
                DamageRemedy::Repairing
            )]
        );
    }

    #[tokio::test]
    async fn sync_repairs_damaged_audio_beyond_the_limit() {
        let dir = tempdir().unwrap();
        store_damaged_nomad(dir.path());
        let older = FeedItem {
            title: "Older Episode",
            pub_date: "Mon, 01 Jan 2024 12:00:00 GMT",
            guid: "older",
        };

        // The limit admits only the newest new episode; the repair of the
        // episode it collides with comes on top.
        let (result, events) = sync_repairing(
            dir.path(),
            &[NOMAD_REUPLOAD, NOMAD_ORIGINAL, older],
            Some(1),
        )
        .await;

        assert_eq!(result.downloaded, 1);
        assert_eq!(result.repaired, 1);
        assert_eq!(result.failed_episodes.len(), 0);
        assert!(!recorded_episodes(dir.path()).contains_key("older"));
        // The repair is reported apart from the new episodes, so the limit
        // stays visible.
        assert!(events.iter().any(|event| matches!(
            event,
            ProgressEvent::SyncPlanReady {
                new_episodes: 2,
                to_download: 1,
                repairs: 1,
                ..
            }
        )));
    }

    #[tokio::test]
    async fn sync_reports_damage_it_cannot_repair() {
        let dir = tempdir().unwrap();
        // The damaged episode has left the feed, so there is nothing to
        // download it from.
        let audio = store_damaged_nomad(dir.path());

        let (result, _) = sync_repairing(dir.path(), &[NOMAD_ORIGINAL], None).await;

        assert_eq!(result.downloaded, 1);
        assert_eq!(result.failed_episodes.len(), 0);
        assert_eq!(result.damaged, vec![damage(DamageRemedy::NoFeedEpisode)]);
        assert_eq!(std::fs::read(&audio).unwrap(), b"interleaved audio");
    }

    #[tokio::test]
    async fn sync_does_not_repair_audio_whose_feed_format_changed() {
        let dir = tempdir().unwrap();
        store_episode_as(
            dir.path(),
            NOMAD_STEM,
            "m4a",
            &NOMAD_REUPLOAD,
            b"clean audio",
        );
        let audio = dir.path().join(format!("{}.m4a", NOMAD_STEM));
        std::fs::write(&audio, b"interleaved audio").unwrap();

        // The feed now offers the episode as mp3; downloading it into the
        // m4a name would mislabel the audio.
        let (result, _) = sync_repairing(dir.path(), &[NOMAD_REUPLOAD, NOMAD_ORIGINAL], None).await;

        assert_eq!(
            result.damaged,
            vec![DamagedAudio {
                audio_filename: format!("{}.m4a", NOMAD_STEM),
                ..damage(DamageRemedy::EnclosureFormatChanged)
            }]
        );
        assert_eq!(std::fs::read(&audio).unwrap(), b"interleaved audio");
    }

    #[tokio::test]
    async fn sync_repairs_audio_whose_extension_differs_only_in_case() {
        let dir = tempdir().unwrap();
        store_episode_as(
            dir.path(),
            NOMAD_STEM,
            "MP3",
            &NOMAD_REUPLOAD,
            b"clean audio",
        );
        let audio = dir.path().join(format!("{}.MP3", NOMAD_STEM));
        std::fs::write(&audio, b"interleaved audio").unwrap();

        let (result, _) = sync_repairing(dir.path(), &[NOMAD_REUPLOAD, NOMAD_ORIGINAL], None).await;

        assert!(result.damaged.is_empty());
        assert_eq!(std::fs::read(&audio).unwrap(), b"fake audio");
    }

    #[tokio::test]
    async fn sync_repairs_every_damaged_copy_of_an_episode() {
        let dir = tempdir().unwrap();
        // A copy of the episode under another name, with its metadata
        // unchanged, as a user might make by hand.
        let copy_stem = "2024-12-19-SFT Bits Sega Nomad Copy";
        store_damaged_nomad(dir.path());
        store_episode(dir.path(), copy_stem, &NOMAD_REUPLOAD, b"clean audio");
        std::fs::write(dir.path().join(format!("{}.mp3", copy_stem)), b"damaged").unwrap();
        let copy_collider = FeedItem {
            title: "SFT Bits: Sega Nomad Copy",
            pub_date: "Thu, 19 Dec 2024 08:00:00 GMT",
            guid: "nomad-copy-collider",
        };

        let (result, _) = sync_repairing(
            dir.path(),
            &[NOMAD_REUPLOAD, NOMAD_ORIGINAL, copy_collider],
            None,
        )
        .await;

        assert_eq!(result.downloaded, 2);
        assert_eq!(result.repaired, 2);
        assert_eq!(result.existing, 0);
        assert!(result.damaged.is_empty());
    }

    /// Panics while a download reports its completion, as a faulty
    /// reporter implementation would
    struct PanickingReporter;

    impl ProgressReporter for PanickingReporter {
        fn report(&self, event: ProgressEvent) {
            if matches!(event, ProgressEvent::DownloadCompleted { .. }) {
                panic!("reporter failure");
            }
        }
    }

    #[tokio::test]
    async fn sync_counts_panicking_downloads_as_failed() {
        let dir = tempdir().unwrap();
        let client = MockHttpClient {
            feed_xml: SAMPLE_FEED.to_string(),
            audio_data: b"fake audio".to_vec(),
        };
        let options = SyncOptions {
            max_concurrent: 1,
            ..Default::default()
        };

        // With a single slot, a download that never hands its slot back
        // would block the second episode forever.
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            sync_podcast(
                &client,
                "https://example.com/feed.xml",
                dir.path(),
                &options,
                &PanickingReporter,
            ),
        )
        .await
        .expect("sync finishes although its downloads panic")
        .unwrap();

        assert_eq!(result.downloaded, 0);
        assert_eq!(result.failed_episodes.len(), 2);
        let mut titles: Vec<_> = result
            .failed_episodes
            .iter()
            .map(|failed| failed.title.as_str())
            .collect();
        titles.sort();
        assert_eq!(titles, vec!["Episode 1", "Episode 2"]);
    }

    // =========================================================
    // Finalizing a single download
    // =========================================================

    fn planned_nomad(purpose: Purpose) -> PlannedDownload {
        let feed = crate::feed::parse_feed(
            feed_xml(&[NOMAD_ORIGINAL]).as_bytes(),
            url::Url::parse("https://example.com/feed.xml").unwrap(),
        )
        .unwrap();
        PlannedDownload {
            episode: feed.episodes[0].clone(),
            stem: NOMAD_STEM.to_string(),
            audio_extension: "mp3".to_string(),
            purpose,
        }
    }

    fn new_episode() -> Purpose {
        Purpose::NewEpisode {
            replaced_candidates: Vec::new(),
        }
    }

    fn repair() -> Purpose {
        Purpose::Repair {
            kept_guids: Vec::new(),
        }
    }

    async fn download_one(dir: &Path, planned: &PlannedDownload) -> Result<(), String> {
        let context = DownloadContext {
            download_id: 0,
            episode_index: 0,
            total_to_download: 1,
        };
        download_planned(
            &client_for(&[NOMAD_ORIGINAL]),
            planned,
            &OutputState::empty(dir),
            &HashSet::new(),
            &context,
            &NoopReporter,
        )
        .await
        .map(|_| ())
    }

    /// Make renaming onto the metadata path fail: a non-empty directory
    /// cannot be replaced by a file.
    fn block_metadata_path(dir: &Path) {
        let metadata_path = dir.join(format!("{}.json", NOMAD_STEM));
        std::fs::create_dir(&metadata_path).unwrap();
        std::fs::write(metadata_path.join("occupant"), b"").unwrap();
    }

    #[tokio::test]
    async fn download_planned_writes_audio_and_metadata() {
        let dir = tempdir().unwrap();

        download_one(dir.path(), &planned_nomad(new_episode()))
            .await
            .unwrap();

        assert_eq!(
            recorded_episodes(dir.path())["nomad-original"],
            format!("{}.mp3", NOMAD_STEM)
        );
    }

    #[tokio::test]
    async fn download_planned_removes_fresh_audio_when_metadata_cannot_be_placed() {
        let dir = tempdir().unwrap();
        block_metadata_path(dir.path());

        let error = download_one(dir.path(), &planned_nomad(new_episode()))
            .await
            .unwrap_err();

        assert!(error.starts_with("Failed to write metadata"));
        assert!(!dir.path().join(format!("{}.mp3", NOMAD_STEM)).exists());
        assert!(
            !dir.path()
                .join(format!("{}.json.partial", NOMAD_STEM))
                .exists()
        );
    }

    #[tokio::test]
    async fn download_planned_keeps_repaired_audio_when_metadata_cannot_be_placed() {
        let dir = tempdir().unwrap();
        block_metadata_path(dir.path());

        // A repair replaced audio that the existing metadata still names, so
        // removing it would leave that metadata without its audio.
        let error = download_one(dir.path(), &planned_nomad(repair()))
            .await
            .unwrap_err();

        assert!(error.starts_with("Failed to write metadata"));
        assert_eq!(
            std::fs::read(dir.path().join(format!("{}.mp3", NOMAD_STEM))).unwrap(),
            b"fake audio"
        );
    }

    #[tokio::test]
    async fn download_planned_discards_metadata_when_audio_cannot_be_placed() {
        let dir = tempdir().unwrap();
        let audio_path = dir.path().join(format!("{}.mp3", NOMAD_STEM));
        std::fs::create_dir(&audio_path).unwrap();
        std::fs::write(audio_path.join("occupant"), b"").unwrap();

        download_one(dir.path(), &planned_nomad(new_episode()))
            .await
            .unwrap_err();

        assert!(!dir.path().join(format!("{}.json", NOMAD_STEM)).exists());
        assert!(
            !dir.path()
                .join(format!("{}.json.partial", NOMAD_STEM))
                .exists()
        );
    }

    // =========================================================
    // Checking the whole archive
    // =========================================================

    async fn sync_checking(
        dir: &Path,
        items: &[FeedItem],
        audio_check: AudioCheck,
    ) -> (SyncResult, Vec<ProgressEvent>) {
        let reporter = RecordingReporter::default();
        let result = sync_podcast(
            &client_for(items),
            "https://example.com/feed.xml",
            dir,
            &SyncOptions {
                audio_check,
                ..Default::default()
            },
            &reporter,
        )
        .await
        .unwrap();
        (result, reporter.events())
    }

    #[tokio::test]
    async fn sync_checks_only_colliding_audio_by_default() {
        let dir = tempdir().unwrap();
        store_damaged_nomad(dir.path());

        let (result, events) =
            sync_checking(dir.path(), &[NOMAD_REUPLOAD], AudioCheck::Collisions).await;

        assert!(result.damaged.is_empty());
        assert!(verifying_events(&events).is_empty());
    }

    #[tokio::test]
    async fn sync_verify_reports_damaged_audio_without_a_collision() {
        let dir = tempdir().unwrap();
        let audio = store_damaged_nomad(dir.path());

        let (result, events) =
            sync_checking(dir.path(), &[NOMAD_REUPLOAD], AudioCheck::Verify).await;

        assert_eq!(result.downloaded, 0);
        assert_eq!(result.damaged, vec![damage(DamageRemedy::RepairAvailable)]);
        assert_eq!(
            verifying_events(&events),
            vec![format!("{}.mp3", NOMAD_STEM)]
        );
        assert_eq!(std::fs::read(&audio).unwrap(), b"interleaved audio");
    }

    #[tokio::test]
    async fn sync_verify_reports_which_file_it_hashes_and_how_far() {
        let dir = tempdir().unwrap();
        let other_stem = "2024-01-01-Other";
        let other = FeedItem {
            title: "Other",
            pub_date: "Mon, 01 Jan 2024 12:00:00 GMT",
            guid: "other",
        };
        store_episode(dir.path(), NOMAD_STEM, &NOMAD_REUPLOAD, b"clean audio");
        store_episode(dir.path(), other_stem, &other, b"other audio");
        // Missing audio needs no reading, so it is not counted.
        let gone_stem = "2024-01-02-Gone";
        let gone = FeedItem {
            title: "Gone",
            pub_date: "Tue, 02 Jan 2024 12:00:00 GMT",
            guid: "gone",
        };
        store_episode(dir.path(), gone_stem, &gone, b"gone audio");
        std::fs::remove_file(dir.path().join(format!("{}.mp3", gone_stem))).unwrap();

        let (_, events) = sync_checking(
            dir.path(),
            &[NOMAD_REUPLOAD, other, gone],
            AudioCheck::Verify,
        )
        .await;

        let progress: Vec<String> = events
            .iter()
            .filter_map(|event| match event {
                ProgressEvent::VerifyingStoredAudio {
                    audio_filename,
                    position,
                    total,
                } => Some(format!("{} {}/{}", audio_filename, position, total)),
                ProgressEvent::HashingProgress {
                    bytes_hashed,
                    total_bytes,
                } => Some(format!("{}/{} bytes", bytes_hashed, total_bytes)),
                _ => None,
            })
            .collect();
        assert_eq!(
            progress,
            vec![
                format!("{}.mp3 1/2", other_stem),
                "11/11 bytes".to_string(),
                format!("{}.mp3 2/2", NOMAD_STEM),
                "11/11 bytes".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn sync_repair_fixes_damaged_audio_without_a_collision() {
        let dir = tempdir().unwrap();
        let audio = store_damaged_nomad(dir.path());

        let (result, _) = sync_checking(dir.path(), &[NOMAD_REUPLOAD], AudioCheck::Repair).await;

        assert_eq!(result.downloaded, 0);
        assert_eq!(result.repaired, 1);
        assert!(result.damaged.is_empty());
        assert_eq!(std::fs::read(&audio).unwrap(), b"fake audio");
    }

    #[tokio::test]
    async fn sync_repair_fixes_damage_reported_by_an_earlier_run() {
        let dir = tempdir().unwrap();
        let audio = store_damaged_nomad(dir.path());
        let feed = [NOMAD_REUPLOAD, NOMAD_ORIGINAL];

        // The first run downloads the colliding episode and reports the
        // damage; afterwards nothing collides any more.
        let (first, _) = sync_checking(dir.path(), &feed, AudioCheck::Collisions).await;
        assert_eq!(first.damaged.len(), 1);

        let (second, _) = sync_checking(dir.path(), &feed, AudioCheck::Repair).await;

        assert_eq!(second.downloaded, 0);
        assert_eq!(second.repaired, 1);
        assert!(second.damaged.is_empty());
        assert_eq!(std::fs::read(&audio).unwrap(), b"fake audio");
    }

    #[tokio::test]
    async fn sync_does_not_store_guidless_episode_again_after_url_token_change() {
        let dir = tempdir().unwrap();
        let feed = |token: &str| {
            format!(
                r#"<?xml version="1.0"?>
<rss version="2.0">
  <channel>
    <title>Test Podcast</title>
    <description>A test podcast</description>
    <item>
      <title>Members Only</title>
      <pubDate>Thu, 19 Dec 2024 10:25:22 GMT</pubDate>
      <enclosure url="https://example.com/media/{}/episode-uuid.mp3" type="audio/mpeg"/>
    </item>
  </channel>
</rss>"#,
                token
            )
        };
        let sync_feed = |feed_xml: String| {
            let dir = dir.path().to_path_buf();
            async move {
                let client = MockHttpClient {
                    feed_xml,
                    audio_data: b"fake audio".to_vec(),
                };
                sync_with(&dir, &client, &SyncOptions::default()).await
            }
        };

        sync_feed(feed("token-a")).await;
        let result = sync_feed(feed("token-b")).await;

        assert_eq!(result.downloaded, 0);
        assert_eq!(result.existing, 1);
        let audio_files = std::fs::read_dir(dir.path())
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .path()
                    .extension()
                    .is_some_and(|ext| ext == "mp3")
            })
            .count();
        assert_eq!(audio_files, 1);
    }

    /// A client that borrows its responses, so it is neither Clone nor
    /// 'static
    struct BorrowingClient<'a> {
        inner: &'a MockHttpClient,
    }

    #[async_trait]
    impl HttpClient for BorrowingClient<'_> {
        async fn get_bytes(&self, url: &str) -> Result<Bytes, reqwest::Error> {
            self.inner.get_bytes(url).await
        }

        async fn get_stream(&self, url: &str) -> Result<HttpResponse, reqwest::Error> {
            self.inner.get_stream(url).await
        }
    }

    #[tokio::test]
    async fn sync_accepts_a_client_that_is_neither_clone_nor_static() {
        let dir = tempdir().unwrap();
        let mock = MockHttpClient {
            feed_xml: SAMPLE_FEED.to_string(),
            audio_data: b"fake audio".to_vec(),
        };

        let result = sync_podcast(
            &BorrowingClient { inner: &mock },
            "https://example.com/feed.xml",
            dir.path(),
            &SyncOptions::default(),
            &NoopReporter,
        )
        .await
        .unwrap();

        assert_eq!(result.downloaded, 2);
    }

    #[test]
    fn sync_future_can_be_spawned() {
        fn assert_send<T: Send>(_: &T) {}
        let mock = MockHttpClient {
            feed_xml: SAMPLE_FEED.to_string(),
            audio_data: Vec::new(),
        };

        let options = SyncOptions::default();

        // Library users run a sync on a spawned task, which needs a Send
        // future; the future is only built here, never polled.
        let sync = sync_podcast(
            &mock,
            "https://example.com/feed.xml",
            Path::new("/nonexistent"),
            &options,
            &NoopReporter,
        );
        assert_send(&sync);
    }

    // =========================================================
    // Re-issued episodes with identical audio
    // =========================================================

    const REISSUED: FeedItem = FeedItem {
        guid: "nomad-reissued",
        ..NOMAD_ORIGINAL
    };

    fn audio_files(dir: &Path) -> Vec<String> {
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".mp3"))
            .collect();
        files.sort();
        files
    }

    #[tokio::test]
    async fn sync_records_new_guid_for_identical_audio_instead_of_a_copy() {
        let dir = tempdir().unwrap();
        // The mock serves "fake audio" for every enclosure, so the re-issued
        // episode downloads byte for byte what is stored.
        store_episode(dir.path(), NOMAD_STEM, &NOMAD_ORIGINAL, b"fake audio");

        let (result, events) = sync_recording(dir.path(), &[REISSUED]).await;

        assert_eq!(result.downloaded, 0);
        assert_eq!(result.adopted, 1);
        assert_eq!(audio_files(dir.path()), vec![format!("{}.mp3", NOMAD_STEM)]);
        let metadata = crate::metadata::read_episode_metadata(
            &dir.path().join(format!("{}.json", NOMAD_STEM)),
        )
        .unwrap();
        assert_eq!(metadata.guid.as_deref(), Some("nomad-original"));
        assert_eq!(
            metadata.additional_guids,
            vec!["nomad-reissued".to_string()]
        );
        assert!(events.iter().any(|event| matches!(
            event,
            ProgressEvent::EpisodeAlreadyStored { audio_filename, .. }
                if *audio_filename == format!("{}.mp3", NOMAD_STEM)
        )));

        let (result, _) = sync_recording(dir.path(), &[REISSUED]).await;

        assert_eq!(result.downloaded, 0);
        assert_eq!(result.adopted, 0);
        assert_eq!(result.existing, 1);
    }

    #[tokio::test]
    async fn sync_keeps_identical_audio_of_two_listed_entries_apart() {
        let dir = tempdir().unwrap();
        store_episode(dir.path(), NOMAD_STEM, &NOMAD_ORIGINAL, b"fake audio");

        // Both entries are in the feed at the same time, so they are two
        // episodes even though their audio is the same.
        let (first, _) = sync_recording(dir.path(), &[NOMAD_ORIGINAL, REISSUED]).await;
        let (second, _) = sync_recording(dir.path(), &[NOMAD_ORIGINAL, REISSUED]).await;

        assert_eq!(first.downloaded, 1);
        assert_eq!(first.adopted, 0);
        assert_eq!(audio_files(dir.path()).len(), 2);
        assert_eq!(second.downloaded, 0);
        assert_eq!(second.existing, 2);
    }

    #[tokio::test]
    async fn sync_keeps_identical_audio_of_a_retitled_entry_apart() {
        let dir = tempdir().unwrap();
        store_episode(dir.path(), NOMAD_STEM, &NOMAD_ORIGINAL, b"fake audio");
        let rerun = FeedItem {
            title: "Best of: Sega Nomad",
            ..REISSUED
        };

        let (result, _) = sync_recording(dir.path(), &[rerun]).await;

        assert_eq!(result.downloaded, 1);
        assert_eq!(result.adopted, 0);
    }

    #[tokio::test]
    async fn sync_keeps_the_download_when_stored_audio_changed_since_its_hash() {
        let dir = tempdir().unwrap();
        // The recorded hash matches the new download, but the file on disk
        // no longer holds those bytes.
        store_episode(dir.path(), NOMAD_STEM, &NOMAD_ORIGINAL, b"fake audio");
        std::fs::write(dir.path().join(format!("{}.mp3", NOMAD_STEM)), b"damaged").unwrap();

        let (result, _) = sync_recording(dir.path(), &[REISSUED]).await;

        assert_eq!(result.adopted, 0);
        assert_eq!(result.downloaded, 1);
        assert_eq!(audio_files(dir.path()).len(), 2);
    }

    #[tokio::test]
    async fn sync_fails_an_identical_download_whose_guid_cannot_be_recorded() {
        let dir = tempdir().unwrap();
        store_episode(dir.path(), NOMAD_STEM, &NOMAD_ORIGINAL, b"fake audio");
        // A directory blocks the partial file the metadata is rewritten
        // through; the scan cannot remove it.
        std::fs::create_dir(dir.path().join(format!("{}.json.partial", NOMAD_STEM))).unwrap();

        let (result, _) = sync_recording(dir.path(), &[REISSUED]).await;

        assert_eq!(result.failed_episodes.len(), 1);
        assert_eq!(result.adopted, 0);
        assert_eq!(audio_files(dir.path()), vec![format!("{}.mp3", NOMAD_STEM)]);
    }

    #[tokio::test]
    async fn sync_repairs_audio_of_an_adopted_episode_and_keeps_its_guids() {
        let dir = tempdir().unwrap();
        let audio = dir.path().join(format!("{}.mp3", NOMAD_STEM));
        store_episode(dir.path(), NOMAD_STEM, &NOMAD_ORIGINAL, b"fake audio");
        sync_recording(dir.path(), &[REISSUED]).await;
        std::fs::write(&audio, b"damaged").unwrap();

        let (result, _) = sync_checking(dir.path(), &[REISSUED], AudioCheck::Repair).await;

        assert!(result.damaged.is_empty());
        assert_eq!(std::fs::read(&audio).unwrap(), b"fake audio");
        // The repaired episode is still the one both GUIDs named.
        let metadata = crate::metadata::read_episode_metadata(
            &dir.path().join(format!("{}.json", NOMAD_STEM)),
        )
        .unwrap();
        assert_eq!(metadata.guid.as_deref(), Some("nomad-reissued"));
        assert_eq!(
            metadata.additional_guids,
            vec!["nomad-original".to_string()]
        );
    }

    #[tokio::test]
    async fn sync_repairs_audio_of_a_guidless_episode_after_url_token_change() {
        let dir = tempdir().unwrap();
        let feed = |token: &str| {
            format!(
                r#"<?xml version="1.0"?>
<rss version="2.0">
  <channel>
    <title>Test Podcast</title>
    <description>A test podcast</description>
    <item>
      <title>Members Only</title>
      <pubDate>Thu, 19 Dec 2024 10:25:22 GMT</pubDate>
      <enclosure url="https://example.com/media/{}/episode-uuid.mp3" type="audio/mpeg"/>
    </item>
  </channel>
</rss>"#,
                token
            )
        };
        let client = |token: &str| MockHttpClient {
            feed_xml: feed(token),
            audio_data: b"fake audio".to_vec(),
        };
        sync_with(dir.path(), &client("token-a"), &SyncOptions::default()).await;
        let audio = dir.path().join(audio_files(dir.path()).remove(0));
        std::fs::write(&audio, b"damaged").unwrap();
        let options = SyncOptions {
            audio_check: AudioCheck::Repair,
            ..Default::default()
        };

        let result = sync_with(dir.path(), &client("token-b"), &options).await;

        assert!(result.damaged.is_empty());
        assert_eq!(std::fs::read(&audio).unwrap(), b"fake audio");
        assert_eq!(audio_files(dir.path()).len(), 1);
    }

    #[tokio::test]
    async fn sync_adopts_any_stored_copy_that_still_holds_the_audio() {
        let dir = tempdir().unwrap();
        // Two copies of one episode with the same recorded hash. The copy
        // first by name has changed on disk since.
        let copy_stem = format!("{} Copy", NOMAD_STEM);
        store_episode(dir.path(), NOMAD_STEM, &NOMAD_ORIGINAL, b"fake audio");
        store_episode(dir.path(), &copy_stem, &NOMAD_ORIGINAL, b"fake audio");
        std::fs::write(dir.path().join(format!("{}.mp3", copy_stem)), b"damaged").unwrap();

        let (result, _) = sync_recording(dir.path(), &[REISSUED]).await;

        assert_eq!(result.adopted, 1);
        assert_eq!(result.downloaded, 0);
        let metadata = crate::metadata::read_episode_metadata(
            &dir.path().join(format!("{}.json", NOMAD_STEM)),
        )
        .unwrap();
        assert_eq!(
            metadata.additional_guids,
            vec!["nomad-reissued".to_string()]
        );
    }

    /// A planned re-issue of NOMAD_ORIGINAL whose one candidate records
    /// the hash of "fake audio" while the file on disk holds other bytes
    fn reissue_with_changed_candidate(dir: &Path) -> (PlannedDownload, String) {
        store_episode(dir, NOMAD_STEM, &NOMAD_ORIGINAL, b"fake audio");
        std::fs::write(dir.join(format!("{}.mp3", NOMAD_STEM)), b"changed").unwrap();
        let state = scan_output_dir(dir, &NoopReporter).unwrap();
        let candidate = state
            .stored_episodes_with_guid("nomad-original")
            .next()
            .unwrap()
            .clone();
        let hash = candidate.content_hash.clone().unwrap();
        let planned = planned_nomad(Purpose::NewEpisode {
            replaced_candidates: vec![candidate],
        });
        (planned, hash)
    }

    #[tokio::test]
    async fn identical_audio_is_confirmed_by_hashing_the_stored_file() {
        let dir = tempdir().unwrap();
        let (planned, hash) = reissue_with_changed_candidate(dir.path());

        let found =
            replaced_with_identical_audio(dir.path(), &planned, &hash, &HashSet::new()).await;

        assert!(found.is_none());
    }

    #[tokio::test]
    async fn identical_audio_verified_in_this_run_is_not_hashed_again() {
        let dir = tempdir().unwrap();
        let (planned, hash) = reissue_with_changed_candidate(dir.path());
        let intact = HashSet::from([format!("{}.mp3", NOMAD_STEM)]);

        // Hashing the file would find the changed bytes; the verification of
        // this run vouches for it instead.
        let found = replaced_with_identical_audio(dir.path(), &planned, &hash, &intact).await;

        assert!(found.is_some());
    }

    #[tokio::test]
    async fn sync_records_which_checked_audio_is_intact() {
        let dir = tempdir().unwrap();
        store_episode(dir.path(), NOMAD_STEM, &NOMAD_ORIGINAL, b"fake audio");
        let state = scan_output_dir(dir.path(), &NoopReporter).unwrap();
        let targets = archive_check_targets(&state, &create_sync_plan(Vec::new(), &state, None));

        let verification = verify_stored_audio(&targets, dir.path(), false, &NoopReporter).await;

        assert_eq!(
            verification.intact,
            HashSet::from([format!("{}.mp3", NOMAD_STEM)])
        );
    }

    #[tokio::test]
    async fn sync_downloads_with_a_concurrency_of_zero_as_one() {
        let dir = tempdir().unwrap();
        let client = MockHttpClient {
            feed_xml: SAMPLE_FEED.to_string(),
            audio_data: b"fake audio".to_vec(),
        };
        let options = SyncOptions {
            max_concurrent: 0,
            ..Default::default()
        };

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            sync_with(dir.path(), &client, &options),
        )
        .await
        .expect("a concurrency of zero still downloads");

        assert_eq!(result.downloaded, 2);
    }

    /// Records every event and panics once a download completes, as a
    /// faulty reporter would
    #[derive(Default)]
    struct RecordingPanickingReporter {
        events: std::sync::Mutex<Vec<ProgressEvent>>,
    }

    impl ProgressReporter for RecordingPanickingReporter {
        fn report(&self, event: ProgressEvent) {
            let completed = matches!(event, ProgressEvent::DownloadCompleted { .. });
            self.events.lock().unwrap().push(event);
            if completed {
                panic!("reporter failure");
            }
        }
    }

    #[tokio::test]
    async fn sync_reports_panicking_downloads_as_failed() {
        let dir = tempdir().unwrap();
        let client = MockHttpClient {
            feed_xml: SAMPLE_FEED.to_string(),
            audio_data: b"fake audio".to_vec(),
        };
        let reporter = RecordingPanickingReporter::default();

        sync_podcast(
            &client,
            "https://example.com/feed.xml",
            dir.path(),
            &SyncOptions::default(),
            &reporter,
        )
        .await
        .unwrap();

        // Every started download ends with an event a reporter can close its
        // progress display on.
        let failed = reporter
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| matches!(event, ProgressEvent::DownloadFailed { .. }))
            .count();
        assert_eq!(failed, 2);
    }

    #[test]
    fn listed_name_finds_a_file_spelled_in_another_normalization() {
        let dir = tempdir().unwrap();
        // Shares mounted on macOS list "ä" decomposed although podpull
        // wrote it composed, and a lookup by the composed name can fail.
        let listed = "2019-12-27-Neuzuga\u{0308}nge #4.mp3";
        std::fs::write(dir.path().join(listed), b"audio").unwrap();

        let found = listed_name(dir.path(), "2019-12-27-Neuzug\u{00e4}nge #4.mp3").unwrap();

        assert_eq!(found, Some(std::ffi::OsString::from(listed)));
    }

    #[test]
    fn listed_name_is_none_without_a_matching_file() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("2019-12-27-Other.mp3"), b"audio").unwrap();

        assert_eq!(
            listed_name(dir.path(), "2019-12-27-Episode.mp3").unwrap(),
            None
        );
    }
}
