# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [2.0.0] - 2026-10-06

### Added

- Before downloading an episode whose usual filename is taken, podpull checks the existing audio file against the hash recorded when it was downloaded and lists a mismatch under "Damaged episodes"
- `--verify` option: checks every downloaded audio file against its recorded hash and lists mismatched or missing audio; reads the whole archive
- `--repair` option: like `--verify`, and downloads damaged or missing episodes again under their existing filenames, provided they are still in the feed in the same audio format
- The sync summary lists repaired episodes and episodes whose audio was already stored

### Fixed

- Episodes that share a title and publication date no longer overwrite each other or get downloaded again on every sync. One keeps the usual filename; the other gets its publication time added, for example `2024-01-08-093000-Listener Questions.mp3`. If the last sync with podpull 1.1.2 or earlier downloaded both at the same time, the file holds bytes of both; the check above reports it, and `--repair` or deleting the file and its `.json` file downloads it again
- New episodes no longer overwrite existing files whose names differ only in letter case or in Unicode normalization, such as names with umlauts listed by network shares mounted on macOS
- New episodes no longer overwrite audio files that have no readable metadata. If such a file belongs to the episode being downloaded, the episode is stored a second time under a new name
- An episode listed twice under the same GUID is downloaded once
- An episode re-issued under a new GUID is no longer stored a second time when its old GUID left the feed, title and publication time stayed the same and the audio is byte-identical; the new GUID is added to the stored episode's metadata
- In feeds without GUIDs, an episode is no longer downloaded again when only an access token in its URL changed and title, publication time and file name stay the same
- If an episode's metadata cannot be written, its freshly downloaded audio is removed again instead of being left without metadata, which made the next sync store the episode a second time
- Quiet mode (`-q`) lists failed and damaged episodes and warnings on stderr instead of hiding them
- `-c 0` is rejected with a message instead of crashing
- Quiet mode (`-q`) no longer prints the banner, so a clean quiet run prints nothing

### Changed

- **Exit codes:** a run in which some downloads failed exits with code 2 instead of 0. Code 2 also reports damaged or missing audio and warnings, such as a leftover `.partial` file that cannot be removed or unreadable metadata; such a file keeps every run at code 2 until it is dealt with. Code 1 still means downloads failed and nothing was downloaded, repaired or recorded as already stored
- Downloaded audio, episode metadata and `podcast.json` are written to `.partial` files, synced to disk and then renamed into place. On filesystems without a full flush to the drive, such as SMB shares mounted on macOS, the sync is a plain `fsync`
- Episode metadata files that hold no valid metadata are reported as a warning with the reason instead of being skipped silently. A metadata file that cannot be read from disk at all stops the sync with an error
- Leftover `.partial` files that cannot be removed are reported, and a download into such a path fails with a message to delete the file
- The status line shows repairs apart from the episode limit

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
