use std::path::Path;

use chrono::Utc;
use serde::{Deserialize, Serialize};

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
    pub fn from_episode(episode: &Episode, audio_filename: &str, content_hash: Option<String>) -> Self {
        Self {
            title: episode.title.clone(),
            description: episode.description.clone(),
            pub_date: episode.pub_date.map(|dt| dt.to_rfc3339()),
            guid: episode.guid.clone(),
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

/// Write episode metadata to a JSON file
pub fn write_episode_metadata(
    episode: &Episode,
    audio_filename: &str,
    content_hash: Option<String>,
    path: &Path,
) -> Result<(), MetadataError> {
    let metadata = EpisodeMetadata::from_episode(episode, audio_filename, content_hash);
    let json = serde_json::to_string_pretty(&metadata)?;
    std::fs::write(path, json).map_err(|e| MetadataError::WriteFailed {
        path: path.to_path_buf(),
        source: e,
    })
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
        let metadata = EpisodeMetadata::from_episode(&episode, "2024-01-15-test-episode.mp3", Some("sha256:abc123".to_string()));

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

        write_episode_metadata(&episode, "test.mp3", Some("sha256:abc123".to_string()), &path).unwrap();
        let read_back = read_episode_metadata(&path).unwrap();

        assert_eq!(read_back.title, "Test Episode");
        assert_eq!(read_back.audio_filename, "test.mp3");
        assert_eq!(read_back.guid, Some("test-guid-123".to_string()));
        assert_eq!(read_back.content_hash, Some("sha256:abc123".to_string()));
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
