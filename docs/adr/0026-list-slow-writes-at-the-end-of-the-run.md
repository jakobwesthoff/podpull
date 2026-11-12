# 26. List slow writes at the end of the run

Date: 2026-11-12

## Status

Accepted

Amends [24. Print plain output when a stream is no terminal](0024-print-plain-output-when-a-stream-is-no-terminal.md)

## Context

When writing an episode to disk takes longer than 30 s, podpull prints a warning above the progress bars (ADR 19). The bars and that line are drawn only when stderr is a terminal (ADR 24), and `-q` draws nothing. The lists at the end of the run repeated every other warning but not this one, so the log of an unattended run on a slow network share showed a run that took long and no reason.

## Decision

**The end of the run lists every episode whose write to disk turned slow, under `Slow writes:`**, after the warnings and before the output folder, on a terminal, on a pipe and with `-q`. The line is in the past tense: `Writing "Sega Nomad" to the disk took longer than 30 s; a slow or network file system is the likely reason`. An episode is listed once however many of its waits turned slow, in the order of the download queue.

**A slow write does not change the exit status.** The episode was written and nothing needs fixing, so it is a list of its own rather than a warning, and a run whose only finding is a slow write exits 0.

**The binary collects the slow writes from the progress events.** The library reports a slow wait as `ProgressEvent::DownloadSyncSlow`. `SlowWriteRecorder` (`src/cli/slow_writes.rs`) wraps the reporter of the run, the progress bars or, with `-q`, the reporter that draws nothing, passes every event on and keeps the slow writes. `SyncResult` stays as it is.

## Consequences

A log file and the output of `-q` say which episodes a slow disk held up. On a terminal a slow write is shown twice, live above the bars and again at the end, like the warnings.

A library user who wants the slow writes collects them from `DownloadSyncSlow` as the binary does.
