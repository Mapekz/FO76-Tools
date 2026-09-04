# Permission-ledger template

Use one row per artifact and requested action. Suggested fields:

```text
artifact_ref, origin, source_url, game_domain, mod_id, file_id,
author_or_rightsholder, source_sha256, license_identifier,
permission_status, rights, conditions, attribution,
evidence[{locator, captured_at, summary, sha256}], reviewer, reviewed_at
```

Recommended statuses are `owned`, `explicit-grant`, `preset-allow`, `inspect-only`, `unknown`,
`denied`, and `needs-human-review`. `modify` and `redistribute` must both be present for a
release. `inspect-only`, `unknown`, conflicting evidence, scope mismatch, and missing
rightsholder evidence fail closed.

Keep private author messages out of shared reports unless a human has approved their storage and
scope. Public page text can be linked and summarized, but it is not a blanket asset license.
