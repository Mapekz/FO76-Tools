# Vanilla surface manifest guide

Record observations as data, not as an SDK claim. A surface entry should contain:

```text
surface_id, game_build_ref, logical_path, source_hash, source_kind,
load_mode, root_symbol, required_symbols, required_classes,
method_contracts, timeline_contracts, provider_names,
localization_assumptions, resolution/input/focus_assumptions,
evidence_refs, confidence
```

## Publicly observed candidates

These names are useful search keys from public HUD projects, but need local confirmation:

- providers: `HUDModeData`, `HUDMessageProvider`, `HUDRightMetersData`,
  `PlayerInventoryData`, `PublicTeamsData`, `AccountInfoData`, `CharacterInfoData`,
  `MessageEvents`, `RecentActivitiesData`, `MapMenuData`, `QuestTrackerProvider`, and
  `DialogueData`;
- roots/regions: `HUDMenu`, `HUDNotificationsGroup_mc`, `LeftMeters_mc`, `RightMeters_mc`,
  `CenterGroup_mc`, `BottomCenterGroup_mc`, `TopCenterGroup_mc`, `TopRightGroup_mc`, and
  `SafeRect_mc`;
- representative child paths: `RightMeters_mc.ActionPointMeter_mc`,
  `RightMeters_mc.HUDActiveEffectsWidget_mc`, `BottomCenterGroup_mc.CompassWidget_mc`,
  `CenterGroup_mc.HUDCrosshair_mc.CrosshairBase_mc`, and
  `HUDNotificationsGroup_mc.Messages_mc`.

Treat provider payloads, child types, z-order, frame labels, and timing as unknown until a
legitimate local artifact or opt-in runtime log establishes them.

## Collision model

Keep a multimap keyed by normalized virtual path, not a single winner. Common community-package
collisions include `interface\\hudmenu.swf`, `interface\\hudmodloader.swf`,
`interface\\pipboy_datapage.swf`, and the loose filename `dxgi.dll`. Preserve package, archive,
source hash, and declared load-order evidence; never choose precedence from filename alone.

## Evidence rule

Autodesk GFx documentation establishes middleware behavior only. HUDModLoader/community source
establishes an integration pattern only. A current game build, local file hash, and runtime log
are required for exact compatibility claims.
