// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The block an error that ends the run is shown as: on a terminal a badge,
//! a bar down the left side and labelled sections, on a pipe plain lines

use super::shown::Safe;
use super::stream_env::StreamEnv;

/// What a person is told when the run ends in an error
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Block {
    /// What went wrong
    pub(crate) title: Safe,
    /// The root cause, when the error has one
    pub(crate) cause: Option<Safe>,
    /// How to fix it, when there is something to say
    pub(crate) fix: Option<Safe>,
    /// What podpull was doing, as the rest of the sentence "Fetching ..."
    /// without capital and full stop
    pub(crate) doing: Safe,
}

/// Prose is never wrapped wider than this, however wide the terminal is
const MAX_WIDTH: usize = 100;

/// Below this width wrapping is off: a narrow column of fragments reads
/// worse than lines the terminal wraps itself
const MIN_WRAP_WIDTH: usize = 40;

/// The lines of `block` for a stream: the block on a terminal, plain lines
/// on a pipe
pub(crate) fn lines(block: &Block, env: &StreamEnv, columns: usize) -> Vec<String> {
    if env.rich {
        block_lines(block, env, columns)
    } else {
        plain_lines(block)
    }
}

/// What a script can search for: the error, its cause and the step
fn plain_lines(block: &Block) -> Vec<String> {
    let mut lines = vec![format!("Error: {}", block.title)];
    if let Some(cause) = &block.cause {
        lines.push(format!("  cause: {cause}"));
    }
    lines.push(format!("  while {}", block.doing));
    lines
}

/// How many columns a line of prose may have after `indent` columns
fn available(columns: usize, indent: usize) -> usize {
    if columns < MIN_WRAP_WIDTH {
        usize::MAX
    } else {
        columns.min(MAX_WIDTH).saturating_sub(indent)
    }
}

/// `text` filled into lines of at most `width` columns; a word wider than
/// the line stands alone on one
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut used = 0;
    for word in text.split_whitespace() {
        let columns = console::measure_text_width(word);
        match lines.last_mut() {
            Some(line) if used + 1 + columns <= width => {
                line.push(' ');
                line.push_str(word);
                used += 1 + columns;
            }
            _ => {
                lines.push(word.to_string());
                used = columns;
            }
        }
    }
    lines
}

/// `phrase` with a capital letter and a full stop
fn sentence(phrase: &str) -> String {
    let mut characters = phrase.chars();
    match characters.next() {
        Some(first) => format!("{}{}.", first.to_uppercase(), characters.as_str()),
        None => String::new(),
    }
}

