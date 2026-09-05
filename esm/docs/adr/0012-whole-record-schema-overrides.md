# A record type TES5Edit doesn't define is hand-authored whole in `fo76.overrides.json`

Status: accepted (2026-09-04)

`schema/fo76.json` is generated from TES5Edit's Pascal definitions, and the two override
mechanisms the extractor had until now both presuppose a Pascal base: `record_patches` splices
one node into an extracted record, `record_additions` appends members to one. Neither can
express a record type TES5Edit has no definition for at all. The 20260903 Pets PTS snapshot
shipped one — `PGTR` ("Progression Track") — and no upstream xEdit build defines it: the
pinned checkout and its full history have nothing, and the newest EcksEdit (4.1.6.18) crashes
on the record rather than decoding it, so third-party dumps exclude it outright. Waiting on
upstream would have left every PGTR record `_unknown_record` for an unbounded time, with the
patch-notes pipeline blind to a live gameplay system.

## Decision

`fo76.overrides.json` gains a third mechanism, `"records"`: a whole-record definition keyed by
signature that the extractor copies into the generated schema verbatim (winning over any
extracted record of the same signature). Reach for it only when TES5Edit has no definition to
patch or extend; `record_patches`/`record_additions` remain the right tool whenever a Pascal
base exists, because they keep parity with upstream for everything they don't touch.

A hand-authored record has no Pascal to cross-check against, so its evidence must be recorded
in the file itself, member by member:

- Field names come from the game's own data model before anything else — the decompiled
  Scaleform UI in `$FO76_DATA_DIR/<snapshot>/interface/*.swf` (`ffdec -export script`), which
  binds wire fields to named properties. xEdit's names for the same *role* on another record
  (e.g. `Reward Record`, `Marker End`) are the next source.
- A member neither source names stays `Unknown`, and its `_comment` records the observed
  constant and sample size (`1 in all 128 rewards of 20260903`) — never a guessed semantic.
- Every `_comment` states where a name or claim came from (file and line, record, snapshot).
  The extractor passes `_comment` keys through into `fo76.json`; the decoder ignores them.

## Consequences

- Such a record is *our* definition, not a mirror of upstream: `just audit`'s parity gate has
  nothing to compare it against, so its correctness rests on the byte-verbatim regression test
  that must accompany it (`tests/decode_records.rs`) and on `esm coverage --gate` staying clean.
- If upstream xEdit later defines the type, the `"records"` entry should be deleted and the
  signature added to `extract.py`'s `SAFELIST`, so parity resumes — the names may change then;
  a snapshot diff across that switch will show renames, not gameplay changes.
- `extract.py`'s `SAFELIST` deliberately excludes these types (they would fail extraction and
  be overwritten anyway); the `jq` one-liner that lists in-file record types must not be pasted
  back over it verbatim.
