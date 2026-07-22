# Triage Labels

The skills speak in terms of five canonical triage roles. This repo uses the canonical names as-is — no remapping needed.

| Label in mattpocock/skills | Label in our tracker | Meaning                                    | Status |
| --------------------------- | --------------------- | ------------------------------------------- | ------ |
| `needs-triage`               | `needs-triage`         | Maintainer needs to evaluate this issue      | created 2026-07-22 |
| `needs-info`                 | `needs-info`           | Waiting on reporter for more information     | created 2026-07-22 |
| `ready-for-agent`            | `ready-for-agent`      | Fully specified, ready for an AFK agent      | created 2026-07-22 |
| `ready-for-human`            | `ready-for-human`      | Requires human implementation                | created 2026-07-22 |
| `wontfix`                    | `wontfix`              | Will not be actioned                         | pre-existing (GitHub default) |

When a skill mentions a role (e.g. "apply the AFK-ready triage label"), use the corresponding label string from this table.

`gh issue edit --add-label` does **not** auto-create a missing label — it must already exist in the repo. All five above already exist on `Mapekz/FO76-Tools`, so no pre-creation step is needed before first use.

Edit the right-hand column if the repo's label vocabulary ever changes.
