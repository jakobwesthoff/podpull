# 13. Library public API design

Date: 2026-01-31

## Status

Accepted

Builds on [2. Project structure: library and binary](0002-project-structure-library-and-binary.md)

## Context

Per ADR-0002, podpull is structured as a library with a thin CLI wrapper. The library needs a well-designed public API that:

- Exposes the right level of abstraction for common use cases
- Allows fine-grained control when needed
- Is consistent and predictable
- Minimizes breaking changes as the library evolves

## Decision

The library exposes a **layered API** with three levels:

### Layer 1: High-Level Orchestration

Single function for the common "sync a podcast" use case:

```rust
pub async fn sync_podcast<C: HttpClient + Clone + 'static>(
    client: &C,
    feed_source: &str,        // URL or local file path
    output_dir: &Path,
    options: &SyncOptions,
    reporter: SharedProgressReporter,
) -> Result<SyncResult, SyncError>;
```

**Configuration via structs:**

```rust
pub struct SyncOptions {
    pub limit: Option<usize>,       // Max episodes to download
    pub max_concurrent: usize,      // Parallel downloads (default: 3)
    pub continue_on_error: bool,    // Continue on individual failures
}

pub struct SyncResult {
    pub downloaded: usize,
    pub skipped: usize,
    pub failed: usize,
    pub failed_episodes: Vec<(String, String)>,
}
```

### Layer 2: Domain Types

Parsed podcast/episode data for inspection or custom processing:

```rust
pub struct Podcast {
    pub title: String,
    pub description: Option<String>,
    pub link: Option<Url>,
    pub author: Option<String>,
    pub image_url: Option<Url>,
    pub feed_url: Url,
    pub episodes: Vec<Episode>,
}

pub struct Episode {
    pub title: String,
    pub pub_date: Option<DateTime<FixedOffset>>,
    pub guid: Option<String>,
    pub enclosure: Enclosure,
    // ...
}
```

### Layer 3: Building Blocks

Individual functions for custom workflows:

```rust
// Feed operations
pub async fn fetch_feed<C: HttpClient>(client: &C, url: &str) -> Result<Podcast, FeedError>;
pub fn parse_feed_file(path: &Path) -> Result<Podcast, FeedError>;

// State management
pub fn scan_output_dir(path: &Path) -> Result<OutputState, StateError>;
pub fn create_sync_plan(episodes: Vec<Episode>, state: &OutputState) -> SyncPlan;

// Downloads
pub async fn download_episode<C: HttpClient>(...) -> Result<u64, DownloadError>;

// Filename generation
pub fn generate_filename(episode: &Episode) -> String;

// Metadata I/O
pub fn write_podcast_metadata(podcast: &Podcast, dir: &Path) -> Result<(), MetadataError>;
pub fn write_episode_metadata(episode: &Episode, filename: &str, path: &Path) -> Result<(), MetadataError>;
```

### Re-exports in lib.rs

All public types are re-exported at the crate root for ergonomic imports:

```rust
use podpull::{sync_podcast, SyncOptions, Podcast, Episode, ReqwestClient};
```

## Consequences

**Benefits:**

- Simple things are simple (`sync_podcast` for common case)
- Complex things are possible (building blocks for custom workflows)
- Domain types are inspectable (not opaque)
- Dependency injection via traits (`HttpClient`, `ProgressReporter`)
- Configuration structs with `Default` implementations

**Trade-offs:**

- Larger API surface to maintain
- Must be careful about what's `pub` to avoid accidental exposure

**Design principles followed:**

- **Progressive disclosure**: High-level first, details available when needed
- **Composition over inheritance**: Functions + traits, not class hierarchies
- **Explicit dependencies**: No global state, all deps passed as parameters
- **Type-safe configuration**: Structs over stringly-typed options
