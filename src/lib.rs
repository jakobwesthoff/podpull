// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

pub mod episode;
pub mod error;
pub mod feed;
pub mod http;
pub mod metadata;
pub mod progress;
pub mod state;
pub mod sync;

// Re-export main types for convenience
pub use episode::{
    DownloadContext, DownloadResult, download_episode, generate_filename, generate_filename_stem,
    get_audio_extension,
};
pub use error::{DownloadError, FeedError, MetadataError, StateError, SyncError};
pub use feed::{
    Enclosure, Episode, Podcast, fetch_feed, fetch_feed_bytes, file_path_to_url, is_url,
    parse_feed, parse_feed_file, read_feed_file,
};
pub use http::{HttpClient, HttpResponse, ReqwestClient};
pub use metadata::{
    EpisodeMetadata, PodcastMetadata, read_episode_metadata, read_podcast_metadata,
    write_episode_metadata, write_podcast_metadata,
};
pub use progress::{NoopReporter, ProgressEvent, ProgressReporter, SharedProgressReporter};
pub use state::{OutputState, SyncPlan, create_sync_plan, scan_output_dir};
pub use sync::{SyncOptions, SyncResult, sync_podcast};
