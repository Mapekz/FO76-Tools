//! Minimal `*`-wildcard matching for record search.
//!
//! Supports only `*` (match any sequence of characters, including empty).
//! All matching is case-insensitive.
//!
//! Plain patterns with no `*` are treated as case-insensitive substring
//! matches (`"plasma"` matches `"PlasmaRifle"` and `"the plasma thing"`).
//!
//! A `*` anchors the remaining pattern: `"HTO_*"` requires `"hto_"` as a
//! prefix; `"*Rifle"` requires `"rifle"` as a suffix; multiple `*` tokens
//! are matched left-to-right by consuming the text greedily.
//!
//! An empty pattern or a bare `"*"` matches everything.

/// Returns `true` if `text` matches `pattern`.
///
/// # Examples
/// ```
/// use esm::wildcard::wildcard_match;
/// assert!(wildcard_match("plasma", "Plasma Rifle"));
/// assert!(wildcard_match("HTO_*", "HTO_AlignedFrame"));
/// assert!(!wildcard_match("HTO_*", "NotHTO_Frame"));
/// ```
pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    let pat = pattern.to_lowercase();
    let txt = text.to_lowercase();
    wildcard_match_lower(&pat, &txt)
}

/// Inner implementation operating on already-lowercased strings.
fn wildcard_match_lower(pat: &str, txt: &str) -> bool {
    if !pat.contains('*') {
        // No wildcard: plain substring search.
        return txt.contains(pat);
    }

    // Split on `*`.  The separator is consumed so adjacent `**` collapses to
    // one empty segment between them, which we skip.
    let segments: Vec<&str> = pat.split('*').collect();

    // `pat.starts_with('*')` iff `segments[0]` is empty — the text may begin
    // anywhere.  Same logic for the suffix.
    let needs_prefix = !segments[0].is_empty();
    let needs_suffix = !segments[segments.len() - 1].is_empty();

    let mut cursor: &str = txt;

    // Check mandatory prefix.
    if needs_prefix {
        let prefix = segments[0];
        if !cursor.starts_with(prefix) {
            return false;
        }
        cursor = &cursor[prefix.len()..];
    }

    // Walk middle segments (everything except first and last).
    let middle_start = if needs_prefix { 1 } else { 0 };
    let middle_end = if needs_suffix {
        segments.len() - 1
    } else {
        segments.len()
    };

    for seg in &segments[middle_start..middle_end] {
        if seg.is_empty() {
            continue; // consecutive stars — skip
        }
        match cursor.find(seg) {
            Some(pos) => cursor = &cursor[pos + seg.len()..],
            None => return false,
        }
    }

    // Check mandatory suffix.
    if needs_suffix {
        let suffix = segments[segments.len() - 1];
        return cursor.ends_with(suffix);
    }

    true
}
