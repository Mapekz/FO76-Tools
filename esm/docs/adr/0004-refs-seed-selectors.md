# `refs` seed selectors are Direct or Carriers; positional auto-detect requires collision-free names

Status: accepted (2026-08-04)

`esm refs` answers "what references this?" by resolving a selector to one or more BFS seed
FormIDs, then walking the reverse-reference index. That resolution has exactly two shapes,
expressed as `RefSeeds` in `src/ipc.rs` and dispatched from one function, `resolve_ref_seeds`:

- **Direct** — a FormID, a real EditorID, or an engine-hardcoded EditorID (the ~228-entry table
  in `src/hardcoded.rs`: FormIDs defined by the game executable rather than by an ESM record,
  e.g. `Strength`, `DamageRecieved`). Resolves to exactly one seed FormID. That seed is never
  itself emitted as an output row; only its referencers are.
- **Carriers** — a selector that matches zero or more *records*, each emitted as a `depth: 0`
  seed row tagged with what it matched, then walked as ordinary BFS roots. Today's Carrier
  selectors are `--entry-point`/`--ep` (every PERK carrying a given Entry Point) and
  `--omod-property`/`--prop` (every OMOD declaring a given property). Carrier identity lives on
  `RefRow.tags: Vec<CarrierTag>` (`kind` is `EntryPoint` or `OmodProperty`; `scope` is `None`
  for entry points, `Some("weap"|"armo"|"npc")` for OMOD properties). `RefList.tag_total` counts
  distinct tags across the result.

The hardcoded-AVIF case is not a third selector kind and not an "overload" of Direct. It is an
ordinary Direct EditorID lookup whose name happened to come from a second table; callers and
dispatch treat it identically to a real-record EditorID.

## Decision

A Carriers selector namespace earns **positional auto-detection** (a bare `esm refs <name>` that
resolves as that selector without an explicit flag) only when its names are empirically
collision-free against real EditorIDs in the corpus. Otherwise the selector stays behind an
explicit flag permanently.

Evidence that drives the rule:

- **Entry points pass.** 213 entry-point names, all long multi-word strings (`Mod Percent
  Blocked`, …), zero EditorID collisions found. A bare `esm refs 'Mod Percent Blocked'`
  therefore auto-detects as `--ep`. A real EditorID still wins over a same-named entry point.
- **OMOD properties fail.** 132 short, generic names (`Speed`, `Value`, `Weight`, `Health`,
  `Keywords`). The sharpest collision: `Health` is also the EditorID of a real AVIF record
  (`0x000002D4`). A bare `esm refs Health` must keep resolving to that AVIF under Direct —
  never silently to OMOD-property carriers. `--omod-property`/`--prop` is therefore flag-only
  forever; this is not an MVP restriction.

The same test applies to any future third Carriers selector: measure name collisions against
EditorIDs first; only a clean namespace gets positional auto-detect.

## Consequences

- `--prop` matching rules (scope prefixes, bare-name cross-space fan-out, bare-id rejection,
  whitespace-insensitive name match) live in the CLI/skill docs, not here — this ADR owns the
  Direct/Carriers taxonomy and the positional auto-detect gate only.
- Generalizing `CarrierTag` (one type, `tag_total`, dynamic `EP`/`PROP` column) means a future
  Carriers kind adds a `kind` variant, not new plumbing.

Update (2026-08-10): `RefSeeds` and `resolve_ref_seeds` moved out of `src/ipc.rs` into the new
`src/refs.rs` module, alongside the rest of the reverse-reference graph engine (the BFS walk and
the bidirectional path search). `RefSeeds` is now `pub` (it was private to `ipc.rs` before). This
is a relocation, not a change to the Direct/Carriers taxonomy or the positional auto-detect gate
above — `ipc.rs` keeps the wire protocol (`Op`, `RefRow`, `RefList`, `dispatch_op`) and calls into
`refs.rs` for the resolution this ADR describes.
