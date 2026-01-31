# 15. File integrity hashing

Date: 2026-01-31

## Status

Accepted

## Context

Episode metadata JSON files store information about downloaded audio files but provide no way to verify that the metadata actually corresponds to the audio file on disk. This creates several issues:

* **Integrity verification**: No way to detect if an audio file was corrupted after download
* **Metadata-file mismatch**: If a file is manually replaced or corrupted, metadata doesn't reflect this
* **Deduplication potential**: Cannot identify duplicate episodes across different feeds without re-downloading

The `sha2` crate provides efficient SHA-256 hashing that can be computed incrementally during streaming downloads with minimal performance impact.

## Decision

We compute a **SHA-256 hash during download** and store it in the episode metadata:

1. **Streaming hash computation**: Hash is computed as chunks are downloaded, avoiding double I/O
2. **Prefixed format**: Hash stored as `"sha256:{hex_digest}"` for future algorithm flexibility
3. **Metadata field**: New optional `content_hash` field in `EpisodeMetadata`

```rust
// During download
let mut hasher = Sha256::new();
while let Some(chunk) = stream.next().await {
    hasher.update(&chunk);
    file.write_all(&chunk).await?;
}
let content_hash = format!("sha256:{:x}", hasher.finalize());
```

**Metadata structure:**
```rust
pub struct EpisodeMetadata {
    // ... existing fields ...
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
}
```

**Example metadata output:**
```json
{
  "title": "Episode 42",
  "audio_filename": "2024-01-15-episode-42.mp3",
  "content_hash": "sha256:a3f2b8c9d4e5f6..."
}
```

**Return type change:**
```rust
pub struct DownloadResult {
    pub bytes_downloaded: u64,
    pub content_hash: String,
}
```

## Consequences

**Benefits:**

* **Future integrity verification**: Can verify files haven't been corrupted
* **Deduplication support**: Identical content produces identical hashes regardless of source
* **Minimal overhead**: Streaming computation adds ~100ms per file on typical hardware
* **Algorithm flexibility**: Prefixed format allows future migration to other hash algorithms
* **No extra I/O**: Hash computed during download, not as separate pass

**Trade-offs:**

* Additional dependency (`sha2` crate)
* Slightly larger metadata files (~70 bytes per episode)
* Hash computation uses CPU during download (negligible impact)

**Future possibilities enabled:**

* Integrity verification command (`podpull verify`)
* Cross-feed deduplication
* Resume interrupted downloads by comparing partial hash (would require protocol changes)
* Content-addressable storage

**Alternatives considered:**

* **MD5**: Faster but cryptographically broken, unsuitable for integrity verification
* **CRC32**: Fast but not collision-resistant, better for error detection than integrity
* **No hashing**: Simpler but loses integrity verification capability
* **Post-download hashing**: Would require reading file twice, doubling I/O
