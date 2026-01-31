// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use chrono::{DateTime, FixedOffset};
use url::Url;

use crate::error::FeedError;

/// Represents a parsed podcast feed
#[derive(Debug, Clone)]
pub struct Podcast {
    pub title: String,
    pub description: Option<String>,
    pub link: Option<Url>,
    pub author: Option<String>,
    pub image_url: Option<Url>,
    pub feed_url: Url,
    pub episodes: Vec<Episode>,
}

/// Represents a single podcast episode
#[derive(Debug, Clone)]
pub struct Episode {
    pub title: String,
    pub description: Option<String>,
    pub pub_date: Option<DateTime<FixedOffset>>,
    pub guid: Option<String>,
    pub enclosure: Enclosure,
    pub duration: Option<String>,
    pub episode_number: Option<u32>,
    pub season_number: Option<u32>,
}

/// Represents the audio file attached to an episode
#[derive(Debug, Clone)]
pub struct Enclosure {
    pub url: Url,
    pub length: Option<u64>,
    pub mime_type: Option<String>,
}

/// Parse RSS feed XML bytes into a Podcast struct
pub fn parse_feed(xml_bytes: &[u8], feed_url: Url) -> Result<Podcast, FeedError> {
    let channel = rss::Channel::read_from(xml_bytes)?;

    let episodes = channel
        .items()
        .iter()
        .filter_map(|item| parse_episode(item).ok())
        .collect();

    let image_url = channel
        .image()
        .and_then(|img| Url::parse(img.url()).ok())
        .or_else(|| {
            channel
                .itunes_ext()
                .and_then(|ext| ext.image())
                .and_then(|url| Url::parse(url).ok())
        });

    let author = channel
        .itunes_ext()
        .and_then(|ext| ext.author().map(String::from))
        .or_else(|| channel.managing_editor().map(String::from));

    Ok(Podcast {
        title: channel.title().to_string(),
        description: Some(channel.description().to_string()).filter(|s| !s.is_empty()),
        link: Url::parse(channel.link()).ok(),
        author,
        image_url,
        feed_url,
        episodes,
    })
}

fn parse_episode(item: &rss::Item) -> Result<Episode, FeedError> {
    let title = item
        .title()
        .map(String::from)
        .unwrap_or_else(|| "Untitled Episode".to_string());

    let enclosure = item
        .enclosure()
        .ok_or_else(|| FeedError::MissingEnclosure {
            title: title.clone(),
        })?;

    let enclosure_url = Url::parse(enclosure.url())?;

    let pub_date = item.pub_date().and_then(|date_str| {
        DateTime::parse_from_rfc2822(date_str)
            .or_else(|_| parse_relaxed_date(date_str))
            .ok()
    });

    let guid = item
        .guid()
        .map(|g| g.value().to_string())
        .or_else(|| Some(enclosure.url().to_string()));

    let itunes = item.itunes_ext();

    Ok(Episode {
        title,
        description: item.description().map(String::from),
        pub_date,
        guid,
        enclosure: Enclosure {
            url: enclosure_url,
            length: enclosure.length().parse().ok(),
            mime_type: Some(enclosure.mime_type().to_string()).filter(|s| !s.is_empty()),
        },
        duration: itunes.and_then(|ext| ext.duration().map(String::from)),
        episode_number: itunes.and_then(|ext| ext.episode().and_then(|e| e.parse().ok())),
        season_number: itunes.and_then(|ext| ext.season().and_then(|s| s.parse().ok())),
    })
}

/// Try to parse dates that don't strictly conform to RFC 2822
fn parse_relaxed_date(date_str: &str) -> Result<DateTime<FixedOffset>, chrono::ParseError> {
    // Try common alternative formats
    let formats = [
        "%a, %d %b %Y %H:%M:%S %z",
        "%Y-%m-%dT%H:%M:%S%:z",
        "%Y-%m-%d %H:%M:%S %z",
    ];

    for format in formats {
        if let Ok(dt) = DateTime::parse_from_str(date_str, format) {
            return Ok(dt);
        }
    }

    // Last resort: try to parse and assume UTC
    Err(chrono::DateTime::parse_from_rfc2822("invalid").unwrap_err())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_FEED: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd">
  <channel>
    <title>Test Podcast</title>
    <description>A test podcast for unit testing</description>
    <link>https://example.com</link>
    <itunes:author>Test Author</itunes:author>
    <itunes:image href="https://example.com/image.jpg"/>
    <item>
      <title>Episode 1</title>
      <description>First episode</description>
      <pubDate>Mon, 01 Jan 2024 12:00:00 +0000</pubDate>
      <guid>ep1-guid</guid>
      <enclosure url="https://example.com/ep1.mp3" length="1234567" type="audio/mpeg"/>
      <itunes:duration>30:00</itunes:duration>
      <itunes:episode>1</itunes:episode>
      <itunes:season>1</itunes:season>
    </item>
    <item>
      <title>Episode 2</title>
      <enclosure url="https://example.com/ep2.mp3" type="audio/mpeg"/>
    </item>
  </channel>
</rss>"#;

    #[test]
    fn parse_feed_extracts_podcast_metadata() {
        let feed_url = Url::parse("https://example.com/feed.xml").unwrap();
        let podcast = parse_feed(SAMPLE_FEED.as_bytes(), feed_url.clone()).unwrap();

        assert_eq!(podcast.title, "Test Podcast");
        assert_eq!(
            podcast.description,
            Some("A test podcast for unit testing".to_string())
        );
        assert_eq!(podcast.author, Some("Test Author".to_string()));
        assert_eq!(podcast.feed_url, feed_url);
    }

    #[test]
    fn parse_feed_extracts_episodes() {
        let feed_url = Url::parse("https://example.com/feed.xml").unwrap();
        let podcast = parse_feed(SAMPLE_FEED.as_bytes(), feed_url).unwrap();

        assert_eq!(podcast.episodes.len(), 2);

        let ep1 = &podcast.episodes[0];
        assert_eq!(ep1.title, "Episode 1");
        assert_eq!(ep1.guid, Some("ep1-guid".to_string()));
        assert_eq!(ep1.duration, Some("30:00".to_string()));
        assert_eq!(ep1.episode_number, Some(1));
        assert_eq!(ep1.season_number, Some(1));
        assert_eq!(ep1.enclosure.length, Some(1234567));
    }

    #[test]
    fn parse_feed_handles_missing_optional_fields() {
        let feed_url = Url::parse("https://example.com/feed.xml").unwrap();
        let podcast = parse_feed(SAMPLE_FEED.as_bytes(), feed_url).unwrap();

        let ep2 = &podcast.episodes[1];
        assert_eq!(ep2.title, "Episode 2");
        assert!(ep2.pub_date.is_none());
        assert!(ep2.duration.is_none());
        assert!(ep2.episode_number.is_none());
    }

    #[test]
    fn parse_feed_skips_items_without_enclosure() {
        let feed_no_enclosure = r#"<?xml version="1.0"?>
<rss version="2.0">
  <channel>
    <title>Test</title>
    <description>Test</description>
    <item>
      <title>No Audio</title>
    </item>
  </channel>
</rss>"#;

        let feed_url = Url::parse("https://example.com/feed.xml").unwrap();
        let podcast = parse_feed(feed_no_enclosure.as_bytes(), feed_url).unwrap();
        assert!(podcast.episodes.is_empty());
    }
}
