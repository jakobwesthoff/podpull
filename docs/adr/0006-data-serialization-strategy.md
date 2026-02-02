# 6. Data serialization strategy

Date: 2026-01-31

## Status

Accepted

## Context

podpull needs to persist metadata as JSON files:
- `podcast.json`: Channel-level metadata (title, description, feed URL)
- `{date}-{title}.json`: Episode metadata (title, description, original URL, download date)

This metadata enables:
- Reconstructing information if the feed disappears
- Tools to browse/search the local podcast library
- Potential future features (played status, notes)

We need a serialization solution that can convert Rust structs to/from JSON.

## Decision

We will use **serde** with **serde_json** for all serialization.

```rust
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct EpisodeMetadata {
    title: String,
    description: Option<String>,
    pub_date: Option<String>,
    original_url: String,
    downloaded_at: String,
}
```

**Alternatives considered:**
- `miniserde`: Smaller but limited (no Option, no enums)
- Manual JSON construction: Error-prone, tedious
- Other formats (TOML, YAML): JSON is more universal for metadata interchange

## Consequences

**Benefits:**
- Derive macros for zero-boilerplate serialization
- De facto standard in Rust ecosystem
- Excellent documentation
- Supports all Rust types (Option, Vec, enums, nested structs)
- Pretty-printing for human-readable output

**Drawbacks:**
- Compile time impact from proc macros
- serde is a large dependency (but already used by many other crates)

**Dependencies added:**
- `serde = { version = "1", features = ["derive"] }`
- `serde_json = "1"`

**Output format:**
- JSON files will be pretty-printed for human readability
- UTF-8 encoding throughout
