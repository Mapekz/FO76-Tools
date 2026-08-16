//! Doc-drift guard: fails when `README.md` / `CLAUDE.md` drift away from the
//! real, built CLI. Four checks:
//!
//! 1. Every `ba2 <subcommand>` invocation named in `README.md`/`CLAUDE.md` is
//!    a real subcommand of the built binary (ground truth: `ba2 --help`).
//! 2. Every real subcommand (except `help`) is mentioned, by name, in
//!    `README.md` (the human CLI doc — `CLAUDE.md` doesn't enumerate
//!    subcommands beyond its architecture table, so it isn't held to this
//!    check).
//! 3. Every `--flag` token that shares a code fence/span with a named `ba2
//!    <subcommand>` invocation in either doc is a real flag of the CLI
//!    (ground truth: the global `--help` plus every subcommand's `--help`).
//! 4. Every backtick-quoted repo-relative path token in `README.md` and
//!    `CLAUDE.md` resolves to a real file or directory, relative to the
//!    crate root (`../`-prefixed cross-repo references resolve relative to
//!    the workspace root instead, since every FO76-Tools subproject lives in
//!    this one repo).
//!
//! Std-only, no game data, no network — safe to run anywhere `cargo test`
//! runs. Cargo builds the `ba2` bin ahead of this test automatically because
//! integration tests depend on `CARGO_BIN_EXE_<name>`.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

// ── Known-intentional / known-but-out-of-scope exceptions ──────────────────
//
// Empty unless a specific token is a deliberate exception. Entries here are
// NOT fixed by this test because it is only permitted to add this one test
// file — see the file's own commit/PR description for why the underlying
// doc wasn't corrected in the same change.

/// Subcommand-shaped words this test's doc extraction pulls out of
/// `README.md`/`CLAUDE.md` that are known false positives (not real `ba2
/// <subcommand>` invocations) and not worth tightening the extractor for.
const SKIP_INVOCATIONS: &[&str] = &[];

/// `--flag` tokens that are known-good despite not appearing in `--help`
/// output verbatim (e.g. a flag documented only under an alias).
const SKIP_DOC_FLAGS: &[&str] = &[];

/// Backtick-quoted repo-relative paths that do not currently resolve, parked
/// here instead of silently passing. Either fix the doc (update the path) or,
/// if the shorthand is intentional prose rather than a literal path, leave
/// it here with a comment.
const SKIP_PATH_REFS: &[&str] = &[];

// ── Doc loading ──────────────────────────────────────────────────────────

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_doc(rel: &str) -> String {
    let path = crate_root().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()))
}

// ── CLI ground truth ────────────────────────────────────────────────────

