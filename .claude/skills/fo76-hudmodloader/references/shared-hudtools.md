# SharedHUDTools reference

This is an interface summary, not a copied implementation. The linked public
[HUD Mod Loader source](https://github.com/GitCrazy-wc/hudmodloader) is reference material only.
Re-check the active source revision, distribution, license, and build metadata before relying on
it.

## Observed wrapper surface

| Area | Operations | Notes |
|---|---|---|
| lifecycle | constructor, `Register`, `Shutdown`, `isActive` | Use a unique mod name and balance cleanup. |
| messages | `SendMessage` and broadcast routing | Receiver availability/processing is separate from a successful send. |
| text | `TextEdit`, `EndTextEdit`, `FormatTextEdit` | Treat null/cancel/error distinctly from empty text. |
| keyboard | `FormatOnScreenKeyboard`, `SetLanguageOnScreenKeyboard` | Position and supported languages are versioned. |
| menu | `RegisterMenu`, `AddMenuItem`, `FormatMenu`, `ShowMenu`, `CloseMenu` | Do not assume the older wiki's one-level limit matches newer source. |

The wrapper sits over HUD message events and a private framed message vocabulary. Depend on the
wrapper and pinned version rather than reproducing the envelope. The documented utility is not a
vanilla Bethesda API.

## Compatibility checklist

- Confirm the exact loader/utility SWF hash and source/distribution version.
- Confirm root symbol and shared application-domain classes.
- Confirm the literal `hudmodloader.ini` path and ordered logical module IDs.
- Confirm `HUDModes` strings from the active source; older snapshots differ in scope labels.
- Confirm provider availability and event timing locally; class names alone are not schemas.
- Detect direct-replacement collisions before selecting archive precedence.
