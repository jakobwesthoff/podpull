# 22. Show names from the feed and the disk in one escaped form

Date: 2026-11-12

## Status

Accepted

## Context

A feed is foreign input. A title, a file name or a path can hold a character that flips the rest of a line (U+202E), a newline that starts a forged line or an escape sequence that moves the cursor. Printed as they are, such names reach the terminal and every log file.

## Decision

**Every name from the feed or the disk shows in one escaped form.** Each Unicode control character (category Cc), each bidi control U+202A to U+202E and U+2066 to U+2069, and U+2028 and U+2029 shows as `%` and the upper-case hex digits of each of its UTF-8 bytes. A `%` shows as `%25`, and a byte of a path that is not UTF-8 shows as `%XX`. The shown form of a name therefore stands for exactly one name. The rule is the same for episode titles, podcast titles, file names and paths: a name with a `%` in it shows as `%25`, whatever it names. Only the display changes: the file names podpull writes are as before.

**Text that is no name keeps its `%`.** The words of a system error and a feed argument that is no URL show the same characters escaped, but a `%` stays.

**A URL shows as it parses.** A `url::Url` holds none of the escaped characters and keeps its percent-encoding, so an enclosure URL with `%20` is shown with `%20`.

**The type system carries the rule.** `Safe`, in `src/cli/shown.rs`, is the text a message builder takes. Only `shown::name`, `shown::path`, `shown::text` and `shown::url` make one, so a value that was not escaped does not compile into a line. Escaping happens before colour is added and before a title is cut to the width of the terminal.

**`Path::display` is forbidden by clippy** (`clippy.toml`), so a path reaches a line only through `shown::path`.

## Consequences

A title that holds a `%`, such as `100% Retro`, shows as `100%25 Retro`.
