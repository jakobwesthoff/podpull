// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::sync::Mutex;

use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};

use podpull::{ProgressEvent, ProgressReporter};

use super::messages::*;
use super::shown::{self, Safe};
use super::*;

/// Progress reporter using indicatif for terminal output
pub(crate) struct IndicatifReporter {
    multi: MultiProgress,
    bars: Mutex<HashMap<usize, ProgressBar>>,
    /// Slots whose download already printed the warning about a slow disk
    slow_warned: Mutex<HashSet<usize>>,
    main_bar: ProgressBar,
    /// Where the lines of warnings go, outside what indicatif tracks
    warnings: Mutex<Box<dyn Write + Send>>,
    env: StreamEnv,
}

impl IndicatifReporter {
    pub(crate) fn new(env: StreamEnv) -> Self {
        Self::with_target(
            env,
            ProgressDrawTarget::stderr(),
            Box::new(std::io::stderr()),
        )
    }

    /// A reporter drawing on `target`, with warning lines going to
    /// `warnings`, which has to show them on the same screen as the target
    pub(crate) fn with_target(
        env: StreamEnv,
        target: ProgressDrawTarget,
        warnings: Box<dyn Write + Send>,
    ) -> Self {
        let multi = MultiProgress::with_draw_target(target);

        let main_style = ProgressStyle::default_bar()
            .template("{spinner:.green} {wide_msg}")
            .unwrap();

        let main_bar = multi.add(ProgressBar::new_spinner());
        main_bar.set_style(main_style);
        main_bar.enable_steady_tick(std::time::Duration::from_millis(100));

        Self {
            multi,
            bars: Mutex::new(HashMap::new()),
            slow_warned: Mutex::new(HashSet::new()),
            main_bar,
            warnings: Mutex::new(warnings),
            env,
        }
    }

    fn get_or_create_bar(&self, download_id: usize) -> ProgressBar {
        let mut bars = self.bars.lock().unwrap();

        if let Some(bar) = bars.get(&download_id) {
            return bar.clone();
        }

        let style = ProgressStyle::default_bar()
            .template(&format!(
                "  {}[{{bar:30.cyan/blue}}] {{bytes}}/{{total_bytes}} {{wide_msg}}",
                self.env.mark(Mark::Saving)
            ))
            .unwrap()
            .progress_chars("█▓░");

        let bar = self.multi.add(ProgressBar::new(0));
        bar.set_style(style);
        bars.insert(download_id, bar.clone());
        bar
    }

    /// Whether the warning about a slow disk is still to be printed for the
    /// download in this slot
    ///
    /// A slow share can hold up several syncs of one download, and the
    /// warning says the same each time.
    fn first_slow_warning(&self, download_id: usize) -> bool {
        self.slow_warned
            .lock()
            .expect("no code panics while holding the warned slots")
            .insert(download_id)
    }

    /// Show an episode's place in the queue, a status and its title on its
    /// bar
    fn show_episode_status(
        &self,
        download_id: usize,
        episode_index: usize,
        total_to_download: usize,
        status: &str,
        episode_title: &str,
    ) {
        self.get_or_create_bar(download_id)
            .set_message(for_progress(&episode_message(
                episode_index,
                total_to_download,
                status,
                &shown::name(episode_title),
                &self.env,
            )));
    }

    /// Print a warning line above the bars
    ///
    /// The line is written outside the lines indicatif keeps track of. It
    /// cannot be one of them: indicatif counts a character followed by
    /// U+FE0F, as the symbol of a warning is, as one column where a
    /// terminal draws two. The line would wrap, indicatif would count the
    /// rows it drew one short and the next redraw would leave a copy of the
    /// bars behind.
    ///
    /// Nothing is written when the bars are not drawn, as they are not when
    /// stderr is no terminal: the end of the run lists the warnings again.
    fn warn(&self, text: String) {
        if self.multi.is_hidden() {
            return;
        }
        let line = format!("{}{}", self.env.mark(Mark::Warning), text);
        self.multi.suspend(|| {
            let mut warnings = self
                .warnings
                .lock()
                .expect("no code panics while holding the warning writer");
            let _ = writeln!(warnings, "{line}");
            let _ = warnings.flush();
        });
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
                self.main_bar.set_message(for_progress(&format!(
                    "{}Fetching feed: {}",
                    self.env.mark(Mark::Globe),
                    self.env.cyan(feed_source(&url))
                )));
            }

