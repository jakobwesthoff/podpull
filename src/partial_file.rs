// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The name of the partial file a file is written to before it is renamed

use std::path::{Path, PathBuf};

/// Suffix of a file that is still being written
///
/// A directory scan removes every file that ends in it, as it can only be
/// left behind by an interrupted write.
const PARTIAL_SUFFIX: &str = ".partial";

/// The partial file `path` is written to before it is renamed to `path`
pub(crate) fn partial_path(path: &Path) -> PathBuf {
    let mut partial = path.as_os_str().to_owned();
    partial.push(PARTIAL_SUFFIX);
    PathBuf::from(partial)
}

/// Whether a file of this name is a partial file
pub(crate) fn is_partial(file_name: &str) -> bool {
    file_name.ends_with(PARTIAL_SUFFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_partial_path_appends_the_suffix_to_the_whole_name() {
        assert_eq!(
            partial_path(Path::new("/podcasts/2024-01-15-Episode.mp3")),
            Path::new("/podcasts/2024-01-15-Episode.mp3.partial")
        );
    }

    #[test]
    fn a_partial_path_is_recognised_as_partial() {
        let partial = partial_path(Path::new("/podcasts/episode.json"));

        assert!(is_partial(partial.file_name().unwrap().to_str().unwrap()));
    }

    #[test]
    fn a_final_name_is_not_partial() {
        assert!(!is_partial("episode.mp3"));
        assert!(!is_partial("partial.mp3"));
    }
}
