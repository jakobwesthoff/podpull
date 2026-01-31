# 3. CLI framework selection

Date: 2026-01-31

## Status

Accepted

## Context

podpull requires a CLI interface to accept user input:
- Feed URL or local file path (positional argument)
- Output directory (positional argument)
- Potentially additional flags in the future (verbose, dry-run, etc.)

We need a robust CLI parsing solution that provides good UX (help text, error messages, shell completions) without excessive boilerplate.

## Decision

We will use **clap** (v4) with the `derive` feature for CLI argument parsing.

```rust
use clap::Parser;

#[derive(Parser)]
#[command(name = "podpull", about = "Download and sync podcasts")]
struct Args {
    /// RSS feed URL or path to local RSS file
    feed: String,
    /// Output directory for downloaded episodes
    output_dir: PathBuf,
}
```

**Alternatives considered:**
- `argh`: Simpler, smaller, but fewer features (no shell completions, limited validation)
- `structopt`: Deprecated, functionality merged into clap 3+
- Manual parsing: Too much boilerplate, poor UX

## Consequences

**Benefits:**
- Type-safe argument parsing with derive macros
- Automatic `--help` and `--version` generation
- Built-in validation and error messages
- Shell completion generation available
- Excellent documentation and community support
- Future extensibility (subcommands, complex flags)

**Drawbacks:**
- Compile time impact from proc macros
- Binary size slightly larger than minimal alternatives

**Dependencies added:**
- `clap = { version = "4", features = ["derive"] }`