fn run_ba2(args: &[&str]) -> String {
    let bin = env!("CARGO_BIN_EXE_ba2");
    let output = Command::new(bin)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("failed to run `ba2 {}`: {e}", args.join(" ")));
    // clap prints --help to stdout on success.
    assert!(
        output.status.success(),
        "`ba2 {}` exited non-zero (status {:?}); stderr:\n{}",
        args.join(" "),
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Parses the `Commands:` section of `ba2 --help`. clap's derived help keeps
/// this section header stable; each command line is two-space indented,
/// starts with the subcommand name, then whitespace, then an optional
/// description.
fn real_subcommands() -> Vec<String> {
    let help = run_ba2(&["--help"]);
    let mut out = Vec::new();
    let mut in_commands = false;
    for line in help.lines() {
        if line.trim_end() == "Commands:" {
            in_commands = true;
            continue;
        }
        if in_commands {
            if line.trim().is_empty() || line.trim_end() == "Options:" || !line.starts_with(' ') {
                break;
            }
            if let Some(name) = line.split_whitespace().next() {
                out.push(name.to_string());
            }
        }
    }
    assert!(
        !out.is_empty(),
        "failed to parse any subcommand names out of `ba2 --help`'s Commands: section — the \
         help text shape changed; update this test's parser:\n{help}"
    );
    out
}

/// Union of every `--flag` token appearing anywhere in the global `--help`
/// text plus every real subcommand's own `--help` text (skipping `help`
/// itself, whose `--help` isn't a normal subcommand invocation).
fn real_flags(subcommands: &[String]) -> HashSet<String> {
    let mut text = run_ba2(&["--help"]);
    for cmd in subcommands {
        if cmd == "help" {
            continue;
        }
        text.push('\n');
        text.push_str(&run_ba2(&[cmd, "--help"]));
    }
    extract_flags(&text).into_iter().collect()
}

// ── Markdown code-region extraction ─────────────────────────────────────
//
// "Code regions" = fenced code-block bodies (between ``` pairs) plus inline
// code spans (between single ` pairs, outside of fences). Restricting
// extraction to these regions is what keeps prose mentions like "querying
// a BA2 archive" or "the `ba2` CLI" from being misread as `ba2
// <subcommand>` invocations.

/// Splits `text` into fenced-block bodies and the remaining (non-fenced)
/// text, by alternating on the ``` delimiter.
fn split_fenced(text: &str) -> (Vec<String>, String) {
    let mut fenced = Vec::new();
    let mut rest = String::new();
    for (i, part) in text.split("```").enumerate() {
        if i % 2 == 1 {
            fenced.push(part.to_string());
        } else {
            rest.push_str(part);
            rest.push(' ');
        }
    }
    (fenced, rest)
}

/// Extracts inline `code span` contents from text that has already had
/// fenced blocks removed (so every remaining backtick is a single-tick
/// inline-code delimiter).
fn inline_spans(text: &str) -> Vec<String> {
    text.split('`')
        .enumerate()
        .filter_map(|(i, s)| {
            if i % 2 == 1 {
                Some(s.to_string())
            } else {
                None
            }
        })
        .collect()
}

fn code_regions(doc: &str) -> Vec<String> {
    let (fenced, rest) = split_fenced(doc);
    let mut regions = fenced;
    regions.extend(inline_spans(&rest));
    regions
}

/// Trims markdown/prose decoration off a token without touching `<`/`>`/
/// `[`/`]` — those must stay so a placeholder like `<subcommand>` keeps its
/// brackets and gets excluded by `is_plausible_subcommand`, rather than
/// being silently unwrapped into a fake "subcommand" hit. Leading `.` is
/// also preserved so a `.ba2` file-extension token (e.g. `output.ba2`) is
/// never collapsed into the bare word `ba2`.
fn trim_tok(s: &str) -> &str {
    const LEADING: &[char] = &['`', '*', '(', ')', '"', '\'', ',', ';', ':', '!', '?', '$'];
    const TRAILING: &[char] = &[
        '`', '*', '(', ')', '"', '\'', ',', ';', ':', '.', '!', '?', '$',
    ];
    s.trim_start_matches(LEADING).trim_end_matches(TRAILING)
}

fn is_plausible_subcommand(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_lowercase())
}

/// One matched `ba2 <subcommand>` invocation, kept with enough context to
/// produce a useful failure message.
struct Invocation {
    subcommand: String,
    region: String,
}

/// Finds `ba2 <subcommand>` invocations in `region`, scanning line-by-line
/// (never crossing a newline) so an unrelated word on the next shell line
/// can't be mistaken for the subcommand that follows `ba2` on this one.
/// Global flags between `ba2` and the subcommand are tolerated: the first
/// lowercase-only token within a short lookahead window is taken as the
/// candidate, and flag values almost never look like a bare lowercase word
/// (paths contain `/`, extensions contain `.`). A `#` token ends the
/// lookahead early — everything after it on the line is a shell comment
/// (e.g. `cargo run --bin ba2 -- <args>  # run CLI`), not the invocation.
fn find_invocations(region: &str) -> Vec<Invocation> {
    const LOOKAHEAD: usize = 4;
    let mut out = Vec::new();
    for line in region.split('\n') {
        let toks: Vec<&str> = line.split_whitespace().collect();
        for i in 0..toks.len() {
            if trim_tok(toks[i]) != "ba2" {
                continue;
            }
            let end = (i + 1 + LOOKAHEAD).min(toks.len());
            for tok in &toks[(i + 1)..end] {
                // A `#` token starts a trailing shell comment (e.g. `cargo
                // run --bin ba2 -- <args>  # run CLI`) — everything after it
                // is prose, not part of the invocation being scanned.
                if *tok == "#" {
                    break;
                }
                let cand = trim_tok(tok);
                if is_plausible_subcommand(cand) {
                    out.push(Invocation {
                        subcommand: cand.to_string(),
                        region: region.to_string(),
                    });
                    break;
                }
            }
        }
    }
    out
}

