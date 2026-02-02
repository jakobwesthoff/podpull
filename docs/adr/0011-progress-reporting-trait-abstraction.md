# 11. Progress reporting trait abstraction

Date: 2026-01-31

## Status

Accepted

Extends [8. User feedback and progress display](0008-user-feedback-and-progress-display.md)

## Context

ADR-0008 decided to use `indicatif` for progress display. However, coupling the library directly to `indicatif` would:

* Force all library consumers to depend on `indicatif`
* Make unit testing difficult (progress bars in tests)
* Prevent alternative UIs (GUI, web, logging-only)

The library needs to report progress without knowing how it will be displayed.

## Decision

We introduce a `ProgressReporter` trait as the library's external interface for progress feedback:

````rust
pub trait ProgressReporter: Send + Sync {
    fn report(&self, event: ProgressEvent);
}

pub type SharedProgressReporter = Arc<dyn ProgressReporter>;
````

**Event-based design** with strongly-typed events:

````rust
pub enum ProgressEvent {
    FetchingFeed { url: String },
    FeedParsed { podcast_title: String, total_episodes: usize, new_episodes: usize },
    DownloadStarting { download_id: usize, episode_title: String, ... },
    DownloadProgress { download_id: usize, bytes_downloaded: u64, total_bytes: Option<u64>, ... },
    DownloadCompleted { download_id: usize, episode_title: String, bytes_downloaded: u64 },
    DownloadFailed { download_id: usize, episode_title: String, error: String },
    SyncCompleted { downloaded_count: usize, skipped_count: usize, failed_count: usize },
}
````

**Key design elements:**

1. **`download_id: usize`** - Identifies concurrent download slots (0 to max_concurrent-1), enabling multi-progress bar management

1. **`SharedProgressReporter`** - `Arc<dyn ProgressReporter>` allows sharing across spawned tasks

1. **`NoopReporter`** - Silent implementation for tests and quiet mode:
   
   ````rust
   impl ProgressReporter for NoopReporter {
       fn report(&self, _event: ProgressEvent) {}
   }
   ````

**Binary implements the trait** with `indicatif`:

````rust
struct IndicatifReporter { ... }

impl ProgressReporter for IndicatifReporter {
    fn report(&self, event: ProgressEvent) {
        match event {
            ProgressEvent::DownloadProgress { download_id, ... } => {
                self.get_bar(download_id).set_position(bytes);
            }
            // ...
        }
    }
}
````

## Consequences

**Benefits:**

* Library is UI-agnostic; no `indicatif` dependency in `lib.rs`
* Easy to test with `NoopReporter`
* Alternative implementations possible (logging, GUI, metrics)
* Concurrent downloads tracked via `download_id`
* Type-safe events prevent stringly-typed progress updates

**Trade-offs:**

* Additional abstraction layer
* Event enum may grow as features are added

**Alternatives considered:**

* Callback closures: Less type-safe, harder to share across threads
* Channel-based: More complex, overkill for progress reporting
* Direct `indicatif` usage in library: Tight coupling, untestable