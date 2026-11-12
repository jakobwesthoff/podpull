# 19. Show the wait for the disk sync

Date: 2026-11-12

## Status

Accepted

Amends [14. Partial download protection](0014-partial-download-protection.md)

Amends [11. Progress reporting trait abstraction](0011-progress-reporting-trait-abstraction.md)

## Context

A file is synced to disk before it is renamed (ADR 17). On a network share the sync takes as long as the server needs to receive the data. Measured on an SMB share mounted on macOS: a write of 100 MiB returns after 6.5 ms because the client caches it, and the `fsync` that follows took 75.5 s, about 1.4 MB/s. In a sync run of three episodes the waits after the last byte were 31 s, 17 s and 1 s. On a local APFS disk the same sync took 16 ms.

During the sync the progress bar stood at 100 % and showed nothing else, which looks like a hang.

## Decision

**The sync stays before the rename.** Its duration is shown instead of being cut.

**The wait is reported as events.** `DownloadSyncing` marks the start of a wait for the disk, `DownloadSyncSlow` the moment it passes a threshold, and `DownloadSynced` its end, whether the sync succeeded or failed. They go through `ProgressReporter` and `NoopReporter` ignores them, so `--quiet` stays quiet.

**Two waits are covered.** The sync of the audio file after its last byte, and the sync of the episode metadata file. Whatever runs between them, such as the check against identical stored audio, shows the plain episode line.

**The threshold is 30 s.** Local syncs finish in milliseconds and the measured waits on a slow share reach 31 s. After the threshold the bar reads "Slow disk, still writing", and one warning line above the bars says that a slow or network file system is the likely reason. podpull keeps waiting without a time limit.

**While waiting, the bar reads "Writing to disk"** in front of the episode title. The bar text is cut at the terminal width, so the title is shortened and the wording is not.

**The wait notice is generic over the waited future.** `with_disk_wait_notice` takes the future and the time after which the wait counts as slow, so the audio sync and the metadata sync share it. `stage_download` takes `SyncSettings`, which hold the sync function and that time, so tests can hold a sync until they release it.

## Consequences

- A wait for the disk of any length is visible on the episode's line.
- A sync that never returns keeps the run waiting.
- Each download reports up to two `DownloadSyncing`/`DownloadSynced` pairs. A download whose audio turns out to be stored already reports one.
- The slow warning is printed once per slow wait, so a run with several concurrent downloads on a slow share prints several warning lines.
