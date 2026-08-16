//! Doc-drift guard: fails when `skills/esm-cli/SKILL.md` / `README.md` /
//! `CLAUDE.md` / `docs/architecture.md` / `docs/adr/*.md` drift away from the
//! real, built CLI. Four checks:
//!
//! 1. Every `esm <subcommand>` invocation named in `SKILL.md`/`README.md` is a
//!    real subcommand of the built binary (ground truth: `esm --help`).
//! 2. Every real subcommand (except `help`) is mentioned, by name, in both
//!    `README.md` and `SKILL.md`.
//! 3. Every `--flag` token that shares a code fence/span with a named `esm
//!    <subcommand>` invocation in `SKILL.md` is a real flag of the CLI
//!    (ground truth: the global `--help` plus every subcommand's `--help`).
//! 4. Every backtick-quoted repo-relative path token in `CLAUDE.md`,
//!    `README.md`, `docs/architecture.md`, and `docs/adr/*.md` resolves to a
//!    real file or directory.
//!
//! Std-only, no game data, no network — safe to run anywhere `cargo test`
//! runs. Cargo builds the `esm` bin ahead of this test automatically because
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
/// `README.md`/`SKILL.md` that are known false positives (not real `esm
/// <subcommand>` invocations) and not worth tightening the extractor for.
const SKIP_INVOCATIONS: &[&str] = &[];

/// `--flag` tokens in `SKILL.md` that are known-good despite not appearing
/// in `--help` output verbatim (e.g. a flag documented only under an alias).
const SKIP_DOC_FLAGS: &[&str] = &[];

/// Backtick-quoted repo-relative paths that do not currently resolve.
/// Verified stale on inspection, but this test is scoped to adding
/// `tests/doc_drift.rs` only — it must not edit `CLAUDE.md`/`docs/adr/*.md`
/// to fix them, so they're parked here instead of silently passing.
///
/// - `src/decode.rs`, `src/bin/cli.rs`: both were split into directories
///   (`src/decode/{mod,rules,scope,vmad}.rs`, `src/bin/cli/{main,...}.rs`)
///   after these docs were written; the flat-file path no longer exists.
/// - `docs/adr/0001`: shorthand ADR reference missing its full filename
///   (`docs/adr/0001-walk-interactive-chase-pipeline-json.md`).
///
/// Either fix the doc (update the path) or, if the shorthand is intentional
/// prose rather than a literal path, leave it here.
const SKIP_PATH_REFS: &[&str] = &[];

// ── Doc loading ──────────────────────────────────────────────────────────

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_doc(rel: &str) -> String {
    let path = crate_root().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()))
}

/// `docs/adr/*.md`, sorted for deterministic test output.
fn adr_files() -> Vec<(String, String)> {
    let dir = crate_root().join("docs/adr");
    let mut entries: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", dir.display()))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "md"))
        .collect();
    entries.sort();
    entries
        .into_iter()
        .map(|p| {
            let rel = format!("docs/adr/{}", p.file_name().unwrap().to_string_lossy());
            let content = fs::read_to_string(&p)
                .unwrap_or_else(|e| panic!("failed to read {}: {e}", p.display()));
            (rel, content)
        })
        .collect()
}

// ── CLI ground truth ────────────────────────────────────────────────────

fn run_esm(args: &[&str]) -> String {
    let bin = env!("CARGO_BIN_EXE_esm");
    let output = Command::new(bin)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("failed to run `esm {}`: {e}", args.join(" ")));
    // clap prints --help to stdout on success.
    assert!(
        output.status.success(),
        "`esm {}` exited non-zero (status {:?}); stderr:\n{}",
        args.join(" "),
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Parses the `Commands:` section of `esm --help`. clap's derived help keeps
/// this section header stable; each command line is two-space indented,
/// starts with the subcommand name, then whitespace, then an optional
/// description.
fn real_subcommands() -> Vec<String> {
    let help = run_esm(&["--help"]);
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
        "failed to parse any subcommand names out of `esm --help`'s Commands: section — the \
         help text shape changed; update this test's parser:\n{help}"
    );
    out
}

/// Union of every `--flag` token appearing anywhere in the global `--help`
/// text plus every real subcommand's own `--help` text (skipping `help`
/// itself, whose `--help` isn't a normal subcommand invocation).
fn real_flags(subcommands: &[String]) -> HashSet<String> {
    let mut text = run_esm(&["--help"]);
    for cmd in subcommands {
        if cmd == "help" {
            continue;
        }
        text.push('\n');
        text.push_str(&run_esm(&[cmd, "--help"]));
    }
    extract_flags(&text).into_iter().collect()
}

// ── Markdown code-region extraction ─────────────────────────────────────
//
// "Code regions" = fenced code-block bodies (between ``` pairs) plus inline
// code spans (between single ` pairs, outside of fences). Restricting
// extraction to these regions is what keeps prose mentions like "querying
// SeventySix.esm records" or "the `esm` CLI" from being misread as `esm
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
/// also preserved so a `.esm` file-extension token (e.g. `old.esm`) is never
/// collapsed into the bare word `esm`.
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

/// One matched `esm <subcommand>` invocation, kept with enough context to
/// produce a useful failure message.
struct Invocation {
    subcommand: String,
    region: String,
}

