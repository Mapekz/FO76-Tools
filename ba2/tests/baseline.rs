//! Real-archive baseline: reads the archives under `$BA2_BASELINE_DIR` (the
//! Playtest install's `Data` folder) and compares each against
//! `tests/baselines/playtest.tsv`: an index digest for every archive, plus a
//! content digest (the bytes `Ba2Archive::read` returns for every entry, or
//! for every Nth entry of a large texture archive) for a representative set.
//!
//! Skips when `BA2_BASELINE_DIR` is unset. An archive whose size no longer
//! matches the manifest (the install updated) is reported and skipped; run
//! `just baseline` to rewrite the manifest from the archives on disk.

use ba2::{Ba2Archive, EntryData, ReadCodec};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Archives whose entry contents are digested, with the stride between
/// digested entries (1 = every entry).
const CONTENT: &[(&str, usize)] = &[
    ("SeventySix - Startup.ba2", 1),
    ("SeventySix - Materials.ba2", 1),
    ("SeventySix - MiscClient.ba2", 1),
    ("SeventySix - Interface_en.ba2", 1),
    ("SeventySix - Textures01.ba2", 50),
];

fn manifest_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/baselines/playtest.tsv")
}

/// FNV-1a 64.
fn fnv(mut h: u64, bytes: &[u8]) -> u64 {
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

const FNV_OFFSET: u64 = 0xcbf29ce484222325;

/// Everything the reader parses out of the header and entry tables.
fn index_digest(archive: &Ba2Archive) -> u64 {
    let mut text = format!(
        "{} {:?} {}\n",
        archive.header.version, archive.header.archive_type, archive.header.file_count
    );
    for e in archive.list() {
        let _ = write!(
            text,
            "{}|{:08x}|{:08x}|{:?}|",
            e.name, e.name_hash, e.dir_hash, e.ext
        );
        match &e.data {
            EntryData::Gnrl {
                flags,
                data_offset,
                packed_size,
                unpacked_size,
            } => {
                let _ = writeln!(text, "{flags}|{data_offset}|{packed_size}|{unpacked_size}");
            }
            EntryData::Texture(t) => {
                let _ = write!(
                    text,
                    "{}x{}|{}|{}|{}|{}",
                    t.width, t.height, t.mip_count, t.dxgi_format, t.cubemap, t.tile_mode
                );
                for c in &t.chunks {
                    let _ = write!(
                        text,
                        "|{}:{}:{}:{}-{}",
                        c.data_offset, c.packed_size, c.unpacked_size, c.mip_first, c.mip_last
                    );
                }
                text.push('\n');
            }
        }
    }
    fnv(FNV_OFFSET, text.as_bytes())
}

/// The bytes the reader returns for every `stride`th entry.
fn content_digest(archive: &Ba2Archive, stride: usize) -> u64 {
    let mut h = FNV_OFFSET;
    for e in archive.list().iter().step_by(stride) {
        let bytes = archive
            .read(&e.name, ReadCodec::Auto)
            .unwrap_or_else(|err| panic!("read {}: {err}", e.name));
        h = fnv(h, e.name.as_bytes());
        h = fnv(h, &(bytes.len() as u64).to_le_bytes());
        h = fnv(h, &bytes);
    }
    h
}

struct Row {
    name: String,
    size: u64,
    index: String,
    content: String,
}

fn measure(dir: &Path, name: &str) -> Row {
    let path = dir.join(name);
    let archive = Ba2Archive::open(&path).unwrap_or_else(|e| panic!("open {name}: {e}"));
    let content = CONTENT
        .iter()
        .find(|(n, _)| *n == name)
        .map_or("-".to_owned(), |(_, stride)| {
            format!("{:016x}", content_digest(&archive, *stride))
        });
    Row {
        name: name.to_owned(),
        size: std::fs::metadata(&path).unwrap().len(),
        index: format!("{:016x}", index_digest(&archive)),
        content,
    }
}

fn write_manifest(dir: &Path) {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.to_ascii_lowercase().ends_with(".ba2"))
        .collect();
    names.sort();
    let mut out = String::from("# archive\tsize\tindex_digest\tcontent_digest\n");
    for name in names {
        let row = measure(dir, &name);
        let _ = writeln!(
            out,
            "{}\t{}\t{}\t{}",
            row.name, row.size, row.index, row.content
        );
    }
    std::fs::create_dir_all(manifest_path().parent().unwrap()).unwrap();
    std::fs::write(manifest_path(), out).unwrap();
}

#[test]
fn playtest_archives_match_the_baseline() {
    let Ok(dir) = std::env::var("BA2_BASELINE_DIR") else {
        return;
    };
    let dir = PathBuf::from(dir);
    if std::env::var_os("BA2_BASELINE_WRITE").is_some() {
        write_manifest(&dir);
        return;
    }
    let manifest = std::fs::read_to_string(manifest_path()).expect("read the baseline manifest");
    let mut mismatches = Vec::new();
    for line in manifest.lines().filter(|l| !l.starts_with('#')) {
        let cols: Vec<&str> = line.split('\t').collect();
        let [name, size, index, content] = cols[..] else {
            panic!("malformed manifest line: {line}");
        };
        let path = dir.join(name);
        let Ok(meta) = std::fs::metadata(&path) else {
            eprintln!("skip {name}: not in {}", dir.display());
            continue;
        };
        if meta.len().to_string() != size {
            eprintln!("skip {name}: size changed since the baseline (run `just baseline`)");
            continue;
        }
        let row = measure(&dir, name);
        if row.index != index {
            mismatches.push(format!("{name}: index digest {} != {index}", row.index));
        }
        if row.content != content {
            mismatches.push(format!(
                "{name}: content digest {} != {content}",
                row.content
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "baseline mismatches:\n{}",
        mismatches.join("\n")
    );
}
