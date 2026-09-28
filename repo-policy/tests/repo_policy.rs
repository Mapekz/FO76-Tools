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

    fn text_files(&self) -> impl Iterator<Item = (&str, String)> {
        self.files.iter().filter(|f| !is_data(f)).filter_map(|f| {
            let text = std::fs::read_to_string(self.root.join(f)).ok()?;
            Some((f.as_str(), text))
        })
    }

    /// Checks `token`, cited from the file at `from`: `Some(message)` when it
    /// is a repo path that does not resolve.
    fn check_citation(&self, from: &str, token: &str) -> Option<String> {
        let token = clean(token)?;
        let first = token.split('/').next()?;
        let mut claimed = false;
        let mut dir = Path::new(from).parent();
        while let Some(d) = dir {
            if let (Some(head), Some(full)) = (normalize(d, first), normalize(d, &token))
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
/// decoration, a `:line` suffix and a `#fragment` are stripped; absolute,
/// home-relative, URL, glob and placeholder tokens are skipped.
fn clean(token: &str) -> Option<String> {
    let t = token.trim_matches(|c: char| "`*\"'(),;:.!?[]".contains(c));
    let t = t.split('#').next()?;
    let t = match t.rsplit_once(':') {
        Some((head, tail)) if tail.chars().all(|c| c.is_ascii_digit() || c == '-') => head,
        _ => t,
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

/// `(line, text)` of each comment in a source file. Rust and TypeScript use
/// `//` and block-comment continuation lines; Python, TOML, YAML, shell and
/// justfiles use `#`; Python docstrings count too.
fn comments(path: &str, text: &str) -> Vec<(usize, String)> {
    let ext = path.rsplit('.').next().unwrap_or("");
    let name = path.rsplit('/').next().unwrap_or(path);
    let slashes = matches!(ext, "rs" | "ts" | "tsx" | "js" | "mjs");
    let hashes = matches!(ext, "py" | "toml" | "yml" | "yaml" | "sh") || name == "justfile";
    let mut out = Vec::new();
    let mut in_docstring = false;
    for (i, line) in text.lines().enumerate() {
        let t = line.trim_start();
        if ext == "py" {
            let quotes = t.matches("\"\"\"").count();
            if in_docstring || quotes > 0 {
                out.push((i + 1, t.to_string()));
            }
            if quotes % 2 == 1 {
                in_docstring = !in_docstring;
            }
            if in_docstring || quotes > 0 {
                continue;
            }
        }
        let comment = if slashes {
            t.find("//")
                .map(|j| &t[j..])
                .or_else(|| t.starts_with('*').then_some(t))
        } else if hashes {
            t.find('#')
                .filter(|&j| j == 0 || t[..j].ends_with(' '))
                .map(|j| &t[j..])
        } else {
            None
        };
        out.extend(comment.map(|c| (i + 1, c.to_string())));
    }
    out
}

fn is_markdown(path: &str) -> bool {
    path.ends_with(".md")
}

/// Local targets of `[text](target)` links outside code.
fn link_targets(doc: &str) -> Vec<String> {
    let prose: String = doc.split("```").step_by(2).collect::<Vec<_>>().join("\n");
    let prose: String = prose.split('`').step_by(2).collect();
    prose
        .match_indices("](")
        .filter_map(|(i, _)| {
            let rest = &prose[i + 2..];
            let target = rest[..rest.find(')')?].split_whitespace().next()?;
            let local = !target.contains("://")
                && !target.starts_with(['#', '<'])
                && !target.starts_with("mailto:");
            local.then(|| target.split('#').next().unwrap_or("").to_string())
        })
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
    for (path, text) in repo.text_files().filter(|(p, _)| *p != this_file) {
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
