# 20. Sync a download in bounded steps

Date: 2026-11-12

## Status

Accepted

Amends [19. Show the wait for the disk sync](0019-show-the-wait-for-the-disk-sync.md)

## Context

Until now the audio was synced once, after the last byte. On a network share the writes only reach the client cache (100 MiB in 6.5 ms), so the progress bar reached 100 % at network speed and then stood there while the single `fsync` covered the whole episode.

podpull installs no signal handler, so Ctrl-C takes the default action. Measured on an SMB share mounted on macOS: a process inside the `fsync` of a 100 MiB file stayed in the uninterruptible state after SIGINT, and ended only when the `fsync` returned, 60 s after the signal and about 76 s after it started. A signal handler does not shorten that wait.

## Decision

**The partial file is synced every 4 MiB while it is written.** Each sync covers the bytes written since the previous one. At the 1.2 to 1.4 MB/s measured on the share, a step takes about three seconds, which bounds how long a sync can block the process.

**A step sync runs while the download goes on.** The download keeps reading from the network while the sync runs on a blocking thread, so the connection is not left unread while a share is slow. Only one sync runs at a time: before the next step starts, and before the final sync, the previous one is waited for. A sync that failed fails the download at that point.

**The progress bar follows what reaches the server.** The download reads on during a step sync and stops for the previous one when the next step is due, so the bar runs ahead of the file server by about a step and not by an episode.

**The final sync stays.** After the last byte, the remainder is synced as before, and the rename and the metadata still come only after it (ADR 17). The durability guarantee is unchanged. The types make this the only way to a staged download: a writer is flushed, the flushed file is synced, and only the synced file can be staged.

**A wait during the download is reported only when it turns slow** (ADR 19). The wait for a step sync that is still running when the next one is due is over in moments on a healthy file system, so the status line does not change at every step. Once it passes the threshold, the line shows the slow status and the warning is printed, once per download; `DownloadSynced` follows when the wait ends. The final sync is announced as before.

**No signal handler is added.** The default action ends the process when the running sync returns.

**`SyncSettings` carry the sync function, the step size and the time after which a sync counts as slow.** Tests choose a step of 25 bytes and a sync that records what it saw or waits for a gate.

## Consequences

- A download makes at most its size divided by 4 MiB, rounded down, plus one calls to `fsync`.
- Data written during a step sync belongs to the next step.
- Ctrl-C ends the run when the running syncs return. Each running download has at most one step sync running, so the wait is the time the share needs for about 4 MiB per running download, and at most `--concurrent` downloads run. `-c 1` gives the shortest wait.
- The cost of the additional syncs on a local disk is not measured.
- Whether a write waits for a running sync on SMB shares is not measured; if it does, the download stops reading for about the time of a step sync.
- While the file system stalls, the download stops reading at the next step. A server that closes a connection it cannot send on fails the download with a stream error, and the next run downloads the episode again. Before, the body was read in full before the sync. This is a risk on the server side; it was not observed.
