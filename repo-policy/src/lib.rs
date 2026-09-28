//! Checks that keep a clap CLI's documentation honest, shared by the
//! `doc_drift` test of each crate that ships a binary.
//!
//! Ground truth is the built binary's `--help`; the docs are markdown. Only
//! code regions (fenced blocks and inline spans) are scanned, so prose such
//! as "the `esm` CLI" or "SeventySix.esm records" is never read as an
//! invocation. Every check returns failure messages rather than panicking, so
//! a test reports all drift at once.

use std::collections::{BTreeSet, HashSet};
use std::path::Path;
use std::process::Command;

/// A built clap binary and its `--help` ground truth.
pub struct Cli {
    name: &'static str,
    bin: &'static str,
    subcommands: Vec<String>,
    flags: HashSet<String>,
}

impl Cli {
    /// Reads the subcommands and flags of `bin` (a `CARGO_BIN_EXE_<name>`
    /// path), which docs name as `name`.
    pub fn new(name: &'static str, bin: &'static str) -> Self {
        let mut cli = Cli {
            name,
            bin,
            subcommands: Vec::new(),
            flags: HashSet::new(),
        };
        let help = cli.help(&["--help"]);
        cli.subcommands = parse_subcommands(&help);
        assert!(
            !cli.subcommands.is_empty(),
            "no subcommands parsed from `{name} --help`'s Commands: section; the help \
             layout changed:\n{help}"
        );
        let mut text = help;
        for cmd in cli.subcommands.iter().filter(|c| *c != "help") {
            text.push('\n');
            text.push_str(&cli.help(&[cmd, "--help"]));
        }
        cli.flags = extract_flags(&text).into_iter().collect();
        cli
    }

    fn help(&self, args: &[&str]) -> String {
        let output = Command::new(self.bin)
            .args(args)
            .output()
            .unwrap_or_else(|e| panic!("failed to run `{} {}`: {e}", self.name, args.join(" ")));
        assert!(
            output.status.success(),
            "`{} {}` exited with {:?}:\n{}",
            self.name,
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// Every `<name> <subcommand>` in `doc`'s code regions is a real subcommand.
    pub fn check_invocations(&self, doc_name: &str, doc: &str) -> Vec<String> {
        let mut failures = Vec::new();
        for region in code_regions(doc) {
            for sub in find_invocations(&region, self.name) {
                if !self.subcommands.contains(&sub) {
                    failures.push(format!(
                        "{doc_name}: `{} {sub}` is not a real subcommand (real: {:?}); in {:?}",
                        self.name,
                        self.subcommands,
                        region.trim()
                    ));
                }
            }
        }
        failures
    }

    /// Every `--flag` in a code region of `doc` that also names a
    /// `<name> <subcommand>` invocation is a flag of some subcommand or the
    /// global options. Regions without an invocation are skipped: they hold
    /// other tools' flags (`cargo --release`) and prose mentions.
    pub fn check_flags(&self, doc_name: &str, doc: &str) -> Vec<String> {
        let mut failures = Vec::new();
        for region in code_regions(doc) {
            if find_invocations(&region, self.name).is_empty() {
                continue;
            }
            for flag in extract_flags(&region) {
                if !self.flags.contains(&flag) {
                    failures.push(format!(
                        "{doc_name}: `{flag}` is not a flag of any `{}` subcommand; in {:?}",
                        self.name,
                        region.trim()
                    ));
                }
            }
        }
        failures
    }

    /// Every real subcommand (except `help`) is mentioned by name in `doc`.
    pub fn check_coverage(&self, doc_name: &str, doc: &str) -> Vec<String> {
        self.subcommands
            .iter()
            .filter(|c| *c != "help" && !contains_word(doc, c))
            .map(|c| {
                format!(
                    "{doc_name}: never mentions subcommand `{c}` (from `{} --help`)",
                    self.name
                )
            })
            .collect()
    }
}

/// Reads `rel` under `root`, panicking with the path on failure.
pub fn read_doc(root: &Path, rel: &str) -> String {
    let path = root.join(rel);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()))
}

/// Panics with every failure, one per paragraph, if there are any.
pub fn assert_no_failures(failures: &[String]) {
    assert!(failures.is_empty(), "\n{}", failures.join("\n\n"));
}

/// The names in the `Commands:` section of clap's `--help`: two-space
/// indented lines, the name first.
fn parse_subcommands(help: &str) -> Vec<String> {
    help.lines()
        .skip_while(|l| l.trim_end() != "Commands:")
        .skip(1)
        .take_while(|l| l.starts_with(' ') && !l.trim().is_empty())
        .filter_map(|l| l.split_whitespace().next().map(str::to_string))
        .collect()
}

/// Every code block's body and inline code span in a Markdown document, as
/// CommonMark reads them (any fence, any backtick-run length), in document
/// order.
pub fn code_regions(doc: &str) -> Vec<String> {
    use pulldown_cmark::{Event, Parser, Tag, TagEnd};
    let mut regions = Vec::new();
    let mut block: Option<String> = None;
    for event in Parser::new(doc) {
        match event {
            Event::Start(Tag::CodeBlock(_)) => block = Some(String::new()),
            Event::End(TagEnd::CodeBlock) => regions.extend(block.take()),
            Event::Text(text) if block.is_some() => {
                block.get_or_insert_default().push_str(&text);
            }
            Event::Code(code) => regions.push(code.into_string()),
            _ => {}
        }
    }
    regions
}

