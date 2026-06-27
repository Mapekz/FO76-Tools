# TODO: tests in `esm/tests/decode_records.rs` that don't do a full clean decode

All items resolved as of 2026-06-27:

- **RACE subset tests** — deleted in `0edea99`; replaced with `race_liberator_decodes_correctly` and `race_mothman_decodes_correctly` using `assert_fully_decoded`.
- **Drift-locked tests** (GMRW/LVLI/LVLN/LVPC/LVLP/RESO/NPC_) — all flipped to `assert_fully_decoded` in `fa9ad13` after overrides were added to `schema/fo76.overrides.json`.
- **MGEF test** — now uses `collect_decode_problems` assertion (in `fa9ad13`).
- **OMOD test** — now uses `assert_fully_decoded` (in `fa9ad13`).
- **RACE `_unmapped` (72 records)** — fixed in `7d4993f` via Pet Commands schema and `insert_unique` guard; RACE is now in `CLEAN_TYPES`.
- **QUST VMAD fragmented** — `decode_vmad_qust` added in `b0faf5b`; QUST in `CLEAN_TYPES`.
- **NPC_ VMAD type-0/type-7 properties** — `decode_vmad_property` updated in `b0faf5b`; NPC_ moved to `CLEAN_TYPES`.
- **INFO/PACK/PERK/SCEN VMAD fragmented** — `decode_vmad_{info,pack,perk,scen}` added; dispatched by `ctx.record_signature`.
