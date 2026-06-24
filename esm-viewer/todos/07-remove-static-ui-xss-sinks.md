# Todo: Remove Static UI XSS Sinks

## Context

The static HTTP UI builds HTML by concatenating record, schema, and error values into `innerHTML`. ESM data is local but still untrusted binary input, and decoded EditorIDs, names, field keys, and diff values should not be treated as HTML.

## Scope

- Replace unsafe `innerHTML` string construction in `static/index.html`.
- Replace unsafe `innerHTML` string construction in `static/compare.html`.
- Prefer DOM APIs (`createElement`, `textContent`, event listeners) for dynamic UI.
- If markup rendering remains necessary, centralize escaping and apply it to every interpolated value.
- Avoid inline event handler strings such as `onclick="..."` for generated rows.

## Files

- `static/index.html`
- `static/compare.html`

## Acceptance Criteria

- No server-provided or decoded data is inserted as HTML without escaping.
- Generated rows and field views use DOM APIs or consistently escaped templates.
- Existing browse, load-more, record-detail, and diff-expand behavior remains intact.
- The UI handles malformed API error messages as text, not markup.

## Verification

- Manual server smoke test with `cargo run --features server --bin fo76-server -- <ESM>`.
- Inspect rendered group labels, EditorIDs, field keys, field values, and diff values containing `<`, `>`, `"`, `'`, and `&`.
- Browser console has no regressions during group browsing and diff viewing.
