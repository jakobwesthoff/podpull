# 25. Show errors that end the run as a block or as plain lines

Date: 2026-11-12

## Status

Accepted

Amends [7. Error handling strategy](0007-error-handling-strategy.md)

## Context

An error that ended the run was printed by the runtime as `Error: Failed to sync podcast`, the chain of its causes (`Caused by:`, seven links for a host that does not resolve, most of them repeating the one before) and, with `RUST_BACKTRACE` set, a stack backtrace. The chain was the text of every wrapper, not a statement of what went wrong or what to do.

## Decision

**The message of a library error that can end the run is what a person is told first.** It holds a host or a status and fixed words, never a name from the feed or the disk. The host of a failed fetch is the one that failed, so after a redirect it is the host that answered, and the step keeps the URL that was asked for: names are shown by the step the run was in (ADR 22). Where the title depends on the cause, a pure function of the kind of the cause computes it in the `#[error]` attribute (`fetch_title`, `feed_file_title`, `folder_title`), so there is one variant per failing operation and not one per kind.

**The fix is a `Hint` of the error.** `Hint::hint` returns a fixed sentence or nothing. `FeedError`, `StateError` and `MetadataError` implement it by the kind of the cause, next to the title function of the same kind, and `SyncError` forwards. The wrappers `SyncError::{Feed, State, Metadata}` and `StateError::Metadata` are transparent: they show the message and the hint of what they wrap. No error carries a code, and no dependency was added for any of this.

**The binary holds a `Failure`:** the failed sync, or a failed write to stdout or stderr. It replaces `anyhow`. The cause is the root text of the chain (ADR 23), left out when the error has no cause.

**The step is decided in the binary.** An error does not know the feed argument or the output folder. `step_of` matches every failure and names what podpull was doing: fetching the feed, reading the feed file, parsing the feed, reading the feed URL, creating, scanning the output folder, reading the episode metadata, writing the podcast metadata, writing its output. The podcast metadata is named by its final file, never the partial file it is written through.

**The report goes to stderr and the exit status is 1.** On a terminal it is a block: a badge, a bar down the left side, the title in bold, and the sections "Cause", "How to fix it" and "What podpull was doing". On a pipe it is the line `Error: <title>`, a line `  cause: <root>` and a line `  while <step>`; the pipe form has no line for the fix. No chain and no backtrace are printed, also with `RUST_BACKTRACE` set. Title, cause, fix and step pass through `shown` (ADR 22).

## Consequences

ADR 7 held that the library says what went wrong and the binary says what to tell the user. The message and the hint of an error that can end the run are now part of the library error, and the binary adds the cause, the step and the layout.
