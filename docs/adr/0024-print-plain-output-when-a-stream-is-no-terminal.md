# 24. Print plain output when a stream is no terminal

Date: 2026-11-12

## Status

Accepted

Amends [10. CLI output styling with color and emoji indicators](0010-cli-output-styling-with-color-and-emoji-indicators.md)

Amended by [26. List slow writes at the end of the run](0026-list-slow-writes-at-the-end-of-the-run.md)

## Context

podpull printed its banner and its emoji whether stdout was a terminal, a pipe or a log file, and decided colour by stdout alone, also for text that goes to stderr. A log file held `🎙️  podpull - Podcast Downloader` and a line of emoji in front of every list.

## Decision

**A stream is rich when it is a terminal.** stdout and stderr are decided separately, and each decision is made once, in `StreamEnv`, together with colour and symbols.

**A stream that is no terminal gets the same structure without banner, symbols and blank lines around it.** On a pipe the summary line, the lists of failed and damaged episodes, the warnings and the output folder keep their layout and their order and stay on stdout. The banner is not written, no line starts with a symbol, and the output neither starts nor ends with a blank line. A terminal is unchanged. `-q` still prints only the problem lists, on stderr, and decides symbols and colour by stderr.

**The progress bars and the warnings above them are drawn only when stderr is a terminal.** indicatif draws to stderr. The lists at the end repeat the warnings about unreadable metadata, leftover partial files, unreadable audio and damaged audio, so a log file holds those once. The warning about a slow disk is printed only while the bars are drawn and is not in a log file.

**Colour follows `console`:** `NO_COLOR`, `CLICOLOR`, `CLICOLOR_FORCE` and a terminal that supports colour. `CLICOLOR_FORCE` colours a stream that is no terminal, also when `NO_COLOR` is set, and does not make it rich: banner and symbols stay off.

**Symbols are shown when `console` says its terminal shows them or when the locale names UTF-8.** The first of `LC_ALL`, `LC_CTYPE` and `LANG` that is set and not empty decides, and a value that holds `utf-8` or `utf8` in any case names UTF-8. Without symbols a terminal gets ASCII stand-ins.

**The width of a progress line is that of stderr**, where the bars are drawn.

**A reader that closes stdout early is no error of the run.** Writing to a closed pipe ends quietly and the exit status is that of the sync.

## Consequences

On Linux a terminal whose locale is spelled `en_US.utf8`, or is set through `LC_ALL`, shows the symbols; `console` alone showed them only for a `LANG` that ends in `UTF-8`.
