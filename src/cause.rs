// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::error::Error;

/// The text of the root cause of `error`: the innermost link of its
/// `source()` chain that has text
///
/// A wrapper's text repeats what it wraps, so the innermost link says what
/// happened. The URL a request error names is left out, as a line names the
/// URL itself. The text is tidied to read inside a sentence: an operating
/// system's ` (os error N)` is dropped and a leading capital is lowercased
/// unless it starts an acronym.
pub(crate) fn root_text(error: &(dyn Error + 'static)) -> String {
    let mut root = error;
    let mut text = error.to_string();
    let mut link = error.source();
    while let Some(inner) = link {
        let inner_text = inner.to_string();
        if !inner_text.is_empty() {
            root = inner;
            text = inner_text;
        }
        link = inner.source();
    }
    tidy(crate::http::without_request_url(root, text))
}

fn tidy(text: String) -> String {
    let text = without_os_error_suffix(text);
    let mut characters = text.chars();
    match (characters.next(), characters.next()) {
        (Some(first), Some(second)) if first.is_uppercase() && second.is_lowercase() => {
            first.to_lowercase().chain(text.chars().skip(1)).collect()
        }
        _ => text,
    }
}

/// `text` without a trailing ` (os error N)`, which the standard library
/// adds to the text of an error from the operating system
fn without_os_error_suffix(text: String) -> String {
    if let Some(start) = text.rfind(" (os error ")
        && let Some(number) = text[start + " (os error ".len()..].strip_suffix(')')
        && !number.is_empty()
        && number.chars().all(|character| character.is_ascii_digit())
    {
        return text[..start].to_string();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt;

    /// An error whose text and cause the test chooses
    #[derive(Debug)]
    struct Link {
        text: &'static str,
        source: Option<Box<Link>>,
    }

    impl Link {
        fn new(text: &'static str) -> Link {
            Link { text, source: None }
        }

        fn caused_by(self, source: Link) -> Link {
            Link {
                source: Some(Box::new(source)),
                ..self
            }
        }
    }

    impl fmt::Display for Link {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(self.text)
        }
    }

    impl Error for Link {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            self.source
                .as_deref()
                .map(|source| source as &(dyn Error + 'static))
        }
    }

    #[test]
    fn the_root_is_the_innermost_link() {
        let error = Link::new("Failed to fetch feed")
            .caused_by(Link::new("error sending request").caused_by(Link::new("dns error")));

        assert_eq!(root_text(&error), "dns error");
    }

    #[test]
    fn an_error_without_a_cause_is_its_own_root() {
        assert_eq!(root_text(&Link::new("nothing below")), "nothing below");
    }

    #[test]
    fn a_link_without_text_is_skipped() {
        let error = Link::new("outer").caused_by(Link::new("middle").caused_by(Link::new("")));

        assert_eq!(root_text(&error), "middle");
    }

    #[test]
    fn the_os_error_suffix_is_dropped() {
        let error = Link::new("outer").caused_by(Link::new("Permission denied (os error 13)"));

        assert_eq!(root_text(&error), "permission denied");
    }

    #[test]
    fn a_leading_capital_is_lowercased() {
        assert_eq!(
            root_text(&Link::new("Connection refused")),
            "connection refused"
        );
    }

    #[test]
    fn a_leading_acronym_keeps_its_capitals() {
        assert_eq!(root_text(&Link::new("HTTP error 503")), "HTTP error 503");
        assert_eq!(
            root_text(&Link::new("EOF while parsing a string")),
            "EOF while parsing a string"
        );
    }

    #[test]
    fn text_that_starts_in_lower_case_stays() {
        assert_eq!(
            root_text(&Link::new("failed to lookup address information")),
            "failed to lookup address information"
        );
    }

    #[tokio::test]
    async fn the_request_url_is_not_part_of_the_root_of_a_status_error() {
        use crate::http::{HttpClient, ReqwestClient};

        let server = crate::test_server::serve_once(
            b"HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\n\r\n".to_vec(),
        );
        let url = format!("http://127.0.0.1:{}/feed.xml", server.port());

        let error = ReqwestClient::without_proxy()
            .get_bytes(&url)
            .await
            .expect_err("the status is an error");

        assert_eq!(
            root_text(&error),
            "HTTP status server error (503 Service Unavailable)"
        );
        server.finish();
    }
}
