// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What an error that ends the run looks like on a pipe

mod common;

use common::{Run, closed_port, podpull_command, run, serve_status, write_empty_feed};

const RSS: &str = "<rss version=\"2.0\"><channel><title>Show</title></channel></rss>";

/// The run of `feed` into a folder below `directory`, which has to end with
/// the exit code 1 and print nothing but the error, on stderr
fn failing_run(directory: &std::path::Path, feed: &str) -> Run {
    let run = run(podpull_command().arg(feed).arg(directory.join("out")));

    assert_eq!(run.code, 1, "{run:?}");
    assert_eq!(run.stdout, "", "{run:?}");
    assert!(!run.stderr.contains("Caused by"), "{run:?}");
    assert!(!run.stderr.contains("Stack backtrace"), "{run:?}");
    run
}

fn lines(run: &Run) -> Vec<&str> {
    run.stderr.lines().collect()
}

#[test]
fn an_unresolvable_host_prints_a_plain_error() {
    let directory = tempfile::tempdir().unwrap();
    let feed = "http://feeds.does-not-exist.invalid/podcast.xml";

    let run = failing_run(directory.path(), feed);

    let lines = lines(&run);
    assert_eq!(lines.len(), 3, "{run:?}");
    assert_eq!(
        lines[0],
        "Error: Cannot reach the feed host feeds.does-not-exist.invalid"
    );
    assert!(lines[1].starts_with("  cause: "), "{run:?}");
    assert_eq!(lines[2], format!("  while fetching the feed {feed}"));
}

#[test]
fn a_refused_connection_prints_a_plain_error() {
    let directory = tempfile::tempdir().unwrap();
    let feed = format!("http://127.0.0.1:{}/feed.xml", closed_port());

    let run = failing_run(directory.path(), &feed);

    assert_eq!(
        lines(&run),
        vec![
            "Error: The feed host 127.0.0.1 refuses connections".to_string(),
            "  cause: connection refused".to_string(),
            format!("  while fetching the feed {feed}"),
        ],
        "{run:?}"
    );
}

#[test]
fn a_feed_that_is_not_found_prints_a_plain_error() {
    let directory = tempfile::tempdir().unwrap();
    let port = serve_status("404 Not Found", RSS);
    let feed = format!("http://127.0.0.1:{port}/feed.xml");

    let run = failing_run(directory.path(), &feed);

    assert_eq!(
        lines(&run),
        vec![
            "Error: The feed was not found on 127.0.0.1 (HTTP 404)".to_string(),
            "  cause: HTTP status client error (404 Not Found)".to_string(),
            format!("  while fetching the feed {feed}"),
        ],
        "{run:?}"
    );
}

#[test]
fn a_feed_that_is_not_rss_prints_a_plain_error() {
    let directory = tempfile::tempdir().unwrap();
    let port = serve_status("200 OK", "<html><body>Not a feed</body></html>");
    let feed = format!("http://127.0.0.1:{port}/feed.xml");

    let run = failing_run(directory.path(), &feed);

    let lines = lines(&run);
    assert_eq!(lines.len(), 3, "{run:?}");
    assert_eq!(lines[0], "Error: The feed is not a valid RSS feed");
    assert!(lines[1].starts_with("  cause: "), "{run:?}");
    assert_eq!(lines[2], format!("  while parsing the feed {feed}"));
}

#[test]
fn a_feed_file_that_does_not_exist_prints_a_plain_error() {
    let directory = tempfile::tempdir().unwrap();
    let feed = directory.path().join("missing.xml");
    let feed = feed.to_str().unwrap();

    let run = failing_run(directory.path(), feed);

    assert_eq!(
        lines(&run),
        vec![
            "Error: The feed file does not exist".to_string(),
            "  cause: no such file or directory".to_string(),
            format!("  while reading the feed file {feed}"),
        ],
        "{run:?}"
    );
}

#[test]
fn a_feed_url_that_is_none_prints_a_plain_error() {
    for feed in ["http://", "https://exa mple.com/%41"] {
        let directory = tempfile::tempdir().unwrap();

        let run = failing_run(directory.path(), feed);

        let lines = lines(&run);
        assert_eq!(lines.len(), 3, "{run:?}");
        assert_eq!(lines[0], "Error: The feed URL is not valid");
        assert!(lines[1].starts_with("  cause: "), "{run:?}");
        assert_eq!(lines[2], format!("  while reading the feed URL {feed}"));
    }
}

#[test]
fn an_output_folder_below_a_file_prints_a_plain_error() {
    let directory = tempfile::tempdir().unwrap();
    let feed = write_empty_feed(directory.path());
    let file = directory.path().join("a-file");
    std::fs::write(&file, b"").unwrap();
    let output = file.join("out");

    let run = run(podpull_command().arg(&feed).arg(&output));

    assert_eq!(run.code, 1, "{run:?}");
    assert_eq!(
        lines(&run),
        vec![
            "Error: Cannot create the output folder".to_string(),
            "  cause: not a directory".to_string(),
            format!(
                "  while creating the output folder {}",
                output.to_string_lossy()
            ),
        ],
        "{run:?}"
    );
}

#[cfg(unix)]
#[test]
fn a_read_only_output_folder_prints_a_plain_error_that_names_podcast_json() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let feed = write_empty_feed(directory.path());
    let output = directory.path().join("out");
    std::fs::create_dir(&output).unwrap();
    std::fs::set_permissions(&output, std::fs::Permissions::from_mode(0o555)).unwrap();
    let can_write_anyway = std::fs::write(output.join("probe"), b"").is_ok();
    if can_write_anyway {
        // A process that may write to a read-only folder, as root does,
        // cannot make the write fail.
        std::fs::set_permissions(&output, std::fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }

    let run = run(podpull_command().arg(&feed).arg(&output));
    std::fs::set_permissions(&output, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(run.code, 1, "{run:?}");
    let lines = lines(&run);
    assert_eq!(lines.len(), 3, "{run:?}");
    assert_eq!(lines[0], "Error: Cannot write to the output folder");
    assert_eq!(lines[1], "  cause: permission denied");
    assert_eq!(
        lines[2],
        format!(
            "  while writing the podcast metadata {}",
            output.join("podcast.json").to_string_lossy()
        )
    );
    assert!(!run.stderr.contains(".partial"), "{run:?}");
}

#[test]
fn a_feed_that_is_not_found_after_a_redirect_names_the_host_that_answered() {
    let directory = tempfile::tempdir().unwrap();
    let answering_port = serve_status("404 Not Found", RSS);
    let location = format!("http://localhost:{answering_port}/moved.xml");
    let redirecting_port = serve_status(
        Box::leak(format!("302 Found\r\nlocation: {location}").into_boxed_str()),
        "",
    );
    let feed = format!("http://127.0.0.1:{redirecting_port}/feed.xml");

    let run = failing_run(directory.path(), &feed);

    assert_eq!(
        lines(&run),
        vec![
            "Error: The feed was not found on localhost (HTTP 404)".to_string(),
            "  cause: HTTP status client error (404 Not Found)".to_string(),
            format!("  while fetching the feed {feed}"),
        ],
        "{run:?}"
    );
}
