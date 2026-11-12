// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::path::Path;

use bytes::Bytes;
use url::Url;

use crate::error::FeedError;
use crate::http::HttpClient;

/// Fetch raw feed bytes from a URL (without parsing)
pub async fn fetch_feed_bytes<C: HttpClient>(client: &C, url: &Url) -> Result<Bytes, FeedError> {
    let bytes = client
        .get_bytes(url.as_str())
        .await
        .map_err(|e| FeedError::FetchFailed {
            url: url.clone(),
            source: e,
        })?;
    Ok(bytes)
}

/// Read raw feed bytes from a local file (without parsing)
pub fn read_feed_file(path: &Path) -> Result<Vec<u8>, FeedError> {
    std::fs::read(path).map_err(|e| FeedError::FileReadFailed {
        path: path.to_path_buf(),
        source: e,
    })
}

/// Construct a file:// URL for a local file path
pub fn file_path_to_url(path: &Path) -> Url {
    Url::from_file_path(path).unwrap_or_else(|_| {
        // The text builds a URL that the parser escapes; nothing is shown.
        #[allow(clippy::disallowed_methods)]
        let text = format!("file://{}", path.display());
        Url::parse(&text).expect("valid file URL")
    })
}

/// Determine if a string is a URL or a file path
pub fn is_url(source: &str) -> bool {
    source.starts_with("http://") || source.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_url_of_an_absolute_path_names_the_path() {
        assert_eq!(
            file_path_to_url(Path::new("/feeds/show.xml")).as_str(),
            "file:///feeds/show.xml"
        );
    }

    #[test]
    fn a_relative_path_still_gets_a_file_url() {
        // The first part of a relative path takes the place of the host.
        assert_eq!(
            file_path_to_url(Path::new("feeds/show.xml")).as_str(),
            "file://feeds/show.xml"
        );
    }

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
