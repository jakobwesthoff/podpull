# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.1.1] - 2026-01-31

### Fixed

- HTML/XML entities in feed titles and descriptions are now properly decoded

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
