// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

use podpull::{ProgressEvent, ProgressReporter};

use super::messages::*;
use super::*;

/// Progress reporter using indicatif for terminal output
pub(crate) struct IndicatifReporter {
    multi: MultiProgress,
    bars: Mutex<HashMap<usize, ProgressBar>>,
    /// Slots whose download already printed the warning about a slow disk
    slow_warned: Mutex<HashSet<usize>>,
    main_bar: ProgressBar,
    env: Env,
}

impl IndicatifReporter {
    pub(crate) fn new(env: Env) -> Self {
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
            slow_warned: Mutex::new(HashSet::new()),
            main_bar,
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
            .set_message(episode_message(
                episode_index,
                total_to_download,
                status,
                episode_title,
                &self.env,
            ));
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
                self.main_bar.set_message(format!(
                    "{}Fetching feed: {}",
                    self.env.mark(Mark::Globe),
                    self.env.cyan(&url)
                ));
            }

            ProgressEvent::ParsingFeed { source } => {
                self.main_bar.set_message(format!(
                    "{}Parsing feed: {}",
                    self.env.mark(Mark::Scroll),
                    self.env.cyan(&source)
                ));
            }

            ProgressEvent::ScanningDirectory {
                files_scanned,
                total_files,
            } => {
                if total_files == 0 {
                    self.main_bar.set_message(format!(
                        "{}Scanning existing episodes...",
                        self.env.mark(Mark::Search)
                    ));
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
                self.main_bar.set_message(format!(
                    "{}{}",
                    self.env.mark(Mark::Headphones),
                    plan_message(
                        &podcast_title,
                        total_episodes,
                        new_episodes,
                        to_download,
                        repairs,
                        &self.env
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
                    let _ = self.multi.println(format!(
                        "{}{}",
                        self.env.mark(Mark::Warning),
                        self.env
                            .yellow(slow_sync_warning(&episode_title, threshold))
                    ));
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
                bar.set_message(format!(
                    "{}{}",
                    self.env.mark(Mark::Success),
                    self.env.green(truncate_title(&episode_title, title_width))
                ));
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
                bar.abandon_with_message(format!(
                    "{}{} - {}",
                    self.env.mark(Mark::Failure),
                    self.env.red(truncate_title(
                        &episode_title,
                        title_width.max(MIN_TITLE_WIDTH)
                    )),
                    self.env.red(&reason.root)
                ));
                self.finish_bar(download_id);
            }

            ProgressEvent::PartialFilesCleanedUp { count } => {
                if count > 0 {
                    self.main_bar.set_message(format!(
                        "{}Cleaned up {} interrupted download{}",
                        self.env.mark(Mark::Broom),
                        self.env.yellow(count),
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
                    "{}{}",
                    self.env.mark(Mark::Success),
                    self.env
                        .green(already_stored_message(&episode_title, &audio_filename))
                ));
                self.finish_bar(download_id);
            }

            ProgressEvent::PartialFileStuck { path } => {
                let _ = self.multi.println(format!(
                    "{}{}",
                    self.env.mark(Mark::Warning),
                    self.env.yellow(stuck_partial_file_message(&path))
                ));
            }

            ProgressEvent::MetadataUnreadable { path, reason } => {
                // Printed above the progress bars so the warning outlives
                // the transient status line.
                let _ = self.multi.println(format!(
                    "{}{}",
                    self.env.mark(Mark::Warning),
                    self.env.yellow(unreadable_metadata_message(&path, &reason))
                ));
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
                self.main_bar.set_prefix(format!(
                    "{}{}",
                    self.env.mark(Mark::Search),
                    checking_prefix(position, total, &self.env)
                ));
                self.main_bar.set_message(self.env.cyan(&audio_filename));
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
                let _ = self.multi.println(format!(
                    "{}{}",
                    self.env.mark(Mark::Warning),
                    self.env
                        .yellow(unverifiable_audio_message(&audio_filename, &reason))
                ));
            }

            ProgressEvent::StoredAudioDamaged {
                episode_title,
                audio_filename,
                kind,
                remedy,
            } => {
                let _ = self.multi.println(format!(
                    "{}{}",
                    self.env.mark(Mark::Warning),
                    self.env.yellow(damage_message(
                        &episode_title,
                        &audio_filename,
                        kind,
                        remedy
                    ))
                ));
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
    episode_title: &str,
    env: &Env,
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

/// Shorten a title to `max_len` terminal columns, ending in "..." when it
/// had to be cut
///
/// Cuts between characters and counts the columns the terminal shows, since
/// titles hold multi-byte characters such as umlauts and dashes.
fn truncate_title(title: &str, max_len: usize) -> String {
    console::truncate_str(title, max_len, "...").into_owned()
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
    use podpull::FailureReason;

    const PLAIN: Env = Env {
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
        assert_eq!(truncate_title("Sega Nomad", 10), "Sega Nomad");
        assert_eq!(truncate_title("Größe", 5), "Größe");
    }

    #[test]
    fn truncate_title_cuts_ascii_with_an_ellipsis() {
        assert_eq!(truncate_title("Sega Nomad Special", 10), "Sega No...");
    }

    #[test]
    fn truncate_title_cuts_before_a_multi_byte_character_instead_of_inside_it() {
        // The cut at byte 2 falls inside the first "ä" (bytes 1 and 2).
        assert_eq!(truncate_title("Mädchen ääää", 5), "Mä...");
        // The cut at byte 6 falls inside the en dash (bytes 5 to 7).
        assert_eq!(truncate_title("Trek – Folge 499", 9), "Trek –...");
    }

    #[test]
    fn episode_message_puts_the_status_between_the_place_and_the_title() {
        assert_eq!(
            episode_message(0, 3, "", "Sega Nomad", &PLAIN),
            "[1/3] Sega Nomad"
        );
        assert_eq!(
            episode_message(0, 3, SYNCING_STATUS, "Sega Nomad", &PLAIN),
            "[1/3] Writing to disk: Sega Nomad"
        );
        assert_eq!(
            episode_message(1, 3, SLOW_SYNC_STATUS, "Sega Nomad", &PLAIN),
            "[2/3] Slow disk, still writing: Sega Nomad"
        );
    }

    #[test]
    fn episode_message_cuts_a_long_title_and_keeps_the_status() {
        let title = "x".repeat(500);

        let message = episode_message(0, 3, SLOW_SYNC_STATUS, &title, &PLAIN);

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
}