/// Scans arbitrary text (markdown or `--help` output) for `--long-flag`
/// tokens by looking for `--` followed by a letter, then consuming
/// alphanumerics/hyphens. Deliberately not whitespace/backtick-aware: it
/// works the same over raw `--help` output and over markdown code regions.
fn extract_flags(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 2 < chars.len() {
        if chars[i] == '-' && chars[i + 1] == '-' && chars[i + 2].is_ascii_alphabetic() {
            let start = i;
            let mut j = i + 2;
            while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '-') {
                j += 1;
            }
            out.push(chars[start..j].iter().collect());
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// True if `word` appears in `haystack` as a standalone word (not as a
/// substring of a longer identifier).
fn contains_word(haystack: &str, word: &str) -> bool {
    let mut start = 0;
    while let Some(idx) = haystack[start..].find(word) {
        let idx = start + idx;
        let before_ok = idx == 0 || !is_word_char(haystack[..idx].chars().next_back().unwrap());
        let after = idx + word.len();
        let after_ok =
            after >= haystack.len() || !is_word_char(haystack[after..].chars().next().unwrap());
        if before_ok && after_ok {
            return true;
        }
        start = idx + 1;
    }
    false
}

// ── Assertion 1 + flag-checking: docs → CLI ─────────────────────────────

#[test]
fn doc_invocations_are_real_subcommands() {
    let real: HashSet<String> = real_subcommands().into_iter().collect();

    let docs = [
        ("README.md", read_doc("README.md")),
        ("CLAUDE.md", read_doc("CLAUDE.md")),
    ];

    let mut failures = Vec::new();
    for (name, content) in &docs {
        for region in code_regions(content) {
            for inv in find_invocations(&region) {
                if !real.contains(&inv.subcommand)
                    && !SKIP_INVOCATIONS.contains(&inv.subcommand.as_str())
                {
                    failures.push(format!(
                        "{name}: `ba2 {}` is not a real subcommand (real set: {:?}) — found in region: {:?}\n\
                         either the doc is stale (fix the invocation) or the CLI changed (this is expected drift — \
                         add \"{}\" to SKIP_INVOCATIONS in tests/doc_drift.rs with a comment if intentional)",
                        inv.subcommand,
                        {
                            let mut v: Vec<&String> = real.iter().collect();
                            v.sort();
                            v
                        },
                        inv.region.trim(),
                        inv.subcommand,
                    ));
                }
            }
        }
    }

    assert!(failures.is_empty(), "\n{}", failures.join("\n\n"));
}

#[test]
fn doc_flags_are_real() {
    let real_cmds = real_subcommands();
    let real: HashSet<String> = real_flags(&real_cmds);

    let docs = [
        ("README.md", read_doc("README.md")),
        ("CLAUDE.md", read_doc("CLAUDE.md")),
    ];

    let mut failures = Vec::new();
    for (name, content) in &docs {
        for region in code_regions(content) {
            // Only check flags in regions that actually name a `ba2
            // <subcommand>` invocation — both docs also contain `cargo`
            // flags (`--release`, `--bin`, …) and prose mentions, neither of
            // which should be flag-checked against the `ba2` CLI's own flag
            // set.
            if find_invocations(&region).is_empty() {
                continue;
            }
            for flag in extract_flags(&region) {
                if !real.contains(&flag) && !SKIP_DOC_FLAGS.contains(&flag.as_str()) {
                    failures.push(format!(
                        "{name}: `{flag}` is not a real flag of any `ba2` subcommand or the \
                         global options — found alongside a `ba2` invocation in region: {:?}\n\
                         either the doc is stale (fix or remove the flag) or the CLI changed (add \"{flag}\" \
                         to SKIP_DOC_FLAGS in tests/doc_drift.rs with a comment if intentional)",
                        region.trim(),
                    ));
                }
            }
        }
    }

    assert!(failures.is_empty(), "\n{}", failures.join("\n\n"));
}

// ── Assertion 2: CLI → docs coverage ────────────────────────────────────

#[test]
fn every_real_subcommand_is_documented() {
    let readme = read_doc("README.md");

    let mut failures = Vec::new();
    for cmd in real_subcommands() {
        if cmd == "help" {
            continue;
        }
        if !contains_word(&readme, &cmd) {
            failures.push(format!(
                "subcommand `{cmd}` (real, from `ba2 --help`) is missing from README.md — add a \
                 mention (heading or prose is fine), or this is a brand-new subcommand that \
                 hasn't been documented yet"
            ));
        }
    }

    assert!(failures.is_empty(), "\n{}", failures.join("\n\n"));
}

// ── Assertion 3: doc cross-references resolve ───────────────────────────

const PATH_PREFIXES: &[&str] = &["src/", "tests/"];

fn matches_path_pattern(s: &str) -> bool {
    if !PATH_PREFIXES.iter().any(|p| s.starts_with(p)) {
        return false;
    }
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '/'))
}

