// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::io::Write;
use std::path::Path;

use podpull::{DamageKind, DamageRemedy, SyncResult};

use super::*;

/// The counts of a finished sync, for its closing line
pub(crate) fn completion_summary(result: &SyncResult, env: &StreamEnv) -> String {
    let mut parts = vec![
        format!("{} downloaded", env.bold_green(result.downloaded)),
        format!("{} existing", env.yellow(result.existing)),
    ];

    if result.repaired > 0 {
        parts.push(format!("{} repaired", env.green(result.repaired)));
    }

    if result.adopted > 0 {
        parts.push(format!("{} already stored", env.yellow(result.adopted)));
    }

    if result.limited > 0 {
        parts.push(format!("{} limited", env.cyan(result.limited)));
    }

    let failed = result.failed_episodes.len();
    parts.push(if failed > 0 {
        format!("{} failed", env.red_bold(failed))
    } else {
        format!("{} failed", env.green(failed))
    });

    if !result.damaged.is_empty() {
        parts.push(format!("{} damaged", env.red_bold(result.damaged.len())));
    }

    parts.join(", ")
}

/// Which of the files a check hashes is being read
pub(crate) fn checking_prefix(position: usize, total: usize, env: &StreamEnv) -> String {
    format!(
        "Checking stored audio [{}/{}]",
        env.cyan(position),
        env.cyan(total)
    )
}

pub(crate) fn already_stored_message(episode_title: &str, audio_filename: &str) -> String {
    format!(
        "\"{}\" is identical to {}; recorded its GUID there",
        episode_title, audio_filename
    )
}

pub(crate) fn plan_message(
    podcast_title: &str,
    total_episodes: usize,
    new_episodes: usize,
    to_download: usize,
    repairs: usize,
    env: &StreamEnv,
) -> String {
    let mut message = format!(
        "{} • {} total, {} new",
        env.bold_green(podcast_title),
        env.cyan(total_episodes),
        env.yellow(new_episodes)
    );
    // Fewer downloads than new episodes means the limit applies.
    if to_download != new_episodes {
        message.push_str(&format!(", downloading {}", env.green(to_download)));
    }
    if repairs > 0 {
        message.push_str(&format!(", repairing {}", env.green(repairs)));
    }
    message
}

pub(crate) fn slow_sync_warning(episode_title: &str, threshold: std::time::Duration) -> String {
    format!(
        "Still writing \"{}\" to the disk after {} s; a slow or network file system is the \
         likely reason, podpull keeps waiting",
        episode_title,
        threshold.as_secs()
    )
}

pub(crate) fn stuck_partial_file_message(path: &Path) -> String {
    format!(
        "Could not remove the leftover partial file {}; its episode cannot be downloaded until it is deleted",
        path.display()
    )
}

pub(crate) fn unreadable_metadata_message(path: &Path, error: &str) -> String {
    format!(
        "Could not read episode metadata {} ({})",
        path.display(),
        error
    )
}

pub(crate) fn unverifiable_audio_message(audio_filename: &str, error: &str) -> String {
    format!(
        "Could not read {} to check it against its recorded hash ({})",
        audio_filename, error
    )
}

pub(crate) fn damage_message(
    episode_title: &str,
    audio_filename: &str,
    kind: DamageKind,
    remedy: DamageRemedy,
) -> String {
    // Missing audio leaves only its metadata to delete.
    let (problem, files) = match kind {
        DamageKind::Missing => ("is missing", "its .json file"),
        DamageKind::Mismatch => (
            "does not match the hash recorded when it was downloaded",
            "it and its .json file",
        ),
    };
    let remedy = match remedy {
        DamageRemedy::Repairing => "downloading it again".to_string(),
        DamageRemedy::RepairAvailable => {
            format!(
                "run with --repair to download it again, or delete {}",
                files
            )
        }
        DamageRemedy::NoFeedEpisode => "it cannot be matched to an episode in the feed, so it \
             cannot be downloaded again and was left untouched"
            .to_string(),
        DamageRemedy::EnclosureFormatChanged => format!(
            "the feed now offers another audio format, so delete {} to download it again",
            files
        ),
    };
    format!(
        "Audio of \"{}\" ({}) {}; {}",
        episode_title, audio_filename, problem, remedy
    )
}

