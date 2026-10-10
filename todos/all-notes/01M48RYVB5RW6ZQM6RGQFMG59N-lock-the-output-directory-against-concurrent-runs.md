---
title: "Lock the output directory against concurrent runs"
kind: bug
component: sync
status: needs-discussion
impact: high
tags: [concurrency]
---
# Lock the output directory against concurrent runs

Two podpull runs on the same output directory at once, for example
overlapping cron jobs, can corrupt a download:

- `scan_output_dir` removes every `.partial` file it finds, including the
  ones the other run is still writing.
- The exclusive creation of `.partial` files only catches conflicts while
  the file exists, so it does not help once the other run's scan removed it.
- Both runs plan the same names, because neither run's new files exist yet
  when the other one scans.

The behaviour is the same on `main` before the filename collision work.
For now README ("Limitations") and ADR 0017 state that concurrent runs on
one directory are unsupported.

## Options discussed

- Lock file created exclusively (`create_new`), e.g. `.podpull.lock`
  holding PID and start time. Works on SMB shares, but a killed run leaves
  it behind, so it needs a staleness rule (PID no longer running on the
  same host, or age) or manual removal.
- Advisory locks (`flock`/`fcntl`, e.g. via the `fs4` or `fd-lock`
  crates). Unverified on macOS smbfs mounts, which the user's archive uses.

Whichever is chosen: take the lock before `scan_output_dir` and fail fast
with a clear message naming the lock.