/// Finds `esm <subcommand>` invocations in `region`, scanning line-by-line
/// (never crossing a newline) so an unrelated word on the next shell line
/// can't be mistaken for the subcommand that follows `esm` on this one.
/// Global flags between `esm` and the subcommand (`--esm <path>`, `--local`,
/// …) are tolerated: the first lowercase-only token within a short lookahead
/// window is taken as the candidate, and flag values almost never look like
/// a bare lowercase word (paths contain `/`, addresses contain `.`/`:`).
fn find_invocations(region: &str) -> Vec<Invocation> {
    const LOOKAHEAD: usize = 4;
    let mut out = Vec::new();
    for line in region.split('\n') {
        let toks: Vec<&str> = line.split_whitespace().collect();
        for i in 0..toks.len() {
            if trim_tok(toks[i]) != "esm" {
                continue;
            }
            let end = (i + 1 + LOOKAHEAD).min(toks.len());
            for tok in &toks[(i + 1)..end] {
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
        (
            "skills/esm-cli/SKILL.md",
            read_doc("skills/esm-cli/SKILL.md"),
        ),
    ];

    let mut failures = Vec::new();
    for (name, content) in &docs {
        for region in code_regions(content) {
            for inv in find_invocations(&region) {
                if !real.contains(&inv.subcommand)
                    && !SKIP_INVOCATIONS.contains(&inv.subcommand.as_str())
                {
                    failures.push(format!(
                        "{name}: `esm {}` is not a real subcommand (real set: {:?}) — found in region: {:?}\n\
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
fn skill_md_flags_are_real() {
    let real_cmds = real_subcommands();
    let real: HashSet<String> = real_flags(&real_cmds);

    let content = read_doc("skills/esm-cli/SKILL.md");
    let mut failures = Vec::new();
    for region in code_regions(&content) {
        // Only check flags in regions that actually name an `esm <subcommand>`
        // invocation — SKILL.md also documents `esm-server --mcp-stdio`
        // (a different binary) and prose mentions like "`--strict` isn't
        // exposed on `walk` yet", neither of which should be flag-checked
        // against the `esm` CLI's own flag set.
        if find_invocations(&region).is_empty() {
            continue;
        }
        for flag in extract_flags(&region) {
            if !real.contains(&flag) && !SKIP_DOC_FLAGS.contains(&flag.as_str()) {
                failures.push(format!(
                    "skills/esm-cli/SKILL.md: `{flag}` is not a real flag of any `esm` subcommand \
                     or the global options — found alongside an `esm` invocation in region: {:?}\n\
                     either the doc is stale (fix or remove the flag) or the CLI changed (add \"{flag}\" \
                     to SKIP_DOC_FLAGS in tests/doc_drift.rs with a comment if intentional)",
                    region.trim(),
                ));
            }
        }
    }

    assert!(failures.is_empty(), "\n{}", failures.join("\n\n"));
}

// ── Assertion 2: CLI → docs coverage ────────────────────────────────────

#[test]
fn every_real_subcommand_is_documented() {
    let readme = read_doc("README.md");
    let skill_md = read_doc("skills/esm-cli/SKILL.md");

    let mut failures = Vec::new();
    for cmd in real_subcommands() {
        if cmd == "help" {
            continue;
        }
        let in_readme = contains_word(&readme, &cmd);
        let in_skill = contains_word(&skill_md, &cmd);
        if !in_readme || !in_skill {
            failures.push(format!(
                "subcommand `{cmd}` (real, from `esm --help`) is missing from: {}{}{} — \
                 add a mention (table row or prose is fine) to the missing doc(s), or this is a \
                 brand-new subcommand that hasn't been documented yet",
                if !in_readme { "README.md" } else { "" },
                if !in_readme && !in_skill { " and " } else { "" },
                if !in_skill {
                    "skills/esm-cli/SKILL.md"
                } else {
                    ""
                },
            ));
        }
    }

    assert!(failures.is_empty(), "\n{}", failures.join("\n\n"));
}

// ── Assertion 3: doc cross-references resolve ───────────────────────────

const PATH_PREFIXES: &[&str] = &[
    "src/",
    "docs/",
    "tools/",
    "schema/",
    "bindings/",
    "skills/",
    "tests/",
];

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
    // esm/ crate root's parent, i.e. FO76-Tools/ — where sibling repos like
    // esm-viewer live.
    let workspace_parent = root
        .parent()
        .expect("esm/ crate root has a parent directory");

    let mut docs: Vec<(String, String)> = vec![
        ("CLAUDE.md".to_string(), read_doc("CLAUDE.md")),
        ("README.md".to_string(), read_doc("README.md")),
        (
            "docs/architecture.md".to_string(),
            read_doc("docs/architecture.md"),
        ),
    ];
    docs.extend(adr_files());

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
                // Only `../esm-viewer/...` is in scope: it's the one sibling
                // repo this doc set legitimately cross-references. `../TES5Edit`
                // is explicitly out of scope — that checkout is optional and
                // may not exist on CI (see CLAUDE.md's schema-tooling note).
                if span.starts_with("../esm-viewer") {
                    let candidate = workspace_parent.join(rest);
                    if !candidate.exists() {
                        failures.push(format!(
                            "{name}: `{span}` does not exist at {} — either the doc is stale or \
                             esm-viewer's layout changed",
                            candidate.display()
                        ));
                    }
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
    // ".esm" must never collapse to the bare word "esm" — that's the whole
    // guard against misreading "old.esm"/"SeventySix.esm" as an invocation.
    assert_eq!(trim_tok(".esm"), ".esm");
    assert_eq!(trim_tok("esm."), "esm");
    assert_eq!(trim_tok("`esm`"), "esm");
}

#[test]
fn is_plausible_subcommand_rejects_placeholders() {
    assert!(!is_plausible_subcommand("<subcommand>"));
    assert!(!is_plausible_subcommand("SIG"));
    assert!(!is_plausible_subcommand("path/to/data"));
    assert!(is_plausible_subcommand("walk"));
}

#[test]
fn contains_word_requires_boundaries() {
    assert!(contains_word("run `esm get X`", "get"));
    assert!(!contains_word("targets", "get"));
    assert!(!contains_word("forget", "get"));
}
