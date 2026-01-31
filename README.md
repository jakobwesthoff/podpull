# podpull

A fast, minimal CLI tool for downloading and synchronizing podcasts from RSS feeds.

Built for podcast lovers who want to **backup their favorite shows** and maintain **offline access** to their personal audio library. No cloud services, no accounts, no tracking — just your podcasts, stored locally, under your control.

## Features

- **Download podcasts** from any RSS feed URL or local RSS file
- **Metadata preservation** — saves episode information as JSON alongside audio files
- **Smart synchronization** — detects already downloaded episodes and only fetches new ones
- **Stateless operation** — no database or configuration required; the output directory *is* the state

## Installation

Build from source:

```bash
git clone https://github.com/jakobwesthoff/podpull.git
cd podpull
cargo build --release
```

## Usage

```bash
podpull <feed> <output-dir>
```

### Sync from a URL

```bash
podpull https://example.com/podcast/feed.xml ~/Podcasts/my-favorite-show/
```

### Sync from a local RSS file

```bash
podpull ./feed.xml ~/Podcasts/my-favorite-show/
```

### Keep your backup up to date

Simply run the same command again. podpull detects existing episodes and only downloads what's new:

```bash
podpull https://example.com/podcast/feed.xml ~/Podcasts/my-favorite-show/
# => "42 episodes already downloaded, 3 new episodes to fetch"
```

## Output Structure

podpull organizes downloads in a clean structure:

```
~/Podcasts/my-favorite-show/
├── podcast.json
├── 2024-01-15-episode-title.mp3
├── 2024-01-15-episode-title.json
├── 2024-01-08-another-episode.mp3
└── 2024-01-08-another-episode.json
```

JSON metadata format is TBD.

## Why podpull?

Podcasts disappear. Feeds go offline. Hosting changes. Episodes get pulled.

If you have podcasts you truly care about, the only way to guarantee access is to keep your own copy. podpull makes that easy — point it at a feed, run it periodically, and rest easy knowing your favorite shows are safely backed up.

## License

MPL-2.0
