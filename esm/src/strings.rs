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

use anyhow::{bail, Context, Result};
use std::collections::HashMap;

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
}

/// A parsed string table mapping LString IDs to UTF-8 text.
pub struct StringTable {
    entries: HashMap<u32, String>,
}

impl StringTable {
    /// Return an empty table (used as a placeholder when a file is absent).
    pub fn empty() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Parse a raw byte slice according to the given [`StringKind`].
    pub fn parse(bytes: &[u8], kind: StringKind) -> Result<Self> {
        if bytes.len() < 8 {
            bail!("string table too small ({} bytes)", bytes.len());
        }
        let count = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let data_size = u32::from_le_bytes(bytes[4..8].try_into().unwrap());

        let index_start = 8usize;
        let index_size = count as usize * 8; // each entry: id(4) + offset(4)
        let index_end = index_start + index_size;
        let data_start = index_end;
        let data_end = data_start + data_size as usize;

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
        let mut entries = HashMap::with_capacity(count as usize);

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
            entries.insert(id, text);
        }

        Ok(Self { entries })
    }

    /// Look up a string by its LString ID.
    pub fn get(&self, id: u32) -> Option<&str> {
        self.entries.get(&id).map(String::as_str)
    }

    /// Number of strings in this table.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` if the table contains no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// All three localization tables for a single language prefix.
pub struct Localization {
    pub strings: StringTable,
    pub dlstrings: StringTable,
    pub ilstrings: StringTable,
}

impl Localization {
    /// Load all three string tables from a GNRL BA2 archive.
    ///
    /// The prefix is discovered automatically by scanning the archive for an
    /// entry matching `strings/<prefix>_<locale>.strings`.  `locale` is the
    /// language code (e.g. `"en"`).
    pub fn from_ba2(ba2_path: impl AsRef<std::path::Path>, locale: &str) -> Result<Self> {
        let ba2_path = ba2_path.as_ref();
        let archive = crate::ba2::Ba2Archive::open(ba2_path)
            .with_context(|| format!("opening BA2 {}", ba2_path.display()))?;

        // Auto-discover the prefix by finding strings/<prefix>_<locale>.strings.
        // All entry names in the archive are already lower-cased by the BA2 reader.
        let suffix = format!("_{}.strings", locale.to_lowercase());
        let prefix = archive
            .list()
            .iter()
            .find_map(|e| {
                let name = &e.name;
                if name.starts_with("strings/") && name.ends_with(&suffix) {
                    let inner = &name["strings/".len()..name.len() - suffix.len()];
                    if !inner.is_empty() {
                        return Some(inner.to_string());
                    }
                }
                None
            })
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "no strings/*_{locale}.strings entry found in {path}",
                    locale = locale,
                    path = ba2_path.display()
                )
            })?;

        let read = |ext: &str| -> Result<StringTable> {
            let name = format!("strings/{}_{}.{}", prefix, locale.to_lowercase(), ext);
            let bytes = archive
                .read(&name)
                .with_context(|| format!("reading {} from BA2", name))?;
            let kind = StringKind::from_extension(ext)
                .ok_or_else(|| anyhow::anyhow!("unknown string extension: {}", ext))?;
            StringTable::parse(&bytes, kind)
                .with_context(|| format!("parsing string table {}", name))
        };

        Ok(Self {
            strings: read("strings")?,
            dlstrings: read("dlstrings")?,
            ilstrings: read("ilstrings")?,
        })
    }

    /// Load all three string tables from loose `.strings` / `.dlstrings` / `.ilstrings` files.
    ///
    /// Looks for files at `<dir>/<prefix>_<locale>.{strings,dlstrings,ilstrings}`.
    /// The prefix is typically the ESM stem (e.g. `"MyMod"`).
    pub fn from_loose_files(
        dir: impl AsRef<std::path::Path>,
        locale: &str,
        prefix: &str,
    ) -> Result<Self> {
        let dir = dir.as_ref();

        let load = |ext: &str| -> Result<StringTable> {
            let filename = format!("{}_{}.{}", prefix, locale, ext);
            let path = dir.join(&filename);
            let bytes =
                std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
            let kind = StringKind::from_extension(ext)
                .ok_or_else(|| anyhow::anyhow!("unknown string extension: {}", ext))?;
            StringTable::parse(&bytes, kind)
                .with_context(|| format!("parsing string table {}", path.display()))
        };

        Ok(Self {
            strings: load("strings")?,
            dlstrings: load("dlstrings")?,
            ilstrings: load("ilstrings")?,
        })
    }

    /// Look up a string by table kind and LString ID.
    pub fn lookup(&self, kind: StringKind, id: u32) -> Option<&str> {
        match kind {
            StringKind::Strings => self.strings.get(id),
            StringKind::DlStrings => self.dlstrings.get(id),
            StringKind::IlStrings => self.ilstrings.get(id),
        }
    }
}
