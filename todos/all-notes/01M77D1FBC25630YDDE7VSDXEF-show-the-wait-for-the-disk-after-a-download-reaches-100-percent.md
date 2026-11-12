---
title: "Show the wait for the disk after a download reaches 100 percent"
kind: bug
component: sync
impact: high
origin: incident
---
# Show the wait for the disk after a download reaches 100 percent

On 2026-11-12 a sync into a folder on an SMB share stopped after all
three new episodes of the first feed showed 100 % and looked hung for a
long time. podpull was waiting in the `fsync` that has to finish before a
`.partial` file is renamed, and it showed nothing while it waited.

## Problem

Established on 2026-11-12 from the files on disk and measurements on the
share `//zocalo/media` (smbfs, macOS):

- The three `.partial` files had exactly the feed's enclosure lengths, and
  no metadata or renamed audio existed for them. Between the last byte
  and the metadata, `stage_download` (`src/episode/download.rs`, around
  line 211) runs `file.flush()` and then `spawn_blocking(sync_file)`
  (`src/fs_sync.rs`); `download_planned` (`src/sync/downloads.rs`,
  around line 192) writes the metadata and renames only after that.
- On smbfs a write returns at once (100 MiB in 6.5 ms, the client cache);
  `F_FULLFSYNC` answers ENOTSUP and the fallback `fsync` waits until the
  server has the data: 75.5 s for 100 MiB (about 1.4 MB/s), `sample`
  showed the thread in `fsync` throughout. On local APFS the same sync
  took 16 ms.
- A repeated run with the installed 2.0.2 finished: 245 MB in 3 min
  20 s, waits after 100 % of 31 s, 17 s and 1 s. The very long wait of
  the first run was not reproduced; a slower or stalled share then is
  the likely reason, not proven.

While it waits, the progress bar stays at 100 %, nothing says what is
happening, there is no time limit, and whether Ctrl-C ends the run in
that moment is untested.

## Decisions

- 2026-11-12, user: keep the durability guarantee (sync before rename)
  and make the wait visible.

## Proposal

- After the last byte, the episode's progress line says that the file is
  being written to the disk, in place of a silent 100 %.
- When the sync takes longer than a threshold (for example 30 s), say so
  explicitly, naming a slow or network filesystem as the likely reason,
  and keep waiting.
- Ctrl-C during the sync ends the run cleanly and leaves the `.partial`
  file, which the next run handles as today.
- Tests drive the wait with a sync that blocks until released (a gate or
  channel), not with sleeps.
