use anyhow::{bail, Context, Result};
use ba2::{
    compress::Codec,
    extract::{extract_all, extract_one, ExtractOptions},
    reader::Ba2Archive,
    write_ba2, WriteOptions,
};
use clap::{Parser, Subcommand, ValueEnum};
use globset::{Glob, GlobSetBuilder};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(
    name = "ba2",
    about = "Extract and create Bethesda BA2 (GNRL) archives — Fallout 76 / Fallout 4"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Print archive header summary.
    Info {
        /// Path to the .ba2 archive.
        archive: PathBuf,
    },

    /// List entries in an archive.
    List {
        /// Path to the .ba2 archive.
        archive: PathBuf,
        /// Long listing: include sizes, codec, and hashes.
        #[arg(long)]
        long: bool,
    },

    /// Extract entries from an archive.
    Extract {
        /// Path to the .ba2 archive.
        archive: PathBuf,
        /// Output directory (default: current directory).
        #[arg(long, default_value = ".")]
        out: PathBuf,
        /// Only extract entries whose path matches this glob (e.g. "strings/*").
        #[arg(long)]
        filter: Option<String>,
        /// Force decompression codec: auto (default), lz4, zlib.
        #[arg(long, default_value = "auto", value_name = "CODEC")]
        format: CodecArg,
        /// Specific archive paths to extract (default: all).
        files: Vec<String>,
    },

    /// Create a BA2 archive from files on disk.
    Create {
        /// Path for the new .ba2 archive.
        archive: PathBuf,
        /// Pack all files in DIR; archive paths are relative to DIR.
        #[arg(long, conflicts_with_all = ["files", "list"])]
        from: Option<PathBuf>,
        /// Explicit source files to pack; archive path = file name (or relative to --base).
        #[arg(long, num_args = 1.., conflicts_with_all = ["from", "list"])]
        files: Vec<PathBuf>,
        /// File containing newline-separated source paths.
        ///
        /// Each line is either `source_path` (archive path = file name) or
        /// `archive_path<TAB>source_path`.
        #[arg(long, conflicts_with_all = ["from", "files"])]
        list: Option<PathBuf>,
        /// Strip this prefix from source paths to derive archive paths
        /// (used with --files or --list when the archive path is not explicit).
        #[arg(long)]
        base: Option<PathBuf>,
        /// Compression codec: lz4 (default, FO76), zlib (FO4), store.
        #[arg(long, default_value = "lz4", value_name = "CODEC")]
        compress: CodecArg,
    },
}

/// Codec selection for CLI arguments.
#[derive(Clone, Copy, ValueEnum)]
enum CodecArg {
    Auto,
    Lz4,
    Zlib,
    Store,
}

impl From<CodecArg> for Codec {
    fn from(a: CodecArg) -> Codec {
        match a {
            CodecArg::Auto => Codec::Auto,
            CodecArg::Lz4 => Codec::Lz4,
            CodecArg::Zlib => Codec::Zlib,
            CodecArg::Store => Codec::Store,
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Info { archive } => cmd_info(&archive),
        Commands::List { archive, long } => cmd_list(&archive, long),
        Commands::Extract {
            archive,
            out,
            filter,
            format,
            files,
        } => cmd_extract(&archive, &out, filter.as_deref(), format.into(), &files),
        Commands::Create {
            archive,
            from,
            files,
            list,
            base,
            compress,
        } => cmd_create(
            &archive,
            from.as_deref(),
            &files,
            list.as_deref(),
            base.as_deref(),
            compress.into(),
        ),
    }
}

// ── Subcommand implementations ────────────────────────────────────────────────

fn cmd_info(archive_path: &Path) -> Result<()> {
    let archive = Ba2Archive::open(archive_path)
        .with_context(|| format!("cannot open '{}'", archive_path.display()))?;
    let h = &archive.header;
    let type_str = std::str::from_utf8(&h.archive_type).unwrap_or("????");
    println!("File:               {}", archive_path.display());
    println!("Magic:              BTDX");
    println!("Version:            {}", h.version);
    println!("Archive type:       {}", type_str);
    println!("File count:         {}", h.file_count);
    println!("Name table offset:  0x{:016X}", h.name_table_offset);
    let meta = std::fs::metadata(archive_path)
        .ok()
        .map(|m| m.len())
        .unwrap_or(0);
    println!("File size:          {} bytes", meta);
    Ok(())
}

fn cmd_list(archive_path: &Path, long: bool) -> Result<()> {
    let archive = Ba2Archive::open(archive_path)
        .with_context(|| format!("cannot open '{}'", archive_path.display()))?;
    let entries = archive.list();
    if long {
        println!(
            "{:<10}  {:<10}  {:<6}  {:<8}  {:<8}  NAME",
            "UNPACKED", "PACKED", "CODEC", "NAME_HASH", "DIR_HASH"
        );
        for e in entries {
            let codec = if !e.is_compressed() {
                "store"
            } else {
                "lz4/zlib"
            };
            let ext_str = std::str::from_utf8(&e.ext)
                .map(|s| s.trim_end_matches('\0'))
                .unwrap_or("????");
            println!(
                "{:<10}  {:<10}  {:<6}  {:08X}  {:08X}  {} [.{}]",
                e.unpacked_size,
                if e.is_compressed() {
                    e.packed_size.to_string()
                } else {
                    "-".to_string()
                },
                codec,
                e.name_hash,
                e.dir_hash,
                e.name,
                ext_str,
            );
        }
    } else {
        for e in entries {
            println!("{}", e.name);
        }
    }
    eprintln!("{} entries", entries.len());
    Ok(())
}

fn cmd_extract(
    archive_path: &Path,
    out_dir: &Path,
    filter: Option<&str>,
    codec: Codec,
    specific: &[String],
) -> Result<()> {
    let archive = Ba2Archive::open(archive_path)
        .with_context(|| format!("cannot open '{}'", archive_path.display()))?;

    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("cannot create output directory '{}'", out_dir.display()))?;

