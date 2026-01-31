use std::path::Path;
use url::Url;

use crate::error::FeedError;
use crate::http::HttpClient;

use super::parse::{Podcast, parse_feed};

/// Fetch and parse a podcast feed from a URL
pub async fn fetch_feed<C: HttpClient>(client: &C, url: &str) -> Result<Podcast, FeedError> {
    let feed_url = Url::parse(url)?;

    let bytes = client
        .get_bytes(url)
        .await
        .map_err(|e| FeedError::FetchFailed {
            url: url.to_string(),
            source: e,
        })?;

    parse_feed(&bytes, feed_url)
}

/// Parse a podcast feed from a local file
pub fn parse_feed_file(path: &Path) -> Result<Podcast, FeedError> {
    let bytes = std::fs::read(path).map_err(|e| FeedError::FileReadFailed {
        path: path.to_path_buf(),
        source: e,
    })?;

    // Use file:// URL as the feed URL for local files
    let feed_url = Url::from_file_path(path).unwrap_or_else(|_| {
        Url::parse(&format!("file://{}", path.display())).expect("valid file URL")
    });

    parse_feed(&bytes, feed_url)
}

/// Determine if a string is a URL or a file path
pub fn is_url(source: &str) -> bool {
    source.starts_with("http://") || source.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_url_detects_http() {
        assert!(is_url("http://example.com/feed.xml"));
        assert!(is_url("https://example.com/feed.xml"));
    }

    #[test]
    fn is_url_rejects_file_paths() {
        assert!(!is_url("/path/to/feed.xml"));
        assert!(!is_url("./feed.xml"));
        assert!(!is_url("feed.xml"));
    }
}
