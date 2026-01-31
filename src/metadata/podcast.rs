// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::path::Path;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::error::MetadataError;
use crate::feed::Podcast;

const PODCAST_METADATA_FILENAME: &str = "podcast.json";

/// Serializable metadata for a podcast feed
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PodcastMetadata {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    pub feed_url: String,
    pub updated_at: String,
}

impl PodcastMetadata {
    /// Create metadata from a parsed Podcast
    pub fn from_podcast(podcast: &Podcast) -> Self {
        Self {
            title: podcast.title.clone(),
            description: podcast.description.clone(),
            link: podcast.link.as_ref().map(|u| u.to_string()),
            author: podcast.author.clone(),
            image_url: podcast.image_url.as_ref().map(|u| u.to_string()),
            feed_url: podcast.feed_url.to_string(),
            updated_at: Utc::now().to_rfc3339(),
        }
    }
}

/// Write podcast metadata to the output directory
pub fn write_podcast_metadata(podcast: &Podcast, output_dir: &Path) -> Result<(), MetadataError> {
    let metadata = PodcastMetadata::from_podcast(podcast);
    let path = output_dir.join(PODCAST_METADATA_FILENAME);

    let json = serde_json::to_string_pretty(&metadata)?;
    std::fs::write(&path, json).map_err(|e| MetadataError::WriteFailed { path, source: e })
}

/// Read podcast metadata from the output directory
pub fn read_podcast_metadata(output_dir: &Path) -> Result<PodcastMetadata, MetadataError> {
    let path = output_dir.join(PODCAST_METADATA_FILENAME);

    let content = std::fs::read_to_string(&path).map_err(|e| MetadataError::ReadFailed {
        path: path.clone(),
        source: e,
    })?;

    serde_json::from_str(&content).map_err(|e| MetadataError::JsonParseFailed { path, source: e })
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::tempdir;
    use url::Url;

    fn make_podcast() -> Podcast {
        Podcast {
            title: "Test Podcast".to_string(),
            description: Some("A test podcast".to_string()),
            link: Some(Url::parse("https://example.com").unwrap()),
            author: Some("Test Author".to_string()),
            image_url: Some(Url::parse("https://example.com/image.jpg").unwrap()),
            feed_url: Url::parse("https://example.com/feed.xml").unwrap(),
            episodes: vec![],
        }
    }

    #[test]
    fn from_podcast_converts_all_fields() {
        let podcast = make_podcast();
        let metadata = PodcastMetadata::from_podcast(&podcast);

        assert_eq!(metadata.title, "Test Podcast");
        assert_eq!(metadata.description, Some("A test podcast".to_string()));
        assert_eq!(metadata.link, Some("https://example.com/".to_string()));
        assert_eq!(metadata.author, Some("Test Author".to_string()));
        assert_eq!(
            metadata.image_url,
            Some("https://example.com/image.jpg".to_string())
        );
        assert_eq!(metadata.feed_url, "https://example.com/feed.xml");
    }

    #[test]
    fn write_and_read_roundtrip() {
        let dir = tempdir().unwrap();
        let podcast = make_podcast();

        write_podcast_metadata(&podcast, dir.path()).unwrap();
        let read_back = read_podcast_metadata(dir.path()).unwrap();

        assert_eq!(read_back.title, "Test Podcast");
        assert_eq!(read_back.description, Some("A test podcast".to_string()));
    }

    #[test]
    fn read_nonexistent_returns_error() {
        let dir = tempdir().unwrap();
        let result = read_podcast_metadata(dir.path());
        assert!(result.is_err());
    }
}
