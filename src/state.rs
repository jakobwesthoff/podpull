// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::error::StateError;
use crate::feed::Episode;
use crate::metadata::read_episode_metadata;

/// State of the output directory, tracking already-downloaded episodes
#[derive(Debug, Clone)]
pub struct OutputState {
    /// GUIDs of episodes that have been downloaded
    pub downloaded_guids: HashSet<String>,
    /// Filenames (without path) of existing files
    pub existing_files: HashSet<String>,
    /// The output directory path
    pub output_dir: PathBuf,
    /// Number of partial files that were cleaned up during scan
    pub partial_files_cleaned: usize,
}

/// Plan for synchronization, indicating what needs to be downloaded
#[derive(Debug, Clone)]
pub struct SyncPlan {
    /// Episodes that need to be downloaded
    pub to_download: Vec<Episode>,
    /// Episodes already present in the output directory
    pub already_present: Vec<Episode>,
    /// Total number of episodes in the feed
    pub total_episodes: usize,
}

/// Scan the output directory to detect existing downloads
///
/// Reads all .json metadata files to extract GUIDs of already-downloaded episodes.
/// Also cleans up any `.partial` files from interrupted downloads.
pub fn scan_output_dir(output_dir: &Path) -> Result<OutputState, StateError> {
    let mut downloaded_guids = HashSet::new();
    let mut existing_files = HashSet::new();
    let mut partial_files_cleaned = 0;

    if !output_dir.exists() {
        // Create the directory if it doesn't exist
        std::fs::create_dir_all(output_dir).map_err(|e| StateError::CreateDirectoryFailed {
            path: output_dir.to_path_buf(),
            source: e,
        })?;

        return Ok(OutputState {
            downloaded_guids,
            existing_files,
            output_dir: output_dir.to_path_buf(),
            partial_files_cleaned,
        });
    }

    let entries = std::fs::read_dir(output_dir).map_err(|e| StateError::ReadDirectoryFailed {
        path: output_dir.to_path_buf(),
        source: e,
    })?;

    for entry in entries {
        let entry = entry.map_err(|e| StateError::ReadDirectoryFailed {
            path: output_dir.to_path_buf(),
            source: e,
        })?;

        let path = entry.path();
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        // Clean up partial files from interrupted downloads
        if filename.ends_with(".partial") {
            if std::fs::remove_file(&path).is_ok() {
                partial_files_cleaned += 1;
            }
            continue;
        }

        existing_files.insert(filename.clone());

        // Read episode metadata files to extract GUIDs
        if filename.ends_with(".json")
            && filename != "podcast.json"
            && let Ok(metadata) = read_episode_metadata(&path)
            && let Some(guid) = metadata.guid
        {
            downloaded_guids.insert(guid);
        }
    }

    Ok(OutputState {
        downloaded_guids,
        existing_files,
        output_dir: output_dir.to_path_buf(),
        partial_files_cleaned,
    })
}

