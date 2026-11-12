---
title: "Scanning bar stays on the terminal after a warning during the scan"
kind: bug
component: cli
impact: low
origin: implementation
---
# Scanning bar stays on the terminal after a warning during the scan

When a warning is printed while podpull scans the output folder, the
scanning progress bar stays on the screen after the run ends, as a stale
line `Scanning existing episodes... [░░░…] 0/1` between the warnings and
the `Sync complete:` line.

## Problem

Seen on 2026-11-12 with podpull 2.0.3 on a terminal (tmux, 110 columns):
an output folder held a broken `Old Episode.json`, so a
`MetadataUnreadable` warning went out through `MultiProgress::println`
during the scan (`ProgressEvent::ScanningDirectory`, total 1). The final
screen kept the scanning bar at 0/1, although `SyncPlanReady` restyles
the main bar and `SyncCompleted` calls `finish_and_clear`. The cause is
not investigated yet; the leading guess is that printing above the bars
mid-scan leaves a drawn copy of the bar behind.

Reproduce: serve a feed with one episode on 127.0.0.1, put a file
`Old Episode.json` containing `{ not json` into an empty output folder,
run `podpull <feed url> <folder>` on a terminal and look at the last
screen. The test feed and the capture are in the logbook of the
2026-11-11 session, `records/podpull-errors/feed.xml` and `today.ansi`.

## Impact

Cosmetic: a misleading progress line remains after every run that warns
during the scan.

## Suggested fix

Find out why the bar survives (a test with a hidden or in-memory draw
target that records what stays drawn), then clear or finish the
scanning bar before printing, or print warnings in a way that does not
leave it behind.
