// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;

use podpull::{
    AudioCheck, NoopReporter, ProgressReporter, ReqwestClient, SyncOptions, SyncResult,
    sync_podcast,
};

mod cli;

use cli::messages::{completion_summary, write_problem_lists};
use cli::reporter::IndicatifReporter;
use cli::{Mark, StreamEnv};

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
    let env = StreamEnv::detect_stdout();

    // Quiet output carries only problems, so a script capturing it sees
    // nothing on a clean run.
    if !args.quiet {
        println!(
            "\n{}{} {}\n",
            env.mark(Mark::Microphone),
            env.bold_magenta("podpull"),
            env.dim("- Podcast Downloader")
        );
    }

    let client = ReqwestClient::new();

    let options = sync_options(&args);

    let indicatif;
    let reporter: &dyn ProgressReporter = if args.quiet {
        &NoopReporter
    } else {
        indicatif = IndicatifReporter::new(env);
        &indicatif
    };

    let result = sync_podcast(&client, &args.feed, &args.output_dir, &options, reporter)
        .await
        .context("Failed to sync podcast")?;

    // Quiet mode suppresses progress, not problems: scripts and cron jobs
    // still see what needs attention, on stderr.
    if args.quiet {
        write_problem_lists(&result, &mut std::io::stderr(), &env)
            .context("write the problem lists to stderr")?;
    } else {
        println!(
            "\n{}{} {}",
            env.mark(Mark::Party),
            env.bold_green("Sync complete:"),
            completion_summary(&result, &env)
        );
        write_problem_lists(&result, &mut std::io::stdout(), &env)
            .context("write the problem lists to stdout")?;
        println!(
            "\n{}Output: {}\n",
            env.mark(Mark::Folder),
            env.cyan(cli::shown::path(&args.output_dir))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::test_result as result;
    use podpull::UnverifiableAudio;

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
    fn exit_code_is_two_for_warnings() {
        let mut stuck = result(0, 0, 0);
        stuck.stuck_partial_files = vec![PathBuf::from("/podcasts/a.mp3.partial")];
        let mut unverifiable = result(0, 0, 0);
        unverifiable.unverifiable_audio = vec![UnverifiableAudio {
            audio_filename: "a.mp3".to_string(),
            reason: "denied".to_string(),
        }];

        assert_eq!(exit_code(&stuck), 2);
        assert_eq!(exit_code(&unverifiable), 2);
    }

    #[test]
    fn exit_code_is_two_for_partial_failure_or_damage() {
        assert_eq!(exit_code(&result(3, 1, 0)), 2);
        assert_eq!(exit_code(&result(3, 0, 1)), 2);
        assert_eq!(exit_code(&result(0, 0, 1)), 2);
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
