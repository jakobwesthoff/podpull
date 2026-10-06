// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use clap::Parser;
use colored::Colorize;
use console::Emoji;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

use podpull::{
    AudioCheck, DamageKind, DamageRemedy, NoopReporter, ProgressEvent, ProgressReporter,
    ReqwestClient, SharedProgressReporter, SyncOptions, SyncResult, sync_podcast,
};

// Emoji with fallback for terminals without Unicode support
static MICROPHONE: Emoji<'_, '_> = Emoji("🎙️  ", "");
static GLOBE: Emoji<'_, '_> = Emoji("🌐 ", "[w] ");
static COG: Emoji<'_, '_> = Emoji("⚙️  ", "[*] ");
static SEARCH: Emoji<'_, '_> = Emoji("🔍 ", "[~] ");
static HEADPHONES: Emoji<'_, '_> = Emoji("🎧 ", "[i] ");
static SAVING: Emoji<'_, '_> = Emoji("💾 ", "[v] ");
static SUCCESS: Emoji<'_, '_> = Emoji("✅ ", "[+] ");
static FAILURE: Emoji<'_, '_> = Emoji("❌ ", "[!] ");
static PARTY: Emoji<'_, '_> = Emoji("🎉 ", "[*] ");
static FOLDER: Emoji<'_, '_> = Emoji("📁 ", "");
static CROSS: Emoji<'_, '_> = Emoji("✗ ", "x ");
static BROOM: Emoji<'_, '_> = Emoji("🧹 ", "[c] ");
static WARNING: Emoji<'_, '_> = Emoji("⚠️  ", "[!] ");

/// Download and synchronize podcasts from RSS feeds
#[derive(Parser, Debug)]
#[command(name = "podpull")]
#[command(about = "Download and synchronize podcasts from RSS feeds")]
#[command(version)]
struct Args {
    /// RSS feed URL or path to local RSS file
    feed: String,

    /// Output directory for downloaded episodes
    output_dir: PathBuf,

    /// Maximum number of concurrent downloads
    #[arg(
        short = 'c',
        long,
        default_value = "3",
        value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..)
    )]
    concurrent: usize,

    /// Maximum number of episodes to download
    #[arg(short, long)]
    limit: Option<usize>,

    /// Quiet mode - suppress progress output
    #[arg(short, long)]
    quiet: bool,

    /// Check every downloaded audio file against the hash recorded when it
    /// was downloaded and report mismatches (reads the whole archive)
    #[arg(long)]
    verify: bool,

    /// Like --verify, and download mismatched episodes still in the feed
    /// again under their existing filenames
    #[arg(long)]
    repair: bool,
}

/// Progress reporter using indicatif for terminal output
struct IndicatifReporter {
    multi: MultiProgress,
    bars: Mutex<HashMap<usize, ProgressBar>>,
    main_bar: ProgressBar,
}

impl IndicatifReporter {
    fn new() -> Self {
        let multi = MultiProgress::new();

        let main_style = ProgressStyle::default_bar()
            .template("{spinner:.green} {wide_msg}")
            .unwrap();

        let main_bar = multi.add(ProgressBar::new_spinner());
        main_bar.set_style(main_style);
        main_bar.enable_steady_tick(std::time::Duration::from_millis(100));

        Self {
            multi,
            bars: Mutex::new(HashMap::new()),
            main_bar,
        }
    }

    fn get_or_create_bar(&self, download_id: usize) -> ProgressBar {
        let mut bars = self.bars.lock().unwrap();

        if let Some(bar) = bars.get(&download_id) {
            return bar.clone();
        }

        let style = ProgressStyle::default_bar()
            .template(&format!(
                "  {SAVING}[{{bar:30.cyan/blue}}] {{bytes}}/{{total_bytes}} {{wide_msg}}"
            ))
            .unwrap()
            .progress_chars("█▓░");

        let bar = self.multi.add(ProgressBar::new(0));
        bar.set_style(style);
        bars.insert(download_id, bar.clone());
        bar
    }

    fn finish_bar(&self, download_id: usize) {
        let mut bars = self.bars.lock().unwrap();
        if let Some(bar) = bars.remove(&download_id) {
            bar.finish_and_clear();
        }
    }
}