fn block_lines(block: &Block, env: &StreamEnv, columns: usize) -> Vec<String> {
    let (bar_symbol, error_symbol) = if env.utf8 { ("▌", "✖") } else { ("|", "x") };
    let bar = env.red_bold(bar_symbol);
    let line = |rest: &str| {
        if rest.is_empty() {
            bar.clone()
        } else {
            format!("{bar} {rest}")
        }
    };

    let badge = format!(" {error_symbol} ERROR ");
    let mut lines = vec![if env.color {
        env.error_badge(&badge)
    } else {
        line(badge.trim())
    }];
    for title in wrap(block.title.as_str(), available(columns, 2)) {
        lines.push(line(&env.bold(title)));
    }

    let mut section = |heading: &str, text: &str| {
        lines.push(line(""));
        lines.push(line(&env.red(heading)));
        for paragraph in wrap(text, available(columns, 4)) {
            lines.push(line(&format!("  {paragraph}")));
        }
    };
    if let Some(cause) = &block.cause {
        section("Cause", cause.as_str());
    }
    if let Some(fix) = &block.fix {
        section("How to fix it", fix.as_str());
    }
    section("What podpull was doing", &sentence(block.doing.as_str()));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::shown;

    fn env(color: bool, utf8: bool, rich: bool) -> StreamEnv {
        StreamEnv { color, utf8, rich }
    }

    fn block(cause: Option<&str>, fix: Option<&str>) -> Block {
        Block {
            title: shown::text("Cannot reach the feed host feeds.example.com"),
            cause: cause.map(shown::text),
            fix: fix.map(shown::text),
            doing: shown::text("fetching the feed https://feeds.example.com/podcast.xml"),
        }
    }

    #[test]
    fn a_pipe_gets_the_error_line_the_cause_and_the_step() {
        let lines = lines(
            &block(Some("dns error"), Some("Check the feed URL.")),
            &env(false, true, false),
            80,
        );

        assert_eq!(
            lines,
            vec![
                "Error: Cannot reach the feed host feeds.example.com",
                "  cause: dns error",
                "  while fetching the feed https://feeds.example.com/podcast.xml",
            ]
        );
    }

    #[test]
    fn a_pipe_gets_no_cause_line_for_an_error_without_a_cause() {
        let lines = lines(&block(None, None), &env(false, true, false), 80);

        assert_eq!(
            lines,
            vec![
                "Error: Cannot reach the feed host feeds.example.com",
                "  while fetching the feed https://feeds.example.com/podcast.xml",
            ]
        );
    }

    #[test]
    fn a_terminal_gets_the_block_with_unicode_and_no_colour() {
        let lines = lines(
            &block(Some("dns error"), Some("Check the feed URL.")),
            &env(false, true, true),
            80,
        );

        assert_eq!(
            lines,
            vec![
                "▌ ✖ ERROR",
                "▌ Cannot reach the feed host feeds.example.com",
                "▌",
                "▌ Cause",
                "▌   dns error",
                "▌",
                "▌ How to fix it",
                "▌   Check the feed URL.",
                "▌",
                "▌ What podpull was doing",
                "▌   Fetching the feed https://feeds.example.com/podcast.xml.",
            ]
        );
    }

    #[test]
    fn a_terminal_without_unicode_gets_ascii_marks() {
        let lines = lines(
            &block(Some("dns error"), None),
            &env(false, false, true),
            80,
        );

        assert_eq!(lines[0], "| x ERROR");
        assert_eq!(lines[1], "| Cannot reach the feed host feeds.example.com");
    }

    #[test]
    fn a_terminal_with_colour_paints_badge_bar_title_and_labels() {
        let lines = lines(&block(Some("dns error"), None), &env(true, true, true), 80);

        let bar = "\u{1b}[31m\u{1b}[1m▌\u{1b}[0m";
        assert_eq!(lines[0], "\u{1b}[30m\u{1b}[41m ✖ ERROR \u{1b}[0m");
        assert_eq!(
            lines[1],
            format!("{bar} \u{1b}[1mCannot reach the feed host feeds.example.com\u{1b}[0m")
        );
        assert_eq!(lines[2], bar);
        assert_eq!(lines[3], format!("{bar} \u{1b}[31mCause\u{1b}[0m"));
        assert_eq!(lines[4], format!("{bar}   dns error"));
    }

    #[test]
    fn a_block_without_a_fix_has_no_fix_section() {
        let lines = lines(&block(Some("dns error"), None), &env(false, true, true), 80);

        assert!(!lines.iter().any(|line| line.contains("How to fix it")));
        assert!(lines.iter().any(|line| line.contains("Cause")));
    }

    #[test]
    fn a_block_without_a_cause_has_no_cause_section() {
        let lines = lines(&block(None, Some("Check it.")), &env(false, true, true), 80);

        assert!(!lines.iter().any(|line| line.contains("Cause")));
        assert!(lines.iter().any(|line| line.contains("How to fix it")));
    }

    #[test]
    fn a_long_cause_wraps_at_the_width_of_the_terminal() {
        let cause =
            "failed to lookup address information: nodename nor servname provided, or not known";

        let lines = lines(&block(Some(cause), None), &env(false, true, true), 50);

        let wrapped: Vec<&String> = lines
            .iter()
            .skip_while(|line| !line.contains("Cause"))
            .skip(1)
            .take_while(|line| line.as_str() != "▌")
            .collect();
        assert!(wrapped.len() > 1, "{lines:#?}");
        for line in &lines {
            assert!(console::measure_text_width(line) <= 50, "{line}");
        }
        assert_eq!(
            wrapped
                .iter()
                .map(|line| line.trim_start_matches("▌   ").to_string())
                .collect::<Vec<_>>()
                .join(" "),
            cause
        );
    }

    #[test]
    fn a_terminal_never_wraps_wider_than_one_hundred_columns() {
        let cause = "word ".repeat(60);

        let lines = lines(
            &block(Some(cause.trim()), None),
            &env(false, true, true),
            300,
        );

        for line in &lines {
            assert!(console::measure_text_width(line) <= 100, "{line}");
        }
    }

    #[test]
    fn a_narrow_terminal_is_not_wrapped_at_all() {
        let cause = "failed to lookup address information: nodename nor servname provided";

        let lines = lines(&block(Some(cause), None), &env(false, true, true), 30);

        assert!(lines.contains(&format!("▌   {cause}")), "{lines:#?}");
    }

    #[test]
    fn a_word_wider_than_the_line_stands_alone() {
        let url = "https://example.com/a-very-long-path-that-does-not-fit-on-one-line.xml";

        let lines = lines(&block(Some(url), None), &env(false, true, true), 45);

        assert!(lines.contains(&format!("▌   {url}")), "{lines:#?}");
    }

    #[test]
    fn text_from_outside_is_shown_escaped() {
        let hostile = Block {
            title: shown::text("Cannot read \u{202E}the feed"),
            cause: Some(shown::text("bad\u{1b}[2J")),
            fix: Some(shown::text("fix\nme")),
            doing: shown::text("reading \u{7}"),
        };

        for rich in [true, false] {
            let lines = lines(&hostile, &env(false, true, rich), 80);

            let text = lines.join("\n");
            assert!(text.contains("%E2%80%AEthe feed"), "{text}");
            assert!(text.contains("bad%1B[2J"), "{text}");
            // A pipe has no line for the fix.
            assert_eq!(text.contains("fix%0Ame"), rich, "{text}");
            assert!(
                !text.contains('\u{202E}') && !text.contains('\u{1b}'),
                "{text:?}"
            );
        }
    }

    #[test]
    fn a_sentence_has_a_capital_and_a_full_stop_and_nothing_stays_nothing() {
        assert_eq!(sentence("fetching the feed"), "Fetching the feed.");
        assert_eq!(sentence(""), "");
    }
}
