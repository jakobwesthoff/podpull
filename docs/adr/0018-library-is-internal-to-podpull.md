# 18. Library is internal to podpull

Date: 2026-10-06

## Status

Accepted

Supersedes [13. Library public API design](0013-library-public-api-design.md)

Amends [2. Project structure: library and binary](0002-project-structure-library-and-binary.md)

## Context

ADR 2 split podpull into a library and a binary and stated that the library could be consumed by other applications. ADR 13 designed a layered public API for such consumers.

The binary is a crate of its own and uses the library through its exports, like any outside consumer. Measures meant for outside consumers therefore applied to the CLI. `#[non_exhaustive]` made the CLI match `ProgressEvent`, `DamageRemedy` and `DamageKind` with catch-all arms. With every module public, rustc treated every public item as used and reported none of the library code that nothing called.

## Decision

The library is internal to podpull. Only podpull's own frontends use it: the CLI, and possibly further frontends of our own. It gives no API stability to anyone else.

- The library's modules are private. The crate root exports what the frontends use, plus the types that appear in the signatures of those items. An item is exported once a frontend needs it.
- Library types are not `#[non_exhaustive]`.
- Version numbers follow the behaviour of the CLI. A change to the library alone is not a breaking change.
- The changelog lists changes CLI users notice. Library changes are recorded in commits and ADRs.

## Consequences

- rustc reports library code that no frontend reaches as dead code.
- The CLI matches progress events and damage types exhaustively, so a new variant fails to compile until the CLI handles it.
