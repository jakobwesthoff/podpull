# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.2.0] - 2026-10-06

### Added

- Before downloading an episode whose usual filename is taken, podpull checks the existing audio file against the hash recorded when it was downloaded and lists a mismatch under "Damaged episodes"
- `--verify` option: checks every downloaded audio file against its recorded hash and lists mismatches; reads the whole archive
- `--repair` option: like `--verify`, and downloads damaged episodes again under their existing filenames, provided they are still in the feed in the same audio format
- Exit code 2 when some downloads failed or damaged audio was found but the run got something done

### Fixed

- Episodes that share a title and publication date no longer overwrite each other or get downloaded again on every sync. One keeps the usual filename; the other gets its publication time added, for example `2024-01-08-093000-Listener Questions.mp3`. If the last sync with podpull 1.1.2 or earlier downloaded both at the same time, the file holds bytes of both; the check above reports it, and `--repair` or deleting the file and its `.json` file downloads it again
- New episodes no longer overwrite existing files whose names differ only in letter case or in Unicode normalization, such as names with umlauts listed by network shares mounted on macOS
- New episodes no longer overwrite audio files that have no readable metadata. If such a file belongs to the episode being downloaded, the episode is stored a second time under a new name
- An episode listed twice under the same GUID is downloaded once
- In feeds without GUIDs, an episode is no longer downloaded again when only an access token in its URL changed and title, publication time and file name stay the same
- If an episode's metadata cannot be written, its freshly downloaded audio is removed again instead of being left without metadata, which made the next sync store the episode a second time
- Quiet mode (`-q`) lists failed and damaged episodes on stderr instead of hiding them

### Changed

- Downloaded audio, episode metadata and `podcast.json` are written to `.partial` files, synced to disk and then renamed into place
- Episode metadata files that hold no valid metadata are reported as a warning with the reason instead of being skipped silently. A metadata file that cannot be read from disk at all stops the sync with an error
- Leftover `.partial` files that cannot be removed are reported, and a download into such a path fails with a message to delete the file
- The status line shows repairs apart from the episode limit
- Library: `create_sync_plan` takes the limit and lists collisions; `PlannedDownload` carries the filenames; `OutputState` is read through methods; `SyncOptions`, `SyncResult`, `ProgressEvent` and the new plan types are `#[non_exhaustive]`; `sync_podcast` accepts clients that are neither `Clone` nor `'static`; `continue_on_error: false` stops starting downloads after the first failure; `generate_filename` is deprecated

## [1.1.2] - 2026-02-01

### Changed

- Filenames now preserve spaces instead of converting them to dashes

## [1.1.1] - 2026-01-31

### Fixed

- HTML/XML entities in feed titles and descriptions are now properly decoded
- Filenames now preserve Unicode characters instead of replacing them with dashes

## [1.1.0] - 2025-01-31

### Changed

- Progress output now shows distinct phases: fetching, parsing, and scanning
- Directory scanning displays a progress bar, improving feedback on network shares

## [1.0.0] - 2025-01-31

### Added

- Initial release
- Download and synchronize podcasts from RSS feeds
- Support for both URL and local file feeds
- Concurrent downloads with configurable limit
- Episode limit option (`--limit`) for incremental downloads
- Atomic downloads with `.partial` file handling
- SHA-256 content hashing for integrity verification
- GUID-based episode deduplication
- Automatic cleanup of interrupted downloads
- Progress bars with episode download status
- Quiet mode for scripted usage
- JSON metadata files for episodes and podcast info
