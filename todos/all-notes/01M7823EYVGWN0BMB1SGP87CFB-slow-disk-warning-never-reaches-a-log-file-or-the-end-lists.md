---
title: "Slow-disk warning never reaches a log file or the end lists"
kind: bug
component: cli
status: needs-discussion
impact: low
origin: review
---
# Slow-disk warning never reaches a log file or the end lists

When writing an episode to disk takes longer than 30 s
(`DISK_WAIT_SLOW_AFTER`, `src/episode/disk_wait.rs`), podpull prints
"Still writing "<title>" to the disk after 30 s; a slow or network file
system is the likely reason, podpull keeps waiting" above the progress
bars. That line is the only trace of the slow write. When stderr is no
terminal (cron, `2>log`, a pipe) or with `-q`, the bars are not drawn,
`IndicatifReporter::warn` in `src/cli/reporter.rs` drops the line, and
the lists at the end of the run do not repeat it. A log of an unattended
run on a slow network share then shows only a run that took long.

Found in the opus review of the 2.1.0 output work (2026-11-12); ADR 0024
states the gap as existing behaviour. Read in the code, not reproduced
on a slow share.

## Impact

Whoever reads the log of an unattended run cannot tell that a slow disk
held it up. The episodes themselves are written correctly.

## Suggested fix

Carry the slow writes in `SyncResult`, filled in the library so that
`-q` (which uses `NoopReporter`) sees them too, and list them at the end
of the run with the other warnings, in the past tense. Amend ADR 0024.

## Decisions

- 2026-11-12, user: fix it and release it as a patch release (2.1.1).

## Open questions

- Does a run whose only problem was a slow write keep exit status 0, or
  exit 2 like the other warnings, and is it listed under "Warnings:" or
  in a list of its own? Asked the user 2026-11-12 (asqr
  01M7822VC5ZRKPNZ064X9FYXZ0).
