# 23. Show the root cause of a failure

Date: 2026-11-12

## Status

Accepted

## Context

The reason of a failed download, of unreadable metadata and of unreadable audio was the text of the whole error chain. Every wrapper repeated what it wrapped, so a refused connection read `HTTP request failed for http://127.0.0.1:9/ep2.mp3: error sending request for url (http://127.0.0.1:9/ep2.mp3)`, and the path of a metadata file stood twice in one line. The line did not say that the connection was refused.

## Decision

**The reason is the root cause.** The root is the text of the innermost link of the `source()` chain that has text. The URL that a request error names is left out of it. The text is tidied to read inside a line: an operating system's ` (os error N)` is dropped, and a leading capital is lowercased unless the second character is not a lowercase letter, so `Connection refused` reads `connection refused` and `HTTP error 503` and `EOF while parsing` keep their capitals.

**What the failing step was about stays beside the root.** `FailureReason` holds the `root` and a `Subject`: a URL, a path or none. A display shows `root (subject)`, and shows a URL and a path by their own rules (ADR 22). A failed download reads `connection refused (http://127.0.0.1:9/ep2.mp3)`, an error status `HTTP error 503 (https://example.com/ep.mp3)`, a file that cannot be written `permission denied (/podcasts/a.mp3.partial)`. A failure that is a sentence of its own, such as a leftover partial file, keeps its sentence and names the path as its subject. An audio file that stayed because removing it failed after a failed metadata write is carried beside the reason, and a display shows its name by the rule for names and its cause as text: `could not write the metadata: disk full (/podcasts/a.json); the audio file a.mp3 could not be removed: busy`.

**Warnings name the file and then the root.** `Could not read episode metadata <path>: <root>` and `Could not read <file> to check it against its recorded hash: <root>`. The warnings about a slow disk, a leftover partial file that cannot be removed and damaged audio have no cause text and keep their words.

**A feed URL that cannot be parsed fails as that before any request.** The sync reports `FeedError::InvalidUrl` with the parse error as its cause.

## Consequences

`FailedEpisode.reason` and the reason of a download failure are a `FailureReason` (root, subject and an optional leftover audio file), not a text. `UnreadableMetadata` and `UnverifiableAudio` hold the root as text next to their path or file name.
