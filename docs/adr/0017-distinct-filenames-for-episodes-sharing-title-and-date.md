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

**Names are assigned by the sync plan.** `create_sync_plan` gives every planned download its audio and metadata filename (`PlannedDownload`), in download order, before any download starts.

**Every existing file claims its stem.** The directory scan collects the stem of every file except `.partial` files, whether or not readable metadata names its episode. Stems are compared by a claim key: Unicode NFC normalization followed by lowercasing (`filename_claim_key`). A planned name is added to the claims as soon as it is assigned.

**A colliding episode takes the first free candidate:**

1. the base stem, `YYYY-MM-DD-<title>`;
2. the publication time added, `YYYY-MM-DD-HHMMSS-<title>`, rendered in the offset the feed states;
3. the base stem followed by the first 8 hex digits of the SHA-256 of the GUID, for undated episodes or when the time is taken too;
4. that hashed stem followed by a counter.

**A changed GUID is a collision like any other.** When a feed re-issues an episode under a new GUID, the earlier files stay and the new copy is stored under a free name.

**Downloads never share a partial file.** `download_episode` creates its `.partial` file exclusively and fails if it already exists. There is no existence check before the final rename, because on the smbfs share above such a check reports an existing file as missing. Distinct paths are guaranteed by the plan.

**Episode metadata is written atomically**, through a `.partial` file that is renamed into place.

**Audio a collision points at is verified, and repaired only on request.** When a planned episode's base stem belongs to an episode with readable metadata and a recorded `content_hash`, that audio file is hashed before downloads start. A mismatch is reported as `StoredAudioMismatch`. By default it is listed among the failed episodes and the file is left untouched. With `SyncOptions::repair_mismatched_audio` (CLI `--repair`), an episode still in the feed is downloaded again under its existing audio and metadata filenames; this repair is not subject to `--limit`.

**Unreadable metadata is reported** as `MetadataUnreadable`. Its stem stays claimed.

## Consequences

- Episodes sharing title and date are all kept, and repeated syncs no longer download them again.
- Which episode of a colliding pair keeps the base name depends on download order: newest first within one run, otherwise whichever was downloaded first.
- An audio file without readable metadata keeps its name. If it belongs to an episode still in the feed, that episode is stored a second time under a free name.
- A feed that re-issues its episodes under new GUIDs leads to a second copy of each re-issued episode.
- `generate_filename` returns the base name only, which can collide. Library users get collision-free names from the `to_download` entries of `SyncPlan`.
