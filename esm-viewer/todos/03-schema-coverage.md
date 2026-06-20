# Plan: Schema Coverage — Close the Remaining `raw_fallback` Fields

## Context

All major decode machinery landed (union deciders `ByteAtOffset`/`FieldValue`, `decode_vmad`,
expanded record whitelist covering 53 types). 24 `raw_fallback` fields remain in
`schema/fo76.json`, each produced by exactly two extractor failure modes. This plan closes
them.

## Remaining raw_fallbacks (24)

| Record | Field | Reason |
|---|---|---|
| ARMO | ENLM (rarray element) | closure union decider |
| ARMO | Combination | rstruct variable ref |
| BOOK | Teaches | closure union decider |
| CHAL | Reward | rstruct variable ref |
| FACT | Location Value | closure union decider |
| GMRW | Reward | rstruct variable ref |
| GMST | DATA | closure union decider (EDID-prefix — special case) |
| LVLI | LVLF | closure union decider |
| LVLI | LVLO | closure union decider |
| LVLN | LVLF | closure union decider |
| LVLN | LVLO | closure union decider |
| MGEF | Assoc. Item | closure union decider |
| MGEF | wbMagicEffectSounds | runtime Pascal decider |
| NPC_ | Level | closure union decider |
| NPC_ | Combination | rstruct variable ref |
| PERK | Quest Stage Data | not yet modeled |
| PROJ | Muzzle Flash Model | rstruct variable ref |
| QUST | Reward | rstruct variable ref |
| RACE | Body Data | rstruct variable ref |
| RACE | Male Behavior Graph | rstruct variable ref |
| RACE | Female Behavior Graph | rstruct variable ref |
| RACE | SPED | closure union decider |
| RACE | Data | rstruct variable ref |
| WEAP | Combination | rstruct variable ref |

## Approach

Three categories, sequenced for maximum extractor leverage first.

### Category B — "rstruct variable ref" (12 fields)

Emitted by `_parse_rstruct` (`tools/extractor/extract.py:480-492`). The extractor can't
inline a `wbXxx := wbRStruct(...)` named variable — it bails to `raw_fallback`. Fix:
**teach `_parse_rstruct` (and the member parser) to resolve a named-var reference** to its
definition and recurse. The `*/Combination` fields are all `wbObjectTemplate` (shared across
WEAP/ARMO/NPC_); resolving that one var clears three fields. Regenerate; hand-model any
variable the extractor still can't follow.

Variables to resolve (check `wbDefinitionsFO76.pas`):
- `wbObjectTemplate` → ARMO/NPC_/WEAP `Combination`
- `wbQuestReward` or similar → CHAL/GMRW/QUST `Reward`
- Behavior graph var → RACE `Male/Female Behavior Graph`, `Body Data`, `Data`
- `wbMuzzleFlashModel` or similar → PROJ `Muzzle Flash Model`

### Category A — "closure union decider" (10 fields)

Emitted by `_parse_union` (`extract.py:528-561`). Hand-model each in `schema/fo76.json`
using the existing `ByteAtOffset` or `FieldValue` deciders. Consult `wbRecord(...)` blocks
in `../TES5Edit/Core/wbDefinitionsFO76.pas` for discriminator + variant layouts.

- **MGEF Assoc. Item** → `FieldValue` keyed on the decoded archetype field (must appear
  before the union in member order in the JSON).
- **LVLI/LVLN LVLF** → flags byte; model as `integer` (the closure is just a flags field).
- **LVLI/LVLN LVLO** → level-list entry struct with Level/FormID/Count; model as `struct`
  inside the existing `rarray`.
- **ARMO ENLM** → `ByteAtOffset` over element's discriminator byte.
- **BOOK Teaches** → `ByteAtOffset` or `FieldValue` over the book's teach-type byte.
- **FACT Location Value** → `ByteAtOffset` or struct after checking the `.pas`.
- **NPC_ Level** → `ByteAtOffset`/`FieldValue` over the level-type byte.
- **RACE SPED** → `ByteAtOffset` over the speed-type byte.
- **GMST DATA** (special) → keyed on the first char of `EDID` string, not a payload byte.
  Use `{"kind": "bytes", "sig": "DATA", "name": "Value", "len": null}` with a `// comment`
  in the `name` field noting the EDID-prefix convention. A proper `EditorIdPrefix` decider
  variant is a follow-up.

### Category C — two one-offs

- **PERK Quest Stage Data** — hand-model the quest-type PERK `DATA` layout from the
  `wbRecord('PERK', ...)` block in `wbDefinitionsFO76.pas`. The quest-perk `PRKE` type
  byte selects this variant via the existing `ByteAtOffset` on `PRKE`.
- **MGEF wbMagicEffectSounds** — model the sound-array struct in JSON (the extractor emits
  `raw_fallback` because `wbMagicEffectSounds` is a closure). Structure: repeating SNAM
  subrecords with a sound type byte + FormID.

## Sequencing

1. **Category B extractor fix** → `python3 tools/extractor/extract.py` → `cargo build`
   → spot-check `*/Combination`, `RACE`, reward fields. Diff vs the old schema to confirm
   only Category B changed.
2. **Category A hand-modeling** — one field at a time; `cargo build` after each batch.
   GMST last (it stays as `bytes` — least important).
3. **Category C** — PERK quest-stage, then MGEF sounds. Rebuild and validate.

## Files to modify

| File | Change |
|---|---|
| `tools/extractor/extract.py` | `_parse_rstruct` (~480-492): resolve named-var references. Optionally improve `_parse_union` (~528-561) for recognizable closures. |
| `schema/fo76.json` | Hand-model Category A unions; Category C structs; any Category B rstruct the extractor still can't follow after the fix. Rebuild after edits. |
| `src/schema.rs` / `src/decode.rs` | Only if a new `EditorIdPrefix` decider is added for GMST (optional follow-up). |

No new crates. No `CACHE_VERSION` bump.

## Edge cases & risks

- Decoder must never panic (CLAUDE.md): all `ByteAtOffset` paths must bounds-check the
  payload and fall back to `default_variant`/raw.
- `FieldValue` ordering: discriminator field must be decoded before the union in member order.
- Extractor regression: diff the regenerated `schema/fo76.json` before committing — the
  change should only affect Category B records.
- Schema is compile-time embedded: every JSON edit needs `cargo build`.

## Optional / deferred follow-ups

- **`--mcp-sse` transport** (from `06`): stdio MCP works; SSE is a nice-to-have.
- **GMST fully typed** (`EditorIdPrefix` decider variant in `schema.rs` + `decode.rs`).
- **napi/wasm bindings** (`05-napi-wasm-binding.md` stays separate).

## Verification

- `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` clean.
- `cargo test` green on fresh checkout.
- `grep -c raw_fallback schema/fo76.json` drops from 24 toward 0 (GMST `bytes` may stay).
- Integration (RUST_TEST_ESM): `fo76 get` on MGEF/LVLI/LVLN/NPC_/RACE/WEAP/ARMO
  Combination/PROJ/CHAL/QUST/GMRW reward/PERK quest-stage shows no `raw_fallback` where
  modeled; previously-clean records (AMMO/ARMO/SPEL) decode identically.
