//! Repo-wide documentation policy, over every tracked file:
//!
//! 1. Every markdown link to a local target resolves.
//! 2. Every repo path cited in markdown code, or in a code comment, resolves.
//! 3. No file names a downstream consumer project: consumers read this repo's
//!    published artifacts, and this repo documents only its own contracts.
//!
//! "Resolves" means the path is tracked (following tracked symlinks), so the
//! verdict is the same locally and in CI. A token is a repo path when it
//! contains `/` and its first segment names something tracked, relative to the
//! citing file's directory or one of its ancestors: a `src/…` path
//! cited in `esm/docs/adr/` is checked against `esm/`, while `read/write` or
//! `Data/<date>` are prose and runtime paths, not citations.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use repo_policy::code_regions;

/// Consumer projects no tracked file may name (case-insensitive). This file
/// is the one exception.
const CONSUMER_NAMES: &[&str] = &["dps-76", "dps76"];

/// Files whose text is data, not documentation.
fn is_data(path: &str) -> bool {
    path.contains("/fixtures/")
        || path.starts_with("esm-viewer/src/shared/generated/")
        || path.ends_with(".lock")
        || path.ends_with("lock.json")
        || path.ends_with("bun.lock")
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo-policy sits one level below the repo root")
        .to_path_buf()
}

struct Repo {
    root: PathBuf,
    files: Vec<String>,
    /// Tracked files plus every directory above them.
    entries: HashSet<String>,
}

impl Repo {
    fn load() -> Self {
        let root = repo_root();
        let out = Command::new("git")
            .args(["ls-files", "-z"])
            .current_dir(&root)
            .output()
            .expect("run git ls-files");
        assert!(out.status.success(), "git ls-files failed");
        let files: Vec<String> = String::from_utf8(out.stdout)
            .expect("utf-8 paths")
            .split('\0')
            .filter(|p| !p.is_empty() && root.join(p).exists())
            .map(str::to_string)
            .collect();
        let mut entries = HashSet::new();
        for f in &files {
            let mut p = f.as_str();
            entries.insert(p.to_string());
            while let Some(i) = p.rfind('/') {
                p = &p[..i];
                entries.insert(p.to_string());
            }
        }
        Repo {
            root,
            files,
            entries,
        }
    }

    /// True if `path` (repo-relative, normalized) is tracked, or lies below a
    /// tracked symlink whose target holds it.
    fn exists(&self, path: &str) -> bool {
        if self.entries.contains(path) {
            return true;
        }
        let mut prefix = path;
        while let Some(i) = prefix.rfind('/') {
            prefix = &prefix[..i];
            if !self.entries.contains(prefix) {
                continue;
            }
            let Ok(target) = std::fs::read_link(self.root.join(prefix)) else {
                return false;
            };
            let parent = Path::new(prefix).parent().unwrap_or(Path::new(""));
            let Some(base) = normalize(parent, &target.to_string_lossy()) else {
                return false;
            };
            return self.exists(&format!("{base}{}", &path[prefix.len()..]));
        }
        false
    }

    /// Tracked documentation and source text, without data files.
    fn text_files(&self) -> impl Iterator<Item = (&str, String)> {
        self.all_text_files().filter(|(f, _)| !is_data(f))
    }

    /// Every tracked file that reads as UTF-8 text.
    fn all_text_files(&self) -> impl Iterator<Item = (&str, String)> {
        self.files.iter().filter_map(|f| {
            let text = std::fs::read_to_string(self.root.join(f)).ok()?;
            Some((f.as_str(), text))
        })
    }

    /// Checks `token`, cited from the file at `from`: `Some(message)` when it
    /// is a repo path that does not resolve.
    fn check_citation(&self, from: &str, token: &str) -> Option<String> {
        let token = clean(token)?;
        // The leading `./`/`../` segments plus the first named one.
        let named = token.split('/').position(|s| s != "." && s != "..")?;
        let first = token
            .split('/')
            .take(named + 1)
            .collect::<Vec<_>>()
            .join("/");
        let mut claimed = false;
        let mut dir = Path::new(from).parent();
        while let Some(d) = dir {
            if let (Some(head), Some(full)) = (normalize(d, &first), normalize(d, &token))
                && !head.is_empty()
                && self.exists(&head)
            {
                claimed = true;
                if self.exists(&full) {
                    return None;
                }
            }
            dir = d.parent();
        }
        claimed.then(|| format!("`{token}` does not resolve"))
    }
}

/// `dir/rel` with `.` and `..` folded; `None` if it leaves the repo.
fn normalize(dir: &Path, rel: &str) -> Option<String> {
    let mut parts: Vec<&str> = dir.to_str()?.split('/').filter(|s| !s.is_empty()).collect();
    for seg in rel.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            s => parts.push(s),
        }
    }
    Some(parts.join("/"))
}