/// Write the failed and damaged episodes of a sync, if there are any
pub(crate) fn write_problem_lists(
    result: &SyncResult,
    out: &mut impl Write,
    env: &StreamEnv,
) -> std::io::Result<()> {
    if !result.failed_episodes.is_empty() {
        writeln!(out, "\n{}", env.red_bold("Failed episodes:"))?;
        for failed in &result.failed_episodes {
            writeln!(
                out,
                "  {}{} - {}",
                env.mark(Mark::Cross),
                env.yellow(&failed.title),
                env.dim(&failed.reason.root)
            )?;
        }
    }

    if !result.damaged.is_empty() {
        writeln!(out, "\n{}", env.red_bold("Damaged episodes:"))?;
        for damaged in &result.damaged {
            writeln!(
                out,
                "  {}{}",
                env.mark(Mark::Cross),
                damage_message(
                    &damaged.episode_title,
                    &damaged.audio_filename,
                    damaged.kind,
                    damaged.remedy
                )
            )?;
        }
    }

    let warnings: Vec<String> =
        result
            .stuck_partial_files
            .iter()
            .map(|path| stuck_partial_file_message(path))
            .chain(result.unreadable_metadata.iter().map(|unreadable| {
                unreadable_metadata_message(&unreadable.path, &unreadable.reason)
            }))
            .chain(result.unverifiable_audio.iter().map(|unverifiable| {
                unverifiable_audio_message(&unverifiable.audio_filename, &unverifiable.reason)
            }))
            .collect();
    if !warnings.is_empty() {
        writeln!(out, "\n{}", env.yellow_bold("Warnings:"))?;
        for warning in warnings {
            writeln!(out, "  {}{}", env.mark(Mark::Warning), warning)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN: StreamEnv = StreamEnv {
        color: false,
        utf8: true,
    };
    const COLOURED: StreamEnv = StreamEnv {
        color: true,
        utf8: true,
    };
    use crate::cli::test_result as result;
    use podpull::UnverifiableAudio;
    use std::path::PathBuf;

    #[test]
    fn plan_message_shows_only_new_episodes_without_limit_or_repairs() {
        assert_eq!(
            plan_message("Show", 10, 2, 2, 0, &PLAIN),
            "Show • 10 total, 2 new"
        );
    }

    #[test]
    fn plan_message_shows_the_limit() {
        assert_eq!(
            plan_message("Show", 10, 2, 1, 0, &PLAIN),
            "Show • 10 total, 2 new, downloading 1"
        );
    }

    #[test]
    fn plan_message_shows_repairs_apart_from_the_limit() {
        assert_eq!(
            plan_message("Show", 10, 2, 2, 1, &PLAIN),
            "Show • 10 total, 2 new, repairing 1"
        );
        assert_eq!(
            plan_message("Show", 10, 2, 1, 1, &PLAIN),
            "Show • 10 total, 2 new, downloading 1, repairing 1"
        );
    }

    #[test]
    fn stuck_partial_file_message_names_the_file() {
        assert_eq!(
            stuck_partial_file_message(Path::new("/podcasts/2024-01-15-Episode.mp3.partial")),
            "Could not remove the leftover partial file /podcasts/2024-01-15-Episode.mp3.partial; \
             its episode cannot be downloaded until it is deleted"
        );
    }

    #[test]
    fn slow_sync_warning_names_the_wait_and_its_likely_reason() {
        assert_eq!(
            slow_sync_warning("Sega Nomad", std::time::Duration::from_secs(30)),
            "Still writing \"Sega Nomad\" to the disk after 30 s; a slow or network file \
             system is the likely reason, podpull keeps waiting"
        );
    }

    #[test]
    fn unreadable_metadata_message_names_the_file() {
        assert_eq!(
            unreadable_metadata_message(
                Path::new("/podcasts/2024-01-15-Episode.json"),
                "EOF while parsing a string at line 1 column 15"
            ),
            "Could not read episode metadata /podcasts/2024-01-15-Episode.json \
             (EOF while parsing a string at line 1 column 15)"
        );
    }

    fn damage(remedy: DamageRemedy) -> String {
        damage_message(
            "Sega Nomad",
            "2024-12-19-Sega Nomad.mp3",
            DamageKind::Mismatch,
            remedy,
        )
    }

    fn missing(remedy: DamageRemedy) -> String {
        damage_message(
            "Sega Nomad",
            "2024-12-19-Sega Nomad.mp3",
            DamageKind::Missing,
            remedy,
        )
    }

    #[test]
    fn damage_message_names_missing_audio() {
        assert_eq!(
            missing(DamageRemedy::RepairAvailable),
            "Audio of \"Sega Nomad\" (2024-12-19-Sega Nomad.mp3) is missing; run with \
             --repair to download it again, or delete its .json file"
        );
        assert_eq!(
            missing(DamageRemedy::EnclosureFormatChanged),
            "Audio of \"Sega Nomad\" (2024-12-19-Sega Nomad.mp3) is missing; the feed \
             now offers another audio format, so delete its .json file to download it again"
        );
        assert_eq!(
            missing(DamageRemedy::NoFeedEpisode),
            "Audio of \"Sega Nomad\" (2024-12-19-Sega Nomad.mp3) is missing; it cannot \
             be matched to an episode in the feed, so it cannot be downloaded again and \
             was left untouched"
        );
    }

    #[test]
    fn damage_message_announces_repair() {
        assert_eq!(
            damage(DamageRemedy::Repairing),
            "Audio of \"Sega Nomad\" (2024-12-19-Sega Nomad.mp3) does not match \
             the hash recorded when it was downloaded; downloading it again"
        );
    }

    #[test]
    fn damage_message_points_to_repair_while_in_feed() {
        assert_eq!(
            damage(DamageRemedy::RepairAvailable),
            "Audio of \"Sega Nomad\" (2024-12-19-Sega Nomad.mp3) does not match \
             the hash recorded when it was downloaded; run with --repair to download \
             it again, or delete it and its .json file"
        );
    }

    #[test]
    fn damage_message_says_why_no_repair_is_possible() {
        assert_eq!(
            damage(DamageRemedy::NoFeedEpisode),
            "Audio of \"Sega Nomad\" (2024-12-19-Sega Nomad.mp3) does not match \
             the hash recorded when it was downloaded; it cannot be matched to an \
             episode in the feed, so it cannot be downloaded again and was left untouched"
        );
        assert_eq!(
            damage(DamageRemedy::EnclosureFormatChanged),
            "Audio of \"Sega Nomad\" (2024-12-19-Sega Nomad.mp3) does not match \
             the hash recorded when it was downloaded; the feed now offers another \
             audio format, so delete it and its .json file to download it again"
        );
    }

    #[test]
    fn already_stored_message_names_episode_and_file() {
        assert_eq!(
            already_stored_message("Sega Nomad", "2024-12-19-Sega Nomad.mp3"),
            "\"Sega Nomad\" is identical to 2024-12-19-Sega Nomad.mp3; recorded its GUID there"
        );
    }

    #[test]
    fn checking_prefix_counts_the_checked_files() {
        assert_eq!(
            checking_prefix(12, 162, &PLAIN),
            "Checking stored audio [12/162]"
        );
    }

    #[test]
    fn unverifiable_audio_message_names_file_and_error() {
        assert_eq!(
            unverifiable_audio_message("2024-12-19-Sega Nomad.mp3", "Permission denied"),
            "Could not read 2024-12-19-Sega Nomad.mp3 to check it against its \
             recorded hash (Permission denied)"
        );
    }

    #[test]
    fn completion_summary_lists_repairs_recorded_guids_and_the_limit() {
        let summary = completion_summary(
            &SyncResult {
                downloaded: 4,
                existing: 5,
                repaired: 2,
                adopted: 1,
                limited: 3,
                ..Default::default()
            },
            &PLAIN,
        );

        assert_eq!(
            summary,
            "4 downloaded, 5 existing, 2 repaired, 1 already stored, 3 limited, 0 failed"
        );
    }

    #[test]
    fn problem_lists_name_warnings() {
        let mut with_warnings = result(1, 0, 0);
        with_warnings.stuck_partial_files = vec![PathBuf::from("/podcasts/a.mp3.partial")];
        with_warnings.unverifiable_audio = vec![UnverifiableAudio {
            audio_filename: "b.mp3".to_string(),
            reason: "Permission denied".to_string(),
        }];
        let mut out = Vec::new();

        write_problem_lists(&with_warnings, &mut out, &PLAIN).unwrap();

        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("Warnings:"));
        assert!(out.contains(&stuck_partial_file_message(Path::new(
            "/podcasts/a.mp3.partial"
        ))));
        assert!(out.contains(&unverifiable_audio_message("b.mp3", "Permission denied")));
    }

    #[test]
    fn problem_lists_name_failed_and_damaged_episodes() {
        let mut out = Vec::new();

        write_problem_lists(&result(1, 1, 1), &mut out, &PLAIN).unwrap();

        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("Failed episodes:"));
        assert!(out.contains("Episode 0"));
        assert!(out.contains("HTTP error 404"));
        assert!(out.contains("Damaged episodes:"));
        assert!(out.contains("2024-01-01-Damaged.mp3"));
    }

    #[test]
    fn problem_lists_are_empty_without_problems() {
        let mut out = Vec::new();

        write_problem_lists(&result(3, 0, 0), &mut out, &PLAIN).unwrap();

        assert!(out.is_empty());
    }

    #[test]
    fn the_summary_counts_damaged_episodes() {
        let summary = completion_summary(&result(0, 0, 2), &PLAIN);

        assert_eq!(summary, "0 downloaded, 0 existing, 0 failed, 2 damaged");
    }

    #[test]
    fn the_summary_colours_its_counts_when_colour_is_on() {
        let summary = completion_summary(&result(2, 1, 0), &COLOURED);

        assert_eq!(
            summary,
            "\u{1b}[32m\u{1b}[1m2\u{1b}[0m downloaded, \u{1b}[33m0\u{1b}[0m existing, \
             \u{1b}[31m\u{1b}[1m1\u{1b}[0m failed"
        );
    }

    #[test]
    fn the_plan_colours_its_counts_when_colour_is_on() {
        let plan = plan_message("Show", 10, 2, 2, 0, &COLOURED);

        assert_eq!(
            plan,
            "\u{1b}[32m\u{1b}[1mShow\u{1b}[0m • \u{1b}[36m10\u{1b}[0m total, \
             \u{1b}[33m2\u{1b}[0m new"
        );
    }

    #[test]
    fn the_problem_lists_colour_their_headings_and_reasons_when_colour_is_on() {
        let mut out = Vec::new();

        write_problem_lists(&result(0, 1, 0), &mut out, &COLOURED).unwrap();

        assert_eq!(
            String::from_utf8(out).unwrap(),
            "\n\u{1b}[31m\u{1b}[1mFailed episodes:\u{1b}[0m\n  \
             ✗ \u{1b}[33mEpisode 0\u{1b}[0m - \u{1b}[2mHTTP error 404\u{1b}[0m\n"
        );
    }
}
