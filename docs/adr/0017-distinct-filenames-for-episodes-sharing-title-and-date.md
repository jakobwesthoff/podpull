# 17. Distinct filenames for episodes sharing title and date

Date: 2026-10-06

## Status

Accepted

Amends [13. Library public API design](0013-library-public-api-design.md)

## Context

An episode's filename is built from its publication date and sanitized title: `YYYY-MM-DD-<title>.<ext>`, with its metadata in `YYYY-MM-DD-<title>.json`. Feeds can contain distinct episodes, with different GUIDs and enclosures, that share title and publication day. For example, a feed may list two episodes titled "Listener Questions", both published on 2024-01-08, one at 09:30:00 and one at 10:15:00.

Up to version 1.1.2 both episodes of such a pair were given the same filename:

- Downloaded concurrently, both wrote into the same `.partial` file. One rename succeeded and left a file holding bytes of both downloads; the other rename failed.
- Only one GUID ended up in the shared metadata file. Every following sync downloaded the other episode again and overwrote the first.

Comparing names as plain strings is not enough to detect such a collision. A network share mounted on macOS (smbfs) lists "ä" decomposed into "a" and a combining diaeresis although podpull wrote the composed form, and a lookup of the composed name on that share reports the file as missing. APFS treats names differing only in letter case as the same file.

## Decision

**Names are assigned by the sync plan.** `create_sync_plan(episodes, state, limit)` applies the limit and gives every planned download its stem and audio extension (`PlannedDownload`), in download order, before any download starts. A feed item repeating a GUID already listed is dropped, so the newest listing of an episode is planned once.

**Every existing file claims its stem.** The directory scan collects the stem of every file except `.partial` files, whether or not readable metadata names its episode. Stems are compared by a claim key: Unicode NFC normalization followed by lowercasing (`filename_claim_key`). A planned name is added to the claims as soon as it is assigned.

**A colliding episode takes the first free candidate:**

1. the base stem, `YYYY-MM-DD-<title>`;
2. the publication time added, `YYYY-MM-DD-HHMMSS-<title>`, rendered in the offset the feed states;
3. the base stem followed by the first 8 hex digits of the SHA-256 of the GUID, for undated episodes or when the time is taken too;
4. that hashed stem followed by a counter.

**A changed GUID is a collision like any other, unless the entry replaced a stored episode with identical audio.** A new download is adopted onto a stored episode only if all of these hold: none of the stored episode's GUIDs is still in the feed, title and exact publication time are the same, the stored episode records the download's content hash, and its file, hashed again, still holds those bytes. The download is then discarded and its GUID added to the stored episode's `additional_guids`, which the scan counts as downloaded. Otherwise the earlier files stay and the new download is stored under a free name; entries listed side by side stay separate episodes even with identical audio. One exception covers feeds without GUIDs, where the enclosure URL is the identity: an episode whose URL differs from a stored episode's only before the file name, with the same title and the same publication time to the second, counts as already downloaded without a download.

**Downloads never share a partial file.** The audio `.partial` file is created exclusively; an existing one fails the download with a message to delete it. There is no existence check before the final rename, because on the smbfs share above such a check reports an existing file as missing. Distinct paths are guaranteed by the plan within one run. Two podpull runs on the same directory at once are not supported: each run's scan removes every `.partial` file, including those of the other run.

**Audio and metadata are put in place together.** The audio is downloaded to its `.partial` file, the episode metadata is written to its own `.partial` file, both are synced to disk, and then the audio and the metadata are renamed in that order. `podcast.json` is written through a `.partial` file as well. If the metadata cannot be put in place, a fresh download's audio is removed again; a repair keeps its audio, because the existing metadata still names that file.

**Stored audio is checked against its recorded hash** (`SyncOptions::audio_check`). By default only audio under a planned download's base name is checked, since only there could podpull 1.1.2 and earlier leave the bytes of two episodes. `AudioCheck::Verify` (`--verify`) checks every stored file. `AudioCheck::Repair` (`--repair`) checks every stored file and downloads a mismatched episode again under its existing names, outside the limit, if the feed still lists it in the same audio format. Every other mismatch is left untouched and reported in `SyncResult::damaged` with a `DamageRemedy`, because audio tags edited by the user also cause a mismatch.

**Metadata problems are told apart.** A metadata file whose content is not valid metadata, including text that is not UTF-8, is reported as `MetadataUnreadable` and keeps its stem claimed. Any other error reading one stops the scan, since it may be a passing network failure.

**Public library types are `#[non_exhaustive]`.** `SyncOptions`, `SyncResult`, `ProgressEvent`, `PlannedDownload`, `StoredEpisode`, `CheckTarget`, `DamagedAudio`, `DamageRemedy`, `AudioCheck`, `SyncPlan` and `UnreadableMetadata` can gain fields or variants without breaking library users.

## Consequences

- Episodes sharing title and date are all kept, and repeated syncs no longer download them again.
- Which episode of a colliding pair keeps the base name depends on download order: newest first within one run, otherwise whichever was downloaded first.
- An audio file without readable metadata keeps its name. If it belongs to an episode still in the feed, that episode is stored a second time under a free name. An interruption between the audio rename and the metadata rename still leaves such a file.
- A feed that re-issues its episodes under new GUIDs costs one download per episode; only episodes whose audio changed are stored a second time.
- Separate entries with byte-identical audio are stored as separate copies.
- A re-issue that also changed the title or publication time is stored a second time.
- `--verify` and `--repair` read the whole archive; on a network share that is a full transfer of every audio file.
- `generate_filename` returns the base name only, which can collide, and is deprecated. Library users get collision-free names from the `to_download` entries of `SyncPlan`.
- Code outside the crate builds `SyncOptions` and `SyncResult` from `Default`, uses constructors for `DamagedAudio` and `PlannedDownload`, and needs a catch-all arm when matching `ProgressEvent` or `DamageRemedy`.