/// The path inside a cited token, or `None` if it is not path-shaped:
/// decoration, a `:line` suffix and a `#fragment` are stripped, a glob
/// stands for the directory before its first wildcard, and absolute,
/// home-relative, URL and placeholder tokens are skipped.
fn clean(token: &str) -> Option<String> {
    let t = token
        .trim_start_matches(|c: char| "`*\"'(),;:!?[".contains(c))
        .trim_end_matches(|c: char| "`*\"'(),;:.!?]".contains(c));
    let t = t.strip_suffix("'s").map_or(t, |t| t.trim_end_matches('`'));
    let t = t.split('#').next()?;
    let t = match t.rsplit_once(':') {
        Some((head, tail)) if tail.chars().all(|c| c.is_ascii_digit() || c == '-') => head,
        _ => t,
    };
    let t = match t.find(['*', '?']) {
        Some(i) => t[..i].rsplit_once('/').map_or("", |(dir, _)| dir),
        None => t,
    };
    let t = t.trim_end_matches('/');
    let path_chars = |c: char| c.is_ascii_alphanumeric() || "_.-/".contains(c);
    let shaped = t.contains('/')
        && !t.starts_with('/')
        && !t.contains("//")
        && t.chars().all(path_chars)
        && t.chars().any(|c| c.is_ascii_alphabetic());
    shaped.then(|| t.to_string())
}

/// `(line, text)` of each comment in a source file, found by a small lexer
/// that skips string literals. Rust and TypeScript comments are `//` and
/// `/* */`; Python, TOML, YAML, shell and justfile comments start at a `#`
/// that opens the line or follows whitespace. Python's triple-quoted strings
/// (docstrings) count as comments.
fn comments(path: &str, text: &str) -> Vec<(usize, String)> {
    let ext = path.rsplit('.').next().unwrap_or("");
    let name = path.rsplit('/').next().unwrap_or(path);
    if matches!(ext, "rs" | "ts" | "tsx" | "js" | "mjs") {
        slash_comments(text, ext == "rs")
    } else if matches!(ext, "py" | "toml" | "yml" | "yaml" | "sh") || name == "justfile" {
        hash_comments(text, ext == "py")
    } else {
        Vec::new()
    }
}

/// Collects comment text per line.
#[derive(Default)]
struct CommentLines {
    out: Vec<(usize, String)>,
    line: usize,
    buf: String,
}

impl CommentLines {
    fn push(&mut self, c: char) {
        if c == '\n' {
            self.newline();
        } else {
            self.buf.push(c);
        }
    }

    fn newline(&mut self) {
        if !self.buf.trim().is_empty() {
            self.out
                .push((self.line + 1, std::mem::take(&mut self.buf)));
        }
        self.buf.clear();
        self.line += 1;
    }
}

fn slash_comments(text: &str, rust: bool) -> Vec<(usize, String)> {
    let c: Vec<char> = text.chars().collect();
    let mut acc = CommentLines::default();
    let ident = |ch: char| ch.is_alphanumeric() || ch == '_';
    let mut i = 0;
    while i < c.len() {
        let next = c.get(i + 1).copied();
        match c[i] {
            '/' if next == Some('/') => {
                while i < c.len() && c[i] != '\n' {
                    acc.push(c[i]);
                    i += 1;
                }
                continue;
            }
            '/' if next == Some('*') => {
                // Rust block comments nest; TypeScript's end at the first `*/`.
                let mut depth = 0;
                while i < c.len() {
                    if c[i] == '/' && c.get(i + 1) == Some(&'*') {
                        depth += 1;
                        i += 2;
                    } else if c[i] == '*' && c.get(i + 1) == Some(&'/') {
                        depth -= 1;
                        i += 2;
                        if depth == 0 || !rust {
                            break;
                        }
                    } else {
                        acc.push(c[i]);
                        i += 1;
                    }
                }
                continue;
            }
            'r' if rust && (i == 0 || !ident(c[i - 1])) => {
                let hashes = c[i + 1..].iter().take_while(|&&ch| ch == '#').count();
                if c.get(i + 1 + hashes) == Some(&'"') {
                    let close: Vec<char> = std::iter::once('"')
                        .chain(std::iter::repeat_n('#', hashes))
                        .collect();
                    i += 2 + hashes;
                    while i < c.len() && !c[i..].starts_with(&close) {
                        if c[i] == '\n' {
                            acc.newline();
                        }
                        i += 1;
                    }
                    i += close.len();
                    continue;
                }
            }
            // A char literal ('x', '\n', '\u{..}'); any other quote is a lifetime.
            '\'' if rust => {
                let end = if next == Some('\\') {
                    c[i + 2..]
                        .iter()
                        .position(|&ch| ch == '\'')
                        .map(|p| i + 2 + p)
                } else {
                    (c.get(i + 2) == Some(&'\'')).then_some(i + 2)
                };
                if let Some(end) = end {
                    i = end + 1;
                    continue;
                }
            }
            q @ ('"' | '\'' | '`') => {
                i += 1;
                while i < c.len() && c[i] != q {
                    if c[i] == '\\' {
                        i += 1;
                    }
                    if c.get(i) == Some(&'\n') {
                        acc.newline();
                    }
                    i += 1;
                }
                i += 1;
                continue;
            }
            '\n' => acc.newline(),
            _ => {}
        }
        i += 1;
    }
    acc.newline();
    acc.out
}

