// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::path::Path;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use super::{StagedMetadata, stage_metadata_file};
use crate::error::MetadataError;
use crate::feed::Episode;

/// Serializable metadata for a downloaded episode
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeMetadata {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pub_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guid: Option<String>,
    /// Further GUIDs the feed has listed this very audio under
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_guids: Vec<String>,
    pub original_url: String,
    pub downloaded_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episode_number: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub season_number: Option<u32>,
    pub audio_filename: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
}

impl EpisodeMetadata {
    /// Create metadata from a parsed Episode
    pub fn from_episode(
        episode: &Episode,
        audio_filename: &str,
        content_hash: Option<String>,
    ) -> Self {
        Self {
            title: episode.title.clone(),
            description: episode.description.clone(),
            pub_date: episode.pub_date.map(|dt| dt.to_rfc3339()),
            guid: episode.guid.clone(),
            additional_guids: Vec::new(),
            original_url: episode.enclosure.url.to_string(),
            downloaded_at: Utc::now().to_rfc3339(),
            duration: episode.duration.clone(),
            episode_number: episode.episode_number,
            season_number: episode.season_number,
            audio_filename: audio_filename.to_string(),
            content_hash,
        }
    }
}

/// Write episode metadata into the partial file next to `path`
pub fn stage_episode_metadata(
    episode: &Episode,
    audio_filename: &str,
    content_hash: Option<String>,
    path: &Path,
) -> Result<StagedMetadata, MetadataError> {
    let metadata = EpisodeMetadata::from_episode(episode, audio_filename, content_hash);
    let json = serde_json::to_string_pretty(&metadata)?;

    stage_metadata_file(path, json.as_bytes())
}

/// Write episode metadata to a JSON file, replacing an existing one atomically
pub fn write_episode_metadata(
    episode: &Episode,
    audio_filename: &str,
    content_hash: Option<String>,
    path: &Path,
) -> Result<(), MetadataError> {
    stage_episode_metadata(episode, audio_filename, content_hash, path)?.commit()
}

/// Record that the feed also lists the episode at `path` under `guid`
///
/// The metadata is rewritten atomically. A GUID it already names is not
/// added again.
pub fn add_guid_to_episode_metadata(path: &Path, guid: &str) -> Result<(), MetadataError> {
    let mut metadata = read_episode_metadata(path)?;
    let already_named = metadata.guid.as_deref() == Some(guid)
        || metadata.additional_guids.iter().any(|known| known == guid);
    if already_named {
        return Ok(());
    }
    metadata.additional_guids.push(guid.to_string());

    let json = serde_json::to_string_pretty(&metadata)?;
    stage_metadata_file(path, json.as_bytes())?.commit()
}