    if !specific.is_empty() {
        // Extract named files.
        let mut count = 0usize;
        for name in specific {
            let dest = extract_one(&archive, name, out_dir, codec)?;
            println!("{}", dest.display());
            count += 1;
        }
        eprintln!("Extracted {} files", count);
        return Ok(());
    }

    // Extract all (optionally filtered).
    let glob_set = match filter {
        Some(pat) => {
            let glob = Glob::new(pat).with_context(|| format!("invalid glob pattern '{}'", pat))?;
            let mut builder = GlobSetBuilder::new();
            builder.add(glob);
            Some(builder.build().context("failed to build glob set")?)
        }
        None => None,
    };

    let opts = ExtractOptions {
        codec,
        filter: glob_set,
    };
    let count = extract_all(&archive, out_dir, &opts)?;
    eprintln!("Extracted {} files to '{}'", count, out_dir.display());
    Ok(())
}

fn cmd_create(
    archive_path: &Path,
    from: Option<&Path>,
    files: &[PathBuf],
    list: Option<&Path>,
    base: Option<&Path>,
    codec: Codec,
) -> Result<()> {
    // Collect (archive_path, source_path) pairs.
    let pairs: Vec<(String, PathBuf)> = if let Some(dir) = from {
        collect_from_dir(dir)?
    } else if !files.is_empty() {
        collect_from_files(files, base)?
    } else if let Some(list_path) = list {
        collect_from_list(list_path, base)?
    } else {
        bail!("specify one of --from, --files, or --list");
    };

    if pairs.is_empty() {
        bail!("no files to pack");
    }

    let opts = WriteOptions {
        codec,
        ..Default::default()
    };
    write_ba2(archive_path, &pairs, &opts)
        .with_context(|| format!("failed to create '{}'", archive_path.display()))?;

    eprintln!(
        "Created '{}' with {} files",
        archive_path.display(),
        pairs.len()
    );
    Ok(())
}

// ── Source collectors for `create` ───────────────────────────────────────────

/// Recursively collect all files under `dir`.  Archive paths are relative to
/// `dir`, backslash-joined, lowercased.
fn collect_from_dir(dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut pairs = Vec::new();
    for entry in walkdir::WalkDir::new(dir)
        .min_depth(1)
        .follow_links(false)
        .sort_by_file_name()
    {
        let entry = entry.with_context(|| format!("walkdir error in '{}'", dir.display()))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let src = entry.path().to_path_buf();
        let rel = src
            .strip_prefix(dir)
            .with_context(|| format!("failed to relativize '{}'", src.display()))?;
        let archive_path = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
            .collect::<Vec<_>>()
            .join("\\");
        pairs.push((archive_path, src));
    }
    Ok(pairs)
}

/// Build pairs from an explicit list of source paths.
fn collect_from_files(files: &[PathBuf], base: Option<&Path>) -> Result<Vec<(String, PathBuf)>> {
    let mut pairs = Vec::new();
    for src in files {
        let archive_path = derive_archive_path(src, base)?;
        pairs.push((archive_path, src.clone()));
    }
    Ok(pairs)
}

/// Build pairs from a line-delimited file.
///
/// Each line is either `source_path` or `archive_path<TAB>source_path`.
fn collect_from_list(list_path: &Path, base: Option<&Path>) -> Result<Vec<(String, PathBuf)>> {
    let content = std::fs::read_to_string(list_path)
        .with_context(|| format!("failed to read list file '{}'", list_path.display()))?;
    let mut pairs = Vec::new();
    for (line_no, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((archive_path, src_str)) = line.split_once('\t') {
            pairs.push((
                archive_path.trim().to_lowercase().replace('/', "\\"),
                PathBuf::from(src_str.trim()),
            ));
        } else {
            let src = PathBuf::from(line);
            let archive_path = derive_archive_path(&src, base).with_context(|| {
                format!("list file line {}: cannot derive archive path", line_no + 1)
            })?;
            pairs.push((archive_path, src));
        }
    }
    Ok(pairs)
}