fn hash_comments(text: &str, python: bool) -> Vec<(usize, String)> {
    let c: Vec<char> = text.chars().collect();
    let mut acc = CommentLines::default();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if python && (ch == '"' || ch == '\'') && c[i..].starts_with(&[ch; 3]) {
            i += 3;
            while i < c.len() && !c[i..].starts_with(&[ch; 3]) {
                acc.push(c[i]);
                i += 1;
            }
            i += 3;
            continue;
        }
        match ch {
            '#' if i == 0 || c[i - 1].is_whitespace() => {
                while i < c.len() && c[i] != '\n' {
                    acc.push(c[i]);
                    i += 1;
                }
                continue;
            }
            // Strings end with their line: an apostrophe in YAML or shell
            // prose must not swallow the rest of the file.
            q @ ('"' | '\'') => {
                i += 1;
                while i < c.len() && c[i] != q && c[i] != '\n' {
                    if c[i] == '\\' {
                        i += 1;
                    }
                    i += 1;
                }
                if c.get(i) == Some(&q) {
                    i += 1;
                }
                continue;
            }
            '\n' => acc.newline(),
            _ => {}
        }
        i += 1;
    }
    acc.newline();
    acc.out
}

fn is_markdown(path: &str) -> bool {
    path.ends_with(".md")
}

