// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::io::Write;
use std::path::PathBuf;

use clap::Parser;

use podpull::{
    AudioCheck, NoopReporter, ProgressReporter, ReqwestClient, SyncOptions, SyncResult,
    sync_podcast,
};

mod cli;

use cli::failure::{Failure, report};
use cli::messages::{write_problem_lists, write_run_summary};
use cli::reporter::IndicatifReporter;
use cli::{Mark, Stream, StreamEnv};

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
async fn main() {
    let args = Args::parse();

    let code = match run(&args).await {
        Ok(code) => code,
        Err(failure) => {
            // The bars of the run are gone by now, so nothing draws over
            // the report. A report that cannot be written has nowhere left
            // to go.
            let _ = report(
                &mut std::io::stderr(),
                &failure,
                &args.feed,
                &args.output_dir,
                &StreamEnv::detect(Stream::Stderr),
                console::Term::stderr().size().1 as usize,
            );
            1
        }
    };
    if code != 0 {
        std::process::exit(code);
    }
}

/// Sync the feed and print the end of the run; the exit status of a run that
/// got to its end
async fn run(args: &Args) -> Result<i32, Failure> {
    let stdout_env = StreamEnv::detect(Stream::Stdout);
    let stderr_env = StreamEnv::detect(Stream::Stderr);

    // Quiet output carries only problems, so a script capturing it sees
    // nothing on a clean run. A pipe gets no banner either.
    if !args.quiet {
        tolerate_a_closed_pipe(write_banner(&mut std::io::stdout(), &stdout_env))
            .map_err(output_failed(Stream::Stdout))?;
    }

    let client = ReqwestClient::new();

    let options = sync_options(args);

    let indicatif;
    let reporter: &dyn ProgressReporter = if args.quiet {
        &NoopReporter
    } else {
        indicatif = IndicatifReporter::new(stderr_env);
        &indicatif
    };

    let result = sync_podcast(&client, &args.feed, &args.output_dir, &options, reporter).await?;

    // Quiet mode suppresses progress, not problems: scripts and cron jobs
    // still see what needs attention, on stderr.
    if args.quiet {
        tolerate_a_closed_pipe(write_problem_lists(
            &result,
            &mut std::io::stderr(),
            &stderr_env,
        ))
        .map_err(output_failed(Stream::Stderr))?;
    } else {
        tolerate_a_closed_pipe(write_run_summary(
            &result,
            &args.output_dir,
            &mut std::io::stdout(),
            &stdout_env,
        ))
        .map_err(output_failed(Stream::Stdout))?;
    }

    Ok(exit_code(&result))
}

/// The failure of a write to `stream`
fn output_failed(stream: Stream) -> impl Fn(std::io::Error) -> Failure {
    move |source| Failure::Output { stream, source }
}

/// The banner on top of a run, for a terminal only
fn write_banner(out: &mut impl Write, env: &StreamEnv) -> std::io::Result<()> {
    if !env.rich {
        return Ok(());
    }
    writeln!(
        out,
        "\n{}{} {}\n",
        env.mark(Mark::Microphone),
        env.bold_magenta("podpull"),
        env.dim("- Podcast Downloader")
    )
}

/// A reader that closed the pipe has seen what it wanted: that is no error
/// of the run, whose exit status stays that of the sync
fn tolerate_a_closed_pipe(written: std::io::Result<()>) -> std::io::Result<()> {
    match written {
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        other => other,
    }
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

    #[test]
    fn a_terminal_gets_the_banner() {
        let mut out = Vec::new();

        write_banner(
            &mut out,
            &StreamEnv {
                color: false,
                utf8: true,
                rich: true,
            },
        )
        .unwrap();

        assert_eq!(
            String::from_utf8(out).unwrap(),
            "\n🎙\u{fe0f}  podpull - Podcast Downloader\n\n"
        );
    }

    #[test]
    fn a_pipe_gets_no_banner() {
        let mut out = Vec::new();

        write_banner(
            &mut out,
            &StreamEnv {
                color: false,
                utf8: true,
                rich: false,
            },
        )
        .unwrap();

        assert_eq!(out, b"");
    }

    #[test]
    fn a_closed_pipe_is_no_error_of_the_run() {
        let closed = Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe));
        let full = Err(std::io::Error::from(std::io::ErrorKind::StorageFull));

        assert!(tolerate_a_closed_pipe(closed).is_ok());
        assert!(tolerate_a_closed_pipe(full).is_err());
        assert!(tolerate_a_closed_pipe(Ok(())).is_ok());
    }

    #[test]
    fn a_failed_write_is_a_failure_to_write_that_stream() {
        let failure =
            output_failed(Stream::Stderr)(std::io::Error::from(std::io::ErrorKind::StorageFull));

        assert!(matches!(
            failure,
            Failure::Output {
                stream: Stream::Stderr,
                ..
            }
        ));
    }
}
