//! String table reader for Fallout 76 localization files.
//!
//! Parses `.strings` / `.dlstrings` / `.ilstrings` files extracted from the
//! localization BA2 archive and provides fast LString ID lookup.
//!
//! File format:
//! - `count`     : u32 LE — number of entries
//! - `data_size` : u32 LE — total byte size of the data block
//! - `count` × (id: u32 LE, offset: u32 LE) — index
//! - data block (size = `data_size`)
//!
//! For `.strings`:   `data[offset..]` is a NUL-terminated (zstring) UTF-8 string.
//! For `.dlstrings` / `.ilstrings`:  `data[offset..]` starts with a u32 LE `len`
//!   followed by `len` UTF-8 bytes; `len` *includes* the NUL terminator.
//!
//! Parsed tables are stored in an rkyv-archivable form (sorted ids plus one
//! text blob), so the tables `Database::open` discovers next to an ESM are
//! parsed once and then mapped zero-copy from the `lstrings` cache section.

use crate::discover::StringsSrc;
use crate::rkyvcache::{ArchiveBuf, SectionKind, SectionSpec};
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

/// Which string table a localised string belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StringKind {
    Strings,
    DlStrings,
    IlStrings,
}

impl StringKind {
    /// Map a file extension (without leading dot) to its [`StringKind`].
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_lowercase().as_str() {
            "strings" => Some(Self::Strings),
            "dlstrings" => Some(Self::DlStrings),
            "ilstrings" => Some(Self::IlStrings),
            _ => None,
        }
    }

    const ALL: [StringKind; 3] = [Self::Strings, Self::DlStrings, Self::IlStrings];

    fn extension(self) -> &'static str {
        match self {
            Self::Strings => "strings",
            Self::DlStrings => "dlstrings",
            Self::IlStrings => "ilstrings",
        }
    }
}

/// A parsed string table: ids sorted ascending, and `ends[i]` the end offset
/// in `text` of entry `i`'s string (its start is `ends[i - 1]`, or 0).
#[derive(Debug, Default, rkyv::Archive, rkyv::Serialize)]
pub struct StringTable {
    ids: Vec<u32>,
    ends: Vec<u32>,
    text: String,
}

impl StringTable {
    /// Parse a raw byte slice according to the given [`StringKind`]. When an
    /// id appears more than once, the last entry wins.
    pub fn parse(bytes: &[u8], kind: StringKind) -> Result<Self> {
        if bytes.len() < 8 {
            bail!("string table too small ({} bytes)", bytes.len());
        }
        let count = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let data_size = u32::from_le_bytes(bytes[4..8].try_into().unwrap());

        let index_start = 8usize;
        let index_size = (count as usize)
            .checked_mul(8) // each entry: id(4) + offset(4)
            .ok_or_else(|| anyhow::anyhow!("strings index size overflow"))?;
        let index_end = index_start
            .checked_add(index_size)
            .ok_or_else(|| anyhow::anyhow!("strings index_end overflow"))?;
        let data_start = index_end;
        let data_end = data_start
            .checked_add(data_size as usize)
            .ok_or_else(|| anyhow::anyhow!("strings data_end overflow"))?;

        if index_end > bytes.len() {
            bail!(
                "string table index out of range: need {} bytes, have {}",
                index_end,
                bytes.len()
            );
        }
        if data_end > bytes.len() {
            bail!(
                "string table data block out of range: data_end={} bytes.len()={}",
                data_end,
                bytes.len()
            );
        }

        let data = &bytes[data_start..data_end];
        let mut entries: Vec<(u32, String)> = Vec::with_capacity(count as usize);

        for i in 0..count as usize {
            let base = index_start + i * 8;
            let id = u32::from_le_bytes(bytes[base..base + 4].try_into().unwrap());
            let offset = u32::from_le_bytes(bytes[base + 4..base + 8].try_into().unwrap()) as usize;

            let text = match kind {
                StringKind::Strings => {
                    if offset >= data.len() {
                        continue;
                    }
                    let end = data[offset..]
                        .iter()
                        .position(|&b| b == 0)
                        .unwrap_or(data.len() - offset);
                    String::from_utf8_lossy(&data[offset..offset + end]).into_owned()
                }
                StringKind::DlStrings | StringKind::IlStrings => {
                    if offset + 4 > data.len() {
                        continue;
                    }
                    let len =
                        u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
                    let str_start = offset + 4;
                    let str_end = (str_start + len).min(data.len());
                    // `len` includes the NUL terminator — trim it.
                    let text_end =
                        if str_end > str_start && data.get(str_end.saturating_sub(1)) == Some(&0) {
                            str_end - 1
                        } else {
                            str_end
                        };
                    if str_start > text_end {
                        String::new()
                    } else {
                        String::from_utf8_lossy(&data[str_start..text_end]).into_owned()
                    }
                }
            };
            entries.push((id, text));
        }

        Ok(Self::from_entries(entries))
    }