impl ProgressReporter for IndicatifReporter {
    fn report(&self, event: ProgressEvent) {
        match event {
            ProgressEvent::FetchingFeed { url } => {
                self.main_bar
                    .set_message(format!("{GLOBE}Fetching feed: {}", url.cyan()));
            }

            ProgressEvent::ParsingFeed { source } => {
                self.main_bar
                    .set_message(format!("{COG}Parsing feed: {}", source.cyan()));
            }

            ProgressEvent::ScanningDirectory {
                files_scanned,
                total_files,
            } => {
                if total_files == 0 {
                    self.main_bar
                        .set_message(format!("{SEARCH}Scanning existing episodes..."));
                } else {
                    // Switch to progress bar style for scanning
                    if files_scanned == 0 {
                        let scan_style = ProgressStyle::default_bar()
                            .template(&format!(
                                "{{spinner:.green}} {SEARCH}Scanning existing episodes... [{{bar:30.cyan/blue}}] {{pos}}/{{len}}"
                            ))
                            .unwrap()
                            .progress_chars("█▓░");
                        self.main_bar.set_style(scan_style);
                        self.main_bar.set_length(total_files as u64);
                    }
                    self.main_bar.set_position(files_scanned as u64);
                }
            }

            ProgressEvent::SyncPlanReady {
                podcast_title,
                total_episodes,
                new_episodes,
                to_download,
                repairs,
            } => {
                // Reset to spinner style after scanning
                let main_style = ProgressStyle::default_bar()
                    .template("{spinner:.green} {wide_msg}")
                    .unwrap();
                self.main_bar.set_style(main_style);
                self.main_bar.set_message(format!(
                    "{HEADPHONES}{}",
                    plan_message(
                        &podcast_title,
                        total_episodes,
                        new_episodes,
                        to_download,
                        repairs
                    )
                ));
            }

            ProgressEvent::DownloadStarting {
                download_id,
                episode_title,
                episode_index,
                total_to_download,
                content_length,
            } => {
                let bar = self.get_or_create_bar(download_id);
                bar.set_length(content_length.unwrap_or(0));
                bar.set_position(0);
                // Calculate width needed for "[idx/total]" part
                let index_width =
                    (episode_index + 1).to_string().len() + total_to_download.to_string().len();
                let title_width = available_title_width(index_width);
                bar.set_message(format!(
                    "[{}/{}] {}",
                    (episode_index + 1).to_string().cyan(),
                    total_to_download.to_string().cyan(),
                    truncate_title(&episode_title, title_width)
                ));
            }

            ProgressEvent::DownloadProgress {
                download_id,
                bytes_downloaded,
                total_bytes,
            } => {
                let bar = self.get_or_create_bar(download_id);
                if let Some(total) = total_bytes {
                    bar.set_length(total);
                }
                bar.set_position(bytes_downloaded);
            }

            ProgressEvent::DownloadCompleted {
                download_id,
                episode_title,
                bytes_downloaded,
            } => {
                let bar = self.get_or_create_bar(download_id);
                bar.set_position(bytes_downloaded);
                // No index displayed, so use 0 for index_width calculation
                let title_width = available_title_width(0);
                bar.set_message(format!(
                    "{SUCCESS}{}",
                    truncate_title(&episode_title, title_width).green()
                ));
                self.finish_bar(download_id);
            }

            ProgressEvent::DownloadFailed {
                download_id,
                episode_title,
                error,
            } => {
                let bar = self.get_or_create_bar(download_id);
                // Reserve space for " - " and some error text (at least 30 chars)
                let title_width = available_title_width(0).saturating_sub(3 + 30);
                bar.abandon_with_message(format!(
                    "{FAILURE}{} - {}",
                    truncate_title(&episode_title, title_width.max(20)).red(),
                    error.red()
                ));
                self.finish_bar(download_id);
            }

            ProgressEvent::PartialFilesCleanedUp { count } => {
                if count > 0 {
                    self.main_bar.set_message(format!(
                        "{BROOM}Cleaned up {} interrupted download{}",
                        count.to_string().yellow(),
                        if count == 1 { "" } else { "s" }
                    ));
                }
            }

            ProgressEvent::EpisodeAlreadyStored {
                download_id,
                episode_title,
                audio_filename,
            } => {
                let bar = self.get_or_create_bar(download_id);
                bar.set_message(format!(
                    "{SUCCESS}{}",
                    already_stored_message(&episode_title, &audio_filename).green()
                ));
                self.finish_bar(download_id);
            }

            ProgressEvent::PartialFileStuck { path } => {
                let _ = self.multi.println(format!(
                    "{WARNING}{}",
                    stuck_partial_file_message(&path).yellow()
                ));
            }

            ProgressEvent::MetadataUnreadable { path, error } => {
                // Printed above the progress bars so the warning outlives
                // the transient status line.
                let _ = self.multi.println(format!(
                    "{WARNING}{}",
                    unreadable_metadata_message(&path, &error).yellow()
                ));
            }

            ProgressEvent::VerifyingStoredAudio { audio_filename } => {
                self.main_bar
                    .set_message(format!("{SEARCH}Checking {}...", audio_filename.cyan()));
            }

            ProgressEvent::StoredAudioUnverifiable {
                audio_filename,
                error,
            } => {
                let _ = self.multi.println(format!(
                    "{WARNING}{}",
                    unverifiable_audio_message(&audio_filename, &error).yellow()
                ));
            }

            ProgressEvent::StoredAudioDamaged {
                episode_title,
                audio_filename,
                kind,
                remedy,
            } => {
                let _ = self.multi.println(format!(
                    "{WARNING}{}",
                    damage_message(&episode_title, &audio_filename, kind, remedy).yellow()
                ));
            }

            ProgressEvent::SyncCompleted => {
                self.main_bar.finish_and_clear();
            }
        }
    }
}

