// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! How the colour variables of the environment act on a piped run

mod common;

use common::{podpull_command, run, write_empty_feed};

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
    let feed = common::write_feed_with_unreachable_episode(directory.path(), "Episode 1");

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
    let feed = common::write_feed_with_unreachable_episode(
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
