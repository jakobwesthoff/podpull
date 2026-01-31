use std::collections::HashMap;
use std::path::PathBuf;
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
static SEARCH: Emoji<'_, '_> = Emoji("🔍 ", "[~] ");
static HEADPHONES: Emoji<'_, '_> = Emoji("🎧 ", "[i] ");
static DOWNLOAD: Emoji<'_, '_> = Emoji("📥 ", "[v] ");
static SUCCESS: Emoji<'_, '_> = Emoji("✅ ", "[+] ");
static FAILURE: Emoji<'_, '_> = Emoji("❌ ", "[!] ");
static PARTY: Emoji<'_, '_> = Emoji("🎉 ", "[*] ");
static FOLDER: Emoji<'_, '_> = Emoji("📁 ", "");
static CROSS: Emoji<'_, '_> = Emoji("✗ ", "x ");

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
                "  {DOWNLOAD}[{{bar:30.cyan/blue}}] {{bytes}}/{{total_bytes}} {{wide_msg}}"
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
                    .set_message(format!("{SEARCH}Fetching feed: {}", url.cyan()));
            }

            ProgressEvent::FeedParsed {
                podcast_title,
                total_episodes,
                new_episodes,
            } => {
                self.main_bar.set_message(format!(
                    "{HEADPHONES}{} • {} episodes total, {} new",
                    podcast_title.bold().green(),
                    total_episodes.to_string().cyan(),
                    new_episodes.to_string().yellow()
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
                bar.set_message(format!(
                    "[{}/{}] {}",
                    (episode_index + 1).to_string().cyan(),
                    total_to_download.to_string().cyan(),
                    truncate_title(&episode_title, 40)
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
                bar.set_message(format!(
                    "{SUCCESS}{}",
                    truncate_title(&episode_title, 40).green()
                ));
                self.finish_bar(download_id);
            }

            ProgressEvent::DownloadFailed {
                download_id,
                episode_title,
                error,
            } => {
                let bar = self.get_or_create_bar(download_id);
                bar.abandon_with_message(format!(
                    "{FAILURE}{} - {}",
                    truncate_title(&episode_title, 30).red(),
                    error.red()
                ));
                self.finish_bar(download_id);
            }

            ProgressEvent::SyncCompleted {
                downloaded_count,
                skipped_count,
                failed_count,
            } => {
                self.main_bar.finish_and_clear();
                println!(
                    "\n{PARTY}{} {} downloaded, {} skipped, {} failed",
                    "Sync complete:".bold().green(),
                    downloaded_count.to_string().green().bold(),
                    skipped_count.to_string().yellow(),
                    if failed_count > 0 {
                        failed_count.to_string().red().bold()
                    } else {
                        failed_count.to_string().green()
                    }
                );
            }
        }
    }
}

fn truncate_title(title: &str, max_len: usize) -> String {
    if title.len() <= max_len {
        title.to_string()
    } else {
        format!("{}...", &title[..max_len.saturating_sub(3)])
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

    let options = SyncOptions {
        limit: args.limit,
        max_concurrent: args.concurrent,
        continue_on_error: true,
    };

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
