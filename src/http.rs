// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::error::Error;
use std::io;
use std::pin::Pin;

use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;

/// A streaming response body
pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, HttpError>> + Send>>;

/// HTTP response with status, content length, and body stream
pub struct HttpResponse {
    /// HTTP status code
    pub status: u16,
    /// Content-Length header value, if present
    pub content_length: Option<u64>,
    /// Response body as a stream of bytes
    pub body: ByteStream,
}

/// What kind of failure an HTTP request ran into
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpErrorKind {
    /// The host name could not be resolved
    Dns,
    /// The host refused the connection
    Refused,
    /// The host did not answer in time
    TimedOut,
    /// The secure connection could not be set up
    Tls,
    /// The connection could not be made for another reason
    Connect,
    /// The host answered with an error status
    Status(u16),
    /// Anything else, such as a malformed URL or a broken body
    Other,
}

/// A failed HTTP request
///
/// The kind is decided once, where the HTTP library's error is turned into
/// this one, so nothing else needs to know that library.
#[derive(Debug, thiserror::Error)]
#[error("{source}")]
pub struct HttpError {
    kind: HttpErrorKind,
    #[source]
    source: Box<dyn Error + Send + Sync>,
}

impl HttpError {
    pub fn new(kind: HttpErrorKind, source: impl Into<Box<dyn Error + Send + Sync>>) -> Self {
        Self {
            kind,
            source: source.into(),
        }
    }

    pub fn kind(&self) -> HttpErrorKind {
        self.kind
    }
}

/// What a failed request is known to be, as plain values
///
/// Kept apart from `reqwest::Error`, which cannot be built outside reqwest,
/// so every rule of [`kind_from`] can be tested.
#[derive(Debug, Default)]
struct Facts {
    dns: bool,
    timeout: bool,
    status: Option<u16>,
    connect: bool,
    /// Kinds of the `io::Error`s in the chain below the request error
    io_kinds: Vec<io::ErrorKind>,
    /// A link below the request error that is not the request error itself
    /// says something about certificates, TLS or a handshake
    tls_words: bool,
}

/// The kind of failure the facts describe; the first matching rule wins
fn kind_from(facts: &Facts) -> HttpErrorKind {
    if facts.dns {
        return HttpErrorKind::Dns;
    }
    if facts.io_kinds.contains(&io::ErrorKind::ConnectionRefused) {
        return HttpErrorKind::Refused;
    }
    if facts.timeout || facts.io_kinds.contains(&io::ErrorKind::TimedOut) {
        return HttpErrorKind::TimedOut;
    }
    if let Some(status) = facts.status {
        return HttpErrorKind::Status(status);
    }
    if facts.connect {
        // A TLS stream reports failures of its protocol as `InvalidData`.
        if facts.io_kinds.contains(&io::ErrorKind::InvalidData) || facts.tls_words {
            return HttpErrorKind::Tls;
        }
        return HttpErrorKind::Connect;
    }
    HttpErrorKind::Other
}

/// Words in the text of a TLS library's failure
///
/// The connector wraps the TLS library's error in an `io::Error` of kind
/// `Other`, so the text is the only sign that the failure was one of TLS
/// short of depending on the library itself.
const TLS_WORDS: [&str; 7] = [
    "certificate",
    "tls",
    "handshake",
    "ssl",
    "corrupt message",
    "fatal alert",
    "peer misbehaved",
];

fn facts_of(error: &reqwest::Error) -> Facts {
    let mut facts = Facts {
        dns: error.is_dns(),
        timeout: error.is_timeout(),
        status: error.status().map(|status| status.as_u16()),
        connect: error.is_connect(),
        ..Facts::default()
    };
    // The text of the request error itself holds the request URL, so a host
    // such as `ssl.example.com` must not count as a word about TLS; only the
    // links below it are read.
    let mut link = error.source();
    while let Some(error) = link {
        if let Some(io_error) = error.downcast_ref::<io::Error>() {
            facts.io_kinds.push(io_error.kind());
        }
        let text = error.to_string().to_lowercase();
        facts.tls_words |= TLS_WORDS.iter().any(|word| text.contains(word));
        link = error.source();
    }
    facts
}

impl From<reqwest::Error> for HttpError {
    fn from(error: reqwest::Error) -> Self {
        Self::new(kind_from(&facts_of(&error)), error)
    }
}

