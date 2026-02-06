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

<!-- docs:start -->
## Documentation

podpull downloads podcast episodes based on a feed URL. Point it at an RSS feed and specify what to download — it handles the rest.

```bash
podpull <FEED_URL> [OPTIONS]
```

### CLI Options

| Option | Default | Description |
|--------|---------|-------------|
| `<feed>` | Required | RSS feed URL or path to local file |
| `<output-dir>` | Required | Directory for downloaded episodes |
| `-c, --concurrent <N>` | 3 | Maximum concurrent downloads |
| `-l, --limit <N>` | — | Only download the N most recent undownloaded episodes |
| `-q, --quiet` | — | Suppress progress output |
| `-h, --help` | — | Print help |
| `-V, --version` | — | Print version |

### Output Structure

Each podcast gets its own directory containing the audio files and metadata:

```bash
~/Podcasts/my-show/
├── podcast.json                      # Feed metadata
├── 2024-01-15-episode-title.mp3      # Audio file
├── 2024-01-15-episode-title.json     # Episode metadata
├── 2024-01-08-another-episode.mp3
└── 2024-01-08-another-episode.json
```

No database. No config files. No hidden state. podpull looks at what's already in the output directory and only downloads what's missing. Want to re-download an episode? Delete its files. Want to start fresh? Delete the directory. Want to know what you have? Just look.

### Metadata Format

Feed-level metadata in `podcast.json`:

```json
{
  "title": "My Favorite Podcast",
  "description": "A podcast about interesting things",
  "author": "Podcast Author",
  "link": "https://example.com/podcast",
  "feed_url": "https://example.com/podcast/feed.xml",
  "last_synced": "2024-01-15T10:30:00Z"
}
```

Episode metadata alongside each audio file:

```json
{
  "guid": "episode-unique-id-123",
  "title": "Episode Title",
  "published": "2024-01-15T08:00:00Z",
  "url": "https://example.com/episode.mp3",
  "content_hash": "sha256:abc123...",
  "downloaded_at": "2024-01-15T10:30:00Z"
}
```

The `content_hash` is a SHA-256 hash of the downloaded file, useful for verifying integrity or detecting if a file was modified.

### How It Works

podpull follows a 4-phase sync process:

| Phase | What Happens |
|-------|-------------|
| **1. Fetching** | Downloads the RSS feed from the URL (or reads a local file) |
| **2. Parsing** | Extracts podcast metadata and episode list from the feed |
| **3. Scanning** | Reads existing episode metadata from the output directory to determine what's already downloaded |
| **4. Downloading** | Downloads missing episodes in parallel, showing progress for each |

The scanning phase displays a progress bar when processing many existing episodes — this is especially helpful on network shares where metadata reads can be slow.

### Smart Sync: How Episodes Are Tracked

podpull identifies episodes using their **GUID** (a unique identifier from the RSS feed). This means:

- Episodes are matched by GUID, not filename or URL
- Moving or renaming files in the output directory won't cause re-downloads (the JSON metadata contains the GUID)
- If a feed lacks GUIDs (rare), podpull falls back to using the episode URL as an identifier

> [!NOTE]
> **When Re-downloads Might Happen**
>
> If a podcast host changes their feed URL structure without preserving GUIDs, episodes may be re-downloaded. This is uncommon but can happen during podcast platform migrations.

### Safe Downloads

podpull uses atomic downloads to ensure file integrity:

- Episodes download to a temporary `.partial` file first
- A SHA-256 hash is computed during download and stored in the metadata
- Only when the download completes successfully is the file renamed to its final name
- If a download is interrupted, the `.partial` file is automatically cleaned up on the next sync

This means you'll never have corrupted files from interrupted downloads, and you can safely run podpull repeatedly.

### Error Handling

When individual episodes fail to download (network errors, 404s, etc.), podpull continues with the remaining episodes. At the end, failed episodes are listed:

```bash
Downloaded 47 of 50 episodes
Failed episodes:
  - Episode 23: Connection timeout
  - Episode 38: HTTP 404 Not Found
  - Episode 41: HTTP 503 Service Unavailable
```

Use `-q` (quiet mode) to suppress progress output but still see the final summary.

### Exit Codes

podpull returns meaningful exit codes for scripting:

| Exit Code | Meaning |
|-----------|---------|
| `0` | Success (episodes downloaded or already up to date) |
| `1` | Failure (no episodes downloaded and at least one failure occurred) |

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

### Advanced Examples

**Cron job with error detection:**
```bash
# In crontab - sync daily, log errors
0 3 * * * podpull -q https://example.com/feed.xml ~/Podcasts/show/ 2>&1 | logger -t podpull || echo "Sync failed" | mail -s "podpull error" you@example.com
```

**Gradual archive download (10 episodes at a time):**
```bash
# Download 10 oldest undownloaded episodes
# Run repeatedly to gradually build up the archive
podpull -l 10 https://example.com/feed.xml ~/Podcasts/huge-archive/
```

**Fast sync with many connections:**
```bash
# Use 8 concurrent downloads on a fast connection
podpull -c 8 https://example.com/feed.xml ~/Podcasts/show/
```

### Troubleshooting

**Episodes keep re-downloading:**
- Check if the podcast host changed their feed URL structure
- Look for missing GUID fields in the RSS feed (podpull will warn about this)
- Ensure the `.json` metadata files haven't been deleted

**Scanning phase is slow:**
- This is normal on network shares (NFS, SMB) with many episodes
- Each episode requires reading its metadata JSON file
- Consider using a local SSD for the podcast directory

**Download failures:**
- Transient network errors usually succeed on the next sync
- Persistent 404s may indicate the episode was removed from the host
- Try increasing `--concurrent` if downloads seem throttled

### Limitations

**Episodes without GUIDs:** Some RSS feeds don't include GUIDs for episodes. In this case, podpull uses the episode's download URL as a fallback identifier. This works fine unless the podcast host changes URLs (CDN migrations, hosting changes, etc.) — then those episodes will be re-downloaded since they appear as "new" episodes with different identifiers.

**Feed quirks:** RSS is a "standard" in the same way that HTML was a standard in 2003 — everyone does it slightly differently. podpull handles the common cases and iTunes podcast extensions, but exotic feeds might not parse perfectly.
<!-- docs:end -->

## Why podpull?

Podcasts disappear. Feeds go offline. Hosting changes. Episodes get pulled. If you have podcasts you truly care about, the only way to guarantee access is to keep your own copy.

podpull makes that easy — point it at a feed, run it periodically (cron job, anyone?), and rest easy knowing your favorite shows are safely backed up.

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
