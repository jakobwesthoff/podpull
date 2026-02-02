# 8. User feedback and progress display

Date: 2026-01-31

## Status

Accepted

Extended by [11. Progress reporting trait abstraction](0011-progress-reporting-trait-abstraction.md)

Enabled by [4. Async runtime and HTTP client](0004-async-runtime-and-http-client.md)

## Context

podpull performs long-running operations:

* Downloading large audio files (50-200MB each)
* Potentially syncing many episodes at once
* Network operations with variable latency

Users need feedback about:

* What's currently happening (fetching feed, downloading episode X of Y)
* Download progress (bytes transferred, speed, ETA)
* Final summary (X episodes downloaded, Y already present, Z failed)

Without progress feedback, users may think the application has hung.

## Decision

We will use **indicatif** for all progress display and user feedback.

**Capabilities we'll use:**

* `ProgressBar`: Download progress with bytes/speed/ETA
* `MultiProgress`: Display multiple concurrent downloads
* `ProgressStyle`: Custom formatting for different operations
* Spinners: For operations without measurable progress (feed parsing)

````rust
use indicatif::{ProgressBar, ProgressStyle};

let pb = ProgressBar::new(file_size);
pb.set_style(ProgressStyle::default_bar()
    .template("{spinner:.green} [{bar:40}] {bytes}/{total_bytes} ({eta})")?);

while let Some(chunk) = response.chunk().await? {
    file.write_all(&chunk)?;
    pb.inc(chunk.len() as u64);
}
pb.finish_with_message("downloaded");
````

**Alternatives considered:**

* `console`: Lower-level, would require building progress bars manually
* Raw `print!`: No progress bars, poor UX for long downloads
* `crossterm`/`termion`: Too low-level

## Consequences

**Benefits:**

* Professional progress bars out of the box
* Handles terminal width, refresh rates automatically
* Works well with async/tokio (we're already using that per ADR-0004)
* `MultiProgress` enables parallel download display
* Customizable styles for different operation types

**Drawbacks:**

* Additional dependency
* May not render well in non-TTY environments (CI, redirected output)
  * Mitigation: indicatif handles this gracefully, falls back to simpler output

**Dependencies added:**

* `indicatif = "0.18"`

**Enabled by:**

* ADR-0004: Async streaming downloads enable incremental progress updates