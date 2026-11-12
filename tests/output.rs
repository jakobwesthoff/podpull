// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! How the colour variables of the environment act on a piped run

mod common;

use common::{podpull_command, run, run_with_stdout_closed, serve_after_gate, write_empty_feed};

#[test]
fn clicolor_force_colours_a_pipe() {
    let directory = tempfile::tempdir().unwrap();
    let feed = write_empty_feed(directory.path());

    let run = run(podpull_command()
        .env("CLICOLOR_FORCE", "1")
        .arg(&feed)
        .arg(directory.path().join("out")));

    assert_eq!(run.code, 0, "{run:?}");
    assert!(run.stdout.contains('\u{1b}'), "{run:?}");
}

#[test]
fn a_pipe_without_colour_variables_has_no_escape_sequences() {
    let directory = tempfile::tempdir().unwrap();
    let feed = write_empty_feed(directory.path());

    let run = run(podpull_command()
        .arg(&feed)
        .arg(directory.path().join("out")));

    assert_eq!(run.code, 0, "{run:?}");
    assert!(!run.stdout.contains('\u{1b}'), "{run:?}");
}

#[test]
fn no_color_leaves_no_escape_sequence() {
    let directory = tempfile::tempdir().unwrap();
    let feed = write_empty_feed(directory.path());

    let run = run(podpull_command()
        .env("NO_COLOR", "1")
        .arg(&feed)
        .arg(directory.path().join("out")));

    assert_eq!(run.code, 0, "{run:?}");
    assert!(!run.stdout.contains('\u{1b}'), "{run:?}");
}

#[test]
fn a_quiet_run_prints_the_problem_lists_on_stderr_only() {
    let directory = tempfile::tempdir().unwrap();
    let (feed, _) = common::write_feed_with_unreachable_episode(directory.path(), "Episode 1");

    let run = run(podpull_command()
        .arg("--quiet")
        .arg(&feed)
        .arg(directory.path().join("out")));

    assert_eq!(run.code, 1, "{run:?}");
    assert_eq!(run.stdout, "", "{run:?}");
    assert!(run.stderr.contains("Failed episodes:"), "{run:?}");
    assert!(run.stderr.contains("Episode 1"), "{run:?}");
}

#[test]
fn a_hostile_title_from_the_feed_is_shown_escaped() {
    let directory = tempfile::tempdir().unwrap();
    let (feed, _) = common::write_feed_with_unreachable_episode(
        directory.path(),
        "Episode 42: \u{202E}3pm.evil 100%",
    );

    let run = run(podpull_command()
        .arg(&feed)
        .arg(directory.path().join("out")));

    assert!(
        run.stdout.contains("Episode 42: %E2%80%AE3pm.evil 100%25"),
        "{run:?}"
    );
    assert!(!run.stdout.contains('\u{202E}'), "{run:?}");
    assert!(!run.stderr.contains('\u{202E}'), "{run:?}");
}

#[test]
fn a_failed_download_shows_the_root_cause_and_the_url() {
    let directory = tempfile::tempdir().unwrap();
    let (feed, audio_url) =
        common::write_feed_with_unreachable_episode(directory.path(), "Episode 1");

    let run = run(podpull_command()
        .arg(&feed)
        .arg(directory.path().join("out")));

    assert!(
        run.stdout
            .contains(&format!("Episode 1 - connection refused ({audio_url})")),
        "{run:?}"
    );
    assert!(!run.stdout.contains("error sending request"), "{run:?}");
}

const EMPTY_FEED: &str = "<rss version=\"2.0\"><channel><title>Show</title>\
    <link>http://example.com/</link><description>d</description></channel></rss>";

#[test]
fn pipe_output_has_no_banner_and_no_emoji() {
    let directory = tempfile::tempdir().unwrap();
    let feed = write_empty_feed(directory.path());
    let output = directory.path().join("out");

    let run = run(podpull_command().arg(&feed).arg(&output));

    assert_eq!(run.code, 0, "{run:?}");
    assert_eq!(
        run.stdout,
        format!(
            "Sync complete: 0 downloaded, 0 existing, 0 failed\n\nOutput: {}\n",
            output.to_string_lossy()
        ),
        "{run:?}"
    );
}

#[test]
fn pipe_output_lists_failed_episodes_without_marks() {
    let directory = tempfile::tempdir().unwrap();
    let (feed, audio_url) =
        common::write_feed_with_unreachable_episode(directory.path(), "Episode 1");
    let output = directory.path().join("out");

    let run = run(podpull_command().arg(&feed).arg(&output));

    assert_eq!(run.code, 1, "{run:?}");
    assert_eq!(
        run.stdout,
        format!(
            "Sync complete: 0 downloaded, 0 existing, 1 failed\n\nFailed episodes:\n  \
             Episode 1 - connection refused ({audio_url})\n\nOutput: {}\n",
            output.to_string_lossy()
        ),
        "{run:?}"
    );
}

#[test]
fn a_forced_colour_pipe_is_coloured_but_has_no_banner_and_no_emoji() {
    let directory = tempfile::tempdir().unwrap();
    let feed = write_empty_feed(directory.path());

    let run = run(podpull_command()
        .env("CLICOLOR_FORCE", "1")
        .arg(&feed)
        .arg(directory.path().join("out")));

    assert!(run.stdout.contains('\u{1b}'), "{run:?}");
    assert!(!run.stdout.contains("Podcast Downloader"), "{run:?}");
    assert!(!run.stdout.contains('\u{1F389}'), "{run:?}");
    assert!(!run.stdout.contains('\u{1F4C1}'), "{run:?}");
}

#[test]
fn a_quiet_run_without_problems_prints_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let feed = write_empty_feed(directory.path());

    let run = run(podpull_command()
        .arg("--quiet")
        .arg(&feed)
        .arg(directory.path().join("out")));

    assert_eq!(run.code, 0, "{run:?}");
    assert_eq!(
        (run.stdout.as_str(), run.stderr.as_str()),
        ("", ""),
        "{run:?}"
    );
}

#[test]
fn a_closed_stdout_keeps_the_exit_code_of_the_sync() {
    let directory = tempfile::tempdir().unwrap();
    let server = serve_after_gate(EMPTY_FEED);

    let run = run_with_stdout_closed(
        podpull_command()
            .arg(server.url("feed.xml"))
            .arg(directory.path().join("out")),
        || server.release(),
    );

    assert!(!run.stderr.contains("panicked"), "{run:?}");
    assert_eq!(run.code, 0, "{run:?}");
}
