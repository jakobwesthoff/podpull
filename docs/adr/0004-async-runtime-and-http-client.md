# 4. Async runtime and HTTP client

Date: 2026-01-31

## Status

Accepted

Enables [8. User feedback and progress display](0008-user-feedback-and-progress-display.md)

## Context

podpull needs to perform network operations:

1. Fetch RSS feeds from remote URLs
1. Download potentially large audio files (50-200MB per episode)
1. Eventually support concurrent downloads for performance

These operations are I/O-bound and benefit from async execution. We need:

* An async runtime
* An HTTP client that supports streaming downloads (to avoid loading entire files into memory)

## Decision

We will use **tokio** as the async runtime and **reqwest** as the HTTP client.

**Tokio** configuration:

* `rt-multi-thread`: Multi-threaded runtime for concurrent downloads
* `macros`: `#[tokio::main]` macro for convenience

**Reqwest** configuration:

* Default features include `rustls` for TLS (pure Rust, no system dependencies)
* Streaming response body for memory-efficient large file downloads

**Alternatives considered:**

* `async-std`: Less ecosystem adoption, fewer integrations
* `ureq`: Synchronous only, would block on downloads
* `hyper`: Lower-level, requires more boilerplate
* `native-tls`: Would require OpenSSL/system TLS libraries

## Consequences

**Benefits:**

* Efficient concurrent I/O without blocking threads
* Streaming downloads prevent memory exhaustion on large files
* `rustls` means no external TLS dependencies, easier cross-compilation
* Strong ecosystem integration (most async Rust libraries support tokio)
* Progress tracking possible via streaming

**Drawbacks:**

* Async Rust has a learning curve
* Larger dependency tree
* Compile times increased

**Dependencies added:**

* `tokio = { version = "1", features = ["rt-multi-thread", "macros"] }`
* `reqwest = "0.13"` (uses rustls by default)

**Enables:**

* ADR-0008: Progress display during async downloads