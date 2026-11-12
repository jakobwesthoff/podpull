// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A terminal in memory, for tests of what stays on the screen
//!
//! It follows the rules of a real terminal that matter for progress bars: a
//! line that does not fit the width wraps onto the next row, the cursor
//! moves by rows, and a character followed by U+FE0F takes two columns,
//! where `unicode-width` (and so indicatif) counts one.

use std::io;
use std::sync::{Arc, Mutex};

use indicatif::TermLike;

#[derive(Debug)]
struct Screen {
    width: usize,
    rows: Vec<String>,
    row: usize,
    column: usize,
    /// Columns taken by the character written last, to widen it when a
    /// variation selector follows
    last_width: usize,
}

impl Screen {
    fn ensure_row(&mut self) {
        while self.rows.len() <= self.row {
            self.rows.push(String::new());
        }
    }

    fn put(&mut self, character: char) {
        match character {
            '\n' => {
                self.row += 1;
                self.column = 0;
                self.last_width = 0;
                self.ensure_row();
            }
            '\r' => self.column = 0,
            '\u{FE0F}' if self.last_width == 1 => {
                self.ensure_row();
                self.rows[self.row].push(character);
                self.column += 1;
                self.last_width = 2;
            }
            _ => {
                let width = console::measure_text_width(&character.to_string());
                if self.column + width > self.width {
                    self.row += 1;
                    self.column = 0;
                }
                self.ensure_row();
                if self.column == 0 {
                    // indicatif writes whole lines, padded to the width, so
                    // a write at the start of a row replaces the row.
                    self.rows[self.row].clear();
                }
                self.rows[self.row].push(character);
                self.column += width;
                self.last_width = width;
            }
        }
    }

    fn put_str(&mut self, text: &str) {
        let mut characters = text.chars().peekable();
        while let Some(character) = characters.next() {
            if character == '\u{1b}' && characters.peek() == Some(&'[') {
                // A style sequence takes no columns.
                for next in characters.by_ref() {
                    if next == 'm' {
                        break;
                    }
                }
                continue;
            }
            self.put(character);
        }
    }
}

/// A shared screen; clones show the same rows
#[derive(Debug, Clone)]
pub(crate) struct VirtualTerminal {
    screen: Arc<Mutex<Screen>>,
}

impl VirtualTerminal {
    pub(crate) fn new(width: usize) -> Self {
        Self {
            screen: Arc::new(Mutex::new(Screen {
                width,
                rows: vec![String::new()],
                row: 0,
                column: 0,
                last_width: 0,
            })),
        }
    }

    /// The rows on the screen, without trailing spaces, empty rows dropped
    pub(crate) fn lines(&self) -> Vec<String> {
        self.screen
            .lock()
            .expect("no test code panics while holding the screen")
            .rows
            .iter()
            .map(|row| row.trim_end().to_string())
            .filter(|row| !row.is_empty())
            .collect()
    }

    /// A writer that writes onto the same screen, at the cursor
    pub(crate) fn writer(&self) -> impl io::Write + Send + 'static {
        ScreenWriter(self.clone())
    }

    fn with_screen<T>(&self, work: impl FnOnce(&mut Screen) -> T) -> T {
        work(
            &mut self
                .screen
                .lock()
                .expect("no test code panics while holding the screen"),
        )
    }
}

struct ScreenWriter(VirtualTerminal);

impl io::Write for ScreenWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let text = String::from_utf8_lossy(bytes).into_owned();
        self.0.with_screen(|screen| screen.put_str(&text));
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl TermLike for VirtualTerminal {
    fn width(&self) -> u16 {
        self.with_screen(|screen| screen.width as u16)
    }

    fn height(&self) -> u16 {
        24
    }

    fn move_cursor_up(&self, n: usize) -> io::Result<()> {
        self.with_screen(|screen| screen.row = screen.row.saturating_sub(n));
        Ok(())
    }

    fn move_cursor_down(&self, n: usize) -> io::Result<()> {
        self.with_screen(|screen| {
            screen.row += n;
            screen.ensure_row();
        });
        Ok(())
    }

    fn move_cursor_right(&self, n: usize) -> io::Result<()> {
        self.with_screen(|screen| screen.column += n);
        Ok(())
    }

    fn move_cursor_left(&self, n: usize) -> io::Result<()> {
        self.with_screen(|screen| screen.column = screen.column.saturating_sub(n));
        Ok(())
    }

    fn write_line(&self, text: &str) -> io::Result<()> {
        self.with_screen(|screen| {
            screen.put_str(text);
            screen.put('\n');
        });
        Ok(())
    }

    fn write_str(&self, text: &str) -> io::Result<()> {
        self.with_screen(|screen| screen.put_str(text));
        Ok(())
    }

    fn clear_line(&self) -> io::Result<()> {
        self.with_screen(|screen| {
            screen.ensure_row();
            screen.rows[screen.row].clear();
            screen.column = 0;
        });
        Ok(())
    }

    fn flush(&self) -> io::Result<()> {
        Ok(())
    }
}
