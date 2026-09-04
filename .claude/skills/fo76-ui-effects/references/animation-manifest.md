# Animation evidence manifest

Use one record per candidate effect. The manifest is a research record, not a license or release
approval.

## Required fields

```text
effect_id:
source_kind: game-owned | user-authored | third-party | synthetic
source_ref:
game_build:
platform:
archive_ref:
logical_path:
swf_sha256:
root_symbol:
symbol_or_linkage:
timeline_labels:
frame_rate:
frame_count:
asset_refs:
transform_color_alpha:
trigger_or_provider:
load_mode: child-widget | direct-replacement | external-preview
permission_status: unknown | local-only | cleared | denied
evidence: confirmed | deduced | unknown
unknowns:
```

## Conversion boundary

```text
BA2 -> selected SWF -> static symbol/timeline report -> optional external raster preview
```

The last step is for inspection or design review. It is not a promise that a raster sheet, edited
timeline, or recompiled SWF is compatible with Fallout 76 GFx. Never copy the extracted payload into
the repository.

## Example Vault Boy leads

| Lead | Evidence | Confidence and action |
| --- | --- | --- |
| `LevelUpAnimation_mc` | A name surfaced by community HUD research | `[Deduced]`; inspect the exact target HUD movie before use |
| `Underarmor/VaultBoy/White.dds` | An example build-specific Vault Boy texture path | `[Unknown]` until the exact artifact and rights are recorded; not animation evidence |

These are search leads, not a reusable asset library or stable runtime interface. Verify the exact
artifact, target build, and rights before depending on them.
