// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use clap::Parser;
use colored::Colorize;
use console::Emoji;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

use podpull::{
    NoopReporter, ProgressEvent, ProgressReporter, ReqwestClient, SharedProgressReporter,
    SyncOptions, sync_podcast,
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
    #[arg(short = 'c', long, default_value = "3")]
    concurrent: usize,

    /// Maximum number of episodes to download
    #[arg(short, long)]
    limit: Option<usize>,

    /// Quiet mode - suppress progress output
    #[arg(short, long)]
    quiet: bool,

    /// Download an existing episode again when a new episode would take its
    /// filename and its audio no longer matches the hash recorded at download
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
            } => {
                // Reset to spinner style after scanning
                let main_style = ProgressStyle::default_bar()
                    .template("{spinner:.green} {wide_msg}")
                    .unwrap();
                self.main_bar.set_style(main_style);
                if new_episodes == to_download {
                    // No limit applied or limit >= new
                    self.main_bar.set_message(format!(
                        "{HEADPHONES}{} • {} total, {} new",
                        podcast_title.bold().green(),
                        total_episodes.to_string().cyan(),
                        new_episodes.to_string().yellow()
                    ));
                } else {
                    // Limit applied
                    self.main_bar.set_message(format!(
                        "{HEADPHONES}{} • {} total, {} new, downloading {}",
                        podcast_title.bold().green(),
                        total_episodes.to_string().cyan(),
                        new_episodes.to_string().yellow(),
                        to_download.to_string().green()
                    ));
                }
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
                ..
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

            ProgressEvent::Finalizing { .. } => {
                // Silent - the rename is fast
            }

            ProgressEvent::HashingCompleted { .. } => {
                // Silent - hashing happens during download
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

            ProgressEvent::StoredAudioMismatch {
                episode_title,
                audio_filename,
                repairing,
            } => {
                let _ = self.multi.println(format!(
                    "{WARNING}{}",
                    stored_audio_mismatch_message(&episode_title, &audio_filename, repairing)
                        .yellow()
                ));
            }

            ProgressEvent::SyncCompleted {
                downloaded_count,
                existing_count,
                limited_count,
                failed_count,
                not_started_count,
            } => {
                self.main_bar.finish_and_clear();

                let mut parts = vec![
                    format!("{} downloaded", downloaded_count.to_string().green().bold()),
                    format!("{} existing", existing_count.to_string().yellow()),
                ];

                if limited_count > 0 {
                    parts.push(format!("{} limited", limited_count.to_string().cyan()));
                }

                if not_started_count > 0 {
                    parts.push(format!(
                        "{} not started",
                        not_started_count.to_string().yellow()
                    ));
                }

                parts.push(if failed_count > 0 {
                    format!("{} failed", failed_count.to_string().red().bold())
                } else {
                    format!("{} failed", failed_count.to_string().green())
                });

                println!(
                    "\n{PARTY}{} {}",
                    "Sync complete:".bold().green(),
                    parts.join(", ")
                );
            }
        }
    }
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

fn stored_audio_mismatch_message(
    episode_title: &str,
    audio_filename: &str,
    repairing: bool,
) -> String {
    format!(
        "Audio of \"{}\" ({}) does not match the hash recorded when it was downloaded{}",
        episode_title,
        audio_filename,
        if repairing {
            "; downloading it again"
        } else {
            ""
        }
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
        continue_on_error: true,
        repair_mismatched_audio: args.repair,
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

    if !args.quiet && !result.failed_episodes.is_empty() {
        println!("\n{}", "Failed episodes:".red().bold());
        for (title, error) in &result.failed_episodes {
            println!(
                "  {}{} - {}",
                CROSS,
                title.yellow(),
                error.to_string().dimmed()
            );
        }
    }

    if !args.quiet {
        println!(
            "\n{FOLDER}Output: {}\n",
            args.output_dir.display().to_string().cyan()
        );
    }

    if result.failed > 0 && result.downloaded == 0 {
        std::process::exit(1);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn reporter_prints_collision_warnings() {
        let reporter = IndicatifReporter::new();

        reporter.report(ProgressEvent::PartialFileStuck {
            path: PathBuf::from("/podcasts/2024-01-15-Episode.mp3.partial"),
        });
        reporter.report(ProgressEvent::MetadataUnreadable {
            path: PathBuf::from("/podcasts/2024-01-15-Episode.json"),
            error: "EOF while parsing".to_string(),
        });
        reporter.report(ProgressEvent::StoredAudioMismatch {
            episode_title: "Sega Nomad".to_string(),
            audio_filename: "2024-12-19-Sega Nomad.mp3".to_string(),
            repairing: true,
        });
    }

    #[test]
    fn stored_audio_mismatch_message_names_episode_and_file() {
        assert_eq!(
            stored_audio_mismatch_message("Sega Nomad", "2024-12-19-Sega Nomad.mp3", false),
            "Audio of \"Sega Nomad\" (2024-12-19-Sega Nomad.mp3) does not match \
             the hash recorded when it was downloaded"
        );
    }

    #[test]
    fn stored_audio_mismatch_message_announces_repair() {
        assert_eq!(
            stored_audio_mismatch_message("Sega Nomad", "2024-12-19-Sega Nomad.mp3", true),
            "Audio of \"Sega Nomad\" (2024-12-19-Sega Nomad.mp3) does not match \
             the hash recorded when it was downloaded; downloading it again"
        );
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
        assert!(options.continue_on_error);
        assert!(options.repair_mismatched_audio);
    }

    #[test]
    fn repair_flag_is_off_by_default() {
        let args = Args::try_parse_from(["podpull", "feed.xml", "out"]).unwrap();
        assert!(!args.repair);

        let args = Args::try_parse_from(["podpull", "--repair", "feed.xml", "out"]).unwrap();
        assert!(args.repair);
    }
}
