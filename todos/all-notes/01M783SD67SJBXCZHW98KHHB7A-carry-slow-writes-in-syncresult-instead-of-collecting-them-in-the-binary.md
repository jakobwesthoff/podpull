---
title: "Carry slow writes in SyncResult instead of collecting them in the binary"
kind: refactor
component: sync
origin: discussion
---
# Carry slow writes in SyncResult instead of collecting them in the binary

Since 2.1.1 the end of a run lists the episodes whose write to disk took
longer than 30 s under `Slow writes:` (ADR 0026). The binary collects
them itself: `SlowWriteRecorder` in `src/cli/slow_writes.rs` wraps the
run's reporter and picks `ProgressEvent::DownloadSyncSlow` out of the
event stream. Everything else the end lists show comes from
`SyncResult`. Move the slow writes into the library, so the result of a
sync is the one record of what happened in it. Output and exit status
stay as they are.

## Problem

- `SyncResult` records every other finding of a run (failed episodes,
  damaged audio, stuck `.partial` files, unreadable metadata and audio).
  A slow write is the same kind of fact, yet the end lists now read from
  two sources.
- Progress events exist to show progress. The summary depends on what
  `DownloadSyncSlow` happens to carry (`episode_index` to list each
  download once, the title, the threshold); a change to the event made
  for the display would break the summary without a failing type check.
- Every other front end of the library has to rebuild the recorder to
  learn what the library already knows, and ADR 0026 tells library users
  to do so.
- Inside `download_all` (`src/sync/downloads.rs`) a slow wait belongs to
  one known download; the binary has to reconstruct that from loose
  events.

The binary was chosen for 2.1.1 only because a new public field on
`SyncResult` breaks the library's API, which a patch release must not
do.

## Suggested change

- Add `pub slow_writes: Vec<SlowWrite>` to `SyncResult`, with a public
  `SlowWrite { episode_title, threshold }` (the shape of
  `src/cli/slow_writes.rs`), one entry per download in the order of the
  download queue, failed downloads included, as today.
- Fill it in the sync code next to `failed_episodes`. The direct way:
  `with_disk_wait_notice` (`src/episode/disk_wait.rs`) marks the
  download as slow in its `DownloadContext` (for example an
  `AtomicBool`), and `download_all` turns the mark and the title into a
  `SlowWrite` when the download ends. Moving the recorder into
  `download_all` would also work but keeps the summary tied to the
  progress event.
- Mark `SyncResult` `#[non_exhaustive]` in the same release, so the next
  field added to it breaks no library user.
- Delete `SlowWriteRecorder`; `main.rs` passes `result.slow_writes` to
  the end lists, whose signatures drop the extra parameter.
- A new ADR that amends ADR 0026 (the slow writes are part of the sync
  result); ADR 0026 gains its "Amended by" link.
- Changelog: the library change under its own entry; nothing changes for
  users of the command.

## Tests

The recorder's tests (one entry per download, queue order, title and
threshold) move to the library. The 30 s threshold cannot be reached in
a test: `stage_download` already takes `SyncSettings::slow_after`, so a
test of `download_planned` or `download_all` with the threshold set to
zero and a gated sync, as in `src/episode/download.rs`, proves the mark
reaches the result. The end-list tests in `src/cli/messages.rs` stay.

## Decisions

- 2026-11-12, user: do it in a later release, not in 2.1.1. The library
  is the right place from a design standpoint; 2.1.1 kept the library
  API unchanged. The change breaks the library API, so its release
  number has to allow that.
