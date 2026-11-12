// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Local servers and ports for tests that need a real network failure
//!
//! Nothing here leaves 127.0.0.1. A test waits for its server with a
//! deadline, so a client that never connects fails the test instead of
//! hanging it.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// How long a test waits for a client to reach its server
const DEADLINE: Duration = Duration::from_secs(10);

/// A port nothing listens on, until something else claims it
pub(crate) fn closed_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a free local port exists");
    listener
        .local_addr()
        .expect("a bound listener has an address")
        .port()
}

/// A server that handled, or is still holding, one connection
pub(crate) struct Server {
    port: u16,
    done: mpsc::Receiver<String>,
}

impl Server {
    pub(crate) fn port(&self) -> u16 {
        self.port
    }

    /// Wait for the server to be done with its connection and return the
    /// head of the request it received
    pub(crate) fn finish(self) -> String {
        self.done
            .recv_timeout(DEADLINE)
            .expect("the client reached the server before the deadline")
    }
}

fn read_request_head(stream: &mut TcpStream) -> String {
    let mut head = Vec::new();
    let mut buffer = [0u8; 512];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => head.extend_from_slice(&buffer[..read]),
        }
    }
    String::from_utf8_lossy(&head).into_owned()
}

/// Answer one connection with `response` and close it
pub(crate) fn serve_once(response: Vec<u8>) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a free local port exists");
    let port = listener
        .local_addr()
        .expect("a bound listener has an address")
        .port();
    let (sender, done) = mpsc::channel();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let head = read_request_head(&mut stream);
            let _ = stream.write_all(&response);
            let _ = sender.send(head);
        }
    });
    Server { port, done }
}

/// Send `bytes` as soon as one connection is made, without reading what the
/// client sent first
///
/// A client that expects a TLS handshake never sends a request head to wait
/// for, so a server that reads one first would wait as long as the client.
pub(crate) fn answer_unasked(bytes: Vec<u8>) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a free local port exists");
    let port = listener
        .local_addr()
        .expect("a bound listener has an address")
        .port();
    let (sender, done) = mpsc::channel();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let _ = stream.write_all(&bytes);
            let _ = sender.send(String::new());
        }
    });
    Server { port, done }
}

/// Accept one connection and answer nothing until the test has ended its
/// wait
pub(crate) fn serve_silently() -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a free local port exists");
    let port = listener
        .local_addr()
        .expect("a bound listener has an address")
        .port();
    let (sender, done) = mpsc::channel();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let head = read_request_head(&mut stream);
            // The request is read, so the client's wait is now on the
            // response; the connection stays open until the thread ends.
            let _ = sender.send(head);
            let mut rest = Vec::new();
            let _ = stream.read_to_end(&mut rest);
        }
    });
    Server { port, done }
}
