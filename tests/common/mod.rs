// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Helpers for the tests that run the podpull binary

// Every test crate includes this module and uses a part of it.
#![allow(dead_code)]

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// How long a run of the binary may take before the test fails
const DEADLINE: Duration = Duration::from_secs(30);

/// Environment variables that change what the binary prints or where it
/// connects
///
/// Proxy settings would send requests to local servers through a proxy; the
/// colour and locale variables change the output. The environment is not
/// cleared as a whole because `cargo llvm-cov` hands the binary its coverage
/// file through it.
const ISOLATED: [&str; 14] = [
    "HTTP_PROXY",
    "http_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "ALL_PROXY",
    "all_proxy",
    "CLICOLOR",
    "CLICOLOR_FORCE",
    "NO_COLOR",
    "TERM",
    "LC_ALL",
    "LC_CTYPE",
    "LANG",
    "RUST_BACKTRACE",
];

/// The podpull binary with an environment that makes its output and its
/// connections the same on every machine
///
/// `NO_PROXY=*` also keeps requests away from a system-wide proxy.
pub fn podpull_command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_podpull"));
    for name in ISOLATED {
        command.env_remove(name);
    }
    command
        .env("NO_PROXY", "*")
        .env("LANG", "en_US.UTF-8")
        .env("RUST_BACKTRACE", "1");
    command
}

/// What a run of the binary printed and how it ended
#[derive(Debug)]
pub struct Run {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}

/// Run `command` with both streams piped, and fail the test if it has not
/// ended by the deadline
pub fn run(command: &mut Command) -> Run {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the podpull binary starts");
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let mut stderr = child.stderr.take().expect("stderr is piped");
    let stdout_reader = thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });
    let stderr_reader = thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });

    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("the child can be polled") {
            break status;
        }
        if started.elapsed() > DEADLINE {
            let _ = child.kill();
            let _ = child.wait();
            let stdout = stdout_reader.join().unwrap_or_default();
            let stderr = stderr_reader.join().unwrap_or_default();
            panic!("podpull did not end in {DEADLINE:?}\nstdout:\n{stdout}\nstderr:\n{stderr}");
        }
        thread::sleep(Duration::from_millis(10));
    };
    let stdout = stdout_reader.join().expect("the stdout reader ends");
    let stderr = stderr_reader.join().expect("the stderr reader ends");
    if stderr.to_lowercase().contains("proxy") {
        eprintln!("note: podpull mentioned a proxy; a system-wide proxy is not switched off");
    }
    Run {
        stdout,
        stderr,
        code: status.code().expect("the child ended without a signal"),
    }
}

/// A feed file without episodes, so a run needs no network
pub fn write_empty_feed(directory: &Path) -> std::path::PathBuf {
    let path = directory.join("feed.xml");
    std::fs::write(
        &path,
        "<?xml version=\"1.0\"?><rss version=\"2.0\"><channel><title>Show</title>\
         <link>http://example.com/</link><description>d</description></channel></rss>",
    )
    .expect("the feed file is written");
    path
}

/// A port nothing listens on, until something else claims it
pub fn closed_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a free local port exists");
    listener
        .local_addr()
        .expect("a bound listener has an address")
        .port()
}

/// A feed file with one episode whose audio cannot be fetched, because
/// nothing listens at its address
pub fn write_feed_with_unreachable_episode(directory: &Path) -> std::path::PathBuf {
    let path = directory.join("feed.xml");
    std::fs::write(
        &path,
        format!(
            "<?xml version=\"1.0\"?><rss version=\"2.0\"><channel><title>Show</title>\
             <link>http://example.com/</link><description>d</description>\
             <item><title>Episode 1</title><guid>ep1</guid>\
             <pubDate>Mon, 09 Nov 2026 10:00:00 +0000</pubDate>\
             <enclosure url=\"http://127.0.0.1:{}/ep1.mp3\" length=\"1\" type=\"audio/mpeg\"/>\
             </item></channel></rss>",
            closed_port()
        ),
    )
    .expect("the feed file is written");
    path
}
