mod episode;
mod podcast;

pub use episode::{read_episode_metadata, write_episode_metadata, EpisodeMetadata};
pub use podcast::{read_podcast_metadata, write_podcast_metadata, PodcastMetadata};
