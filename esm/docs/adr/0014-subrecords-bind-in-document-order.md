# Subrecords bind to schema members in document order, as xEdit binds them

Status: accepted (2026-09-27).

The decoder used to queue each record's subrecords per signature and let every schema member pop
the front of its own signature's queue. Optional and repeated members then took subrecords that
belonged elsewhere whenever a signature was shared: a scene action's `ALID` ended up in the
scene's actor list, a patrol's `INAM` in the reference's bound data, an NPC's own `FULL` in an
object-template combination. Each case was patched with document-index windows, per-member
`stop_before` lists and a hardcoded `ALED` terminator, and most decode fixes landed there.

## Decision

`decode/bind.rs` binds a record's subrecords in one pass in file order, following xEdit's
`TwbMainRecord`/`TwbSubRecordStruct`/`TwbSubRecordArray`:

- A record or `rstruct` keeps a cursor over its members; each subrecord goes to the next member
  at or after the cursor that can take its signature. An `unordered` container looks the member
  up by signature instead.
- An `rstruct` opens on its first member's signature, or on any member's when it is marked
  `any_member`, and ends at the first subrecord it can't place. An `rarray` takes elements while
  its element can open on the next subrecord. A signature-less union (`by_signature`) takes the
  first variant that can.
- The extractor carries xEdit's `aAllowUnordered` (`unordered`), `dfAllowAnyMember`
  (`any_member`) and `aSkipSigs` (`skip_sigs`) into the schema, including for the groups its
  hand-written stubs model.
- A member that repeats an earlier sibling's definition keeps its name (xEdit lists `FULL`,
  `OPDS`, `LODP` and others at several positions so they bind wherever the data places them).
  The binder merges a repeat into the earlier value and keeps the field at the first position.

One deliberate difference from xEdit: xEdit leaves every subrecord after an out-of-order one
unbound at the record level. Here the out-of-order subrecord binds to its member and the cursor
stays put, so one stray subrecord doesn't cost the rest of the record.

## Consequences

On the 20260918 snapshot 27,070 of 5,818,719 records decode differently, every sampled shape a
subrecord moving to the member xEdit binds it to. No record gains `_unmapped` subrecords, and
`esm coverage --gate` reports zero markers. Schema overrides that existed only to steer the old
binder are gone. Output changes land with a snapshot re-extraction so snapshot-over-snapshot
diffs compare like with like.
