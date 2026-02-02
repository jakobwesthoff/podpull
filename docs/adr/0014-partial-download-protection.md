# 14. Partial download protection

Date: 2026-01-31

## Status

Accepted

## Context

Interrupted downloads (network failures, user cancellation, power loss) leave corrupt partial files in the output directory. These partial files have the same names as complete downloads, making it impossible to distinguish them from fully downloaded episodes. On subsequent sync operations, these corrupt files are not detected, leaving users with incomplete audio files.

The stateless design of podpull (ADR implied by the project's architecture) means the output directory is the single source of truth. Any file present is assumed to be complete.

## Decision

We implement a **partial file suffix pattern** with **atomic rename on completion**:

1. **Download to `.partial` suffix**: Episodes are downloaded to `{filename}.partial` instead of the final filename
2. **Atomic rename on completion**: After successful download and flush, `tokio::fs::rename()` atomically moves the file to its final name
3. **Cleanup on startup**: During `scan_output_dir()`, any `.partial` files are silently deleted as they represent interrupted downloads

```rust
// Download phase
let partial_path = PathBuf::from(format!("{}.partial", output_path.display()));
let mut file = File::create(&partial_path).await?;
// ... streaming download ...
file.flush().await?;
tokio::fs::rename(&partial_path, output_path).await?;
```

**Progress events added:**
- `Finalizing { download_id, episode_title }` - Reported before atomic rename
- `PartialFilesCleanedUp { count }` - Reported if cleanup occurred during scan

**Error handling:**
- New `DownloadError::RenameFailed` variant for rename failures
- Partial files from failed renames are cleaned up on next sync

## Consequences

**Benefits:**

* **No corrupt files**: Interrupted downloads never appear as complete episodes
* **Automatic recovery**: Failed downloads are automatically retried on next sync
* **Zero user intervention**: No manual cleanup required
* **Filesystem atomicity**: Rename operations are atomic on most filesystems

**Trade-offs:**

* Slight code complexity increase in download path
* Extra filesystem operation (rename) per download
* Temporary disk space usage during download (file exists twice briefly during rename)

**Alternatives considered:**

* **Checksum validation**: Would require storing expected checksums, adds complexity for feeds that don't provide them
* **Content-length validation**: Not all servers provide accurate content-length headers
* **Separate tracking database**: Violates stateless design principle
