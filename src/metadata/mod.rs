// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

mod episode;
mod podcast;

pub use episode::{EpisodeMetadata, read_episode_metadata, write_episode_metadata};
pub use podcast::{PodcastMetadata, read_podcast_metadata, write_podcast_metadata};
