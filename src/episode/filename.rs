use crate::feed::Episode;

/// Maximum length for the title portion of a filename
const MAX_TITLE_LENGTH: usize = 100;

/// Check if a character is allowed in filenames (whitelist approach)
fn is_valid_filename_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ' ')
}

/// Generate a filename stem (without extension) for an episode
///
/// Format: "YYYY-MM-DD-sanitized-title" or "undated-sanitized-title"
pub fn generate_filename_stem(episode: &Episode) -> String {
    let date_prefix = episode
        .pub_date
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "undated".to_string());

    let sanitized_title = sanitize_title(&episode.title);

    format!("{}-{}", date_prefix, sanitized_title)
}

/// Get the audio file extension from an episode's enclosure
///
/// Attempts to extract from URL path or MIME type, defaults to "mp3"
pub fn get_audio_extension(episode: &Episode) -> String {
    // Try to get extension from URL path
    if let Some(ext) = episode
        .enclosure
        .url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .and_then(|filename| filename.rsplit('.').next())
        .filter(|ext| is_valid_audio_extension(ext))
    {
        return ext.to_lowercase();
    }

    // Try to get extension from MIME type
    if let Some(ref mime) = episode.enclosure.mime_type
        && let Some(ext) = mime_to_extension(mime)
    {
        return ext.to_string();
    }

    // Default to mp3
    "mp3".to_string()
}

/// Generate a complete filename for an episode (with extension)
pub fn generate_filename(episode: &Episode) -> String {
    let stem = generate_filename_stem(episode);
    let ext = get_audio_extension(episode);
    format!("{}.{}", stem, ext)
}

/// Sanitize a title for use in a filename using whitelist approach
fn sanitize_title(title: &str) -> String {
    let sanitized: String = title
        .chars()
        .map(|c| if is_valid_filename_char(c) { c } else { '-' })
        .collect();

    // Collapse multiple spaces/dashes into single dash
    let collapsed = collapse_separators(&sanitized);

    // Trim and limit length
    let trimmed = collapsed.trim_matches(|c: char| c == '-' || c.is_whitespace());

    if trimmed.len() > MAX_TITLE_LENGTH {
        // Truncate at word boundary if possible
        truncate_at_boundary(trimmed, MAX_TITLE_LENGTH)
    } else {
        trimmed.to_string()
    }
}

/// Collapse multiple spaces and dashes into single dashes
fn collapse_separators(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut last_was_separator = false;

    for c in s.chars() {
        if c == '-' || c.is_whitespace() {
            if !last_was_separator {
                result.push('-');
                last_was_separator = true;
            }
        } else {
            result.push(c);
            last_was_separator = false;
        }
    }

    result
}

/// Truncate string at a word boundary
fn truncate_at_boundary(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        return s.to_string();
    }

    // Find the last separator before max_len
    let truncated: String = s.chars().take(max_len).collect();
    if let Some(pos) = truncated.rfind('-')
        && pos > max_len / 2
    {
        return truncated[..pos].to_string();
    }

    truncated.trim_end_matches('-').to_string()
}

/// Check if a string is a valid audio file extension
fn is_valid_audio_extension(ext: &str) -> bool {
    matches!(
        ext.to_lowercase().as_str(),
        "mp3" | "m4a" | "mp4" | "aac" | "ogg" | "opus" | "wav" | "flac"
    )
}

