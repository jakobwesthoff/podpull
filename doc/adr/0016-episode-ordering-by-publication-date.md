# 16. Episode ordering by publication date

Date: 2026-01-31

## Status

Accepted

## Context

The `--limit N` option allows users to download only a subset of episodes from a feed. However, the behavior was non-deterministic because:

* RSS feeds don't guarantee any particular episode ordering
* Different feeds use different ordering conventions (some newest-first, some oldest-first)
* The same feed might change order between fetches

This meant `--limit 10` could download any 10 episodes, not necessarily the most recent ones. Users typically want the most recent episodes when using `--limit`.

## Decision

The sync plan sorts episodes by `pub_date` (newest first) before returning them:

```rust
to_download.sort_by(|a, b| match (&b.pub_date, &a.pub_date) {
    (Some(b_date), Some(a_date)) => b_date.cmp(a_date),
    (Some(_), None) => std::cmp::Ordering::Greater, // b has date, a doesn't => b comes first
    (None, Some(_)) => std::cmp::Ordering::Less,    // a has date, b doesn't => a comes first
    (None, None) => std::cmp::Ordering::Equal,
});
```

**Ordering rules:**

1. Episodes with `pub_date` are sorted newest-first
2. Episodes without `pub_date` are placed at the end
3. Episodes without `pub_date` preserve their relative RSS feed order (stable sort)

## Consequences

**Benefits:**

* `--limit N` now means "download the N most recent episodes"
* Deterministic behavior across syncs
* Intuitive default for podcast consumption (newest content first)
* Works with Rust's stable sort, preserving relative order of equal elements

**Trade-offs:**

* Episodes without publication dates may never be downloaded when using `--limit`
* Slight overhead from sorting (negligible for typical feed sizes of <1000 episodes)

**Edge cases:**

* Feeds with no `pub_date` on any episode: Falls back to RSS feed order
* Feeds with mixed dated/undated episodes: Dated episodes first, then undated
