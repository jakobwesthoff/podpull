# 2. Project structure: library and binary

Date: 2026-01-31

## Status

Accepted

Enables [7. Error handling strategy](0007-error-handling-strategy.md)

## Context

podpull is a CLI tool for downloading podcasts, but its core functionality (fetching feeds, parsing RSS, downloading episodes, managing metadata) could potentially be useful to other Rust applications or embedded in different interfaces (GUI, web service, etc.).

We need to decide whether to structure the project as a single binary or split it into a library and binary.

## Decision

We will structure podpull as a Cargo workspace-style project with:

* **Library** (`src/lib.rs`): Contains all core logic including feed fetching, RSS parsing, download management, metadata handling, and error types
* **Binary** (`src/main.rs`): A thin CLI wrapper that handles argument parsing, progress display, and user interaction

The library will expose a clean public API that the binary consumes, and could be consumed by other applications.

## Consequences

**Benefits:**

* Core logic is reusable by other Rust applications
* Clear separation of concerns between business logic and user interface
* Easier to test core functionality in isolation
* Enables different frontends (CLI, GUI, etc.) to share the same logic
* Forces explicit API design for the library boundary

**Drawbacks:**

* Slightly more complexity in project structure
* Must maintain a stable library API even for internal use
* Some duplication may occur at the boundary (e.g., re-exporting types)

**Implications for error handling:**

* Library uses typed errors (`thiserror`) that callers can match on
* Binary wraps these in `anyhow` for user-friendly CLI output

See ADR-0007 for the error handling strategy that builds on this decision.