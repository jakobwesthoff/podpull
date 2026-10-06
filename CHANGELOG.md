# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- When a new episode would take the name of an existing file, that file is checked against the hash recorded when it was downloaded. A mismatch is listed among the failed episodes. podpull 1.1.2 and earlier downloaded episodes sharing title and date into one file at the same time, which leaves such a mismatch; delete the reported audio file and its `.json` file to download the episode again

### Fixed

- Episodes that share a title and publication date no longer overwrite each other or get downloaded again on every sync. One keeps the usual filename; the other gets its publication time added, for example `2024-01-08-093000-Listener Questions.mp3`
- New episodes no longer overwrite existing files whose names differ only in letter case or in Unicode normalization, such as names with umlauts listed by network shares mounted on macOS
- New episodes no longer overwrite audio files that have no readable metadata. If such a file belongs to the episode being downloaded, the episode is stored a second time under a new name

### Changed

- Episode metadata files that cannot be read are now reported as a warning instead of being skipped silently

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
