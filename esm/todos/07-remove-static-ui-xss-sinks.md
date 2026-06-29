# Todo: Remove Static UI XSS Sinks

## Status

Still relevant. The static pages still use `innerHTML`, `insertAdjacentHTML`,
and generated inline event handlers for server-provided or decoded values.

Verified state (audit 2026-06-29):
- `static/index.html` already has an `escHtml` helper (line 156) but it is only
  called inside `renderJson` (string values and object keys). ~8 unescaped sinks
  remain: group labels/counts (`innerHTML` lines ~67, 86), error messages (lines ~71,
  116, 133), `formId` (lines ~125, 132), `editor_id` and record signature header
  (line ~129).
- `static/compare.html` has **no escape helper and no DOM APIs at all**. The entire
  render is one big string-built HTML block assigned via `innerHTML` (line 87),
  carrying raw FormIDs, EditorIDs, record types, field keys/values (`JSON.stringify`),
  and a generated `onclick="toggleFields('changed-'+type+...)"` handler — all
  unescaped.

## Context

The static HTTP UI renders data from local ESM files, but the ESM bytes and
decoded strings are still untrusted input. EditorIDs, names, group labels, field
keys, field values, diff values, and API error messages should not be treated as
HTML.

## Remaining Scope

- `static/index.html`: apply `escHtml` (already defined) to the ~8 unescaped
  injection sites — group labels/counts, section headers, `formId`, `editor_id`,
  record type signature header, and error messages. Switch remaining `innerHTML`
  sinks to DOM APIs (`createElement`, `textContent`, `append`) or consistently
  escaped templates.
- `static/compare.html`: introduce an `escHtml` helper; apply it to every
  interpolated value in `render()` and `renderFieldChanges()` — type headers,
  record rows, FormIDs, EditorIDs, field keys, `JSON.stringify`-ed field values,
  and API errors. Fix the `onclick="toggleFields('changed-'+type+...)"` generated
  inline handler (escape or switch to `addEventListener` on the element).
- Remove remaining generated inline handler strings such as `onclick="..."`;
  attach event listeners instead.

## Files

- `static/index.html`
- `static/compare.html`

## Acceptance Criteria

- No server-provided or decoded data is inserted as HTML without escaping.
- Generated rows and field views use DOM APIs or consistently escaped templates.
- Existing browse, load-more, record-detail, and diff-expand behavior remains intact.
- Malformed API error messages render as text, not markup.
- Inline `onclick="..."` event handlers with interpolated data are replaced with
  `addEventListener` attachments.

## Verification

- Manual server smoke test with
  `cargo run --features server --bin esm-server -- <ESM>`.
- Inspect rendered group labels, EditorIDs, field keys, field values, and diff
  values containing `<`, `>`, `"`, `'`, and `&`.
- Browser console has no regressions during group browsing and diff viewing.
- Add lightweight DOM or HTML-output tests for escaping correctness where practical
  (e.g. a small `escHtml` unit test in a `<script type="module">` test harness, or
  a Node.js smoke script that asserts escaped output for crafted inputs).
