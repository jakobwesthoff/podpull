// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::future::Future;
use std::time::Duration;

use super::DownloadContext;
use crate::progress::{ProgressEvent, ProgressReporter};

/// How long waiting for the file system to confirm a write goes unremarked
///
/// After it, the progress display says that the disk is slow. The sync of a
/// local disk takes milliseconds; a network share can take minutes.
pub(crate) const DISK_WAIT_SLOW_AFTER: Duration = Duration::from_secs(30);

/// What a wait for the file system reports before it turns slow
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WaitNotice {
    /// The wait is announced when it starts. For the waits that come after
    /// the last byte of a file, when the download has nothing else to do.
    Always,
    /// The wait is reported only once it has become slow. For a wait during a
    /// download, which is over in moments unless the file system is slow.
    OnlyWhenSlow,
}

/// Await `work`, which waits for the file system to confirm a write, and
/// report the wait
///
/// With [`WaitNotice::Always`] reports `DownloadSyncing` first. If `work`
/// takes longer than `slow_after`, reports `DownloadSyncSlow` once and keeps
/// waiting, since giving up would leave the file unsynced. Reports
/// `DownloadSynced` when `work` ends, whatever it returns, unless nothing was
/// reported before.
pub(crate) async fn with_disk_wait_notice<T>(
    work: impl Future<Output = T>,
    context: &DownloadContext,
    episode_title: &str,
    reporter: &dyn ProgressReporter,
    slow_after: Duration,
    notice: WaitNotice,
) -> T {
    if notice == WaitNotice::Always {
        reporter.report(ProgressEvent::DownloadSyncing {
            download_id: context.download_id,
            episode_title: episode_title.to_string(),
            episode_index: context.episode_index,
            total_to_download: context.total_to_download,
        });
    }

    let mut reported = notice == WaitNotice::Always;
    let mut work = std::pin::pin!(work);
    let result = tokio::select! {
        result = &mut work => result,
        () = tokio::time::sleep(slow_after) => {
            reporter.report(ProgressEvent::DownloadSyncSlow {
                download_id: context.download_id,
                episode_title: episode_title.to_string(),
                episode_index: context.episode_index,
                total_to_download: context.total_to_download,
                threshold: slow_after,
            });
            reported = true;
            work.await
        }
    };

    if reported {
        reporter.report(ProgressEvent::DownloadSynced {
            download_id: context.download_id,
            episode_title: episode_title.to_string(),
            episode_index: context.episode_index,
            total_to_download: context.total_to_download,
        });
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::episode::test_support::{GATE_TIMEOUT, GateReporter, context, gate};

    #[tokio::test]
    async fn a_fast_wait_reports_syncing_then_synced() {
        let reporter = GateReporter::new(|| {});

        let result = with_disk_wait_notice(
            async { 7 },
            &context(),
            "Test Episode",
            &reporter,
            Duration::from_secs(3600),
            WaitNotice::Always,
        )
        .await;

        assert_eq!(result, 7);
        assert_eq!(reporter.waits(), ["syncing", "synced"]);
    }

    #[tokio::test]
    async fn a_slow_wait_is_reported_once_and_awaited_to_its_end() {
        let (open_gate, closed_gate) = gate();
        let reporter = GateReporter::new(open_gate);

        let work = async {
            tokio::task::spawn_blocking(move || closed_gate.recv().map(|()| "synced"))
                .await
                .unwrap()
        };
        let result = tokio::time::timeout(
            GATE_TIMEOUT,
            with_disk_wait_notice(
                work,
                &context(),
                "Test Episode",
                &reporter,
                Duration::ZERO,
                WaitNotice::Always,
            ),
        )
        .await
        .expect("no DownloadSyncSlow opened the gate");

        assert_eq!(result, Ok("synced"));
        assert_eq!(reporter.waits(), ["syncing", "slow", "synced"]);
    }

    #[tokio::test]
    async fn a_failed_wait_still_ends_with_synced() {
        let reporter = GateReporter::new(|| {});

        let result: std::io::Result<()> = with_disk_wait_notice(
            async { Err(std::io::Error::from_raw_os_error(libc::EIO)) },
            &context(),
            "Test Episode",
            &reporter,
            Duration::from_secs(3600),
            WaitNotice::Always,
        )
        .await;

        assert_eq!(result.unwrap_err().raw_os_error(), Some(libc::EIO));
        assert_eq!(reporter.waits(), ["syncing", "synced"]);
    }

    #[tokio::test]
    async fn a_fast_wait_that_is_only_reported_when_slow_reports_nothing() {
        let reporter = GateReporter::new(|| {});

        let result = with_disk_wait_notice(
            async { 7 },
            &context(),
            "Test Episode",
            &reporter,
            Duration::from_secs(3600),
            WaitNotice::OnlyWhenSlow,
        )
        .await;

        assert_eq!(result, 7);
        assert_eq!(reporter.waits(), Vec::<&str>::new());
    }

    #[tokio::test]
    async fn a_wait_that_is_only_reported_when_slow_ends_with_synced_once_it_was() {
        let (open_gate, closed_gate) = gate();
        let reporter = GateReporter::new(open_gate);

        let work = async {
            tokio::task::spawn_blocking(move || closed_gate.recv().map(|()| "synced"))
                .await
                .unwrap()
        };
        let result = tokio::time::timeout(
            GATE_TIMEOUT,
            with_disk_wait_notice(
                work,
                &context(),
                "Test Episode",
                &reporter,
                Duration::ZERO,
                WaitNotice::OnlyWhenSlow,
            ),
        )
        .await
        .expect("no DownloadSyncSlow opened the gate");

        assert_eq!(result, Ok("synced"));
        assert_eq!(reporter.waits(), ["slow", "synced"]);
    }
}
