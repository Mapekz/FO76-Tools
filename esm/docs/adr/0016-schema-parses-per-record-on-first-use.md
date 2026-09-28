# The embedded schema is per-record JSON, validated at build time and parsed on first use

Status: accepted (2026-09-28).

The decoder reads its record definitions from `schema/fo76.json`, embedded in the binary. Parsing
the whole 2.6 MB document on every database open cost ~6 ms, most of what a warm open then took.
The alternative was an archived (rkyv) schema, prepared by `build.rs` and read zero-copy.

## Decision

`build.rs` splits `fo76.json` into one raw-JSON string per record type. It also parses and
validates every definition with the decoder's own types (`src/schema/defs.rs`), so an invalid
definition fails the build. At run time `Schema` parses a record type's definition the first time
a decode needs it. It does not archive the schema.

- The split alone took schema loading from ~2 ms to ~22 µs and a warm `esm get` from ~3.6 ms to
  ~1.5 ms. A query touches a handful of record types, and each parses in microseconds, once per
  open database (each `Database::open` loads its own `Schema`).
- Build-time validation gives the guarantee an archived schema would have: an embedded
  definition the decoder can't interpret never reaches a user.
- Archiving would put rkyv derives on the whole definition tree (recursive enums, decider maps)
  and keep a second representation of every definition in step with the serde one. That buys no
  measurable open time over the split.

## Consequences

- The first decode of a record type in an open database pays that type's parse; later ones in
  that database don't. A process that opens two databases (a diff) parses a shared type twice.
- A schema loaded from a file (`Schema::load_path`) is parsed and validated eagerly, since it
  has no build step.
- Revisit this if a measurement shows definition parsing on a hot path, not before.
