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
| `-c, --concurrent <N>` | 3 | Maximum concurrent downloads (at least 1) |
| `-l, --limit <N>` | — | Only download the N most recent undownloaded episodes |
| `-q, --quiet` | — | Suppress progress output |
| `--verify` | — | Check every downloaded audio file against its recorded hash and report damaged or missing ones (reads the whole archive) |
| `--repair` | — | Like `--verify`, and download damaged or missing episodes still in the feed again under their existing filenames |
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
├── 2024-01-08-another-episode.json
├── 2024-01-08-093000-another-episode.mp3   # Second episode with the same title and date
└── 2024-01-08-093000-another-episode.json
```

Episodes that share a title and publication date cannot share a filename. One keeps the usual name, the other gets its publication time added (`HHMMSS`). If that name is taken as well, or the episode has no publication date, a short hash of its GUID is appended instead. New downloads avoid the names of all existing files, so an earlier download is not overwritten. Names that differ only in letter case or Unicode normalization count as the same name: APFS treats them as one file, and network shares mounted on macOS can list a name in a different Unicode normalization than it was written in.

No database. No config files. No hidden state. podpull looks at what's already in the output directory and only downloads what's missing. Want to re-download an episode? Delete its files. Want to start fresh? Delete the directory. Want to know what you have? Just look.

### Metadata Format

Feed-level metadata in `podcast.json`:

```json
{
  "title": "My Favorite Podcast",
  "description": "A podcast about interesting things",
  "link": "https://example.com/podcast",
  "author": "Podcast Author",
  "image_url": "https://example.com/podcast/cover.jpg",
  "feed_url": "https://example.com/podcast/feed.xml",
  "updated_at": "2024-01-15T10:30:00.123456+00:00"
}
```

Episode metadata alongside each audio file:

```json
{
  "title": "Episode Title",
  "description": "What this episode is about",
  "pub_date": "2024-01-15T08:00:00+00:00",
  "guid": "episode-unique-id-123",
  "original_url": "https://example.com/episode.mp3",
  "downloaded_at": "2024-01-15T10:30:00.234567+00:00",
  "duration": "45:12",
  "episode_number": 42,
  "season_number": 3,
  "audio_filename": "2024-01-15-Episode Title.mp3",
  "content_hash": "sha256:9f86d0..."
}
```

Fields the feed does not provide are left out. An episode that the feed re-issued under a new GUID with unchanged audio records that GUID in an `additional_guids` list. The `content_hash` is a SHA-256 hash of the downloaded audio; `--verify` and `--repair` check the files against it.

### How It Works

podpull follows a 5-phase sync process:

| Phase | What Happens |
|-------|-------------|
| **1. Fetching** | Downloads the RSS feed from the URL (or reads a local file) |
| **2. Parsing** | Extracts podcast metadata and episode list from the feed |
| **3. Scanning** | Reads existing episode metadata from the output directory to determine what's already downloaded |
| **4. Checking** | Checks stored audio against its recorded hash: the files new downloads would collide with, or every file with `--verify` and `--repair` |
| **5. Downloading** | Downloads new episodes and repairs in parallel, showing progress for each |

The scanning phase displays a progress bar when processing many existing episodes — this is especially helpful on network shares where metadata reads can be slow.

### Smart Sync: How Episodes Are Tracked

podpull identifies episodes using their **GUID** (a unique identifier from the RSS feed). This means:

- Episodes are matched by GUID, not filename or URL
- Moving or renaming files in the output directory won't cause re-downloads (the JSON metadata contains the GUID)
- If a feed lacks GUIDs (rare), podpull falls back to using the episode URL as an identifier

> [!NOTE]
> **When Re-downloads Might Happen**
>
> If a podcast host changes their feed URL structure without preserving GUIDs, episodes may be re-downloaded. This is uncommon but can happen during podcast platform migrations. If the new entry took the place of a stored episode (that episode's GUID is no longer in the feed, and title and publication time are unchanged) and its audio is byte for byte identical, podpull keeps no copy: it adds the new GUID to the stored episode's metadata (`additional_guids`). Entries listed in the feed side by side are separate episodes, even with identical audio. Otherwise the earlier files are kept, and a re-downloaded episode whose title and date did not change is stored next to them with its publication time in the filename.
>
> A feed without GUIDs is an exception for one common case: private feeds that put an access token into their audio URLs. When only the token changed, an episode with the same title, the same publication time to the second, and the same file name at the end of its URL counts as already downloaded.

### Safe Downloads

podpull uses atomic downloads to ensure file integrity:

- Episodes download to a temporary `.partial` file first
- A SHA-256 hash is computed during download and stored in the metadata
- Only when the download completes successfully is the file renamed to its final name
- Episode metadata is written to its own `.partial` file before the audio is renamed, then renamed right after it; `podcast.json` goes through a `.partial` file as well
- Both are synced to disk before they are renamed. Where the filesystem does not support a full flush to the drive, such as SMB shares mounted on macOS, podpull falls back to a plain `fsync`, which hands the data to the file server
- The renames themselves are not synced. After a power loss the metadata can be in place while its audio is not; `--verify` lists that audio as missing and `--repair` downloads it again
- If the metadata cannot be written, the audio of a new download is removed again, so no audio is left without metadata
- If a download is interrupted, the `.partial` files are automatically cleaned up on the next sync; a `.partial` file that cannot be removed is reported, and its episode cannot be downloaded until it is deleted

This means you'll never have corrupted files from interrupted downloads, and you can safely run podpull repeatedly.

### Error Handling

When individual episodes fail to download (network errors, 404s, etc.), podpull continues with the remaining episodes. At the end, failed episodes are listed:

```bash
🎉 Sync complete: 47 downloaded, 120 existing, 3 failed