/// Derive an archive-internal path for a source file.
///
/// If `base` is given, the source path is made relative to it; otherwise just
/// the file name is used.
fn derive_archive_path(src: &Path, base: Option<&Path>) -> Result<String> {
    let rel = match base {
        Some(b) => src
            .strip_prefix(b)
            .with_context(|| {
                format!(
                    "'{}' does not start with base '{}'",
                    src.display(),
                    b.display()
                )
            })?
            .to_path_buf(),
        None => PathBuf::from(
            src.file_name()
                .ok_or_else(|| anyhow::anyhow!("'{}' has no file name component", src.display()))?,
        ),
    };
    Ok(rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect::<Vec<_>>()
        .join("\\"))
}

// ── Tests ─────────────────────────────────────────────────────────────────────
//
// The collector functions are binary-private, so these tests must live inline.

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::{NamedTempFile, TempDir};

    // ── derive_archive_path ──────────────────────────────────────────────────

    #[test]
    fn derive_path_no_base_uses_filename() {
        let src = Path::new("/some/dir/strings.dlstrings");
        let ap = derive_archive_path(src, None).unwrap();
        assert_eq!(ap, "strings.dlstrings");
    }

    #[test]
    fn derive_path_with_base_strips_prefix() {
        let src = Path::new("/assets/data/strings/en.strings");
        let base = Path::new("/assets");
        let ap = derive_archive_path(src, Some(base)).unwrap();
        assert_eq!(ap, "data\\strings\\en.strings");
    }

    #[test]
    fn derive_path_lowercases_result() {
        let src = Path::new("/root/Strings/EN.STRINGS");
        let base = Path::new("/root");
        let ap = derive_archive_path(src, Some(base)).unwrap();
        assert_eq!(ap, "strings\\en.strings");
    }

    #[test]
    fn derive_path_base_mismatch_returns_error() {
        let src = Path::new("/other/file.txt");
        let base = Path::new("/assets");
        assert!(derive_archive_path(src, Some(base)).is_err());
    }

    // ── collect_from_list ────────────────────────────────────────────────────

    fn make_list_file(content: &str) -> NamedTempFile {
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(content.as_bytes()).unwrap();
        tmp.flush().unwrap();
        tmp
    }

    #[test]
    fn collect_list_tab_separated_explicit_archive_path() {
        let list = make_list_file("data\\strings.bin\t/src/strings.bin\n");
        let pairs = collect_from_list(list.path(), None).unwrap();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0, "data\\strings.bin");
        assert_eq!(pairs[0].1, PathBuf::from("/src/strings.bin"));
    }

    #[test]
    fn collect_list_skips_comments_and_blank_lines() {
        let list = make_list_file("# this is a comment\n\n  \n/src/file.txt\n");
        // The non-TAB line uses derive_archive_path(no base) → filename only.
        let pairs = collect_from_list(list.path(), None).unwrap();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0, "file.txt");
    }

    #[test]
    fn collect_list_tab_path_is_lowercased_and_normalised() {
        // Archive path in the list file uses forward slashes and mixed case.
        let list = make_list_file("Data/Strings/EN.bin\t/src/en.bin\n");
        let pairs = collect_from_list(list.path(), None).unwrap();
        assert_eq!(pairs[0].0, "data\\strings\\en.bin");
    }

    // ── collect_from_files ───────────────────────────────────────────────────

    #[test]
    fn collect_files_no_base_uses_filenames() {
        let dir = TempDir::new().unwrap();
        let f1 = dir.path().join("Alpha.txt");
        let f2 = dir.path().join("beta.bin");
        std::fs::write(&f1, b"a").unwrap();
        std::fs::write(&f2, b"b").unwrap();

        let pairs = collect_from_files(&[f1, f2], None).unwrap();
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].0, "alpha.txt");
        assert_eq!(pairs[1].0, "beta.bin");
    }

    #[test]
    fn collect_files_with_base_strips_prefix() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("strings");
        std::fs::create_dir(&sub).unwrap();
        let f = sub.join("en.strings");
        std::fs::write(&f, b"data").unwrap();

        let pairs = collect_from_files(&[f], Some(dir.path())).unwrap();
        assert_eq!(pairs[0].0, "strings\\en.strings");
    }

    // ── collect_from_dir ─────────────────────────────────────────────────────

    #[test]
    fn collect_dir_produces_relative_backslash_paths() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("Data").join("Strings");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("EN.strings"), b"").unwrap();
        std::fs::write(dir.path().join("readme.txt"), b"").unwrap();

        let mut pairs = collect_from_dir(dir.path()).unwrap();
        pairs.sort_by(|a, b| a.0.cmp(&b.0));

        // All paths lowercased, backslash-separated, relative to dir.
        assert!(
            pairs
                .iter()
                .any(|(ap, _)| ap == "data\\strings\\en.strings"),
            "expected data\\strings\\en.strings in {:?}",
            pairs
        );
        assert!(
            pairs.iter().any(|(ap, _)| ap == "readme.txt"),
            "expected readme.txt in {:?}",
            pairs
        );
    }
}
