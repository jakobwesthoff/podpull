---
title: "Cut long episode titles at a character boundary in the progress bar"
kind: bug
component: cli
impact: high
origin: implementation
---
# Cut long episode titles at a character boundary in the progress bar

Found on 2026-11-12 while adding the disk-wait texts to the progress
bar. `truncate_title` in `src/main.rs` (around line 497) shortens a title
that is longer than the bar's width by slicing its bytes:
`&title[..max_len.saturating_sub(3)]`. When the cut falls inside a
multi-byte character, such as an umlaut or an en dash, slicing a `str`
there panics and the run ends. German podcast titles hold such
characters often.

## Problem

Read in the code (2026-11-12), not reproduced. `title.len()` counts
bytes, so the width check is also in bytes, while the terminal shows
characters.

## Suggested fix

Cut at a character boundary and measure in display width (the
`console` crate already in use offers `truncate_str` and `measure_text_width`).
A test with a title whose cut point falls inside `ä` or `–`.
