# FormID input is hex-first with no implicit decimal fallback; `--decimal` is a client-side, flag-gated override

Status: accepted (2026-08-27)

`parse_formid` (`src/formid.rs`) previously read a bare (no `0x` prefix) all-hex-digit token as
hex only when it contained at least one ASCII letter — a purely numeric-looking token like
`"00568635"` fell through to `s.parse::<u32>()` and was read as *decimal* 568635 (`0x0008AD3B`)
instead. FormIDs are written in hex everywhere in this domain (xEdit, the wiki, this crate's own
`FormId::display()`), so `esm get 00568635` reporting "not found" for a record that plainly
exists at `0x00568635` was a bug, not an edge case: scanning 90,208 FormIDs across 10 record
types found 10,427 whose 8-digit hex spelling is also a plain decimal string.

## Decision

**Input**: `parse_formid` now reads any bare ≤8-hex-digit token as hex, full stop — matching an
explicit `0x` prefix's behavior and this crate's own output convention. There is **no implicit
decimal fallback**: if the hex reading of a bare token has no record, resolution proceeds
straight to EditorID, then the engine-hardcoded table, exactly as it always did — `ipc::
resolve_sel`'s `Auto` arm is otherwise unchanged from before this fix. Decimal is reachable only
through an explicit override.

**The `--decimal` override**: the CLI gets a global `--decimal` flag (joining `--esm` and
`--no-wait` as the third `global = true` flag; the same override reaches MCP tools and the
legacy HTTP routes via a `decimal` argument/query-param) that does two things for a caller who
specifically wants the decimal reading: a bare all-digit token is read as decimal *directly* —
committing to a concrete `RecordSel::FormId` without ever attempting the hex reading at all — and
*identity* FormIDs (a record's own FormID, never a FormID appearing as a decoded reference field
inside `fields`/`digest`) render as decimal in `list`/`search`/`refs`/`get`/`tree`/`diff`/`walk`
output. `chase` is exempt from the output half: its JSON is a machine pipeline contract (ADR
0001) consumed by `../patch-notes/pn/formids.py`'s `is_rendered`, which requires literal `0x` plus 8
hex digits — decimal output there would silently break the patch-notes pipeline, so `--decimal`
only affects `chase`'s *input* selector parsing, for consistency with every other subcommand.

**Why no implicit fallback, and no attempt to resolve the "both readings exist" case
automatically.** An earlier iteration of this fix tried exactly that: a hex miss on a bare token
would silently try the decimal reading before falling to EditorID, and when both hex and decimal
readings were real records, hex would win with no warning. That version was reverted before
shipping. Two problems drove the reversal: it added a second, always-on lookup against the
daemon's shared per-path `Database` (see `docs/architecture.md`'s `Registry`) for something the
caller never asked for, and once both a hex-first rule and a decimal-existence fallback were in
play, getting the *relative order* against EditorID resolution exactly right — decimal only after
EditorID also misses? before it? does the hardcoded table sit between them? — turned into real
design surface with no clearly-better answer, none of it justified by anything actually asked
for. Keeping decimal strictly behind an explicit flag sidesteps all of it: without the flag,
resolution is provably identical to the pre-fix code's non-decimal paths (hex → EditorID →
hardcoded), and with the flag, resolution never even consults hex — there is no ordering
question because the two paths never run in the same request. A future request for "try decimal
automatically when hex misses" should design that fallback (and its ordering against EditorID)
fresh, informed by real usage, rather than resurrecting this reverted attempt.

**Why the output half is implemented entirely client-side, with no wire change.** Tracing the
existing daemon/CLI split showed every FormID is already rendered to a `"0x…"` string before it
crosses the IPC boundary — `ipc::dispatch_op` calls `serde_json::to_value` on structs that already
hold pre-formatted hex strings (`RecordRow::form_id`, `RecordHeaderInfo::form_id` via the
`hex_string` serde helper, etc.) — so `src/bin/cli/` never had to format a FormID itself, only
move already-formatted strings into columns. Converting output under `--decimal` is therefore a
handful of small, explicit rewrites at each CLI handler — reparsing an already-hex string back to
a `FormId` and re-rendering via `FormId::display_base(FormIdBase::Dec)` — never a change to
`FormId::display()` itself (whose exact output every JSON contract depends on) and never a new
field on `Op`.

**Why identity-only, not every FormID in the payload.** `src/decode/model_info.rs` renders BA2
path hashes as `"File Hash": "0xDA67C578"` — byte-identical in shape to a rendered FormID but not
one. A blanket regex/JSON-value walk over decoded output would have no way to tell those apart
from a real reference FormID and would corrupt them under `--decimal`. Restricting conversion to
each result's own identity field (`header.form_id`, a `RecordRow`/`RefRow`/`RecordStub`'s
`form_id`, a tree node's `label.form_id`/`label.cell`) sidesteps this by construction — those
fields are typed `String` on structs the CLI already deserializes, never a walk over the free-form
JSON `fields`/`digest` payload. It also keeps `refs --paths` correct for free: `Database::
collect_formid_paths` string-matches rendered hex against decoded field values to build that
output, and those values are never touched.

## Consequences

- `RecordSel::from_input`/`from_parts` keep their existing signatures and behavior (implicitly
  `FormIdBase::Hex`); `from_input_with`/`from_parts_with` are the base-aware entry points every
  CLI/MCP/HTTP call site now uses. Under `FormIdBase::Dec` these commit directly to a concrete
  `FormId` — they never touch a `Database`/`Registry`, and never reach `resolve_sel`'s `Auto` arm.
- `ipc::resolve_sel`'s `Auto` arm is otherwise untouched by this feature: the daemon, `--local`,
  the HTTP legacy routes, and the MCP tools all resolve a bare token identically to before this
  ADR, per its own doc comment ("do not reimplement this locally") — the only behavioral change
  reaching that shared code path is the hex-first parse fix itself.
- MCP tools that resolve a selector (`esm_get_record`, `esm_refs`, `esm_walk`, `esm_chase`,
  `esm_lvli_drop_table`) and the legacy HTTP record routes (`GET /records/:formid`,
  `GET /records?id=`) all gained an optional `decimal` argument/query-param — input interpretation
  only; none of these surfaces render output through the CLI's `--decimal`-aware paths, so their
  JSON responses stay hex regardless.
- `../patch-notes/pn/formids.py`'s `to_int` was re-synced to match the same hex-first rule — it
  previously had no bare-hex branch at all and would raise on a token like `"463F"` that the Rust
  side already accepted, despite its own doc comment claiming to mirror `src/formid.rs`. It has no
  decimal-fallback logic to remove, since none was ever added there.
- A future subcommand or MCP tool that resolves a selector should thread `FormIdBase` (or a
  `decimal` argument) through from the start, following this same shape, rather than defaulting
  silently to `Hex` and being surprised later that it can't reach a decimal-reading FormID at all.
