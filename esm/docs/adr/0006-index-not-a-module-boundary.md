# `Index` is not a module boundary separate from `Database`

Status: accepted (2026-08-10)

`Index` (`src/index.rs`) held the five lazy `Section<...>` cache fields plus their `ensure_*_index`
build methods, while `Database` (`src/lib.rs`) held `esm`/`schema`/`is_localized`/`localization`/
`curves` and embedded an `Index`. The two never had independent lifecycles — an `Index` on its own
cannot build `edid`/`search`/`xref` (those builds decode records, which needs the mmap'd ESM and
schema `Database` owns) and a `Database` is useless without its `Index`. The split was pure module
boundary, not a capability boundary, and it cost more than it bought:

- `ensure_xref_index` needed five of `Database`'s own fields handed back in as parameters —
  `self.index.ensure_xref_index(&self.esm, &self.schema, self.is_localized,
  self.localization.as_ref(), self.curves.as_ref())` — at its one real call site
  (`Database::referenced_by`), and `ensure_edid_index`/`ensure_search_index` needed one or two more.
- `Registry::warm_indexes` had to destructure `Database` field-by-field (`let crate::Database { esm,
  index, is_localized, schema, localization, curves, .. } = &mut *db;`) purely to borrow `index`
  mutably alongside the fields each `ensure_*_index` call needed — a tell that the fields those
  methods needed lived on the wrong type.
- That parameter-threading was also why every `Database` field but one (`filter_cache`) was `pub`:
  nothing *inside* `Database` needed them exposed: `Index`'s methods, living in a different module,
  did.

## Decision

`Index` stays as a struct — it is still the natural place for the five `Section<...>` fields, `path`,
and every **pure read** over them (`get_by_formid`, `contains`, `records_by_type`, `tree()`, etc.,
none of which need anything `Database` alone has). But the three **build** methods
(`ensure_edid_index`/`ensure_search_index`/`ensure_xref_index`) moved to `impl Database`, where they
close over `self` directly and take no extra parameters. The data each build computes moved with a
matching split: `index.rs` gained three free `pub(crate)` functions
(`build_edid_section`/`build_search_section`/`build_xref_section`) that take `&Index` plus whatever
raw inputs they need and return the section's data — keeping each section's construction logic
colocated with its own type — while `Database`'s three methods own the surrounding acquire/recheck/
write/publish protocol (`progress::BuildLease::acquire_or_recheck` +
`rkyvcache::write_and_remap`, see ADR-0007) and are the only place that assigns the result back onto
`self.index`.

`Registry::warm_indexes` no longer destructures anything — `db.ensure_edid_index()?` is a single
mutable borrow of `db`, nothing else to hold disjointly. `Database`'s fields dropped to `pub(crate)`
except `is_localized` (read directly by `src/bin/cli.rs`'s `diff` command, across the bin/lib crate
boundary, so it has to stay `pub`).

The three "ensure, then get" idioms this crate had (an `assert!` after `ensure_search_index`, nothing
at all before `get_xref`, and four separate `.expect("populated by ensure_filter_cache")` panics for
the filter-cache trio) collapsed into one shape: `Index::get_xref`/`iter_search`/`get_by_edid` are now
`pub(crate)`, reachable only through `Database` wrapper methods
(`xref_lookup`/`resolve_edid_indexed`, plus `ensure_search_index` before `search()`'s own
`iter_search` call) that ensure internally — there is no code path left, in this crate, that can read
a lazy index's data without having just guaranteed it is built. The filter-cache `.expect()`s became
one `filter_cache_entries` helper returning `anyhow::Result`, matching this crate's `anyhow::Result`
convention instead of being the one panic-shaped exception to it.

## Consequences

- No caller anywhere in the crate ever again needs to pass `Database`'s other fields into an
  `ensure_*_index` call, or destructure `Database` to get around the borrow checker.
- `tests/curves.rs` (an external integration-test crate) still calls `Index::build(&esm)` directly to
  get a `&Index` for `CurveIndex::build` — `Index` stays a `pub` type with a `pub fn build`/`pub fn
  empty` for exactly that reason. Its five `Section` fields and lazy-index accessors are `pub(crate)`,
  invisible outside this crate either way.
- A future architecture review should not re-propose splitting index-cache concerns back out of
  `Database` into a second top-level type — the two do not have, and have never had, independent
  lifecycles. If a genuinely new capability needs its own lifecycle (e.g. something that outlives a
  single `Database::open`), that is a different question from where the *existing* five sections live.
