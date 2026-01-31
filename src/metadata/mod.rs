mod episode;
mod podcast;

pub use episode::{EpisodeMetadata, read_episode_metadata, write_episode_metadata};
pub use podcast::{PodcastMetadata, read_podcast_metadata, write_podcast_metadata};
