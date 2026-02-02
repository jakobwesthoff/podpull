# 9. Date and time handling

Date: 2026-01-31

## Status

Accepted

Related to [5. RSS feed parsing approach](0005-rss-feed-parsing-approach.md)

## Context

podpull works with dates in several contexts:

**Input (from RSS feeds):**

* `pubDate` in RFC 822 format: `"Mon, 15 Jan 2024 09:00:00 GMT"`
* Some feeds use non-standard formats or omit timezone

**Output (filenames and metadata):**

* Episode filenames: `2024-01-15-episode-title.mp3`
* Metadata JSON: ISO 8601 timestamps for `downloaded_at`

**Operations needed:**

* Parse RFC 822 dates from RSS
* Format dates as `YYYY-MM-DD` for filenames
* Get current timestamp for download records

## Decision

We will use **chrono** for all date and time handling.

````rust
use chrono::{DateTime, FixedOffset, Utc};

// Parse RFC 822 from RSS pubDate
let pub_date = DateTime::parse_from_rfc2822(date_str)?;

// Format for filename prefix
let prefix = pub_date.format("%Y-%m-%d").to_string();

// Current time for metadata
let downloaded_at = Utc::now().to_rfc3339();
````

**Alternatives considered:**

* `time`: Lighter alternative, but chrono has better RFC 822 support and is more battle-tested
* Manual parsing: Error-prone, doesn't handle timezone variations
* String manipulation: Would break on edge cases

## Consequences

**Benefits:**

* Robust RFC 822 parsing handles timezone variations
* Locale-independent formatting (always `YYYY-MM-DD`, not system locale)
* Timezone-aware types prevent common datetime bugs
* Comprehensive formatting options
* Well-documented, widely used

**Drawbacks:**

* Relatively large dependency
* chrono has had historical security issues (now resolved)

**Dependencies added:**

* `chrono = "0.4"`

**Related decisions:**

* ADR-0005: RSS crate provides `pubDate` strings that chrono parses

**Filename format:**

* Dates formatted as `YYYY-MM-DD` (ISO 8601 date portion)
* Enables chronological sorting in file browsers
* Example: `2024-01-15-my-podcast-episode.mp3`