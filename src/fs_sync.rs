// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::fs::File;
use std::io;

/// Flush a file's data to stable storage
///
/// On macOS `File::sync_all` asks the drive itself to flush its cache
/// (`F_FULLFSYNC`). Network filesystems such as smbfs reject that as
/// unsupported, which would fail every write to a network share. Those get
/// a plain `fsync` instead, which hands the data to the file server.
pub(crate) fn sync_file(file: &File) -> io::Result<()> {
    with_fallback(file.sync_all(), || plain_fsync(file))
}

/// Use `fallback` when `full_sync` failed because the filesystem does not
/// support it; return every other outcome unchanged
fn with_fallback(
    full_sync: io::Result<()>,
    fallback: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    match full_sync {
        Err(error) if is_unsupported(&error) => fallback(),
        outcome => outcome,
    }
}

fn is_unsupported(error: &io::Error) -> bool {
    // ENOTSUP and EOPNOTSUPP are one code on macOS but two on Linux.
    // Filesystems without F_FULLFSYNC answer with ENOTTY as well.
    matches!(
        error.raw_os_error(),
        Some(code) if code == libc::ENOTSUP || code == libc::EOPNOTSUPP || code == libc::ENOTTY
    )
}

#[cfg(unix)]
fn plain_fsync(file: &File) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    // SAFETY: the descriptor belongs to `file`, which outlives this call.
    if unsafe { libc::fsync(file.as_raw_fd()) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Outside Unix, `sync_all` does not report the error codes the fallback
/// answers, so it is never reached.
#[cfg(not(unix))]
fn plain_fsync(file: &File) -> io::Result<()> {
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn os_error(code: i32) -> io::Error {
        io::Error::from_raw_os_error(code)
    }

    #[test]
    fn unsupported_full_sync_falls_back() {
        for code in [libc::ENOTSUP, libc::EOPNOTSUPP, libc::ENOTTY] {
            let fell_back = Cell::new(false);

            let result = with_fallback(Err(os_error(code)), || {
                fell_back.set(true);
                Ok(())
            });

            assert!(result.is_ok());
            assert!(fell_back.get(), "no fallback for os error {}", code);
        }
    }

    #[test]
    fn other_sync_errors_are_returned_without_fallback() {
        let fell_back = Cell::new(false);

        let result = with_fallback(Err(os_error(libc::EIO)), || {
            fell_back.set(true);
            Ok(())
        });

        assert_eq!(result.unwrap_err().raw_os_error(), Some(libc::EIO));
        assert!(!fell_back.get());
    }

    #[test]
    fn successful_full_sync_needs_no_fallback() {
        let fell_back = Cell::new(false);

        let result = with_fallback(Ok(()), || {
            fell_back.set(true);
            Ok(())
        });

        assert!(result.is_ok());
        assert!(!fell_back.get());
    }

    #[test]
    fn failing_fallback_is_reported() {
        let result = with_fallback(Err(os_error(libc::ENOTSUP)), || Err(os_error(libc::EIO)));

        assert_eq!(result.unwrap_err().raw_os_error(), Some(libc::EIO));
    }

    #[test]
    fn sync_file_syncs_a_local_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = std::fs::File::create(dir.path().join("audio.mp3")).unwrap();

        sync_file(&file).unwrap();
    }
}
