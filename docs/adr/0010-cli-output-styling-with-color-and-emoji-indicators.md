# 10. CLI output styling with color and emoji indicators

Date: 2026-01-31

## Status

Accepted

Amended by [24. Print plain output when a stream is no terminal](0024-print-plain-output-when-a-stream-is-no-terminal.md)

## Context

Command-line tools need to communicate status and progress effectively to users. Traditional CLI output can be dense and hard to scan quickly. Modern terminals support ANSI color codes and Unicode characters, enabling richer visual feedback.

We need to balance:
- Readability and quick visual scanning
- Professional appearance
- Accessibility (not relying solely on color)
- Clean, uncluttered output

## Decision

We adopt a structured, modern CLI output style using:

1. **Unicode emoji indicators** for quick visual recognition:
   - 🎙️ Application branding
   - 🔍 Fetching/searching
   - 🎧 Podcast information
   - 📥 Downloads in progress
   - ✅ Success
   - ❌ Failure
   - 🎉 Completion
   - 📁 File/directory information

2. **ANSI colors** for semantic highlighting:
   - Green: success, positive counts, podcast title
   - Cyan: URLs, file paths, counts
   - Yellow: skipped items, warnings
   - Red: errors, failures
   - Magenta: branding
   - Dimmed: secondary information

3. **Libraries**:
   - `console` crate for `Emoji` type with graceful fallback on non-Unicode terminals
   - `colored` crate for ANSI color support
   - `indicatif` for progress bars with Unicode block characters (█▓░)

4. **Graceful degradation**: Using `console::Emoji` which provides ASCII fallbacks:
   ```rust
   static SUCCESS: Emoji<'_, '_> = Emoji("✅ ", "[+] ");
   ```
   On terminals without Unicode support, `[+]` is displayed instead.

5. **Quiet mode** (`-q` flag) suppresses all visual output for scripting.

## Consequences

**Benefits:**
- Faster visual scanning of output
- Clear distinction between success and failure states
- Modern, polished user experience
- Emoji provide redundant cues (not color-dependent)

**Trade-offs:**
- Colors may not render in all environments (handled gracefully by `colored`)
- Slightly larger binary due to additional dependencies

**Mitigations:**
- `console::Emoji` provides ASCII fallbacks for terminals without Unicode
- `colored` respects `NO_COLOR` environment variable
- Terminal detection is handled automatically
- Quiet mode available for CI/scripting environments
