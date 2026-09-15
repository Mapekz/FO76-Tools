//! Whole-record decode goldens: `decode_record` against verbatim subrecord
//! bytes captured from `esm get --raw`, so the whole suite runs in CI with no
//! game data.
//!
//! Cargo compiles `tests/decode_records/main.rs` as a single test binary named
//! `decode_records`, so `cargo test --test decode_records` still selects the
//! whole suite. The modules below split the fixtures by record family — the
//! decoder itself is schema-driven and has no per-record source module, so
//! family is the only axis that actually partitions these tests. Engine-level
//! regressions (scoping, optional trailers, array partitioning, VMAD layouts)
//! live beside the record whose bytes provoked them, where the reproducing
//! fixture is.
//!
//! Shared helpers come from `tests/common/mod.rs`, pulled in below by path
//! because Cargo only treats a `tests/` *subdirectory* as a plain module.

#[path = "../common/mod.rs"]
mod common;

mod actors;
mod effects;
mod items;
mod leveled;
mod perks;
mod perks_stat;
mod pet_tracks;
mod primitives;
mod progression;
mod quests;
mod races;
mod weapons;
mod world;