/// HTTP client abstraction for testability
#[async_trait]
pub trait HttpClient: Send + Sync {
    /// Fetch the entire response body as bytes
    async fn get_bytes(&self, url: &str) -> Result<Bytes, HttpError>;

    /// Get a streaming response for large downloads
    async fn get_stream(&self, url: &str) -> Result<HttpResponse, HttpError>;
}

/// Default HTTP client implementation using reqwest
pub struct ReqwestClient {
    client: reqwest::Client,
}

impl ReqwestClient {
    /// Create a new ReqwestClient with default settings
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }

    /// A client that ignores proxy settings of the environment and the
    /// system, so tests reach the local servers they start
    #[cfg(test)]
    pub(crate) fn without_proxy() -> Self {
        Self {
            client: reqwest::Client::builder()
                .no_proxy()
                .build()
                .expect("a client without proxy settings builds"),
        }
    }
}

impl Default for ReqwestClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HttpClient for ReqwestClient {
    async fn get_bytes(&self, url: &str) -> Result<Bytes, HttpError> {
        Ok(self.client.get(url).send().await?.bytes().await?)
    }

    async fn get_stream(&self, url: &str) -> Result<HttpResponse, HttpError> {
        use futures::StreamExt;

        let response = self.client.get(url).send().await?;
        let status = response.status().as_u16();
        let content_length = response.content_length();

        let body: ByteStream = Box::pin(
            response
                .bytes_stream()
                .map(|result| result.map_err(HttpError::from)),
        );

        Ok(HttpResponse {
            status,
            content_length,
            body,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_server::{answer_unasked, closed_port, serve_once};

    #[test]
    fn reqwest_client_can_be_created() {
        let _client = ReqwestClient::new();
        let _client_default = ReqwestClient::default();
    }

    // =========================================================
    // The rules that turn the facts of a failure into its kind
    // =========================================================

    fn kind_of(facts: Facts) -> HttpErrorKind {
        kind_from(&facts)
    }

    #[test]
    fn a_dns_failure_is_dns_even_with_other_facts() {
        let facts = Facts {
            dns: true,
            connect: true,
            io_kinds: vec![io::ErrorKind::TimedOut],
            ..Facts::default()
        };

        assert_eq!(kind_of(facts), HttpErrorKind::Dns);
    }

    #[test]
    fn a_refused_connection_is_refused() {
        let facts = Facts {
            connect: true,
            io_kinds: vec![io::ErrorKind::ConnectionRefused],
            ..Facts::default()
        };

        assert_eq!(kind_of(facts), HttpErrorKind::Refused);
    }

    #[test]
    fn a_timeout_of_the_request_is_timed_out() {
        let facts = Facts {
            timeout: true,
            ..Facts::default()
        };

        assert_eq!(kind_of(facts), HttpErrorKind::TimedOut);
    }

    #[test]
    fn a_timed_out_connection_is_timed_out() {
        let facts = Facts {
            connect: true,
            io_kinds: vec![io::ErrorKind::TimedOut],
            ..Facts::default()
        };

        assert_eq!(kind_of(facts), HttpErrorKind::TimedOut);
    }

    #[test]
    fn an_error_status_is_a_status() {
        let facts = Facts {
            status: Some(404),
            ..Facts::default()
        };

        assert_eq!(kind_of(facts), HttpErrorKind::Status(404));
    }

    #[test]
    fn invalid_data_below_a_connect_error_is_tls() {
        let facts = Facts {
            connect: true,
            io_kinds: vec![io::ErrorKind::InvalidData],
            ..Facts::default()
        };

        assert_eq!(kind_of(facts), HttpErrorKind::Tls);
    }

    #[test]
    fn tls_words_below_a_connect_error_are_tls() {
        let facts = Facts {
            connect: true,
            tls_words: true,
            ..Facts::default()
        };

        assert_eq!(kind_of(facts), HttpErrorKind::Tls);
    }

    #[test]
    fn a_reset_connection_is_connect() {
        let facts = Facts {
            connect: true,
            io_kinds: vec![io::ErrorKind::ConnectionReset],
            ..Facts::default()
        };

        assert_eq!(kind_of(facts), HttpErrorKind::Connect);
    }

    #[test]
    fn tls_words_without_a_connect_error_are_other() {
        let facts = Facts {
            tls_words: true,
            ..Facts::default()
        };

        assert_eq!(kind_of(facts), HttpErrorKind::Other);
    }

    // =========================================================
    // Real failures, against local servers and names that never resolve
    // =========================================================

    async fn kind_of_get(client: &ReqwestClient, url: &str) -> HttpErrorKind {
        match client.get_bytes(url).await {
            Ok(_) => panic!("{url} was expected to fail"),
            Err(error) => error.kind(),
        }
    }

    #[tokio::test]
    async fn a_closed_port_is_refused() {
        let url = format!("http://127.0.0.1:{}/feed.xml", closed_port());

        let kind = kind_of_get(&ReqwestClient::without_proxy(), &url).await;

        assert_eq!(kind, HttpErrorKind::Refused);
    }

    #[tokio::test]
    async fn a_name_that_never_resolves_is_dns() {
        let kind = kind_of_get(
            &ReqwestClient::without_proxy(),
            "http://feeds.does-not-exist.invalid/feed.xml",
        )
        .await;

        assert_eq!(kind, HttpErrorKind::Dns);
    }

    #[tokio::test]
    async fn a_plain_server_behind_an_https_url_is_tls() {
        let server = answer_unasked(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n".to_vec());
        let url = format!("https://127.0.0.1:{}/feed.xml", server.port());

        let kind = kind_of_get(&ReqwestClient::without_proxy(), &url).await;

        assert_eq!(kind, HttpErrorKind::Tls);
        server.finish();
    }

    #[tokio::test]
    async fn a_malformed_url_is_other() {
        let kind = kind_of_get(&ReqwestClient::without_proxy(), "http://").await;

        assert_eq!(kind, HttpErrorKind::Other);
    }

    #[tokio::test]
    async fn a_host_that_never_answers_times_out() {
        let server = crate::test_server::serve_silently();
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_millis(100))
            .build()
            .expect("a client with a timeout builds");
        let url = format!("http://127.0.0.1:{}/feed.xml", server.port());

        let error = HttpError::from(
            client
                .get(&url)
                .send()
                .await
                .expect_err("the server never answers"),
        );

        assert_eq!(error.kind(), HttpErrorKind::TimedOut);
        server.finish();
    }

    #[tokio::test]
    async fn get_bytes_returns_the_body() {
        let server = serve_once(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nhi".to_vec());
        let url = format!("http://127.0.0.1:{}/feed.xml", server.port());

        let body = ReqwestClient::without_proxy().get_bytes(&url).await;

        assert_eq!(body.expect("the server answers").as_ref(), b"hi");
        server.finish();
    }

    #[tokio::test]
    async fn get_stream_returns_the_status_and_the_body() {
        use futures::StreamExt;

        let server = serve_once(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nhi".to_vec());
        let url = format!("http://127.0.0.1:{}/ep.mp3", server.port());

        let response = ReqwestClient::without_proxy()
            .get_stream(&url)
            .await
            .expect("the server answers");
        let chunks: Vec<_> = response.body.collect().await;

        assert_eq!(response.status, 200);
        assert_eq!(response.content_length, Some(2));
        assert_eq!(chunks.len(), 1);
        assert_eq!(
            chunks[0].as_ref().expect("the body is whole").as_ref(),
            b"hi"
        );
        server.finish();
    }

    #[tokio::test]
    async fn a_body_cut_short_fails_the_stream_with_an_http_error() {
        use futures::StreamExt;

        let server = serve_once(b"HTTP/1.1 200 OK\r\ncontent-length: 10\r\n\r\nhi".to_vec());
        let url = format!("http://127.0.0.1:{}/ep.mp3", server.port());

        let response = ReqwestClient::without_proxy()
            .get_stream(&url)
            .await
            .expect("the head arrives whole");
        let mut body = response.body;
        let mut failure = None;
        while let Some(chunk) = body.next().await {
            if let Err(error) = chunk {
                failure = Some(error);
                break;
            }
        }
        let failure = failure.expect("the body ends before its length");

        assert_eq!(failure.kind(), HttpErrorKind::Other);
        server.finish();
    }

    #[tokio::test]
    async fn the_display_of_an_http_error_is_the_text_of_its_source() {
        let error = HttpError::new(HttpErrorKind::Other, "something broke");

        assert_eq!(error.to_string(), "something broke");
        assert_eq!(error.kind(), HttpErrorKind::Other);
        assert!(error.source().is_some());
    }
}