    /// Build a table from `(id, text)` pairs; a repeated id keeps its last text.
    fn from_entries(mut entries: Vec<(u32, String)>) -> Self {
        // Stable sort keeps file order among equal ids, so the last of each run
        // is the last one in the file.
        entries.sort_by_key(|(id, _)| *id);
        let mut table = StringTable::default();
        let mut iter = entries.into_iter().peekable();
        while let Some((id, text)) = iter.next() {
            if iter.peek().is_some_and(|(next, _)| *next == id) {
                continue;
            }
            table.text.push_str(&text);
            table.ids.push(id);
            table.ends.push(table.text.len() as u32);
        }
        table
    }

    /// Number of strings in this table.
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    /// `true` if the table contains no entries.
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
}

impl ArchivedStringTable {
    fn get(&self, id: u32) -> Option<&str> {
        let i = self.ids.binary_search_by_key(&id, |x| x.to_native()).ok()?;
        let start = if i == 0 {
            0
        } else {
            self.ends[i - 1].to_native() as usize
        };
        let end = self.ends[i].to_native() as usize;
        self.text.as_str().get(start..end)
    }
}

/// All three tables for one ESM and locale, plus a stamp of the files they
/// were parsed from — the `lstrings` cache section.
#[derive(rkyv::Archive, rkyv::Serialize)]
pub(crate) struct StringsSection {
    source: u64,
    strings: StringTable,
    dlstrings: StringTable,
    ilstrings: StringTable,
}

impl ArchivedStringsSection {
    fn table(&self, kind: StringKind) -> &ArchivedStringTable {
        match kind {
            StringKind::Strings => &self.strings,
            StringKind::DlStrings => &self.dlstrings,
            StringKind::IlStrings => &self.ilstrings,
        }
    }
}

const LSTRINGS_LAYOUT_FINGERPRINT: u64 = {
    use crate::rkyvcache::{FNV_OFFSET_BASIS, fnv1a_u64};
    let acc = fnv1a_u64(
        FNV_OFFSET_BASIS,
        core::mem::size_of::<ArchivedStringsSection>() as u64,
    );
    let acc = fnv1a_u64(acc, core::mem::align_of::<ArchivedStringsSection>() as u64);
    let acc = fnv1a_u64(acc, core::mem::size_of::<ArchivedStringTable>() as u64);
    fnv1a_u64(acc, core::mem::align_of::<ArchivedStringTable>() as u64)
};

impl SectionSpec for ArchivedStringsSection {
    const KIND: SectionKind = SectionKind::Strings;
    const LAYOUT_FINGERPRINT: u64 = LSTRINGS_LAYOUT_FINGERPRINT;
}

/// The three localization tables for a single language prefix.
pub struct Localization {
    tables: ArchiveBuf<ArchivedStringsSection>,
}

impl Localization {
    /// Load all three string tables from a GNRL BA2 archive.
    ///
    /// `prefix` is the ESM's file stem (e.g. `"SeventySix"`) and `locale` is
    /// the language code (e.g. `"en"`); entries are looked up at
    /// `strings/<prefix>_<locale>.{strings,dlstrings,ilstrings}` (matched
    /// case-insensitively — see [`Ba2Archive::read`](crate::ba2::Ba2Archive::read)).
    ///
    /// The prefix is **not** auto-discovered by scanning the archive: a real
    /// Localization BA2 can bundle more than one product's string tables
    /// alongside the game's own (e.g. a shared `nw_<locale>.strings` family
    /// next to `<esm-stem>_<locale>.strings`), so picking "the first
    /// `strings/*_<locale>.strings` entry found" is not reliable — it can
    /// silently select the wrong table's IDs. Always resolve against the
    /// known ESM stem instead.
    pub fn from_ba2(ba2_path: impl AsRef<Path>, locale: &str, prefix: &str) -> Result<Self> {
        let section = parse_ba2(ba2_path.as_ref(), locale, prefix, 0)?;
        Ok(Self {
            tables: ArchiveBuf::serialize(&section)?,
        })
    }

    /// Load all three string tables from loose `.strings` / `.dlstrings` / `.ilstrings` files.
    ///
    /// Looks for files at `<dir>/<prefix>_<locale>.{strings,dlstrings,ilstrings}`.
    /// The prefix is typically the ESM stem (e.g. `"MyMod"`).
    pub fn from_loose_files(dir: impl AsRef<Path>, locale: &str, prefix: &str) -> Result<Self> {
        let section = parse_loose(dir.as_ref(), locale, prefix, 0)?;
        Ok(Self {
            tables: ArchiveBuf::serialize(&section)?,
        })
    }

