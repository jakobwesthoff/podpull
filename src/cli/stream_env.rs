// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What a stream can show: colour and symbols.

use std::fmt::Display;

use console::{Emoji, Style};

/// The symbols in front of the lines podpull prints
///
/// Each has an ASCII stand-in for terminals without Unicode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mark {
    Microphone,
    Globe,
    Scroll,
    Search,
    Headphones,
    Saving,
    Success,
    Failure,
    Party,
    Folder,
    Cross,
    Broom,
    Warning,
}

impl Mark {
    /// Every mark, for tests that must not miss one
    #[cfg(test)]
    pub(crate) const ALL: [Mark; 13] = [
        Mark::Microphone,
        Mark::Globe,
        Mark::Scroll,
        Mark::Search,
        Mark::Headphones,
        Mark::Saving,
        Mark::Success,
        Mark::Failure,
        Mark::Party,
        Mark::Folder,
        Mark::Cross,
        Mark::Broom,
        Mark::Warning,
    ];

    /// The symbol, then its stand-in
    pub(crate) fn symbols(self) -> (&'static str, &'static str) {
        match self {
            Mark::Microphone => ("🎙️  ", ""),
            Mark::Globe => ("🌐 ", "[w] "),
            Mark::Scroll => ("📜 ", "[*] "),
            Mark::Search => ("🔍 ", "[~] "),
            Mark::Headphones => ("🎧 ", "[i] "),
            Mark::Saving => ("💾 ", "[v] "),
            Mark::Success => ("✅ ", "[+] "),
            Mark::Failure => ("❌ ", "[!] "),
            Mark::Party => ("🎉 ", "[*] "),
            Mark::Folder => ("📁 ", ""),
            Mark::Cross => ("✗ ", "x "),
            Mark::Broom => ("🧹 ", "[c] "),
            Mark::Warning => ("⚠️  ", "[!] "),
        }
    }
}

const GREEN: Style = Style::new().green();
const YELLOW: Style = Style::new().yellow();
const RED: Style = Style::new().red();
const CYAN: Style = Style::new().cyan();
const DIM: Style = Style::new().dim();
const BOLD_GREEN: Style = Style::new().bold().green();
const RED_BOLD: Style = Style::new().red().bold();
const YELLOW_BOLD: Style = Style::new().yellow().bold();
const BOLD_MAGENTA: Style = Style::new().bold().magenta();

/// How one output stream is rendered
///
/// Colour is decided per value and applied per call, so no global switch
/// is involved and tests can render with colour on and off side by side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StreamEnv {
    /// Colour sequences may be written
    pub(crate) color: bool,
    /// Unicode symbols may be written
    pub(crate) utf8: bool,
}

impl StreamEnv {
    pub(crate) fn new(color: bool, utf8: bool) -> StreamEnv {
        StreamEnv { color, utf8 }
    }

    /// The environment of stdout as the process sees it
    ///
    /// `console` decides the colour: `NO_COLOR`, `CLICOLOR`, `CLICOLOR_FORCE`
    /// and a terminal that supports colour. The symbols follow the rule of
    /// `console::Emoji`, which is the rule of every symbol podpull has shown
    /// so far.
    pub(crate) fn detect_stdout() -> StreamEnv {
        StreamEnv::new(
            console::colors_enabled(),
            Emoji("unicode", "").to_string() == "unicode",
        )
    }

    /// `text` in `style` when colour is on, as it is otherwise
    fn paint(&self, style: Style, text: impl Display) -> String {
        if self.color {
            style.force_styling(true).apply_to(text).to_string()
        } else {
            text.to_string()
        }
    }

    pub(crate) fn green(&self, text: impl Display) -> String {
        self.paint(GREEN, text)
    }

    pub(crate) fn yellow(&self, text: impl Display) -> String {
        self.paint(YELLOW, text)
    }

    pub(crate) fn red(&self, text: impl Display) -> String {
        self.paint(RED, text)
    }

    pub(crate) fn cyan(&self, text: impl Display) -> String {
        self.paint(CYAN, text)
    }

    pub(crate) fn dim(&self, text: impl Display) -> String {
        self.paint(DIM, text)
    }

    pub(crate) fn bold_green(&self, text: impl Display) -> String {
        self.paint(BOLD_GREEN, text)
    }

    pub(crate) fn red_bold(&self, text: impl Display) -> String {
        self.paint(RED_BOLD, text)
    }

    pub(crate) fn yellow_bold(&self, text: impl Display) -> String {
        self.paint(YELLOW_BOLD, text)
    }

    pub(crate) fn bold_magenta(&self, text: impl Display) -> String {
        self.paint(BOLD_MAGENTA, text)
    }

    /// The symbol of `mark`, or its ASCII stand-in
    pub(crate) fn mark(&self, mark: Mark) -> &'static str {
        let (symbol, stand_in) = mark.symbols();
        if self.utf8 { symbol } else { stand_in }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ON: StreamEnv = StreamEnv {
        color: true,
        utf8: true,
    };
    const OFF: StreamEnv = StreamEnv {
        color: false,
        utf8: false,
    };

    #[test]
    fn without_colour_text_stays_as_it_is() {
        assert_eq!(OFF.green("ok"), "ok");
        assert_eq!(OFF.red_bold("failed"), "failed");
        assert_eq!(OFF.dim(7), "7");
    }

    #[test]
    fn with_colour_each_style_wraps_the_text_in_its_sequences() {
        assert_eq!(ON.green("a"), "\u{1b}[32ma\u{1b}[0m");
        assert_eq!(ON.yellow("a"), "\u{1b}[33ma\u{1b}[0m");
        assert_eq!(ON.red("a"), "\u{1b}[31ma\u{1b}[0m");
        assert_eq!(ON.cyan("a"), "\u{1b}[36ma\u{1b}[0m");
        assert_eq!(ON.dim("a"), "\u{1b}[2ma\u{1b}[0m");
        assert_eq!(ON.bold_green("a"), "\u{1b}[32m\u{1b}[1ma\u{1b}[0m");
        assert_eq!(ON.red_bold("a"), "\u{1b}[31m\u{1b}[1ma\u{1b}[0m");
        assert_eq!(ON.yellow_bold("a"), "\u{1b}[33m\u{1b}[1ma\u{1b}[0m");
        assert_eq!(ON.bold_magenta("a"), "\u{1b}[35m\u{1b}[1ma\u{1b}[0m");
    }

    #[test]
    fn a_mark_is_its_symbol_with_unicode_and_its_stand_in_without() {
        assert_eq!(ON.mark(Mark::Globe), "🌐 ");
        assert_eq!(OFF.mark(Mark::Globe), "[w] ");
        assert_eq!(ON.mark(Mark::Microphone), "🎙️  ");
        assert_eq!(OFF.mark(Mark::Microphone), "");
    }

    #[test]
    fn every_mark_has_a_symbol_and_a_stand_in_that_differ() {
        for mark in Mark::ALL {
            let (symbol, stand_in) = mark.symbols();
            assert!(!symbol.is_empty(), "{mark:?}");
            assert_ne!(symbol, stand_in, "{mark:?}");
        }
    }

    #[test]
    fn detect_reads_the_environment_of_stdout() {
        // The values depend on where the tests run; the call has to work
        // anywhere.
        let env = StreamEnv::detect_stdout();

        assert_eq!(env, StreamEnv::new(env.color, env.utf8));
    }
}