            ProgressEvent::ParsingFeed { source } => {
                self.main_bar.set_message(for_progress(&format!(
                    "{}Parsing feed: {}",
                    self.env.mark(Mark::Scroll),
                    self.env.cyan(feed_source(&source))
                )));
            }

            ProgressEvent::ScanningDirectory {
                files_scanned,
                total_files,
            } => {
                if total_files == 0 {
                    self.main_bar.set_message(for_progress(&format!(
                        "{}Scanning existing episodes...",
                        self.env.mark(Mark::Search)
                    )));
                } else {
                    // Switch to progress bar style for scanning
                    if files_scanned == 0 {
                        let scan_style = ProgressStyle::default_bar()
                            .template(&format!(
                                "{{spinner:.green}} {}Scanning existing episodes... [{{bar:30.cyan/blue}}] {{pos}}/{{len}}", self.env.mark(Mark::Search)
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
                self.main_bar.set_message(for_progress(&format!(
                    "{}{}",
                    self.env.mark(Mark::Headphones),
                    plan_message(
                        &shown::name(&podcast_title),
                        total_episodes,
                        new_episodes,
                        to_download,
                        repairs,
                        &self.env
                    )
                )));
            }

            ProgressEvent::DownloadStarting {
                download_id,
                episode_title,
                episode_index,
                total_to_download,
                content_length,
            } => {
                self.slow_warned
                    .lock()
                    .expect("no code panics while holding the warned slots")
                    .remove(&download_id);
                let bar = self.get_or_create_bar(download_id);
                bar.set_length(content_length.unwrap_or(0));
                bar.set_position(0);
                self.show_episode_status(
                    download_id,
                    episode_index,
                    total_to_download,
                    "",
                    &episode_title,
                );
            }

            ProgressEvent::DownloadSyncing {
                download_id,
                episode_title,
                episode_index,
                total_to_download,
            } => {
                self.show_episode_status(
                    download_id,
                    episode_index,
                    total_to_download,
                    SYNCING_STATUS,
                    &episode_title,
                );
            }

            ProgressEvent::DownloadSyncSlow {
                download_id,
                episode_title,
                episode_index,
                total_to_download,
                threshold,
            } => {
                self.show_episode_status(
                    download_id,
                    episode_index,
                    total_to_download,
                    SLOW_SYNC_STATUS,
                    &episode_title,
                );
                // The bar text is cut at the terminal width, so the reason
                // for the wait goes on a line of its own above the bars.
                if self.first_slow_warning(download_id) {
                    self.warn(
                        self.env
                            .yellow(slow_sync_warning(&shown::name(&episode_title), threshold)),
                    );
                }
            }

            ProgressEvent::DownloadSynced {
                download_id,
                episode_title,
                episode_index,
                total_to_download,
            } => {
                self.show_episode_status(
                    download_id,
                    episode_index,
                    total_to_download,
                    "",
                    &episode_title,
                );
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
                bar.set_message(for_progress(&format!(
                    "{}{}",
                    self.env.mark(Mark::Success),
                    self.env
                        .green(truncate_title(&shown::name(&episode_title), title_width))
                )));
                self.finish_bar(download_id);
            }

            ProgressEvent::DownloadFailed {
                download_id,
                episode_title,
                reason,
            } => {
                let bar = self.get_or_create_bar(download_id);
                // Reserve space for " - " and some error text (at least 30 chars)
                let title_width = available_title_width(0).saturating_sub(3 + 30);
                bar.abandon_with_message(for_progress(&format!(
                    "{}{} - {}",
                    self.env.mark(Mark::Failure),
                    self.env.red(truncate_title(
                        &shown::name(&episode_title),
                        title_width.max(MIN_TITLE_WIDTH)
                    )),
                    self.env.red(shown::text(&reason.root))
                )));
                self.finish_bar(download_id);
            }

            ProgressEvent::PartialFilesCleanedUp { count } => {
                if count > 0 {
                    self.main_bar.set_message(for_progress(&format!(
                        "{}Cleaned up {} interrupted download{}",
                        self.env.mark(Mark::Broom),
                        self.env.yellow(count),
                        if count == 1 { "" } else { "s" }
                    )));
                }
            }

            ProgressEvent::EpisodeAlreadyStored {
                download_id,
                episode_title,
                audio_filename,
            } => {
                let bar = self.get_or_create_bar(download_id);
                bar.set_message(for_progress(&format!(
                    "{}{}",
                    self.env.mark(Mark::Success),
                    self.env.green(already_stored_message(
                        &shown::name(&episode_title),
                        &shown::name(&audio_filename)
                    ))
                )));
                self.finish_bar(download_id);
            }

            ProgressEvent::PartialFileStuck { path } => {
                self.warn(
                    self.env
                        .yellow(stuck_partial_file_message(&shown::path(&path))),
                );
            }

            ProgressEvent::MetadataUnreadable { path, reason } => {
                // Printed above the progress bars so the warning outlives
                // the transient status line.
                self.warn(self.env.yellow(unreadable_metadata_message(
                    &shown::path(&path),
                    &shown::text(&reason),
                )));
            }

            ProgressEvent::VerifyingStoredAudio {
                audio_filename,
                position,
                total,
            } => {
                // Hashing a file on a network share takes a while, so the
                // main line shows which file of how many and how far it is.
                let check_style = ProgressStyle::default_bar()
                    .template(
                        "{spinner:.green} {prefix} [{bar:30.cyan/blue}] {bytes}/{total_bytes} {wide_msg}",
                    )
                    .unwrap()
                    .progress_chars("█▓░");
                // Every setter may redraw the line, so the bytes of the
                // previous file go first and the new style last.
                self.main_bar.set_position(0);
                self.main_bar.set_length(0);
                self.main_bar.set_prefix(for_progress(&format!(
                    "{}{}",
                    self.env.mark(Mark::Search),
                    checking_prefix(position, total, &self.env)
                )));
                self.main_bar
                    .set_message(for_progress(&self.env.cyan(shown::name(&audio_filename))));
                self.main_bar.set_style(check_style);
            }

            ProgressEvent::HashingProgress {
                bytes_hashed,
                total_bytes,
            } => {
                self.main_bar.set_length(total_bytes);
                self.main_bar.set_position(bytes_hashed);
            }

            ProgressEvent::StoredAudioUnverifiable {
                audio_filename,
                reason,
            } => {
                self.warn(self.env.yellow(unverifiable_audio_message(
                    &shown::name(&audio_filename),
                    &shown::text(&reason),
                )));
            }

            ProgressEvent::StoredAudioDamaged {
                episode_title,
                audio_filename,
                kind,
                remedy,
            } => {
                self.warn(self.env.yellow(damage_message(
                    &shown::name(&episode_title),
                    &shown::name(&audio_filename),
                    kind,
                    remedy,
                )));
            }

            ProgressEvent::SyncCompleted => {
                self.main_bar.finish_and_clear();
            }
        }
    }
}

/// Fewest characters of an episode title a progress line keeps
const MIN_TITLE_WIDTH: usize = 20;

/// Status shown on an episode's bar while podpull waits for the file system
/// to confirm a write
const SYNCING_STATUS: &str = "Writing to disk: ";

/// Status shown on an episode's bar once that wait has become slow
const SLOW_SYNC_STATUS: &str = "Slow disk, still writing: ";

/// Text of an episode's bar: its place in the queue, a status and its title
///
/// The title is cut to what the terminal leaves after the status, so the
/// status is never the part that gets lost.
fn episode_message(
    episode_index: usize,
    total_to_download: usize,
    status: &str,
    episode_title: &Safe,
    env: &StreamEnv,
) -> String {
    let index_width = (episode_index + 1).to_string().len() + total_to_download.to_string().len();
    let title_width = available_title_width(index_width)
        .saturating_sub(status.len())
        .max(MIN_TITLE_WIDTH);
    format!(
        "[{}/{}] {}{}",
        env.cyan(episode_index + 1),
        env.cyan(total_to_download),
        status,
        truncate_title(episode_title, title_width)
    )
}

/// The source of a feed as a line shows it: a URL as it parses, a file as a
/// name
fn feed_source(source: &str) -> Safe {
    if source.starts_with("http://") || source.starts_with("https://") {
        shown::url(source)
    } else {
        shown::name(source)
    }
}

/// `text` as a progress bar can show it
///
/// indicatif measures a character followed by U+FE0F (an emoji variation
/// selector) as one column and a terminal draws two, so the line it pads to
/// the terminal width is one column too wide. It wraps, indicatif miscounts
/// the rows it drew and every redraw leaves a copy of the bar behind.
/// Episode titles from a feed can hold such a character, so the selector is
/// dropped from every text a bar shows.
fn for_progress(text: &str) -> String {
    text.chars()
        .filter(|character| !matches!(character, '\u{FE0F}' | '\u{FE0E}'))
        .collect()
}

/// Shorten a title to `max_len` terminal columns, ending in "..." when it
/// had to be cut
///
/// Cuts between characters and counts the columns the terminal shows, since
/// titles hold multi-byte characters such as umlauts and dashes.
fn truncate_title(title: &Safe, max_len: usize) -> String {
    console::truncate_str(title.as_str(), max_len, "...").into_owned()
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

    term_width.saturating_sub(fixed_width).max(MIN_TITLE_WIDTH)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::virtual_terminal::VirtualTerminal;
    use podpull::FailureReason;

    const PLAIN: StreamEnv = StreamEnv {
        color: false,
        utf8: true,
    };
    use std::path::PathBuf;

    #[test]
    fn the_slow_disk_warning_is_printed_once_per_download() {
        let reporter = IndicatifReporter::new(PLAIN);

        assert!(reporter.first_slow_warning(0));
        assert!(!reporter.first_slow_warning(0));
        assert!(reporter.first_slow_warning(1), "another download");

        reporter.report(ProgressEvent::DownloadStarting {
            download_id: 0,
            episode_title: "Sega Nomad".to_string(),
            episode_index: 1,
            total_to_download: 2,
            content_length: None,
        });
        assert!(
            reporter.first_slow_warning(0),
            "the next download of the slot"
        );
    }

    #[test]
    fn truncate_title_keeps_a_title_that_fits() {
        assert_eq!(truncate_title(&shown::name("Sega Nomad"), 10), "Sega Nomad");
        assert_eq!(truncate_title(&shown::name("Größe"), 5), "Größe");
    }

    #[test]
    fn truncate_title_cuts_ascii_with_an_ellipsis() {
        assert_eq!(
            truncate_title(&shown::name("Sega Nomad Special"), 10),
            "Sega No..."
        );
    }

    #[test]
    fn truncate_title_cuts_before_a_multi_byte_character_instead_of_inside_it() {
        // The cut at byte 2 falls inside the first "ä" (bytes 1 and 2).
        assert_eq!(truncate_title(&shown::name("Mädchen ääää"), 5), "Mä...");
        // The cut at byte 6 falls inside the en dash (bytes 5 to 7).
        assert_eq!(
            truncate_title(&shown::name("Trek – Folge 499"), 9),
            "Trek –..."
        );
    }

    #[test]
    fn episode_message_puts_the_status_between_the_place_and_the_title() {
        assert_eq!(
            episode_message(0, 3, "", &shown::name("Sega Nomad"), &PLAIN),
            "[1/3] Sega Nomad"
        );
        assert_eq!(
            episode_message(0, 3, SYNCING_STATUS, &shown::name("Sega Nomad"), &PLAIN),
            "[1/3] Writing to disk: Sega Nomad"
        );
        assert_eq!(
            episode_message(1, 3, SLOW_SYNC_STATUS, &shown::name("Sega Nomad"), &PLAIN),
            "[2/3] Slow disk, still writing: Sega Nomad"
        );
    }

    #[test]
    fn episode_message_cuts_a_long_title_and_keeps_the_status() {
        let title = "x".repeat(500);

        let message = episode_message(0, 3, SLOW_SYNC_STATUS, &shown::name(&title), &PLAIN);

        assert!(message.starts_with("[1/3] Slow disk, still writing: xxx"));
        assert!(message.ends_with("..."));
    }

    #[test]
    fn reporter_accepts_the_events_of_a_run_without_collisions() {
        // indicatif draws to the terminal, so this only checks that every
        // match arm runs.
        let reporter = IndicatifReporter::new(PLAIN);

        reporter.report(ProgressEvent::FetchingFeed {
            url: "https://example.com/feed.xml".to_string(),
        });
        reporter.report(ProgressEvent::ParsingFeed {
            source: "https://example.com/feed.xml".to_string(),
        });
        reporter.report(ProgressEvent::ScanningDirectory {
            files_scanned: 0,
            total_files: 0,
        });
        reporter.report(ProgressEvent::ScanningDirectory {
            files_scanned: 0,
            total_files: 2,
        });
        reporter.report(ProgressEvent::ScanningDirectory {
            files_scanned: 1,
            total_files: 2,
        });
        reporter.report(ProgressEvent::SyncPlanReady {
            podcast_title: "Show".to_string(),
            total_episodes: 3,
            new_episodes: 2,
            to_download: 2,
            repairs: 0,
        });
        reporter.report(ProgressEvent::PartialFilesCleanedUp { count: 1 });
        reporter.report(ProgressEvent::DownloadProgress {
            download_id: 0,
            bytes_downloaded: 5,
            total_bytes: Some(10),
        });
        reporter.report(ProgressEvent::DownloadCompleted {
            download_id: 0,
            episode_title: "Sega Nomad".to_string(),
            bytes_downloaded: 10,
        });
        reporter.report(ProgressEvent::DownloadFailed {
            download_id: 1,
            episode_title: "Sega Nomad".to_string(),
            reason: FailureReason::without_subject("HTTP error 404"),
        });
        reporter.report(ProgressEvent::EpisodeAlreadyStored {
            download_id: 2,
            episode_title: "Sega Nomad".to_string(),
            audio_filename: "2024-12-19-Sega Nomad.mp3".to_string(),
        });
        reporter.report(ProgressEvent::SyncCompleted);
    }

    #[test]
    fn reporter_accepts_collision_events() {
        // indicatif draws to the terminal, so this only checks that every
        // match arm runs; the message texts are asserted by the helper tests.
        let reporter = IndicatifReporter::new(PLAIN);

        reporter.report(ProgressEvent::DownloadStarting {
            download_id: 0,
            episode_title: "Sega Nomad".to_string(),
            episode_index: 0,
            total_to_download: 1,
            content_length: Some(1024),
        });
        reporter.report(ProgressEvent::DownloadSyncing {
            download_id: 0,
            episode_title: "Sega Nomad".to_string(),
            episode_index: 0,
            total_to_download: 1,
        });
        reporter.report(ProgressEvent::DownloadSyncSlow {
            download_id: 0,
            episode_title: "Sega Nomad".to_string(),
            episode_index: 0,
            total_to_download: 1,
            threshold: std::time::Duration::from_secs(30),
        });
        reporter.report(ProgressEvent::DownloadSynced {
            download_id: 0,
            episode_title: "Sega Nomad".to_string(),
            episode_index: 0,
            total_to_download: 1,
        });
        reporter.report(ProgressEvent::PartialFileStuck {
            path: PathBuf::from("/podcasts/2024-01-15-Episode.mp3.partial"),
        });
        reporter.report(ProgressEvent::MetadataUnreadable {
            path: PathBuf::from("/podcasts/2024-01-15-Episode.json"),
            reason: "EOF while parsing".to_string(),
        });
        reporter.report(ProgressEvent::VerifyingStoredAudio {
            audio_filename: "2024-12-19-Sega Nomad.mp3".to_string(),
            position: 1,
            total: 1,
        });
        reporter.report(ProgressEvent::HashingProgress {
            bytes_hashed: 512,
            total_bytes: 1024,
        });
        reporter.report(ProgressEvent::StoredAudioUnverifiable {
            audio_filename: "2024-12-19-Sega Nomad.mp3".to_string(),
            reason: "Permission denied".to_string(),
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

    #[test]
    fn progress_line_emoji_carry_no_variation_selector() {
        // indicatif pads a progress line to the terminal width, but counts a
        // character followed by U+FE0F one column narrower than the terminal
        // draws it. The line then wraps, and every redraw leaves the previous
        // frame behind.
        let progress_marks = [
            Mark::Saving,
            Mark::Globe,
            Mark::Scroll,
            Mark::Search,
            Mark::Headphones,
            Mark::Success,
            Mark::Failure,
            Mark::Broom,
        ];

        let with_selector: Vec<_> = progress_marks
            .iter()
            .filter(|mark| mark.symbols().0.contains('\u{FE0F}'))
            .collect();

        assert!(with_selector.is_empty(), "{:?}", with_selector);
    }

    // =========================================================
    // What stays on the screen
    // =========================================================

    /// The rows left on a terminal `width` columns wide after `events`
    fn screen_after(width: usize, events: impl FnOnce(&IndicatifReporter)) -> Vec<String> {
        let terminal = VirtualTerminal::new(width);
        let reporter = IndicatifReporter::with_target(
            PLAIN,
            ProgressDrawTarget::term_like(Box::new(terminal.clone())),
            Box::new(terminal.writer()),
        );
        events(&reporter);
        // Read before the reporter ends, which clears the bars that are
        // still drawn.
        terminal.lines()
    }

    fn starting(title: &str) -> ProgressEvent {
        ProgressEvent::DownloadStarting {
            download_id: 0,
            episode_title: title.to_string(),
            episode_index: 0,
            total_to_download: 1,
            content_length: Some(100),
        }
    }

    #[test]
    fn a_live_warning_leaves_no_progress_line_behind() {
        let screen = screen_after(100, |reporter| {
            reporter.report(ProgressEvent::ScanningDirectory {
                files_scanned: 0,
                total_files: 1,
            });
            reporter.report(ProgressEvent::ScanningDirectory {
                files_scanned: 1,
                total_files: 1,
            });
            reporter.report(ProgressEvent::MetadataUnreadable {
                path: PathBuf::from("Retro-Talk/Old Episode.json"),
                reason: "key must be a string at line 1 column 3".to_string(),
            });
            reporter.report(ProgressEvent::SyncPlanReady {
                podcast_title: "Retro Talk".to_string(),
                total_episodes: 1,
                new_episodes: 1,
                to_download: 1,
                repairs: 0,
            });
            reporter.report(ProgressEvent::SyncCompleted);
        });

        assert!(
            screen
                .iter()
                .any(|line| line.contains("Could not read episode metadata")),
            "the warning is on the screen:\n{screen:#?}"
        );
        assert!(
            !screen
                .iter()
                .any(|line| line.contains("Scanning existing episodes")),
            "stale bar left behind:\n{screen:#?}"
        );
    }

    #[test]
    fn a_warning_with_an_ascii_mark_leaves_the_virtual_terminal_clean_either_way() {
        // Without the selector there is nothing to miscount, so the screen
        // is clean however the line is printed. This is what the real
        // terminal showed for `[!] ` and `📁 `, and what keeps the virtual
        // terminal honest.
        let terminal = VirtualTerminal::new(100);
        let reporter = IndicatifReporter::with_target(
            StreamEnv {
                color: false,
                utf8: false,
            },
            ProgressDrawTarget::term_like(Box::new(terminal.clone())),
            Box::new(terminal.writer()),
        );
        reporter.report(ProgressEvent::ScanningDirectory {
            files_scanned: 0,
            total_files: 1,
        });
        reporter.report(ProgressEvent::MetadataUnreadable {
            path: PathBuf::from("Retro-Talk/Old Episode.json"),
            reason: "key must be a string".to_string(),
        });
        reporter.report(ProgressEvent::SyncCompleted);
        drop(reporter);

        assert_eq!(
            terminal.lines(),
            vec![
                "[!] Could not read episode metadata Retro-Talk/Old Episode.json (key must be a \
                 string)"
            ]
        );
    }

    #[test]
    fn a_slow_disk_warning_leaves_no_progress_line_behind() {
        let screen = screen_after(100, |reporter| {
            reporter.report(starting("Sega Nomad"));
            reporter.report(ProgressEvent::DownloadSyncSlow {
                download_id: 0,
                episode_title: "Sega Nomad".to_string(),
                episode_index: 0,
                total_to_download: 1,
                threshold: std::time::Duration::from_secs(30),
            });
            reporter.report(ProgressEvent::DownloadCompleted {
                download_id: 0,
                episode_title: "Sega Nomad".to_string(),
                bytes_downloaded: 100,
            });
            reporter.report(ProgressEvent::SyncCompleted);
        });

        assert!(
            screen.iter().any(|line| line.contains("Still writing")),
            "the warning is on the screen:\n{screen:#?}"
        );
        // The warning is wider than the screen and takes two rows; any
        // other row is a copy of a bar.
        assert!(screen.len() <= 2, "stale bar left behind:\n{screen:#?}");
    }

    #[test]
    fn a_title_with_an_emoji_selector_leaves_no_progress_line_behind() {
        let screen = screen_after(100, |reporter| {
            reporter.report(starting("I \u{2764}\u{FE0F} Retro"));
            reporter.report(ProgressEvent::DownloadProgress {
                download_id: 0,
                bytes_downloaded: 50,
                total_bytes: Some(100),
            });
            reporter.report(ProgressEvent::DownloadCompleted {
                download_id: 0,
                episode_title: "I \u{2764}\u{FE0F} Retro".to_string(),
                bytes_downloaded: 100,
            });
            reporter.report(ProgressEvent::SyncCompleted);
        });

        assert_eq!(screen, Vec::<String>::new(), "stale bar left behind");
    }

    /// A writer whose bytes the test can read afterwards
    #[derive(Clone, Default)]
    struct Captured(std::sync::Arc<Mutex<Vec<u8>>>);

    impl Write for Captured {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_hidden_target_gets_no_live_warning() {
        let captured = Captured::default();
        let reporter = IndicatifReporter::with_target(
            PLAIN,
            ProgressDrawTarget::hidden(),
            Box::new(captured.clone()),
        );

        reporter.report(ProgressEvent::MetadataUnreadable {
            path: PathBuf::from("Retro-Talk/Old Episode.json"),
            reason: "key must be a string".to_string(),
        });

        assert!(captured.0.lock().unwrap().is_empty());
    }

    #[test]
    fn a_hostile_title_is_escaped_on_the_screen() {
        let screen = screen_after(200, |reporter| {
            reporter.report(starting("\u{202E}3pm.evil"));
        });

        let text = screen.join("\n");
        assert!(text.contains("%E2%80%AE3pm.evil"), "{text}");
        assert!(!text.contains('\u{202E}'), "{text:?}");
    }

    #[test]
    fn a_hostile_feed_source_is_escaped_on_the_screen() {
        let screen = screen_after(120, |reporter| {
            reporter.report(ProgressEvent::ParsingFeed {
                source: "feeds/a\u{1b}[2J.xml".to_string(),
            });
        });

        let text = screen.join("\n");
        assert!(text.contains("feeds/a%1B[2J.xml"), "{text}");
        assert!(!text.contains('\u{1b}'), "{text:?}");
    }

    #[test]
    fn a_feed_url_shows_as_it_parses() {
        let screen = screen_after(120, |reporter| {
            reporter.report(ProgressEvent::FetchingFeed {
                url: "https://example.com/My%20Show/feed.xml".to_string(),
            });
        });

        assert!(
            screen
                .join("\n")
                .contains("https://example.com/My%20Show/feed.xml")
        );
    }
}
