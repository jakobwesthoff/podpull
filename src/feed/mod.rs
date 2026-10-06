// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

mod fetch;
mod parse;

pub use fetch::{fetch_feed_bytes, file_path_to_url, is_url, read_feed_file};
#[cfg(test)]
pub use parse::Enclosure;
pub use parse::{Episode, Podcast, parse_feed};
