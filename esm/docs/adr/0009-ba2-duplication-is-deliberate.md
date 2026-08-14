# `esm/src/ba2.rs` deliberately duplicates the `ba2` crate, rather than depending on it

Status: accepted (2026-08-13)

`esm/src/ba2.rs` (224 lines) is a minimal, read-only, GNRL-only BA2 reader: it parses the same
24-byte BTDX header, the same 36-byte GNRL record layout, and calls the same LZ4 raw-block
decompressor as the sibling `ba2` crate's `src/format.rs` + `src/reader.rs` + `src/compress.rs`.
`esm`'s only two consumers of this module are `src/strings.rs` (`Localization::from_ba2`) and
`src/curves.rs` (loading the Startup BA2's curve-table JSON) — both read-only, both asking only
for "open this archive, read this named entry by path." `ba2/src/lib.rs`'s public
`Ba2Archive::open`/`Ba2Archive::read` already cover that exact surface.

This looks, on the surface, like an oversight: two crates in the same workspace-adjacent repo
independently reimplementing the same binary format, with `esm` simply forgetting to add a
`ba2 = { path = "../ba2" }` dependency. It is not — the duplication was evaluated and kept.

The duplication has already drifted once, which is the sharpest argument for *why* this needs to
stay a recorded decision rather than an implicit assumption: `esm/src/compress.rs`'s
`MAX_DECOMP_SIZE` guard (a 64 MiB cap on any declared decompressed size, added to harden
`decompress_lz4`/`decompress_zlib` against a decompression bomb, with three colocated tests) had no
equivalent in `ba2/src/compress.rs` until this same change set ported it over. Two independent
copies of "decompress an LZ4 block to a declared size" silently diverged on a real security
property for as long as they existed side by side. This ADR is not a plan to prevent that kind of
drift from recurring — by design, keeping the crates separate means it *can* recur — it is naming
the tradeoff so future changes to either copy's safety/behavior are made with eyes open, not
assumed to be mirrored automatically.

## Decision

`esm/src/ba2.rs` stays a separate, minimal copy. `esm` will not add a dependency on the `ba2`
crate to replace it.

Two concrete reasons, both real behavior/build-graph differences, not just "it would be a bigger
refactor":

- **`Codec` semantics differ, and the difference is exactly the kind of divergence you don't want
  silently swapped in.** `ba2::Ba2Archive::read` takes an explicit `Codec` parameter, including
  `Codec::Auto`, which sniffs the first two bytes and transparently accepts either LZ4 or
  zlib-compressed content. `esm/src/ba2.rs::Ba2Archive::read` has no codec parameter — every
  compressed entry (`packed_size != 0`) is unconditionally routed through
  `crate::compress::decompress_lz4`, so a zlib-compressed entry hard-fails (LZ4 decode error) today.
  Depending on `ba2` and calling `read(name, Codec::Auto)` (the only call shape that would let
  `esm` drop its own zlib-agnostic assumption) would silently start accepting zlib-compressed BA2
  content that `esm` currently rejects outright — a real behavior change smuggled in as a "just
  delete the duplicate" refactor, not a transparent swap.
- **`ba2`'s write-side pulls in three dependencies `esm` never needs, with no feature gate to
  exclude them.** `ba2/src/writer.rs` (the two-pass archive writer) and `ba2/src/extract.rs` (glob
  filtering for `extract`) — plus the `create`/`extract` CLI subcommands in
  `ba2/src/bin/cli.rs` — depend on `tempfile` (writer.rs's two-pass temp files), `globset`
  (extract.rs's/cli.rs's `--filter` glob matching), and `walkdir` (cli.rs's directory walk for
  `create`). All three are unconditional entries in `ba2/Cargo.toml`'s `[dependencies]`, not gated
  behind a Cargo feature `esm` could opt out of. `esm` never writes a BA2 (see the crate's own
  "READ-ONLY: no ESM write path exists" invariant) and never will — depending on `ba2` today would
  add `tempfile`/`globset`/`walkdir` to `esm`'s build graph for zero runtime benefit.

## Consequences

- `strings.rs`/`curves.rs` keep reading through `esm/src/ba2.rs`, not `ba2::reader::Ba2Archive`.
  Any future third consumer inside `esm` should do the same — it is not a stopgap awaiting removal.
- A change to BA2 parsing correctness or safety (bounds checks, size caps, header validation) made
  in one copy is **not** automatically mirrored in the other. Whoever touches `ba2/src/compress.rs`,
  `ba2/src/reader.rs`, or `ba2/src/format.rs` for a correctness/security fix should check whether
  `esm/src/ba2.rs`'s independent implementation has the same gap (and vice versa) — this ADR
  documents why the copy exists, it does not make the copies stay in sync on its own.
- If `ba2` ever grows a feature-gated read-only surface (e.g. `default-features = false, features =
  ["read"]` that excludes `writer`/`extract` and their dependencies) **and** `esm` needs to consume
  zlib or DX10 content it doesn't today, that would remove both objections above at once and this
  decision should be revisited then — against what that surface actually offers, not spurred
  speculatively now.
