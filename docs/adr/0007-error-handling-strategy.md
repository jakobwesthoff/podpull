# 7. Error handling strategy

Date: 2026-01-31

## Status

Accepted

Builds on [2. Project structure: library and binary](0002-project-structure-library-and-binary.md)

## Context

podpull can fail in many ways:

* Network errors (unreachable host, timeout, TLS failure)
* RSS parsing errors (malformed XML, missing required fields)
* File system errors (permission denied, disk full)
* URL parsing errors (invalid feed URL)
* Date parsing errors (malformed pubDate)

Per ADR-0002, we have a library/binary split. Error handling needs differ:

* **Library**: Callers need typed errors to handle specific failure cases
* **Binary**: Users need readable error messages with context

## Decision

We will use a two-layer error handling strategy:

**Library layer: `thiserror`**

````rust
#[derive(Debug, thiserror::Error)]
pub enum FeedError {
    #[error("failed to fetch feed from {url}")]
    Fetch { url: String, #[source] source: reqwest::Error },

    #[error("invalid RSS: {0}")]
    Parse(#[from] rss::Error),

    #[error("no enclosure found for episode")]
    MissingEnclosure,
}
````

**Binary layer: `anyhow`**

````rust
fn main() -> anyhow::Result<()> {
    let feed = podpull::fetch_feed(&url)
        .context("failed to load podcast feed")?;
    // ...
}
````

**Alternatives considered:**

* `thiserror` everywhere: Too verbose for CLI error output
* `anyhow` everywhere: Library users can't pattern match on errors
* Custom error types without macros: Excessive boilerplate

## Consequences

**Benefits:**

* Library consumers can match on specific error variants
* CLI users see readable, contextual error messages
* Automatic `From` implementations via `#[from]`
* Error chains preserved with `#[source]`
* Clear separation between "what went wrong" (library) and "what to tell the user" (binary)

**Drawbacks:**

* Two error handling crates to learn
* Library error types require maintenance as features are added

**Dependencies added:**

* `thiserror = "2"`
* `anyhow = "1"`

**Builds on:**

* ADR-0002: Project structure enables the two-layer approach