Failed episodes:
  ✗ Episode 23 - HTTP error 503 for https://example.com/episode-23.mp3
  ✗ Episode 38 - HTTP error 404 for https://example.com/episode-38.mp3
  ✗ Episode 41 - HTTP error 404 for https://example.com/episode-41.mp3
```

Use `-q` (quiet mode) to suppress progress output. Failed and damaged episodes and warnings are still listed, on stderr.

**Damaged audio.** When a new episode would take the name of an existing file, podpull first checks that file against the `content_hash` in its metadata. If the last sync with podpull 1.1.2 or earlier downloaded two episodes sharing title and date at the same time, that file holds bytes of both and fails this check. `--verify` runs the same check on every audio file in the directory, which reads the whole archive. A mismatch is listed under "Damaged episodes" and the file is left untouched, because audio tags edited after the download cause a mismatch as well. `--repair` downloads a damaged episode again under its existing filename, provided it is still in the feed in the same audio format; otherwise the list says why it cannot be repaired. A checked episode whose audio file the directory no longer lists is listed as missing and repaired the same way. Deleting the audio file and its `.json` file also makes the next sync download the episode again.

**Unreadable metadata.** An episode metadata file whose content is not valid metadata is reported as a warning with the reason. Its name stays reserved, so if the episode it belonged to is still in the feed, it is downloaded again under a new name. If a metadata file cannot be read from disk at all, for example because a network share dropped the connection, the sync stops with an error instead, and the next run tries again.

### Exit Codes

podpull returns meaningful exit codes for scripting:

| Exit Code | Meaning |
|-----------|---------|
| `0` | Success (episodes downloaded or already up to date, nothing to report) |
| `1` | Failure (downloads failed and no episode was downloaded, repaired or recorded as already stored) |
| `2` | Problems found (some downloads failed, damaged or missing audio was found, or a warning was reported); code 1 takes precedence when downloads failed and none succeeded |

Warnings are leftover `.partial` files that cannot be removed, unreadable episode metadata and audio that cannot be read for checking. A leftover `.partial` file or unreadable metadata stays until you deal with it, so every run exits with code 2 until then.

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
# In crontab - sync daily, log the output, mail it when podpull exits with 1 or 2
0 3 * * * out=$(podpull -q https://example.com/feed.xml ~/Podcasts/show/ 2>&1); status=$?; echo "$out" | logger -t podpull; [ $status -eq 0 ] || echo "$out" | mail -s "podpull exit $status" you@example.com
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

**Episodes without GUIDs:** Some RSS feeds don't include GUIDs for episodes. In this case, podpull uses the episode's download URL as a fallback identifier. This works fine unless the podcast host changes URLs (CDN migrations, hosting changes, etc.) — then those episodes will be re-downloaded since they appear as "new" episodes with different identifiers. A changed access token in the URL is the exception described under "When Re-downloads Might Happen".

**One sync per directory at a time:** Two podpull runs on the same output directory at once, for example overlapping cron jobs, are not supported. Each run's scan removes the other run's `.partial` files, which can leave a broken download behind.

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
