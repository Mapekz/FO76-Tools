# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

Anyone who wants to datamine and look up Fallout 76 game records with ease. Fallout 76 cannot be modded (no supported path to alter game logic), so this serves research/datamining use, not mod authoring — the same audience xEdit/TES5Edit already serves for other Bethesda titles, applied to a game where the only thing to build is understanding, not mods.

## Product Purpose

FO76 ESM Viewer is a desktop GUI for browsing, searching, diffing, and cross-referencing decoded Fallout 76 ESM (game data) records — record tree/table navigation, full record detail, search, filtering, referenced-by lookups, snapshot diffing, and schema decode-coverage reporting. It is strictly read-only; no write/save path exists or is planned, matching the underlying `esm` engine's core invariant.

## Positioning

A faster, Rust-based, cross-platform clone of TES5Edit/xEdit: the incumbent tool is Pascal, Windows/Wine-only, and blocks on backend processing. This product's primary differentiator is cross-platform reach (Electron, ships on Windows/Mac/Linux), backed by a UI that stays warm and loads data quickly (a persistent background daemon in the `esm` engine) instead of reloading or blocking per query.

## Operating Context

- Browsing and searching decoded records from FO76 ESM snapshots
- Diffing two ESM snapshots to see what changed between game patches
- Tracing FormID cross-references via referenced-by lookups (e.g. "what drops this item")
- Checking schema decode coverage to see what's fully understood vs. raw/unmapped data
- Querying against a warm background daemon (from the `esm` Rust crate) for fast repeated lookups instead of reloading the ESM per query

## Capabilities and Constraints

- Strictly read-only: no write/save path exists or is in scope — this is a permanent constraint, not a deferred feature
- Depends on `esm/bindings/napi`, a native addon built from the `esm` Rust crate; the UI's capabilities are bounded by what that addon exposes
- Built with Electron + React + TypeScript, packaged via electron-builder for cross-platform distribution — parity with xEdit's install base (Windows) plus Mac/Linux it can't reach
- No signed/distributed release channel established yet (packaging currently `electron-builder --dir` only) — open/undecided

## Brand Commitments

Name: "FO76 ESM Viewer." No further brand identity (logo, palette, voice) established yet.

## Evidence on Hand

None on hand — no testimonials, user research, or case studies exist. Future work must not fabricate any.

## Product Principles

1. Read-only, always — never add a feature that writes back to a game file.
2. Cross-platform reach is the headline advantage over TES5Edit/xEdit — never regress Windows/Mac/Linux parity for a platform-specific shortcut.
3. Stay fast and warm — favor architecture and UI choices that avoid blocking on backend processing; datamining workflows involve many rapid successive lookups.
4. Show decode coverage honestly — never hide unknown/unmapped record data behind a falsely "complete" looking view.
5. Track the `esm` engine's schema/decoder as the source of truth — the UI reflects what the engine can decode, not a hand-maintained parallel model.
