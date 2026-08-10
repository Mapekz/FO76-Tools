# A cache section owns its own identity — kind, fingerprint, and recheck protocol in one place

Status: accepted (2026-08-10)

Nothing bound a `rkyvcache::SectionKind` to its layout-fingerprint constant or its archived rkyv
type. The `(SectionKind, CACHE_VERSION, LAYOUT_FINGERPRINT)` triple `Section::map`/`write_section`
need was spelled out by hand at roughly 41 call sites across `index.rs` and `tree.rs`. A wrong
pairing — e.g. accidentally passing `EDID_LAYOUT_FINGERPRINT` while mapping the `search` file —
**compiles fine** and produces a section that permanently reads as `Section::Absent` (its stored
fingerprint never matches what the caller now asks for), silently forcing a full rebuild on every
single open. No test caught this class of bug, because nothing tied the two together in the first
place. Three smaller versions of the same "spelled out by hand, no test to catch a slip" pattern sat
next to it:

- The five-step *write → drop → re-map → assert-mapped* protocol for publishing a freshly-built
  section was copy-pasted five times verbatim (`ensure_edid_index`/`ensure_search_index`/
  `ensure_xref_index`, plus `build_tree_and_forms`'s two writes).
- `progress::BuildLease::acquire`'s doc comment said, in capitals, that "every caller MUST re-check
  whether the section it wanted now exists immediately after this returns" — four call sites
  implemented that recheck by hand, identically. A missed one would cost a redundant multi-minute
  rebuild, silently, with no test catching it either.
- `progress::BuildStage` and `rkyvcache::SectionKind` were the same five variants declared twice in
  two files, manually paired in `index.rs`'s `cache_inventory`. Adding a sixth section touched at
  least 8 places across 4 files with zero compiler enforcement that all 8 were actually updated.

## Decision

**One trait binds each section's kind, fingerprint, and archived type together, in exactly one
place.** `rkyvcache::SectionSpec` is implemented once per section, in an `impl SectionSpec for
rkyv::Archived<X>` block sitting directly next to `X`'s own type definition (`tree.rs` for
`TreeIndex`; `index.rs` for `FormsSection`/`EdidSection`/`SearchSection`/`XrefSection`):

```rust
pub(crate) trait SectionSpec: /* rkyv bounds */ {
    const KIND: SectionKind;
    const LAYOUT_FINGERPRINT: u64;
}
```

`Section::<A>::map(path, sig, cache_version)` and `write_section::<T>(path, sig, cache_version,
value)` now read `KIND`/`LAYOUT_FINGERPRINT` off the type parameter itself instead of taking them as
caller-supplied arguments. Since Rust's coherence rules permit at most one `SectionSpec` impl per
archived type, there is now exactly one place in the whole crate where a section's kind/fingerprint
pairing is chosen — the same "wrong pairing" bug class is no longer expressible, not just less
likely. (The old explicit-parameter forms survive as `Section::map_raw`/`write_section_raw`, used
only by `rkyvcache.rs`'s own adversarial tests, which deliberately construct mismatched pairings to
prove `Section::map` rejects them.) A test (`index.rs`'s
`section_spec_pairing_matches_named_fingerprint_constants`) asserts all five `SectionSpec` impls
agree with their named `_LAYOUT_FINGERPRINT` constants and have five distinct `KIND`s.

**The write→drop→re-map→ensure-mapped protocol is one function**, `rkyvcache::write_and_remap`,
used by all five section builds (the three lazy `ensure_*_index` methods on `Database`, plus
`build_tree_and_forms`'s two writes).

**`BuildLease::acquire_or_recheck` makes the recheck impossible to skip**, not just documented as a
MUST. It runs the caller-supplied recheck closure itself, immediately after acquiring the lock, and
returns an `Acquired<T>` enum with two arms: `AlreadyBuilt(T)` (another process finished first — the
lease is dropped without this caller building anything) or `NeedsBuild(BuildLease)`. There is no
third way to obtain a live `BuildLease` — the only path to one runs through a recheck that has
already found the section missing. All four hand-written recheck call sites (three `ensure_*_index`
methods, `build_tree_and_forms`) now go through this.

**`SectionKind` is a crate-private alias for `progress::BuildStage`**, not a second enum —
`pub(crate) use crate::progress::BuildStage as SectionKind` in `rkyvcache.rs`. `BuildStage` gained
`#[repr(u32)]` with explicit discriminants preserving the pre-existing on-disk values (`Tree = 1`,
`Forms = 2`, `Edid = 3`, `Search = 4`, `Xref = 5`) so this is not a cache-invalidating change. Every
exhaustive `match` over the type (`BuildStage::label`/`unit`, `index.rs`'s `cache_inventory`, the
`SectionSpec`-pairing test) has no wildcard arm, so adding a sixth section without updating all of
them is a compile error, not a silent gap.

**`CacheSig` is still read once per build call**, not cached on `Database`/`Index` across their whole
lifetime. Considered and rejected: each of the three lazy `ensure_*_index` methods, plus
`Index::build`, is a legitimate point at which the ESM's on-disk identity could have changed since
the last read (a snapshot swap mid-session — exactly what `create_esm_archive.sh` does routinely) and
each already reads `CacheSig::read` right before using it, cheaply (one `fs::metadata` call). Caching
it once at construction and never refreshing it would reintroduce the two-different-ESM-identities
risk this ADR's fingerprint work is trying to close, for a syscall cost that was never the bottleneck
in any measured build.

## Consequences

- Adding a sixth cache section now means: define its type, add one `impl SectionSpec` block next to
  it, add one `BuildStage` variant (compiler then forces every exhaustive match to handle it), and
  write its `build_*_section` data function plus its `Database::ensure_*_index` orchestration method.
  There is no longer a place to *forget* the kind/fingerprint pairing, because there is no longer a
  place that spells it out by hand.
- A future architecture review should not re-propose "just be careful to keep the triples in sync" as
  a sufficient fix for this class of bug — this ADR's decision is that the type system enforces it
  instead.