/// The counts of a finished sync, for its closing line
fn completion_summary(result: &SyncResult) -> String {
    let mut parts = vec![
        format!(
            "{} downloaded",
            result.downloaded.to_string().green().bold()
        ),
        format!("{} existing", result.existing.to_string().yellow()),
    ];

    if result.repaired > 0 {
        parts.push(format!("{} repaired", result.repaired.to_string().green()));
    }

    if result.adopted > 0 {
        parts.push(format!(
            "{} already stored",
            result.adopted.to_string().yellow()
        ));
    }

    if result.limited > 0 {
        parts.push(format!("{} limited", result.limited.to_string().cyan()));
    }

    let failed = result.failed_episodes.len();
    parts.push(if failed > 0 {
        format!("{} failed", failed.to_string().red().bold())
    } else {
        format!("{} failed", failed.to_string().green())
    });

    if !result.damaged.is_empty() {
        parts.push(format!(
            "{} damaged",
            result.damaged.len().to_string().red().bold()
        ));
    }

    parts.join(", ")
}

fn already_stored_message(episode_title: &str, audio_filename: &str) -> String {
    format!(
        "\"{}\" is identical to {}; recorded its GUID there",
        episode_title, audio_filename
    )
}

fn plan_message(
    podcast_title: &str,
    total_episodes: usize,
    new_episodes: usize,
    to_download: usize,
    repairs: usize,
) -> String {
    let mut message = format!(
        "{} • {} total, {} new",
        podcast_title.bold().green(),
        total_episodes.to_string().cyan(),
        new_episodes.to_string().yellow()
    );
    // Fewer downloads than new episodes means the limit applies.
    if to_download != new_episodes {
        message.push_str(&format!(
            ", downloading {}",
            to_download.to_string().green()
        ));
    }
    if repairs > 0 {
        message.push_str(&format!(", repairing {}", repairs.to_string().green()));
    }
    message
}

fn stuck_partial_file_message(path: &Path) -> String {
    format!(
        "Could not remove the leftover partial file {}; its episode cannot be downloaded until it is deleted",
        path.display()
    )
}

fn unreadable_metadata_message(path: &Path, error: &str) -> String {
    format!(
        "Could not read episode metadata {} ({})",
        path.display(),
        error
    )
}

fn unverifiable_audio_message(audio_filename: &str, error: &str) -> String {
    format!(
        "Could not read {} to check it against its recorded hash ({})",
        audio_filename, error
    )
}