/// Local link targets in a Markdown document outside code: inline and
/// reference links and images, and every reference definition, as
/// CommonMark reads them.
pub fn link_targets(doc: &str) -> Vec<String> {
    use pulldown_cmark::{Event, Parser, Tag};
    let parser = Parser::new(doc);
    let definitions: Vec<String> = parser
        .reference_definitions()
        .iter()
        .map(|(_, def)| def.dest.to_string())
        .collect();
    let links = parser.filter_map(|event| match event {
        Event::Start(Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. }) => {
            Some(dest_url.into_string())
        }
        _ => None,
    });
    let mut targets: Vec<String> = links
        .chain(definitions)
        .filter(|t| !t.contains("://") && !t.starts_with('#') && !t.starts_with("mailto:"))
        .map(|t| t.split('#').next().unwrap_or("").to_string())
        .filter(|t| !t.is_empty())
        .collect();
    targets.dedup();
    targets
}

/// Strips markdown and prose decoration from a token. `<`, `>`, `[`, `]`
/// stay, so a `<subcommand>` placeholder is never unwrapped into a name, and
/// so does a leading `.`, so `old.esm` never becomes the word `esm`.
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

/// The subcommand of each `<name> <subcommand>` invocation in `region`.
/// Scans line by line, so the next shell line never supplies the
/// subcommand. Global flags between the name and the subcommand are
/// tolerated: the first all-lowercase token within a short window is the
/// candidate (flag values are paths or addresses, never bare words). A `#`
/// token starts a shell comment and ends the window.
pub fn find_invocations(region: &str, name: &str) -> Vec<String> {
    const LOOKAHEAD: usize = 4;
    let mut out = Vec::new();
    for line in region.lines() {
        let toks: Vec<&str> = line.split_whitespace().collect();
        for (i, tok) in toks.iter().enumerate() {
            if trim_tok(tok) != name {
                continue;
            }
            let found = toks[i + 1..]
                .iter()
                .take(LOOKAHEAD)
                .take_while(|t| **t != "#")
                .map(|t| trim_tok(t))
                .find(|t| is_plausible_subcommand(t));
            out.extend(found.map(str::to_string));
        }
    }
    out
}

/// Every `--long-flag` token in `text`: `--`, a letter, then alphanumerics
/// and hyphens. Works the same over `--help` output and markdown.
pub fn extract_flags(text: &str) -> BTreeSet<String> {
    let bytes = text.as_bytes();
    let mut out = BTreeSet::new();
    let mut i = 0;
    while i + 2 < bytes.len() {
        if bytes[i] == b'-' && bytes[i + 1] == b'-' && bytes[i + 2].is_ascii_alphabetic() {
            let mut j = i + 2;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'-') {
                j += 1;
            }
            out.insert(text[i..j].to_string());
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

/// True if `word` appears in `haystack` as a whole word.
pub fn contains_word(haystack: &str, word: &str) -> bool {
    haystack.match_indices(word).any(|(idx, _)| {
        let before = haystack[..idx].chars().next_back();
        let after = haystack[idx + word.len()..].chars().next();
        !before.is_some_and(is_word_char) && !after.is_some_and(is_word_char)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_tok_keeps_dot_prefixed_extensions() {
        assert_eq!(trim_tok(".esm"), ".esm");
        assert_eq!(trim_tok("esm."), "esm");
        assert_eq!(trim_tok("`esm`"), "esm");
    }

    #[test]
    fn placeholders_are_not_subcommands() {
        assert!(!is_plausible_subcommand("<subcommand>"));
        assert!(!is_plausible_subcommand("SIG"));
        assert!(!is_plausible_subcommand("path/to/data"));
        assert!(is_plausible_subcommand("walk"));
    }

    #[test]
    fn invocations_skip_flags_and_stop_at_comments() {
        assert_eq!(find_invocations("esm --decimal get X", "esm"), ["get"]);
        assert_eq!(
            find_invocations("cargo run --bin esm -- # run CLI", "esm"),
            Vec::<String>::new()
        );
        assert_eq!(find_invocations("esm\nget", "esm"), Vec::<String>::new());
    }

    #[test]
    fn code_regions_take_fences_and_spans() {
        let doc = "a `esm get` b\n```\nesm walk X\n```\nc `ba2 list`";
        assert_eq!(code_regions(doc), ["esm get", "esm walk X\n", "ba2 list"]);
    }

    #[test]
    fn code_regions_read_any_fence_and_backtick_run() {
        let doc = "``a `b` c`` and ``esm/x.rs``\n\n~~~text\n[link](esm/y.md)\n~~~\n";
        assert_eq!(
            code_regions(doc),
            ["a `b` c", "esm/x.rs", "[link](esm/y.md)\n"]
        );
        assert_eq!(link_targets(doc), Vec::<String>::new());
    }

    #[test]
    fn flags_and_words() {
        assert_eq!(
            extract_flags("--a-b x --c1 -- --"),
            BTreeSet::from(["--a-b".into(), "--c1".into()])
        );
        assert!(contains_word("run `esm get X`", "get"));
        assert!(!contains_word("targets", "get"));
        assert!(!contains_word("forget", "get"));
    }

    #[test]
    fn subcommands_come_from_the_commands_section() {
        let help = "Usage: x\n\nCommands:\n  get   Get one\n  walk  Walk\n\nOptions:\n  -h\n";
        assert_eq!(parse_subcommands(help), ["get", "walk"]);
    }
}