/// Strips a trailing `:123` or `:123-456` line-reference suffix, if present.
fn strip_line_ref(s: &str) -> &str {
    if let Some(idx) = s.rfind(':') {
        let tail = &s[idx + 1..];
        if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit() || c == '-') {
            return &s[..idx];
        }
    }
    s
}

#[test]
fn doc_cross_references_resolve() {
    let root = crate_root();
    // ba2/ crate root's parent, i.e. FO76-Tools/ — where sibling repos like
    // esm live.
    let workspace_parent = root
        .parent()
        .expect("ba2/ crate root has a parent directory");

    let docs = [
        ("CLAUDE.md".to_string(), read_doc("CLAUDE.md")),
        ("README.md".to_string(), read_doc("README.md")),
    ];

    let mut failures = Vec::new();
    for (name, content) in &docs {
        for span in inline_spans(content) {
            let span = span.trim();
            if span.is_empty() {
                continue;
            }
            // Globs, placeholders, and multi-word prose accidentally caught
            // inside backticks are not real path tokens.
            if span.contains(['*', '<', '>', '{', ' ']) {
                continue;
            }

            if let Some(rest) = span.strip_prefix("../") {
                // Every FO76-Tools subproject lives in this one repo, so any
                // `../<sibling>/...` cross-repo reference is safe to resolve
                // unconditionally against the workspace root — no
                // optional-checkout carve-out needed.
                let rest = strip_line_ref(rest);
                let rest = rest.trim_end_matches([',', '.', ';', ')', ']']);
                if SKIP_PATH_REFS.contains(&rest) {
                    continue;
                }
                let candidate = workspace_parent.join(rest);
                if !candidate.exists() {
                    failures.push(format!(
                        "{name}: `{span}` does not exist at {} — either the doc is stale or the \
                         sibling repo's layout changed",
                        candidate.display()
                    ));
                }
                continue;
            }

            let stripped = strip_line_ref(span);
            let stripped = stripped.trim_end_matches([',', '.', ';', ')', ']']);
            if !matches_path_pattern(stripped) {
                continue;
            }
            if SKIP_PATH_REFS.contains(&stripped) {
                continue;
            }

            let candidate = root.join(stripped);
            if !candidate.exists() {
                failures.push(format!(
                    "{name}: `{span}` does not exist at {} — either the doc is stale (fix the \
                     path) or this is intentional (add \"{stripped}\" to SKIP_PATH_REFS in \
                     tests/doc_drift.rs with a comment explaining why)",
                    candidate.display()
                ));
            }
        }
    }

    assert!(failures.is_empty(), "\n{}", failures.join("\n\n"));
}

// ── Sanity checks on the extraction machinery itself ────────────────────

#[test]
fn trim_tok_preserves_dot_prefixed_extensions() {
    // ".ba2" must never collapse to the bare word "ba2" — that's the whole
    // guard against misreading "output.ba2"/"archive.ba2" as an invocation.
    assert_eq!(trim_tok(".ba2"), ".ba2");
    assert_eq!(trim_tok("ba2."), "ba2");
    assert_eq!(trim_tok("`ba2`"), "ba2");
}

#[test]
fn is_plausible_subcommand_rejects_placeholders() {
    assert!(!is_plausible_subcommand("<subcommand>"));
    assert!(!is_plausible_subcommand("SIG"));
    assert!(!is_plausible_subcommand("path/to/data"));
    assert!(is_plausible_subcommand("extract"));
}

#[test]
fn contains_word_requires_boundaries() {
    assert!(contains_word("run `ba2 list X`", "list"));
    assert!(!contains_word("enlist", "list"));
    assert!(!contains_word("checklist", "list"));
}