/// Local targets of inline links (`[text](target)`, `[text](<target>)`) and
/// reference definitions (`[label]: target`) outside code.
fn link_targets(doc: &str) -> Vec<String> {
    let prose: String = doc.split("```").step_by(2).collect::<Vec<_>>().join("\n");
    let prose: String = prose.split('`').step_by(2).collect();
    let inline = prose.match_indices("](").filter_map(|(i, _)| {
        let rest = &prose[i + 2..];
        match rest.strip_prefix('<') {
            Some(angled) => angled.split_once('>').map(|(t, _)| t),
            None => rest[..rest.find(')')?].split_whitespace().next(),
        }
    });
    let definitions = prose.lines().filter_map(|l| {
        let (label, target) = l.trim_start().strip_prefix('[')?.split_once("]:")?;
        let target = target.split_whitespace().next()?;
        (!label.is_empty()).then(|| target.trim_start_matches('<').trim_end_matches('>'))
    });
    inline
        .chain(definitions)
        .filter(|t| !t.contains("://") && !t.starts_with('#') && !t.starts_with("mailto:"))
        .map(|t| t.split('#').next().unwrap_or("").to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

fn line_of(text: &str, needle: &str) -> usize {
    text.find(needle)
        .map_or(0, |i| text[..i].lines().count().max(1))
}

#[test]
fn markdown_links_resolve() {
    let repo = Repo::load();
    let mut failures = Vec::new();
    for (path, text) in repo.text_files().filter(|(p, _)| is_markdown(p)) {
        let dir = Path::new(path).parent().unwrap_or(Path::new(""));
        for target in link_targets(&text) {
            let ok = normalize(dir, &target).is_some_and(|p| repo.exists(&p));
            if !ok {
                failures.push(format!(
                    "{path}:{}: link target `{target}` does not resolve",
                    line_of(&text, &target)
                ));
            }
        }
    }
    repo_policy::assert_no_failures(&failures);
}

#[test]
fn cited_paths_resolve() {
    let repo = Repo::load();
    let mut failures = Vec::new();
    for (path, text) in repo.text_files() {
        let cited: Vec<(usize, String)> = if is_markdown(path) {
            code_regions(&text)
                .iter()
                .flat_map(|r| {
                    r.split_whitespace()
                        .map(|t| (line_of(&text, t), t.to_string()))
                        .collect::<Vec<_>>()
                })
                .collect()
        } else {
            comments(path, &text)
                .into_iter()
                .flat_map(|(n, c)| {
                    c.split_whitespace()
                        .map(|t| (n, t.to_string()))
                        .collect::<Vec<_>>()
                })
                .collect()
        };
        for (line, token) in cited {
            if let Some(msg) = repo.check_citation(path, &token) {
                failures.push(format!("{path}:{line}: {msg}"));
            }
        }
    }
    failures.sort();
    failures.dedup();
    repo_policy::assert_no_failures(&failures);
}

#[test]
fn no_file_names_a_consumer() {
    let repo = Repo::load();
    let this_file = "repo-policy/tests/repo_policy.rs";
    let mut failures = Vec::new();
    for (path, text) in repo.all_text_files().filter(|(p, _)| *p != this_file) {
        let lower = text.to_lowercase();
        for name in CONSUMER_NAMES {
            if lower.contains(name) {
                failures.push(format!(
                    "{path}:{}: names consumer `{name}`",
                    line_of(&lower, name)
                ));
            }
        }
    }
    repo_policy::assert_no_failures(&failures);
}

#[test]
fn clean_accepts_paths_and_rejects_prose() {
    assert_eq!(
        clean("`src/decode/mod.rs:40`,").as_deref(),
        Some("src/decode/mod.rs")
    );
    assert_eq!(
        clean("docs/adr/0001-x.md#why").as_deref(),
        Some("docs/adr/0001-x.md")
    );
    assert_eq!(clean("esm/"), None);
    assert_eq!(clean("`src/lib.rs`'s").as_deref(), Some("src/lib.rs"));
    assert_eq!(clean("(../esm/src)").as_deref(), Some("../esm/src"));
    assert_eq!(
        clean("`src/some-dir/*.ts`").as_deref(),
        Some("src/some-dir")
    );
    assert_eq!(clean("docs/adr/*.md").as_deref(), Some("docs/adr"));
    for prose in [
        "/tmp/x",
        "~/dev",
        "https://a/b",
        "Data/<date>",
        "*.rs",
        "1/2",
    ] {
        assert_eq!(clean(prose), None, "{prose}");
    }
}

#[test]
fn normalize_folds_dots_and_refuses_to_escape() {
    assert_eq!(
        normalize(Path::new("esm/docs"), "../src/x.rs").as_deref(),
        Some("esm/src/x.rs")
    );
    assert_eq!(normalize(Path::new(""), "../x"), None);
}

#[test]
fn citations_resolve_from_the_citing_directory_or_an_ancestor() {
    let files = [
        "esm/src/diff/mod.rs",
        "esm/docs/adr/0001.md",
        "esm-viewer/src/main.ts",
    ];
    let repo = Repo {
        root: repo_root(),
        files: files.map(str::to_string).to_vec(),
        entries: files
            .iter()
            .flat_map(|f| f.match_indices('/').map(|(i, _)| &f[..i]).chain([*f]))
            .map(str::to_string)
            .collect(),
    };
    let from = "esm/docs/adr/0001.md";
    assert_eq!(repo.check_citation(from, "`src/diff/mod.rs`"), None);
    assert_eq!(
        repo.check_citation(from, "../../../esm-viewer/src/main.ts"),
        None
    );
    assert!(repo.check_citation(from, "`src/diff.rs`").is_some());
    assert!(
        repo.check_citation(from, "../../../esm-viewer/src/addon.ts")
            .is_some()
    );
    assert_eq!(repo.check_citation(from, "read/write"), None);
}

#[test]
fn comments_skip_strings_and_read_block_comments() {
    let rs = "let s = \"https://x esm/a.rs\"; // see esm/b.rs\n\
              /* see esm/c.rs\n   esm/d.rs */ let c = '\"'; // esm/e.rs\n\
              let r = r#\"// esm/f.rs\"#; fn f<'a>(x: &'a str) {} // esm/g.rs\n";
    let text = comments("x.rs", rs)
        .into_iter()
        .map(|(_, c)| c)
        .collect::<Vec<_>>()
        .join("\n");
    for cited in ["esm/b.rs", "esm/c.rs", "esm/d.rs", "esm/e.rs", "esm/g.rs"] {
        assert!(text.contains(cited), "{cited} missing from {text:?}");
    }
    for literal in ["esm/a.rs", "esm/f.rs"] {
        assert!(
            !text.contains(literal),
            "{literal} read from a string: {text:?}"
        );
    }
    let py = "x = 'a # b'  # esm/g.py\n'''doc esm/h.py'''\ny = \"#\"\n";
    let found: Vec<(usize, String)> = comments("x.py", py);
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found[0].1.contains("esm/g.py") && found[1].1.contains("esm/h.py"));
}

#[test]
fn links_include_angled_and_reference_forms() {
    let doc = "[a](<esm/a.md>) [b](esm/b.md#x) [c](https://x/y)\n\
               [label]: esm/c.md\n`[d](esm/d.md)`\n";
    assert_eq!(link_targets(doc), ["esm/a.md", "esm/b.md", "esm/c.md"]);
}
