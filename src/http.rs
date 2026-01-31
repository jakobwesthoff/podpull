// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use std::pin::Pin;

/// A streaming response body
pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>;

/// HTTP response with status, content length, and body stream
pub struct HttpResponse {
    /// HTTP status code
    pub status: u16,
    /// Content-Length header value, if present
    pub content_length: Option<u64>,
    /// Response body as a stream of bytes
    pub body: ByteStream,
}

/// HTTP client abstraction for testability
#[async_trait]
pub trait HttpClient: Send + Sync {
    /// Fetch the entire response body as bytes
    async fn get_bytes(&self, url: &str) -> Result<Bytes, reqwest::Error>;

    /// Get a streaming response for large downloads
    async fn get_stream(&self, url: &str) -> Result<HttpResponse, reqwest::Error>;
}

/// Default HTTP client implementation using reqwest
#[derive(Clone)]
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

    /// Create a new ReqwestClient with a custom reqwest::Client
    pub fn with_client(client: reqwest::Client) -> Self {
        Self { client }
    }
}

impl Default for ReqwestClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HttpClient for ReqwestClient {
    async fn get_bytes(&self, url: &str) -> Result<Bytes, reqwest::Error> {
        self.client.get(url).send().await?.bytes().await
    }

    async fn get_stream(&self, url: &str) -> Result<HttpResponse, reqwest::Error> {
        use futures::StreamExt;

        let response = self.client.get(url).send().await?;
        let status = response.status().as_u16();
        let content_length = response.content_length();

        let body: ByteStream = Box::pin(response.bytes_stream().map(|result| result));

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

    #[test]
    fn reqwest_client_can_be_created() {
        let _client = ReqwestClient::new();
        let _client_default = ReqwestClient::default();
    }

    #[test]
    fn reqwest_client_can_be_cloned() {
        let client = ReqwestClient::new();
        let _cloned = client.clone();
    }
}