/// Map MIME types to file extensions
fn mime_to_extension(mime: &str) -> Option<&'static str> {
    match mime.to_lowercase().as_str() {
        "audio/mpeg" | "audio/mp3" => Some("mp3"),
        "audio/mp4" | "audio/m4a" | "audio/x-m4a" => Some("m4a"),
        "audio/aac" => Some("aac"),
        "audio/ogg" => Some("ogg"),
        "audio/opus" => Some("opus"),
        "audio/wav" | "audio/x-wav" => Some("wav"),
        "audio/flac" | "audio/x-flac" => Some("flac"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed::Enclosure;
    use chrono::DateTime;
    use url::Url;

    fn make_episode(title: &str, date: Option<&str>, url: &str) -> Episode {
        make_episode_with_mime(title, date, url, Some("audio/mpeg"))
    }

    fn make_episode_with_mime(
        title: &str,
        date: Option<&str>,
        url: &str,
        mime: Option<&str>,
    ) -> Episode {
        Episode {
            title: title.to_string(),
            description: None,
            pub_date: date.and_then(|d| DateTime::parse_from_rfc2822(d).ok()),
            guid: Some("test-guid".to_string()),
            enclosure: Enclosure {
                url: Url::parse(url).unwrap(),
                length: None,
                mime_type: mime.map(String::from),
            },
            duration: None,
            episode_number: None,
            season_number: None,
        }
    }

    // === Sanitization tests ===

    #[test]
    fn sanitize_preserves_alphanumeric() {
        assert_eq!(sanitize_title("Hello123World"), "Hello123World");
    }

    #[test]
    fn sanitize_preserves_underscores_and_dots() {
        assert_eq!(sanitize_title("hello_world.test"), "hello_world.test");
    }

    #[test]
    fn sanitize_replaces_special_chars_with_dash() {
        assert_eq!(sanitize_title("a:b/c\\d"), "a-b-c-d");
    }

    #[test]
    fn sanitize_replaces_quotes_and_brackets() {
        assert_eq!(
            sanitize_title("\"quoted\" <angle> [square]"),
            "quoted-angle-square"
        );
    }

    #[test]
    fn sanitize_handles_unicode_chars() {
        // Non-ASCII chars should be replaced
        assert_eq!(sanitize_title("Café résumé"), "Caf-r-sum");
    }

    #[test]
    fn sanitize_handles_emoji() {
        assert_eq!(sanitize_title("Hello 🎙️ World"), "Hello-World");
    }

    #[test]
    fn sanitize_collapses_consecutive_invalid_chars() {
        assert_eq!(sanitize_title("a:::b///c"), "a-b-c");
    }

    #[test]
    fn sanitize_collapses_mixed_spaces_and_dashes() {
        assert_eq!(sanitize_title("a - - - b"), "a-b");
    }

    #[test]
    fn sanitize_trims_leading_trailing_separators() {
        assert_eq!(sanitize_title("  --hello--  "), "hello");
    }

    #[test]
    fn sanitize_handles_empty_string() {
        assert_eq!(sanitize_title(""), "");
    }

    #[test]
    fn sanitize_handles_only_invalid_chars() {
        assert_eq!(sanitize_title(":::///"), "");
    }

    #[test]
    fn sanitize_preserves_numbers() {
        assert_eq!(sanitize_title("Episode 42"), "Episode-42");
    }

    #[test]
    fn sanitize_handles_path_separators() {
        assert_eq!(sanitize_title("path/to\\file"), "path-to-file");
    }

    #[test]
    fn sanitize_handles_newlines_and_tabs() {
        assert_eq!(sanitize_title("line1\nline2\ttab"), "line1-line2-tab");
    }

    // === Truncation tests ===

    #[test]
    fn truncate_preserves_short_strings() {
        assert_eq!(truncate_at_boundary("short", 100), "short");
    }

    #[test]
    fn truncate_cuts_at_word_boundary() {
        let long = "word1-word2-word3-word4-word5";
        let result = truncate_at_boundary(long, 20);
        assert!(result.len() <= 20);
        assert!(!result.ends_with('-'));
    }

    #[test]
    fn truncate_handles_no_boundaries() {
        let long = "a".repeat(150);
        let result = truncate_at_boundary(&long, 100);
        assert_eq!(result.len(), 100);
    }

    // === Filename stem tests ===

    #[test]
    fn filename_stem_includes_date_prefix() {
        let episode = make_episode(
            "Test Episode",
            Some("Mon, 15 Jan 2024 12:00:00 +0000"),
            "https://example.com/ep.mp3",
        );

        assert_eq!(generate_filename_stem(&episode), "2024-01-15-Test-Episode");
    }

    #[test]
    fn filename_stem_uses_undated_when_no_date() {
        let episode = make_episode("Test Episode", None, "https://example.com/ep.mp3");

        assert_eq!(generate_filename_stem(&episode), "undated-Test-Episode");
    }

    #[test]
    fn filename_stem_handles_different_timezones() {
        let episode = make_episode(
            "Test",
            Some("Mon, 15 Jan 2024 23:00:00 -0800"),
            "https://example.com/ep.mp3",
        );
        // Date should be preserved as-is from the timezone
        let stem = generate_filename_stem(&episode);
        assert!(stem.starts_with("2024-01-15") || stem.starts_with("2024-01-16"));
    }

    #[test]
    fn sanitizes_invalid_characters() {
        let episode = make_episode(
            "Episode: A \"Test\" <Episode>",
            None,
            "https://example.com/ep.mp3",
        );

        let stem = generate_filename_stem(&episode);
        assert!(!stem.contains(':'));
        assert!(!stem.contains('"'));
        assert!(!stem.contains('<'));
        assert!(!stem.contains('>'));
    }

    #[test]
    fn collapses_multiple_spaces_and_dashes() {
        let episode = make_episode(
            "Episode   with   spaces",
            None,
            "https://example.com/ep.mp3",
        );

        let stem = generate_filename_stem(&episode);
        assert!(!stem.contains("--"));
        assert!(!stem.contains("  "));
    }

    #[test]
    fn truncates_long_titles() {
        let long_title = "A".repeat(200);
        let episode = make_episode(&long_title, None, "https://example.com/ep.mp3");

        let stem = generate_filename_stem(&episode);
        assert!(stem.len() <= MAX_TITLE_LENGTH + 10); // date prefix + title
    }

    // === Extension extraction tests ===

    #[test]
    fn extracts_extension_from_url() {
        let episode = make_episode("Test", None, "https://example.com/episode.m4a");
        assert_eq!(get_audio_extension(&episode), "m4a");
    }

    #[test]
    fn extracts_mp3_extension() {
        let episode = make_episode("Test", None, "https://example.com/episode.mp3");
        assert_eq!(get_audio_extension(&episode), "mp3");
    }

    #[test]
    fn extracts_ogg_extension() {
        let episode = make_episode("Test", None, "https://example.com/episode.ogg");
        assert_eq!(get_audio_extension(&episode), "ogg");
    }

    #[test]
    fn extracts_opus_extension() {
        let episode = make_episode("Test", None, "https://example.com/episode.opus");
        assert_eq!(get_audio_extension(&episode), "opus");
    }

    #[test]
    fn normalizes_extension_to_lowercase() {
        let episode = make_episode("Test", None, "https://example.com/episode.MP3");
        assert_eq!(get_audio_extension(&episode), "mp3");
    }

    #[test]
    fn handles_url_with_query_params() {
        let episode = make_episode("Test", None, "https://example.com/episode.mp3?token=abc");
        assert_eq!(get_audio_extension(&episode), "mp3");
    }

    #[test]
    fn falls_back_to_mime_type() {
        let episode = make_episode_with_mime(
            "Test",
            None,
            "https://example.com/episode",
            Some("audio/mpeg"),
        );
        assert_eq!(get_audio_extension(&episode), "mp3");
    }

    #[test]
    fn mime_m4a_maps_correctly() {
        let episode = make_episode_with_mime(
            "Test",
            None,
            "https://example.com/episode",
            Some("audio/mp4"),
        );
        assert_eq!(get_audio_extension(&episode), "m4a");
    }

    #[test]
    fn mime_ogg_maps_correctly() {
        let episode = make_episode_with_mime(
            "Test",
            None,
            "https://example.com/episode",
            Some("audio/ogg"),
        );
        assert_eq!(get_audio_extension(&episode), "ogg");
    }

    #[test]
    fn defaults_to_mp3_for_unknown_extension() {
        let episode = make_episode("Test", None, "https://example.com/episode");
        assert_eq!(get_audio_extension(&episode), "mp3");
    }

    #[test]
    fn defaults_to_mp3_for_unknown_mime() {
        let episode = make_episode_with_mime(
            "Test",
            None,
            "https://example.com/episode",
            Some("application/octet-stream"),
        );
        assert_eq!(get_audio_extension(&episode), "mp3");
    }

    #[test]
    fn defaults_to_mp3_for_no_mime() {
        let episode = make_episode_with_mime("Test", None, "https://example.com/episode", None);
        assert_eq!(get_audio_extension(&episode), "mp3");
    }

    #[test]
    fn ignores_non_audio_extensions() {
        let episode = make_episode("Test", None, "https://example.com/episode.html");
        assert_eq!(get_audio_extension(&episode), "mp3");
    }

    // === Full filename tests ===

    #[test]
    fn generate_filename_combines_stem_and_extension() {
        let episode = make_episode(
            "My Episode",
            Some("Mon, 15 Jan 2024 12:00:00 +0000"),
            "https://example.com/audio.mp3",
        );

        assert_eq!(generate_filename(&episode), "2024-01-15-My-Episode.mp3");
    }

    #[test]
    fn generate_filename_with_m4a() {
        let episode = make_episode(
            "Audio Book",
            Some("Tue, 16 Jan 2024 12:00:00 +0000"),
            "https://example.com/book.m4a",
        );

        assert_eq!(generate_filename(&episode), "2024-01-16-Audio-Book.m4a");
    }

    // === Collapse separators tests ===

    #[test]
    fn collapse_single_space() {
        assert_eq!(collapse_separators("hello world"), "hello-world");
    }

    #[test]
    fn collapse_multiple_spaces() {
        assert_eq!(collapse_separators("hello    world"), "hello-world");
    }

    #[test]
    fn collapse_multiple_dashes() {
        assert_eq!(collapse_separators("hello----world"), "hello-world");
    }

    #[test]
    fn collapse_mixed() {
        assert_eq!(collapse_separators("hello - - world"), "hello-world");
    }

    #[test]
    fn collapse_preserves_non_separators() {
        assert_eq!(collapse_separators("ab cd ef"), "ab-cd-ef");
    }

    // === Valid char tests ===

    #[test]
    fn valid_char_accepts_lowercase() {
        assert!(is_valid_filename_char('a'));
        assert!(is_valid_filename_char('z'));
    }

    #[test]
    fn valid_char_accepts_uppercase() {
        assert!(is_valid_filename_char('A'));
        assert!(is_valid_filename_char('Z'));
    }

    #[test]
    fn valid_char_accepts_digits() {
        assert!(is_valid_filename_char('0'));
        assert!(is_valid_filename_char('9'));
    }

    #[test]
    fn valid_char_accepts_special() {
        assert!(is_valid_filename_char('-'));
        assert!(is_valid_filename_char('_'));
        assert!(is_valid_filename_char('.'));
        assert!(is_valid_filename_char(' '));
    }

    #[test]
    fn valid_char_rejects_special_chars() {
        assert!(!is_valid_filename_char('/'));
        assert!(!is_valid_filename_char('\\'));
        assert!(!is_valid_filename_char(':'));
        assert!(!is_valid_filename_char('*'));
        assert!(!is_valid_filename_char('?'));
        assert!(!is_valid_filename_char('"'));
        assert!(!is_valid_filename_char('<'));
        assert!(!is_valid_filename_char('>'));
        assert!(!is_valid_filename_char('|'));
    }

    #[test]
    fn valid_char_rejects_unicode() {
        assert!(!is_valid_filename_char('é'));
        assert!(!is_valid_filename_char('ñ'));
        assert!(!is_valid_filename_char('中'));
    }
}