/// Create a sync plan by comparing episodes against the output state
///
/// Determines which episodes need to be downloaded based on:
/// 1. GUID matching (if episode has a GUID that matches a downloaded one, skip)
/// 2. If no GUID match, episode will be downloaded
///
/// Episodes are sorted by publication date (newest first). Episodes without
/// a publication date are placed at the end, preserving their relative order.
pub fn create_sync_plan(episodes: Vec<Episode>, state: &OutputState) -> SyncPlan {
    let total_episodes = episodes.len();
    let mut to_download = Vec::new();
    let mut already_present = Vec::new();

    for episode in episodes {
        let is_downloaded = episode
            .guid
            .as_ref()
            .is_some_and(|guid| state.downloaded_guids.contains(guid));

        if is_downloaded {
            already_present.push(episode);
        } else {
            to_download.push(episode);
        }
    }

    // Sort episodes by publication date (newest first)
    // Episodes without pub_date are placed at the end
    to_download.sort_by(|a, b| match (&b.pub_date, &a.pub_date) {
        (Some(b_date), Some(a_date)) => b_date.cmp(a_date),
        (Some(_), None) => std::cmp::Ordering::Greater, // b has date, a doesn't => b comes first
        (None, Some(_)) => std::cmp::Ordering::Less,    // a has date, b doesn't => a comes first
        (None, None) => std::cmp::Ordering::Equal,
    });

    SyncPlan {
        to_download,
        already_present,
        total_episodes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed::Enclosure;
    use crate::metadata::write_episode_metadata;
    use chrono::{DateTime, FixedOffset, TimeZone, Utc};
    use tempfile::tempdir;
    use url::Url;

    fn make_episode(title: &str, guid: Option<&str>) -> Episode {
        Episode {
            title: title.to_string(),
            description: None,
            pub_date: None,
            guid: guid.map(String::from),
            enclosure: Enclosure {
                url: Url::parse("https://example.com/ep.mp3").unwrap(),
                length: None,
                mime_type: None,
            },
            duration: None,
            episode_number: None,
            season_number: None,
        }
    }

    fn make_episode_with_date(
        title: &str,
        guid: Option<&str>,
        pub_date: Option<DateTime<FixedOffset>>,
    ) -> Episode {
        Episode {
            title: title.to_string(),
            description: None,
            pub_date,
            guid: guid.map(String::from),
            enclosure: Enclosure {
                url: Url::parse("https://example.com/ep.mp3").unwrap(),
                length: None,
                mime_type: None,
            },
            duration: None,
            episode_number: None,
            season_number: None,
        }
    }

    fn make_date(year: i32, month: u32, day: u32) -> DateTime<FixedOffset> {
        Utc.with_ymd_and_hms(year, month, day, 12, 0, 0)
            .unwrap()
            .with_timezone(&FixedOffset::east_opt(0).unwrap())
    }

    #[test]
    fn scan_empty_dir_returns_empty_state() {
        let dir = tempdir().unwrap();
        let state = scan_output_dir(dir.path()).unwrap();

        assert!(state.downloaded_guids.is_empty());
        assert!(state.existing_files.is_empty());
        assert_eq!(state.partial_files_cleaned, 0);
    }

    #[test]
    fn scan_creates_nonexistent_dir() {
        let dir = tempdir().unwrap();
        let output_dir = dir.path().join("new_podcast");

        assert!(!output_dir.exists());
        let state = scan_output_dir(&output_dir).unwrap();
        assert!(output_dir.exists());
        assert!(state.downloaded_guids.is_empty());
    }

    #[test]
    fn scan_finds_downloaded_episodes() {
        let dir = tempdir().unwrap();
        let episode = make_episode("Test Episode", Some("test-guid-123"));

        // Write episode metadata
        let meta_path = dir.path().join("2024-01-15-test-episode.json");
        write_episode_metadata(&episode, "2024-01-15-test-episode.mp3", None, &meta_path).unwrap();

        let state = scan_output_dir(dir.path()).unwrap();

        assert!(state.downloaded_guids.contains("test-guid-123"));
        assert!(
            state
                .existing_files
                .contains("2024-01-15-test-episode.json")
        );
    }

    #[test]
    fn scan_ignores_podcast_json() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("podcast.json"),
            r#"{"title": "Test", "feed_url": "http://example.com", "updated_at": "2024-01-01"}"#,
        )
        .unwrap();

        let state = scan_output_dir(dir.path()).unwrap();

        // podcast.json should be in existing_files but not affect downloaded_guids
        assert!(state.existing_files.contains("podcast.json"));
        assert!(state.downloaded_guids.is_empty());
    }

    #[test]
    fn sync_plan_identifies_new_episodes() {
        let state = OutputState {
            downloaded_guids: HashSet::new(),
            existing_files: HashSet::new(),
            output_dir: PathBuf::from("/tmp"),
            partial_files_cleaned: 0,
        };

        let episodes = vec![
            make_episode("Ep 1", Some("guid-1")),
            make_episode("Ep 2", Some("guid-2")),
        ];

        let plan = create_sync_plan(episodes, &state);

        assert_eq!(plan.to_download.len(), 2);
        assert_eq!(plan.already_present.len(), 0);
        assert_eq!(plan.total_episodes, 2);
    }

    #[test]
    fn sync_plan_skips_downloaded_episodes() {
        let mut downloaded_guids = HashSet::new();
        downloaded_guids.insert("guid-1".to_string());

        let state = OutputState {
            downloaded_guids,
            existing_files: HashSet::new(),
            output_dir: PathBuf::from("/tmp"),
            partial_files_cleaned: 0,
        };

        let episodes = vec![
            make_episode("Ep 1", Some("guid-1")),
            make_episode("Ep 2", Some("guid-2")),
        ];

        let plan = create_sync_plan(episodes, &state);

        assert_eq!(plan.to_download.len(), 1);
        assert_eq!(plan.to_download[0].title, "Ep 2");
        assert_eq!(plan.already_present.len(), 1);
        assert_eq!(plan.already_present[0].title, "Ep 1");
    }

    #[test]
    fn sync_plan_downloads_episodes_without_guid() {
        let mut downloaded_guids = HashSet::new();
        downloaded_guids.insert("guid-1".to_string());

        let state = OutputState {
            downloaded_guids,
            existing_files: HashSet::new(),
            output_dir: PathBuf::from("/tmp"),
            partial_files_cleaned: 0,
        };

        let episodes = vec![
            make_episode("Ep 1", Some("guid-1")),
            make_episode("Ep 2", None), // No GUID, should be downloaded
        ];

        let plan = create_sync_plan(episodes, &state);

        assert_eq!(plan.to_download.len(), 1);
        assert_eq!(plan.to_download[0].title, "Ep 2");
    }

    #[test]
    fn scan_cleans_up_partial_files() {
        let dir = tempdir().unwrap();

        // Create some partial files
        std::fs::write(dir.path().join("episode1.mp3.partial"), b"partial data 1").unwrap();
        std::fs::write(dir.path().join("episode2.mp3.partial"), b"partial data 2").unwrap();
        // Create a normal file
        std::fs::write(dir.path().join("episode3.mp3"), b"complete audio").unwrap();

        let state = scan_output_dir(dir.path()).unwrap();

        // Partial files should have been cleaned up
        assert_eq!(state.partial_files_cleaned, 2);
        assert!(!dir.path().join("episode1.mp3.partial").exists());
        assert!(!dir.path().join("episode2.mp3.partial").exists());
        // Normal file should still exist
        assert!(dir.path().join("episode3.mp3").exists());
        assert!(state.existing_files.contains("episode3.mp3"));
        // Partial files should not be in existing_files
        assert!(!state.existing_files.contains("episode1.mp3.partial"));
        assert!(!state.existing_files.contains("episode2.mp3.partial"));
    }

    #[test]
    fn sync_plan_sorts_episodes_by_pub_date_newest_first() {
        let state = OutputState {
            downloaded_guids: HashSet::new(),
            existing_files: HashSet::new(),
            output_dir: PathBuf::from("/tmp"),
            partial_files_cleaned: 0,
        };

        // Create episodes in random order
        let episodes = vec![
            make_episode_with_date("Old Episode", Some("guid-1"), Some(make_date(2024, 1, 1))),
            make_episode_with_date(
                "Newest Episode",
                Some("guid-2"),
                Some(make_date(2024, 3, 15)),
            ),
            make_episode_with_date(
                "Middle Episode",
                Some("guid-3"),
                Some(make_date(2024, 2, 10)),
            ),
        ];

        let plan = create_sync_plan(episodes, &state);

        // Should be sorted newest first
        assert_eq!(plan.to_download.len(), 3);
        assert_eq!(plan.to_download[0].title, "Newest Episode");
        assert_eq!(plan.to_download[1].title, "Middle Episode");
        assert_eq!(plan.to_download[2].title, "Old Episode");
    }

    #[test]
    fn sync_plan_places_episodes_without_date_at_end() {
        let state = OutputState {
            downloaded_guids: HashSet::new(),
            existing_files: HashSet::new(),
            output_dir: PathBuf::from("/tmp"),
            partial_files_cleaned: 0,
        };

        let episodes = vec![
            make_episode_with_date("No Date 1", Some("guid-1"), None),
            make_episode_with_date("With Date", Some("guid-2"), Some(make_date(2024, 1, 15))),
            make_episode_with_date("No Date 2", Some("guid-3"), None),
        ];

        let plan = create_sync_plan(episodes, &state);

        // Episode with date should be first, undated ones at the end
        assert_eq!(plan.to_download.len(), 3);
        assert_eq!(plan.to_download[0].title, "With Date");
        // Undated episodes preserve relative order
        assert_eq!(plan.to_download[1].title, "No Date 1");
        assert_eq!(plan.to_download[2].title, "No Date 2");
    }
}