fn damage_message(
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

fn truncate_title(title: &str, max_len: usize) -> String {
    if title.len() <= max_len {
        title.to_string()
    } else {
        format!("{}...", &title[..max_len.saturating_sub(3)])
    }
}

/// Calculate available width for episode title in progress bar
/// Layout: "  📥 [{bar:30}] XX.XX MiB/XX.XX MiB [idx/total] title"
fn available_title_width(index_width: usize) -> usize {
    let term_width = console::Term::stdout().size().1 as usize;

    // Fixed parts:
    // - "  " prefix: 2
    // - emoji + space: 4 (📥 + space, accounting for unicode width)
    // - "[" + "]": 2
    // - bar: 30
    // - " ": 1
    // - bytes display "XX.XX MiB/XX.XX MiB": ~21 (max reasonable)
    // - " ": 1
    // - index "[idx/total] ": index_width + 4 brackets/slash + 1 space
    let fixed_width = 2 + 4 + 2 + 30 + 1 + 21 + 1 + index_width + 4 + 1;

    term_width.saturating_sub(fixed_width).max(20) // minimum 20 chars for title
}

fn sync_options(args: &Args) -> SyncOptions {
    SyncOptions {
        limit: args.limit,
        max_concurrent: args.concurrent,
        // --repair includes everything --verify does.
        audio_check: if args.repair {
            AudioCheck::Repair
        } else if args.verify {
            AudioCheck::Verify
        } else {
            AudioCheck::Collisions
        },
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    println!(
        "\n{}{} {}\n",
        MICROPHONE,
        "podpull".bold().magenta(),
        "- Podcast Downloader".dimmed()
    );

    let client = ReqwestClient::new();

    let options = sync_options(&args);

    let reporter: SharedProgressReporter = if args.quiet {
        NoopReporter::shared()
    } else {
        Arc::new(IndicatifReporter::new())
    };

    let result = sync_podcast(&client, &args.feed, &args.output_dir, &options, reporter)
        .await
        .context("Failed to sync podcast")?;

    // Quiet mode suppresses progress, not problems: scripts and cron jobs
    // still see what needs attention, on stderr.
    if args.quiet {
        write_problem_lists(&result, &mut std::io::stderr())
            .context("write the problem lists to stderr")?;
    } else {
        println!(
            "\n{PARTY}{} {}",
            "Sync complete:".bold().green(),
            completion_summary(&result)
        );
        write_problem_lists(&result, &mut std::io::stdout())
            .context("write the problem lists to stdout")?;
        println!(
            "\n{FOLDER}Output: {}\n",
            args.output_dir.display().to_string().cyan()
        );
    }

    let code = exit_code(&result);
    if code != 0 {
        std::process::exit(code);
    }

    Ok(())
}

/// Exit status of a finished sync
///
/// 1 when downloads failed and nothing was downloaded, repaired or recorded
/// as already stored; otherwise 2 when downloads failed, damaged or missing
/// audio was found, or a warning was reported.
fn exit_code(result: &SyncResult) -> i32 {
    let succeeded = result.downloaded + result.repaired + result.adopted;
    let warned = !result.stuck_partial_files.is_empty()
        || !result.unreadable_metadata.is_empty()
        || !result.unverifiable_audio.is_empty();
    let failed = !result.failed_episodes.is_empty();
    if failed && succeeded == 0 {
        1
    } else if failed || !result.damaged.is_empty() || warned {
        2
    } else {
        0
    }
}

/// Write the failed and damaged episodes of a sync, if there are any
fn write_problem_lists(result: &SyncResult, out: &mut impl Write) -> std::io::Result<()> {
    if !result.failed_episodes.is_empty() {
        writeln!(out, "\n{}", "Failed episodes:".red().bold())?;
        for failed in &result.failed_episodes {
            writeln!(
                out,
                "  {}{} - {}",
                CROSS,
                failed.title.yellow(),
                failed.error.dimmed()
            )?;
        }
    }

    if !result.damaged.is_empty() {
        writeln!(out, "\n{}", "Damaged episodes:".red().bold())?;
        for damaged in &result.damaged {
            writeln!(
                out,
                "  {}{}",
                CROSS,
                damage_message(
                    &damaged.episode_title,
                    &damaged.audio_filename,
                    damaged.kind,
                    damaged.remedy
                )
            )?;
        }
    }

    let warnings: Vec<String> = result
        .stuck_partial_files
        .iter()
        .map(|path| stuck_partial_file_message(path))
        .chain(
            result
                .unreadable_metadata
                .iter()
                .map(|unreadable| unreadable_metadata_message(&unreadable.path, &unreadable.error)),
        )
        .chain(result.unverifiable_audio.iter().map(|unverifiable| {
            unverifiable_audio_message(&unverifiable.audio_filename, &unverifiable.error)
        }))
        .collect();
    if !warnings.is_empty() {
        writeln!(out, "\n{}", "Warnings:".yellow().bold())?;
        for warning in warnings {
            writeln!(out, "  {}{}", WARNING, warning)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use podpull::{DamagedAudio, FailedEpisode, UnverifiableAudio};

    #[test]
    fn plan_message_shows_only_new_episodes_without_limit_or_repairs() {
        colored::control::set_override(false);
        assert_eq!(plan_message("Show", 10, 2, 2, 0), "Show • 10 total, 2 new");
    }

    #[test]
    fn plan_message_shows_the_limit() {
        colored::control::set_override(false);
        assert_eq!(
            plan_message("Show", 10, 2, 1, 0),
            "Show • 10 total, 2 new, downloading 1"
        );
    }

    #[test]
    fn plan_message_shows_repairs_apart_from_the_limit() {
        colored::control::set_override(false);
        assert_eq!(
            plan_message("Show", 10, 2, 2, 1),
            "Show • 10 total, 2 new, repairing 1"
        );
        assert_eq!(
            plan_message("Show", 10, 2, 1, 1),
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

    #[test]
    fn reporter_accepts_collision_events() {
        // indicatif draws to the terminal, so this only checks that every
        // match arm runs; the message texts are asserted by the helper tests.
        let reporter = IndicatifReporter::new();

        reporter.report(ProgressEvent::PartialFileStuck {
            path: PathBuf::from("/podcasts/2024-01-15-Episode.mp3.partial"),
        });
        reporter.report(ProgressEvent::MetadataUnreadable {
            path: PathBuf::from("/podcasts/2024-01-15-Episode.json"),
            error: "EOF while parsing".to_string(),
        });
        reporter.report(ProgressEvent::VerifyingStoredAudio {
            audio_filename: "2024-12-19-Sega Nomad.mp3".to_string(),
        });
        reporter.report(ProgressEvent::StoredAudioUnverifiable {
            audio_filename: "2024-12-19-Sega Nomad.mp3".to_string(),
            error: "Permission denied".to_string(),
        });
        reporter.report(ProgressEvent::StoredAudioDamaged {
            episode_title: "Sega Nomad".to_string(),
            audio_filename: "2024-12-19-Sega Nomad.mp3".to_string(),
            kind: DamageKind::Mismatch,
            remedy: DamageRemedy::Repairing,
        });
        reporter.report(ProgressEvent::EpisodeAlreadyStored {
            download_id: 0,
            episode_title: "Sega Nomad".to_string(),
            audio_filename: "2024-12-19-Sega Nomad.mp3".to_string(),
        });
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
    fn unverifiable_audio_message_names_file_and_error() {
        assert_eq!(
            unverifiable_audio_message("2024-12-19-Sega Nomad.mp3", "Permission denied"),
            "Could not read 2024-12-19-Sega Nomad.mp3 to check it against its \
             recorded hash (Permission denied)"
        );
    }

    fn result(downloaded: usize, failed: usize, damaged: usize) -> SyncResult {
        SyncResult {
            downloaded,
            failed_episodes: (0..failed)
                .map(|n| FailedEpisode {
                    title: format!("Episode {}", n),
                    error: "HTTP error 404".to_string(),
                })
                .collect(),
            damaged: (0..damaged)
                .map(|n| DamagedAudio {
                    episode_title: format!("Damaged {}", n),
                    audio_filename: format!("2024-01-0{}-Damaged.mp3", n + 1),
                    kind: DamageKind::Mismatch,
                    remedy: DamageRemedy::RepairAvailable,
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn exit_code_is_zero_without_problems() {
        assert_eq!(exit_code(&result(3, 0, 0)), 0);
        assert_eq!(exit_code(&result(0, 0, 0)), 0);
    }

    #[test]
    fn exit_code_is_one_when_nothing_could_be_downloaded() {
        assert_eq!(exit_code(&result(0, 2, 0)), 1);
    }

    #[test]
    fn exit_code_counts_repairs_and_recorded_guids_as_success() {
        let mut repaired = result(0, 1, 0);
        repaired.repaired = 1;
        let mut adopted = result(0, 1, 0);
        adopted.adopted = 1;

        assert_eq!(exit_code(&repaired), 2);
        assert_eq!(exit_code(&adopted), 2);
    }

    #[test]
    fn completion_summary_lists_repairs_recorded_guids_and_the_limit() {
        colored::control::set_override(false);

        let summary = completion_summary(&SyncResult {
            downloaded: 4,
            existing: 5,
            repaired: 2,
            adopted: 1,
            limited: 3,
            ..Default::default()
        });

        assert_eq!(
            summary,
            "4 downloaded, 5 existing, 2 repaired, 1 already stored, 3 limited, 0 failed"
        );
    }

    #[test]
    fn exit_code_is_two_for_warnings() {
        let mut stuck = result(0, 0, 0);
        stuck.stuck_partial_files = vec![PathBuf::from("/podcasts/a.mp3.partial")];
        let mut unverifiable = result(0, 0, 0);
        unverifiable.unverifiable_audio = vec![UnverifiableAudio {
            audio_filename: "a.mp3".to_string(),
            error: "denied".to_string(),
        }];

        assert_eq!(exit_code(&stuck), 2);
        assert_eq!(exit_code(&unverifiable), 2);
    }

    #[test]
    fn problem_lists_name_warnings() {
        colored::control::set_override(false);
        let mut with_warnings = result(1, 0, 0);
        with_warnings.stuck_partial_files = vec![PathBuf::from("/podcasts/a.mp3.partial")];
        with_warnings.unverifiable_audio = vec![UnverifiableAudio {
            audio_filename: "b.mp3".to_string(),
            error: "Permission denied".to_string(),
        }];
        let mut out = Vec::new();

        write_problem_lists(&with_warnings, &mut out).unwrap();

        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("Warnings:"));
        assert!(out.contains(&stuck_partial_file_message(Path::new(
            "/podcasts/a.mp3.partial"
        ))));
        assert!(out.contains(&unverifiable_audio_message("b.mp3", "Permission denied")));
    }

    #[test]
    fn exit_code_is_two_for_partial_failure_or_damage() {
        assert_eq!(exit_code(&result(3, 1, 0)), 2);
        assert_eq!(exit_code(&result(3, 0, 1)), 2);
        assert_eq!(exit_code(&result(0, 0, 1)), 2);
    }

    #[test]
    fn problem_lists_name_failed_and_damaged_episodes() {
        colored::control::set_override(false);
        let mut out = Vec::new();

        write_problem_lists(&result(1, 1, 1), &mut out).unwrap();

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

        write_problem_lists(&result(3, 0, 0), &mut out).unwrap();

        assert!(out.is_empty());
    }

    #[test]
    fn sync_options_carry_the_command_line_arguments() {
        let args = Args::try_parse_from([
            "podpull", "-c", "5", "-l", "10", "--repair", "feed.xml", "out",
        ])
        .unwrap();

        let options = sync_options(&args);

        assert_eq!(options.limit, Some(10));
        assert_eq!(options.max_concurrent, 5);
        assert_eq!(options.audio_check, AudioCheck::Repair);
    }

    #[test]
    fn sync_options_check_the_archive_for_verify() {
        let args = Args::try_parse_from(["podpull", "--verify", "feed.xml", "out"]).unwrap();
        assert_eq!(sync_options(&args).audio_check, AudioCheck::Verify);
    }

    #[test]
    fn sync_options_check_collisions_by_default() {
        let args = Args::try_parse_from(["podpull", "feed.xml", "out"]).unwrap();
        assert_eq!(sync_options(&args).audio_check, AudioCheck::Collisions);
    }

    #[test]
    fn repair_takes_precedence_over_verify() {
        let args =
            Args::try_parse_from(["podpull", "--verify", "--repair", "feed.xml", "out"]).unwrap();
        assert_eq!(sync_options(&args).audio_check, AudioCheck::Repair);
    }

    #[test]
    fn concurrency_of_zero_is_rejected() {
        assert!(Args::try_parse_from(["podpull", "-c", "0", "feed.xml", "out"]).is_err());
    }

    #[test]
    fn repair_flag_is_off_by_default() {
        let args = Args::try_parse_from(["podpull", "feed.xml", "out"]).unwrap();
        assert!(!args.repair);

        let args = Args::try_parse_from(["podpull", "--repair", "feed.xml", "out"]).unwrap();
        assert!(args.repair);
    }
}
