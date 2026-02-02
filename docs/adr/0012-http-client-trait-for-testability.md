# 12. HTTP client trait for testability

Date: 2026-01-31

## Status

Accepted

Extends [4. Async runtime and HTTP client](0004-async-runtime-and-http-client.md)

## Context

ADR-0004 selected `reqwest` as the HTTP client. However, directly using `reqwest::Client` throughout the library creates problems:

- Unit tests require network access or complex mocking
- Tests are slow and flaky due to real HTTP calls
- Cannot test error handling paths (network failures, HTTP errors)
- Library consumers cannot substitute their own HTTP implementation

## Decision

We introduce an `HttpClient` trait as the library's HTTP abstraction:

```rust
#[async_trait]
pub trait HttpClient: Send + Sync {
    async fn get_bytes(&self, url: &str) -> Result<Bytes, reqwest::Error>;
    async fn get_stream(&self, url: &str) -> Result<HttpResponse, reqwest::Error>;
}

pub struct HttpResponse {
    pub status: u16,
    pub content_length: Option<u64>,
    pub body: ByteStream,
}

pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>;
```

**Default implementation** wraps `reqwest`:

```rust
pub struct ReqwestClient {
    client: reqwest::Client,
}

#[async_trait]
impl HttpClient for ReqwestClient {
    async fn get_bytes(&self, url: &str) -> Result<Bytes, reqwest::Error> {
        self.client.get(url).send().await?.bytes().await
    }

    async fn get_stream(&self, url: &str) -> Result<HttpResponse, reqwest::Error> {
        let response = self.client.get(url).send().await?;
        // ... wrap in HttpResponse
    }
}
```

**Test implementations** are trivial:

```rust
struct MockHttpClient {
    response_data: Vec<u8>,
    status: u16,
}

#[async_trait]
impl HttpClient for MockHttpClient {
    async fn get_stream(&self, _url: &str) -> Result<HttpResponse, reqwest::Error> {
        Ok(HttpResponse {
            status: self.status,
            content_length: Some(self.response_data.len() as u64),
            body: Box::pin(stream::once(async { Ok(Bytes::from(self.response_data.clone())) })),
        })
    }
}
```

**Generic library functions** accept any `HttpClient`:

```rust
pub async fn sync_podcast<C: HttpClient + Clone + 'static>(
    client: &C,
    feed_source: &str,
    output_dir: &Path,
    options: &SyncOptions,
    reporter: SharedProgressReporter,
) -> Result<SyncResult, SyncError>;
```

## Consequences

**Benefits:**

- Unit tests run without network access
- Tests are fast and deterministic
- Can test error scenarios (404, timeouts, malformed responses)
- Library consumers can provide custom HTTP implementations
- Streaming interface preserved for memory-efficient downloads

**Trade-offs:**

- Additional abstraction layer
- `async_trait` dependency for async trait methods
- Error type still tied to `reqwest::Error` (acceptable trade-off)

**Dependencies added:**

- `async-trait = "0.1"` (for async trait methods)
- `futures = "0.3"` (for `Stream` trait)
- `bytes = "1"` (for `Bytes` type)

**Alternatives considered:**

- `mockall` crate: Heavy, macro-based, overkill for simple interface
- Feature flags for test mode: Doesn't help library consumers
- No abstraction: Untestable, inflexible