    /// The tables `Database::open` discovered for `esm_path`, served from the
    /// `lstrings` cache section: parsed and published on first use, then
    /// mapped. A section built from different source files (or another
    /// locale) is rebuilt.
    pub(crate) fn cached(
        esm_path: &Path,
        src: &StringsSrc,
        locale: &str,
        prefix: &str,
    ) -> Result<Self> {
        let files = match src {
            StringsSrc::Loose(dir) => loose_paths(dir, locale, prefix),
            StringsSrc::Ba2(path) => vec![path.clone()],
        };
        let stamp = crate::rkyvcache::source_stamp(&files, &format!("{prefix}_{locale}"))?;
        let section = crate::rkyvcache::map_or_build::<StringsSection>(
            esm_path,
            StringKind::ALL.len() as u64,
            |cached| cached.source.to_native() == stamp,
            |_lease| match src {
                StringsSrc::Loose(dir) => parse_loose(dir, locale, prefix, stamp),
                StringsSrc::Ba2(path) => parse_ba2(path, locale, prefix, stamp),
            },
        )?;
        Ok(Self {
            tables: ArchiveBuf::Mapped(section),
        })
    }

    /// Look up a string by table kind and LString ID.
    pub fn lookup(&self, kind: StringKind, id: u32) -> Option<&str> {
        self.tables.get().table(kind).get(id)
    }

    /// Number of strings in one table.
    pub fn len(&self, kind: StringKind) -> usize {
        self.tables.get().table(kind).ids.len()
    }
}

fn loose_paths(dir: &Path, locale: &str, prefix: &str) -> Vec<PathBuf> {
    StringKind::ALL
        .iter()
        .map(|kind| dir.join(format!("{prefix}_{locale}.{}", kind.extension())))
        .collect()
}

fn parse_loose(dir: &Path, locale: &str, prefix: &str, source: u64) -> Result<StringsSection> {
    let [strings, dlstrings, ilstrings] = StringKind::ALL.map(|kind| {
        let path = dir.join(format!("{prefix}_{locale}.{}", kind.extension()));
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        StringTable::parse(&bytes, kind)
            .with_context(|| format!("parsing string table {}", path.display()))
    });
    Ok(StringsSection {
        source,
        strings: strings?,
        dlstrings: dlstrings?,
        ilstrings: ilstrings?,
    })
}

fn parse_ba2(ba2_path: &Path, locale: &str, prefix: &str, source: u64) -> Result<StringsSection> {
    let archive = crate::ba2::Ba2Archive::open(ba2_path)
        .with_context(|| format!("opening BA2 {}", ba2_path.display()))?;
    let [strings, dlstrings, ilstrings] = StringKind::ALL.map(|kind| {
        let name = format!(
            "strings/{}_{}.{}",
            prefix.to_lowercase(),
            locale.to_lowercase(),
            kind.extension()
        );
        let bytes = archive
            .read(&name)
            .with_context(|| format!("reading {name} from BA2"))?;
        StringTable::parse(&bytes, kind).with_context(|| format!("parsing string table {name}"))
    });
    Ok(StringsSection {
        source,
        strings: strings?,
        dlstrings: dlstrings?,
        ilstrings: ilstrings?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A string table buffer with `count = u32::MAX` must be rejected.
    ///
    /// On 64-bit targets `checked_mul(8)` does not overflow (u32::MAX * 8 fits
    /// in u64), but the subsequent `index_end > bytes.len()` bounds check
    /// catches the out-of-range index.  On 32-bit targets the `checked_mul`
    /// itself overflows.  Either way `StringTable::parse` must return an error.
    #[test]
    fn strings_large_count_rejected() {
        let count: u32 = u32::MAX;
        let data_size: u32 = 0;
        let mut buf = Vec::new();
        buf.extend_from_slice(&count.to_le_bytes());
        buf.extend_from_slice(&data_size.to_le_bytes());
        // No index or data follows — the bounds check must fire.

        let result = StringTable::parse(&buf, StringKind::Strings);
        assert!(
            result.is_err(),
            "expected error for overflowing count, got Ok"
        );
    }

    /// A string table buffer with `data_size` pointing past the end of the
    /// buffer must be rejected even when `count = 0`.
    #[test]
    fn strings_oversized_data_block_rejected() {
        let count: u32 = 0;
        let data_size: u32 = u32::MAX;
        let mut buf = Vec::new();
        buf.extend_from_slice(&count.to_le_bytes());
        buf.extend_from_slice(&data_size.to_le_bytes());

        let result = StringTable::parse(&buf, StringKind::Strings);
        assert!(
            result.is_err(),
            "expected error for oversized data_size, got Ok"
        );
    }

    #[test]
    fn repeated_id_keeps_the_last_entry_and_lookup_is_exact() {
        let table = StringTable::from_entries(vec![
            (5, "first".into()),
            (2, "two".into()),
            (5, "last".into()),
        ]);
        assert_eq!(table.len(), 2);
        let section = StringsSection {
            source: 0,
            strings: table,
            dlstrings: StringTable::default(),
            ilstrings: StringTable::default(),
        };
        let loc = Localization {
            tables: ArchiveBuf::serialize(&section).unwrap(),
        };
        assert_eq!(loc.lookup(StringKind::Strings, 5), Some("last"));
        assert_eq!(loc.lookup(StringKind::Strings, 2), Some("two"));
        assert_eq!(loc.lookup(StringKind::Strings, 3), None);
        assert_eq!(loc.len(StringKind::DlStrings), 0);
    }
}
