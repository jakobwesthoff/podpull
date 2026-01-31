# podpull

A fast, minimal CLI tool for downloading and synchronizing podcasts from RSS feeds. No cloud services, no accounts, no databases — just your podcasts, stored locally, under your control.

## Installation

```bash
cargo install podpull
```

## Quick Start

```bash
# Download a podcast
podpull https://example.com/podcast/feed.xml ~/Podcasts/my-show/

# Run again later to sync new episodes
podpull https://example.com/podcast/feed.xml ~/Podcasts/my-show/
# => "42 episodes already downloaded, 3 new episodes to fetch"
```

## Core Concepts

### The Output Directory IS the State

No database. No config files. No hidden state. podpull looks at what's already in the output directory and only downloads what's missing. Want to re-download an episode? Delete its files. Want to start fresh? Delete the directory. Want to know what you have? Just look.

```
~/Podcasts/my-show/
├── podcast.json                      # Feed metadata
├── 2024-01-15-episode-title.mp3      # Audio file
├── 2024-01-15-episode-title.json     # Episode metadata
├── 2024-01-08-another-episode.mp3
└── 2024-01-08-another-episode.json
```

Episodes are identified by their GUID from the RSS feed. If an episode file exists with a matching GUID in its metadata, it won't be downloaded again.

## Usage

```
podpull [OPTIONS] <feed> <output-dir>
```

### Arguments

| Argument | Description |
|----------|-------------|
| `<feed>` | RSS feed URL or path to a local RSS file |
| `<output-dir>` | Directory where episodes will be downloaded |

### Options

| Option | Default | Description |
|--------|---------|-------------|
| `-c, --concurrent <N>` | 3 | Maximum concurrent downloads |
| `-l, --limit <N>` | — | Only download the N most recent episodes |
| `-q, --quiet` | — | Suppress progress output |
| `-h, --help` | — | Print help |
| `-V, --version` | — | Print version |

### Examples

**Sync from a URL:**
```bash
podpull https://feeds.example.com/podcast.xml ~/Podcasts/my-show/
```

**Sync from a local RSS file:**
```bash
podpull ./feed.xml ~/Podcasts/my-show/
```

**Download faster with more connections:**
```bash
podpull -c 5 https://feeds.example.com/podcast.xml ~/Podcasts/my-show/
```

**Gradually download a large back-catalog:**
```bash
podpull -l 10 https://feeds.example.com/podcast.xml ~/Podcasts/my-show/
# => Downloads 10 newest episodes

# Run again later...
podpull -l 10 https://feeds.example.com/podcast.xml ~/Podcasts/my-show/
# => Downloads the NEXT 10 episodes (previously downloaded ones are skipped)
```

The `--limit` option applies to episodes that haven't been downloaded yet. Already-downloaded episodes (identified by their GUID) are excluded before the limit is applied. This means you can incrementally download a large archive by running the same command repeatedly — each run fetches the next batch of episodes until the entire catalog is downloaded.

Episodes are sorted by publication date (newest first), so you always get the most recent undownloaded episodes. Episodes without a publication date are sorted last.

## Metadata Format

### podcast.json

Feed-level metadata, updated on every sync:

```json
{
  "title": "My Favorite Show",
  "description": "A podcast about interesting things",
  "link": "https://example.com/podcast",
  "author": "Podcast Author",
  "image_url": "https://example.com/cover.jpg",
  "feed_url": "https://example.com/podcast/feed.xml",
  "updated_at": "2024-01-15T10:30:00Z"
}
```

### Episode Metadata

Per-episode metadata stored alongside each audio file:

```json
{
  "title": "Episode Title",
  "description": "Episode description...",
  "pub_date": "2024-01-15T08:00:00Z",
  "guid": "unique-episode-identifier",
  "original_url": "https://example.com/episode.mp3",
  "downloaded_at": "2024-01-15T10:30:00Z",
  "duration": "01:23:45",
  "episode_number": 42,
  "season_number": 2,
  "audio_filename": "2024-01-15-episode-title.mp3",
  "content_hash": "sha256:abc123..."
}
```

The `content_hash` is a SHA-256 hash of the downloaded file, useful for verifying integrity or detecting if a file was modified.

## Why podpull?

Podcasts disappear. Feeds go offline. Hosting changes. Episodes get pulled. If you have podcasts you truly care about, the only way to guarantee access is to keep your own copy.

podpull makes that easy — point it at a feed, run it periodically (cron job, anyone?), and rest easy knowing your favorite shows are safely backed up.

## Limitations

**Episodes without GUIDs:** Some RSS feeds don't include GUIDs for episodes. podpull can't reliably detect duplicates in this case and will re-download those episodes on every sync. Complain to your podcast's publisher, not me.

**Feed quirks:** RSS is a "standard" in the same way that HTML was a standard in 2003 — everyone does it slightly differently. podpull handles the common cases and iTunes podcast extensions, but exotic feeds might not parse perfectly.

## Development

```bash
# Clone
git clone https://github.com/jakobwesthoff/podpull.git
cd podpull

# Build
cargo build --release

# Run tests
cargo test

# Run from source
cargo run -- https://example.com/feed.xml ./output/
```

## License

This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

Copyright (c) 2025 Jakob Westhoff <jakob@westhoffswelt.de>
