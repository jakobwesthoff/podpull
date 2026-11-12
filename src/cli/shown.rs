// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! How names and texts from the feed and from the disk appear in a line
//!
//! A title, a file name or a path can hold characters that end a line,
//! forge another one, move the cursor or make the name look different from
//! what it is. A name shows each of them as `%` and the upper-case hex of
//! each of its UTF-8 bytes. The characters are the Unicode control
//! characters (category Cc), the bidi controls U+202A to U+202E and U+2066
//! to U+2069, and U+2028 and U+2029. A `%` shows as `%25` and a byte that is
//! not UTF-8 as `%XX`, so the shown form of a name stands for exactly one
//! name.
//!
//! Text that is not a name, such as the words of a system error, shows the
//! same characters escaped but keeps its `%`. A URL shows as the URL
//! parses, which holds no such character and keeps its `%20`.
//!
//! Every message builder takes [`Safe`], which only the functions of this
//! module make, so a raw value cannot reach a line.

use std::fmt;
use std::path::Path;

use podpull::{FailureReason, Subject};
use url::Url;

/// Text that is safe to put in a line: it holds no character of the
/// escaped class
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Safe(String);

impl Safe {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Safe {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Whether a line shows `character` in its escaped form
pub(crate) fn is_escaped(character: char) -> bool {
    character.is_control()
        || matches!(character, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
        || matches!(character, '\u{2028}' | '\u{2029}')
}

/// `character` as `%` and the upper-case hex digits of each of its UTF-8
/// bytes
fn push_percent_encoded(shown: &mut String, character: char) {
    let mut buffer = [0u8; 4];
    for byte in character.encode_utf8(&mut buffer).bytes() {
        shown.push_str(&format!("%{byte:02X}"));
    }
}

/// `text` with every escaped character written as `%` and the hex digits of
/// each of its bytes; everything else, `%` included, stays
fn escape_controls(text: &str) -> String {
    let mut shown = String::with_capacity(text.len());
    for character in text.chars() {
        if is_escaped(character) {
            push_percent_encoded(&mut shown, character);
        } else {
            shown.push(character);
        }
    }
    shown
}

/// `text` as a name: the escaped characters and `%` written out
fn escape_str(text: &str) -> String {
    let mut shown = String::with_capacity(text.len());
    for character in text.chars() {
        if character == '%' {
            shown.push_str("%25");
        } else if is_escaped(character) {
            push_percent_encoded(&mut shown, character);
        } else {
            shown.push(character);
        }
    }
    shown
}

/// `bytes` as a name: what is valid UTF-8 as [`escape_str`], every other byte
/// as `%XX`
fn escape_bytes(bytes: &[u8]) -> String {
    let mut shown = String::new();
    let mut rest = bytes;
    while !rest.is_empty() {
        match std::str::from_utf8(rest) {
            Ok(text) => {
                shown.push_str(&escape_str(text));
                break;
            }
            Err(error) => {
                let (valid, after) = rest.split_at(error.valid_up_to());
                shown.push_str(&escape_str(
                    std::str::from_utf8(valid).expect("the prefix is valid UTF-8"),
                ));
                let invalid = error.error_len().unwrap_or(after.len());
                for byte in &after[..invalid] {
                    shown.push_str(&format!("%{byte:02X}"));
                }
                rest = &after[invalid..];
            }
        }
    }
    shown
}

/// A name from the feed or from the disk, such as a title or a file name
pub(crate) fn name(name: &str) -> Safe {
    Safe(escape_str(name))
}

/// A path, shown as a name
pub(crate) fn path(path: &Path) -> Safe {
    Safe(escape_path(path))
}

/// Text that is no name, such as the words of an error
pub(crate) fn text(text: &str) -> Safe {
    Safe(escape_controls(text))
}

/// The reason of a failure: its root as text, then what the failing step was
/// about in parentheses, shown by the rule for its kind, then an audio file
/// that could not be removed, shown as a name
pub(crate) fn reason(reason: &FailureReason) -> Safe {
    let mut shown = escape_controls(&reason.root);
    match &reason.subject {
        Subject::Url(subject) => shown.push_str(&format!(" ({})", url(subject))),
        Subject::Path(subject) => shown.push_str(&format!(" ({})", path(subject))),
        Subject::None => {}
    }
    if let Some(leftover) = &reason.leftover_audio {
        shown.push_str(&format!(
            "; the audio file {} could not be removed: {}",
            name(&leftover.audio_filename),
            escape_controls(&leftover.reason)
        ));
    }
    Safe(shown)
}

/// The source of a feed: a URL as it parses, a file as a name
pub(crate) fn feed_source(source: &str) -> Safe {
    if source.starts_with("http://") || source.starts_with("https://") {
        url(source)
    } else {
        name(source)
    }
}

/// A URL as it parses, or as text when it does not parse
pub(crate) fn url(url: &str) -> Safe {
    match Url::parse(url) {
        Ok(parsed) => Safe(parsed.as_str().to_string()),
        Err(_) => text(url),
    }
}

/// A path as a name where its bytes are not available: what is not valid
/// text became the replacement character
// Only the build for other targets than unix calls it.
#[cfg_attr(unix, allow(dead_code))]
fn escape_path_lossy(path: &Path) -> String {
    escape_str(&path.to_string_lossy())
}

#[cfg(unix)]
fn escape_path(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    escape_bytes(path.as_os_str().as_bytes())
}

#[cfg(not(unix))]
fn escape_path(path: &Path) -> String {
    escape_path_lossy(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn a_name_shows_control_characters_as_percent_and_hex() {
        assert_eq!(name("a\nb\u{1b}[31m").as_str(), "a%0Ab%1B[31m");
        assert_eq!(name("tab\there").as_str(), "tab%09here");
    }

    #[test]
    fn a_name_shows_bidi_controls_as_percent_and_hex() {
        assert_eq!(name("\u{202E}3pm.evil").as_str(), "%E2%80%AE3pm.evil");
        assert_eq!(name("a\u{2066}b\u{2069}").as_str(), "a%E2%81%A6b%E2%81%A9");
    }

    #[test]
    fn a_name_shows_line_and_paragraph_separators_as_percent_and_hex() {
        assert_eq!(
            name("a\u{2028}b\u{2029}c").as_str(),
            "a%E2%80%A8b%E2%80%A9c"
        );
    }

    #[test]
    fn a_name_shows_a_percent_as_percent_25() {
        assert_eq!(name("100% Retro").as_str(), "100%25 Retro");
        assert_eq!(name("%1B").as_str(), "%251B");
    }

    #[test]
    fn a_name_keeps_umlauts_dashes_and_emoji() {
        assert_eq!(
            name("Größe – Folge 4 \u{2764}\u{FE0F}").as_str(),
            "Größe – Folge 4 \u{2764}\u{FE0F}"
        );
    }

    #[test]
    fn a_text_escapes_the_class_and_keeps_its_percent() {
        assert_eq!(
            text("50%20 off\n\u{202E}").as_str(),
            "50%20 off%0A%E2%80%AE"
        );
    }

    #[test]
    fn a_url_shows_as_it_parses_and_keeps_its_percent_encoding() {
        assert_eq!(
            url("https://cdn.example.com/My%20Show/a b.mp3").as_str(),
            "https://cdn.example.com/My%20Show/a%20b.mp3"
        );
    }

    #[test]
    fn a_reason_shows_its_url_subject_in_parentheses() {
        let reason = FailureReason::new(
            "connection refused",
            Subject::Url("http://127.0.0.1:9/My%20Show/ep2.mp3".to_string()),
        );

        assert_eq!(
            super::reason(&reason).as_str(),
            "connection refused (http://127.0.0.1:9/My%20Show/ep2.mp3)"
        );
    }

    #[test]
    fn a_reason_shows_its_path_subject_escaped_in_parentheses() {
        let reason = FailureReason::new(
            "permission denied",
            Subject::Path(PathBuf::from("/podcasts/a\n100%.mp3")),
        );

        assert_eq!(
            super::reason(&reason).as_str(),
            "permission denied (/podcasts/a%0A100%25.mp3)"
        );
    }

    #[test]
    fn a_reason_shows_the_audio_file_that_could_not_be_removed_by_the_rule_for_names() {
        let reason = FailureReason::new(
            "could not write the metadata: disk full",
            Subject::Path(PathBuf::from("/podcasts/a.json")),
        )
        .with_leftover_audio("100%\u{1b}.mp3", "busy\u{7}");

        assert_eq!(
            super::reason(&reason).as_str(),
            "could not write the metadata: disk full (/podcasts/a.json); the audio file \
             100%25%1B.mp3 could not be removed: busy%07"
        );
    }

    #[test]
    fn a_reason_without_a_subject_is_its_root_escaped() {
        let reason = FailureReason::without_subject("refused\u{1b}[2J");

        assert_eq!(super::reason(&reason).as_str(), "refused%1B[2J");
    }

    #[test]
    fn a_url_that_does_not_parse_shows_as_text() {
        assert_eq!(url("not a url\n").as_str(), "not a url%0A");
    }

    #[test]
    fn bytes_that_are_not_utf8_show_as_percent_xx() {
        assert_eq!(escape_bytes(b"a\xFFb%\n").as_str(), "a%FFb%25%0A");
        assert_eq!(escape_bytes("Größe".as_bytes()).as_str(), "Größe");
    }

    #[test]
    fn a_path_shows_as_a_name() {
        assert_eq!(path(Path::new("/a/%\n.mp3")).as_str(), "/a/%25%0A.mp3");
    }

    #[test]
    fn a_lossy_path_shows_as_a_name() {
        assert_eq!(
            escape_path_lossy(Path::new("/a/%\u{202E}.mp3")),
            "/a/%25%E2%80%AE.mp3"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_path_that_is_not_utf8_shows_its_bytes() {
        use std::os::unix::ffi::OsStrExt;

        let raw = Path::new(std::ffi::OsStr::from_bytes(b"/a\xFFb"));

        assert_eq!(path(raw).as_str(), "/a%FFb");
    }

    #[test]
    fn a_safe_value_displays_its_text() {
        assert_eq!(name("a%").to_string(), "a%25");
    }

    #[test]
    fn the_escaped_class_is_control_bidi_and_separators() {
        assert!(is_escaped('\u{7}'));
        assert!(is_escaped('\u{202A}'));
        assert!(is_escaped('\u{2069}'));
        assert!(is_escaped('\u{2028}'));
        assert!(!is_escaped('a'));
        assert!(!is_escaped('%'));
    }
}
