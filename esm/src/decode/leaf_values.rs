//! Registry of "value-bearing leaf" record types: types whose entire useful
//! payload is small and bounded enough to inline flat onto a FormID
//! reference's stub, rather than leaving the reader to make a second `get`.
//!
//! A type earns a row here only if it passes all three: (1) bounded scalar
//! payload — not an unbounded list (FLST) or a record with real prose (AVIF);
//! (2) the EditorID doesn't already tell the reader the value — ruling out
//! KYWD, whose decoded body is an editor swatch and whose EditorID *is* the
//! semantics; (3) it is actually referenced by other records — ruling out
//! GMST, whose payload is a perfect scalar leaf that nothing can point at
//! ($FO76_ESM_PATH$: 0 real referrers, verified via `esm refs`, 2026-08).
//! GLOB and CURV pass; see `esm/docs/adr/0011-value-bearing-leaf-inlining.md`
//! for the full membership analysis and for other candidates that were
//! considered and rejected (KYWD, AVIF, FLST, GMST, DMGT, EQUP, ...).
//!
//! The two rows here are NOT interchangeable, and deliberately so:
//!
//! - [`InlineSource::CurveIndex`] (CURV) surfaces data that lives *outside*
//!   the ESM entirely — a CURV record's schema is only `EDID` plus two file
//!   path strings; the actual points live in an external curve-table JSON
//!   loaded from the Startup BA2 (`crate::curves`). Nothing else can ever
//!   surface them, so this variant is consulted even at `ResolveDepth::None`
//!   (see `resolve_formid`'s first branch) — it needs no resolver at all.
//! - [`InlineSource::Fields`] (GLOB) lifts data that is already *inside* the
//!   record — it exists purely to save a second `get` round-trip. It fires
//!   only at `Stub`/`Full`, which requires a resolver. Firing it at `None`
//!   too would (a) break `--resolve none`'s documented contract of leaving
//!   FormIDs untouched, and (b) force every `None` decode to build a
//!   resolver it doesn't otherwise need (`Database::record_at_meta_with_depth`
//!   skips resolver construction entirely at `None` today) — including
//!   `diff::run`, which decodes both snapshot sides at `None` and can process
//!   tens of thousands of records in one call. (Measured, not assumed: gating
//!   was *not* chosen to suppress diff re-stamp noise — a byte-equality fast
//!   path in `diff::run` already skips unchanged records before either side
//!   is decoded, and across all locally available snapshot diffs, zero
//!   changed GLOB records had any referrer that was itself in the same
//!   week's changed set. See the ADR for the numbers.)
//!
//! No memoization: `DatabaseResolver::stub` already calls `parse_record_at`,
//! which decompresses and splits every subrecord out of the target record —
//! the cost `leaf_inline` adds on top is one schema decode over a record with
//! at most a handful of members. The worst case measured against a real ESM
//! was 43 GLOB references on a single LVLI record. Caching would mean giving
//! `DatabaseResolver` (a `Send + Sync` trait object) interior mutability for
//! a cost this small — not worth it unless a future row's target type is
//! measured to be expensive.

/// How a value-bearing leaf record type's payload is inlined onto a FormID
/// reference at `--resolve stub`/`full` (or, for [`CurveIndex`](Self::CurveIndex),
/// at any resolve depth).
pub(crate) enum InlineSource {
    /// Lift these decoded field names off the target record, flat, alongside
    /// the three stub keys (`formid`/`editor_id`/`record_type`). Needs a
    /// resolver, so only ever consulted at `Stub`/`Full`.
    Fields(&'static [&'static str]),
    /// Replace the stub with the curve-index projection
    /// (`{formid, editor_id, curve_path, curve}` — no `record_type`, see the
    /// module doc). Needs no resolver; consulted at every resolve depth via
    /// the declaring field's `valid_refs`.
    CurveIndex,
}

/// Value-bearing leaf record types, keyed by record signature. Adding a type
/// is one row; see the module doc for the membership test new rows must pass.
pub(crate) const VALUE_BEARING_LEAVES: &[(&str, InlineSource)] = &[
    ("GLOB", InlineSource::Fields(&["Value"])),
    ("CURV", InlineSource::CurveIndex),
];

/// Look up the inline source for a record signature (e.g. `"GLOB"`, `"CURV"`).
pub(crate) fn lookup(signature: &str) -> Option<&'static InlineSource> {
    VALUE_BEARING_LEAVES
        .iter()
        .find(|(sig, _)| *sig == signature)
        .map(|(_, source)| source)
}
