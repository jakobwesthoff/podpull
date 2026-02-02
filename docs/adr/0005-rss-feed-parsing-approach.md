# 5. RSS feed parsing approach

Date: 2026-01-31

## Status

Accepted

Related to [9. Date and time handling](0009-date-and-time-handling.md)

## Context

Podcast feeds use RSS 2.0 with iTunes podcast extensions. We need to parse:

**Channel-level data:**

* Title, description, link
* iTunes: author, image, categories

**Item-level data:**

* Title, description, publication date (`pubDate`)
* Enclosure URL (the actual audio file)
* iTunes: duration, episode number, season

The parser must handle real-world feeds that may be malformed or use extensions inconsistently.

## Decision

We will use the **rss** crate for RSS feed parsing.

````rust
use rss::Channel;

let channel = Channel::read_from(xml_bytes.as_slice())?;
for item in channel.items() {
    let title = item.title();
    let enclosure = item.enclosure(); // Audio file URL
    let pub_date = item.pub_date();   // RFC 822 date string
    let itunes = item.itunes_ext();   // iTunes extensions
}
````

**Alternatives considered:**

* `feed-rs`: Supports RSS, Atom, JSON Feed, but more complex API for our RSS-only use case
* `quick-xml`: Manual XML parsing, too low-level
* `atom_syndication`: Atom only, not RSS

## Consequences

**Benefits:**

* Dedicated RSS 2.0 parser with clean API
* Built-in iTunes namespace extension support (crucial for podcasts)
* Handles common malformations gracefully
* Well-maintained, widely used

**Drawbacks:**

* Only supports RSS, not Atom (acceptable for podcast feeds which are predominantly RSS)
* Some podcasts use non-standard extensions that may not be parsed

**Dependencies added:**

* `rss = "2"`

**Related decisions:**

* ADR-0009: Parsing `pubDate` strings with chrono