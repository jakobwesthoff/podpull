// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The library behind the podpull CLI. It is internal to podpull and has
//! no API stability guarantees (ADR 18).

// The crate root exports the types in the signatures of what it exports.
// This lint reports a type a frontend can reach but not name.
#![warn(unnameable_types)]

mod damage;
mod episode;
mod error;
mod feed;
mod fs_sync;
mod http;
mod metadata;
mod progress;
mod state;
mod sync;

pub use damage::{DamageKind, DamageRemedy, DamagedAudio};
pub use error::{FeedError, MetadataError, StateError, SyncError};
pub use http::{ByteStream, HttpClient, HttpResponse, ReqwestClient};
pub use progress::{NoopReporter, ProgressEvent, ProgressReporter};
pub use state::UnreadableMetadata;
pub use sync::{
    AudioCheck, FailedEpisode, SyncOptions, SyncResult, UnverifiableAudio, sync_podcast,
};