/// Read episode metadata from a JSON file
pub fn read_episode_metadata(path: &Path) -> Result<EpisodeMetadata, MetadataError> {
    let content = std::fs::read_to_string(path).map_err(|e| MetadataError::ReadFailed {
        path: path.to_path_buf(),
        source: e,
    })?;

    serde_json::from_str(&content).map_err(|e| MetadataError::JsonParseFailed {
        path: path.to_path_buf(),
        source: e,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed::Enclosure;
    use chrono::DateTime;
    use tempfile::tempdir;
    use url::Url;

    fn make_episode() -> Episode {
        Episode {
            title: "Test Episode".to_string(),
            description: Some("A test episode".to_string()),
            pub_date: DateTime::parse_from_rfc2822("Mon, 15 Jan 2024 12:00:00 +0000").ok(),
            guid: Some("test-guid-123".to_string()),
            enclosure: Enclosure {
                url: Url::parse("https://example.com/episode.mp3").unwrap(),
                length: Some(1234567),
                mime_type: Some("audio/mpeg".to_string()),
            },
            duration: Some("30:00".to_string()),
            episode_number: Some(42),
            season_number: Some(2),
        }
    }

    #[test]
    fn from_episode_converts_all_fields() {
        let episode = make_episode();
        let metadata = EpisodeMetadata::from_episode(
            &episode,
            "2024-01-15-test-episode.mp3",
            Some("sha256:abc123".to_string()),
        );

        assert_eq!(metadata.title, "Test Episode");
        assert_eq!(metadata.description, Some("A test episode".to_string()));
        assert!(metadata.pub_date.is_some());
        assert_eq!(metadata.guid, Some("test-guid-123".to_string()));
        assert_eq!(metadata.original_url, "https://example.com/episode.mp3");
        assert_eq!(metadata.duration, Some("30:00".to_string()));
        assert_eq!(metadata.episode_number, Some(42));
        assert_eq!(metadata.season_number, Some(2));
        assert_eq!(metadata.audio_filename, "2024-01-15-test-episode.mp3");
        assert_eq!(metadata.content_hash, Some("sha256:abc123".to_string()));
    }

    #[test]
    fn write_and_read_roundtrip() {
        let dir = tempdir().unwrap();
        let episode = make_episode();
        let path = dir.path().join("episode.json");

        write_episode_metadata(
            &episode,
            "test.mp3",
            Some("sha256:abc123".to_string()),
            &path,
        )
        .unwrap();
        let read_back = read_episode_metadata(&path).unwrap();

        assert_eq!(read_back.title, "Test Episode");
        assert_eq!(read_back.audio_filename, "test.mp3");
        assert_eq!(read_back.guid, Some("test-guid-123".to_string()));
        assert_eq!(read_back.content_hash, Some("sha256:abc123".to_string()));
    }

    #[test]
    fn write_replaces_existing_metadata_without_leftovers() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("episode.json");
        std::fs::write(&path, "previous content").unwrap();

        write_episode_metadata(&make_episode(), "test.mp3", None, &path).unwrap();

        assert_eq!(read_episode_metadata(&path).unwrap().title, "Test Episode");
        assert!(!dir.path().join("episode.json.partial").exists());
    }

    #[test]
    fn failed_write_leaves_existing_metadata_intact() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("episode.json");
        let previous = r#"{"title":"Previous"}"#;
        std::fs::write(&path, previous).unwrap();

        // A directory in place of the partial file makes the write fail
        // before the existing metadata could be touched.
        std::fs::create_dir(dir.path().join("episode.json.partial")).unwrap();

        let result = write_episode_metadata(&make_episode(), "test.mp3", None, &path);

        assert!(matches!(result, Err(MetadataError::WriteFailed { .. })));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), previous);
    }

    #[test]
    fn failed_rename_reports_the_metadata_path() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("episode.json");

        // A non-empty directory at the target path cannot be replaced by
        // renaming a file onto it.
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("occupant"), b"").unwrap();

        let result = write_episode_metadata(&make_episode(), "test.mp3", None, &path);

        match result {
            Err(MetadataError::WriteFailed { path: failed, .. }) => assert_eq!(failed, path),
            other => panic!("Expected WriteFailed, got {other:?}"),
        }
        assert!(!dir.path().join("episode.json.partial").exists());
    }

    #[test]
    fn staged_metadata_stays_partial_until_committed() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("episode.json");

        let staged = stage_episode_metadata(&make_episode(), "test.mp3", None, &path).unwrap();

        assert!(!path.exists());
        assert!(dir.path().join("episode.json.partial").exists());

        staged.commit().unwrap();

        assert_eq!(read_episode_metadata(&path).unwrap().title, "Test Episode");
        assert!(!dir.path().join("episode.json.partial").exists());
    }

    #[test]
    fn discarded_metadata_leaves_nothing_behind() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("episode.json");

        stage_episode_metadata(&make_episode(), "test.mp3", None, &path)
            .unwrap()
            .discard();

        assert!(!path.exists());
        assert!(!dir.path().join("episode.json.partial").exists());
    }

    #[test]
    fn metadata_without_additional_guids_omits_and_reads_the_field() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("episode.json");

        write_episode_metadata(&make_episode(), "test.mp3", None, &path).unwrap();

        // Metadata written before the field existed reads the same way.
        assert!(
            !std::fs::read_to_string(&path)
                .unwrap()
                .contains("additional_guids")
        );
        assert!(
            read_episode_metadata(&path)
                .unwrap()
                .additional_guids
                .is_empty()
        );
    }

    #[test]
    fn add_guid_records_another_guid_once() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("episode.json");
        write_episode_metadata(
            &make_episode(),
            "test.mp3",
            Some("sha256:abc".into()),
            &path,
        )
        .unwrap();

        add_guid_to_episode_metadata(&path, "reissued-guid").unwrap();
        add_guid_to_episode_metadata(&path, "reissued-guid").unwrap();
        add_guid_to_episode_metadata(&path, "test-guid-123").unwrap();

        let metadata = read_episode_metadata(&path).unwrap();
        assert_eq!(metadata.guid.as_deref(), Some("test-guid-123"));
        assert_eq!(metadata.additional_guids, vec!["reissued-guid".to_string()]);
        assert_eq!(metadata.content_hash.as_deref(), Some("sha256:abc"));
    }

    #[test]
    fn read_nonexistent_returns_error() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nonexistent.json");
        let result = read_episode_metadata(&path);
        assert!(result.is_err());
    }

    #[test]
    fn handles_missing_optional_fields() {
        let episode = Episode {
            title: "Minimal Episode".to_string(),
            description: None,
            pub_date: None,
            guid: None,
            enclosure: Enclosure {
                url: Url::parse("https://example.com/ep.mp3").unwrap(),
                length: None,
                mime_type: None,
            },
            duration: None,
            episode_number: None,
            season_number: None,
        };

        let metadata = EpisodeMetadata::from_episode(&episode, "minimal.mp3", None);

        assert_eq!(metadata.title, "Minimal Episode");
        assert!(metadata.description.is_none());
        assert!(metadata.pub_date.is_none());
        assert!(metadata.guid.is_none());
        assert!(metadata.duration.is_none());
        assert!(metadata.episode_number.is_none());
        assert!(metadata.season_number.is_none());
        assert!(metadata.content_hash.is_none());
    }
}
