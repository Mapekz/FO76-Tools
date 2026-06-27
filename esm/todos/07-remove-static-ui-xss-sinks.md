# Todo: Remove Static UI XSS Sinks

## Status

Still relevant. The static pages still use `innerHTML`, `insertAdjacentHTML`,
and generated inline event handlers for server-provided or decoded values.

## Context

The static HTTP UI renders data from local ESM files, but the ESM bytes and
decoded strings are still untrusted input. EditorIDs, names, group labels, field
keys, field values, diff values, and API error messages should not be treated as
HTML.

## Remaining Scope

- Replace unsafe `innerHTML` and `insertAdjacentHTML` construction in
  `static/index.html`, including group labels/counts, section headers, record
  status text, rendered JSON, and error messages.
- Replace unsafe HTML string construction in `static/compare.html`, including
  type headers, record rows, EditorIDs, FormIDs, field keys, field values, and
  API errors.
- Prefer DOM APIs (`createElement`, `textContent`, `append`, and event
  listeners) for dynamic UI.
- If small trusted markup templates remain, centralize escaping and apply it to
  every interpolated value.
- Remove generated inline handler strings such as `onclick="..."`; attach event
  listeners instead.

## Files

- `static/index.html`
- `static/compare.html`

## Acceptance Criteria

- No server-provided or decoded data is inserted as HTML without escaping.
- Generated rows and field views use DOM APIs or consistently escaped templates.
- Existing browse, load-more, record-detail, and diff-expand behavior remains
  intact.
- Malformed API error messages render as text, not markup.

## Verification

- Manual server smoke test with
  `cargo run --features server --bin esm-server -- <ESM>`.
- Inspect rendered group labels, EditorIDs, field keys, field values, and diff
  values containing `<`, `>`, `"`, `'`, and `&`.
- Browser console has no regressions during group browsing and diff viewing.
