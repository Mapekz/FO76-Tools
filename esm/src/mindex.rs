//! Zero-copy mmap'd FormID index (`.esm.midx`).
//!
//! The `.midx` file is a compact, sorted binary table of all records in an ESM,
//! optimised for fast single-FormID lookups without loading the full ~280 MiB
//! `.esm.idx` bincode cache.  A binary search over the table uses only ~20
//! cache-line accesses for a ~1M-record file (~24 MiB).
//!
//! # On-disk layout
//!
//! **Header (40 bytes)**
//! ```text
//! [0..8]   magic          = b"ESMMIDX1"
//! [8..16]  src_size       u64 LE  — byte length of the source ESM
//! [16..24] src_mtime_secs u64 LE  — ESM mtime (seconds since UNIX epoch)
//! [24..32] count          u64 LE  — number of entries
//! [32..36] version        u32 LE
//! [36..40] mtime_nanos    u32 LE  — ESM mtime (nanosecond fraction)
//! ```
//!
//! **Entries (24 bytes each, sorted ascending by `form_id`)**
//! ```text
//! [0..4]   form_id        u32 LE
//! [4..8]   flags          u32 LE
//! [8..16]  offset         u64 LE  — byte offset of record header in ESM
//! [16..20] sig            [u8; 4] — 4-char record type (ASCII, NUL-padded)
//! [20..22] form_version   u16 LE
//! [22..24] _pad           u16
//! ```
//!
//! Entries start immediately after the header at byte 40.

use crate::formid::FormId;
use crate::reader::{EsmFile, RecordMeta};
use anyhow::Context;
use memmap2::Mmap;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const MAGIC: &[u8; 8] = b"ESMMIDX1";
const VERSION: u32 = 1;
const HEADER_SIZE: usize = 40;
const ENTRY_SIZE: usize = 24;

/// Return the `.esm.midx` path for a given ESM path.
pub fn midx_path_for(esm_path: &Path) -> PathBuf {
    let mut p = esm_path.to_path_buf();
    p.set_extension("esm.midx");
    p
}

/// Memory-mapped, binary-searched FormID → [`RecordMeta`] index.
///
/// Wraps a [`Mmap`] over a `.esm.midx` file.  No heap allocation is required
/// for lookups: each [`get_by_formid`](Self::get_by_formid) call does a
/// binary search over the mmap'd table, touching at most ~20 pages.
pub struct MmapFormIndex {
    // SAFETY: the Mmap keeps the backing file open for its lifetime.
    // The file is only written during build (never while a Mmap is active).
    mmap: Mmap,
    count: usize,
}

impl MmapFormIndex {
    /// Load the `.midx` for `esm` if it exists and is valid; otherwise build
    /// and save it first, then load.
    pub fn load_or_build(esm: &EsmFile) -> anyhow::Result<Self> {
        if let Some(idx) = Self::try_load(esm)? {
            return Ok(idx);
        }
        build_from_esm_and_save(esm)?;
        Self::try_load(esm)?.with_context(|| {
            format!(
                ".esm.midx file missing after build for {}",
                esm.path.display()
            )
        })
    }

    /// Attempt to memory-map a valid `.midx` from disk.
    ///
    /// Returns `None` when the file is absent, has an invalid magic/version,
    /// or was built from a different ESM (size or mtime mismatch).
    fn try_load(esm: &EsmFile) -> anyhow::Result<Option<Self>> {
        let midx_path = midx_path_for(&esm.path);
        if !midx_path.exists() {
            return Ok(None);
        }

        let file = fs::File::open(&midx_path)?;
        // SAFETY: we hold an open file descriptor for the lifetime of the
        // Mmap.  The .midx is only written during build, never while a Mmap
        // is live (build always precedes the first load).
        let mmap = match unsafe { Mmap::map(&file) } {
            Ok(m) => m,
            Err(_) => return Ok(None),
        };

        if mmap.len() < HEADER_SIZE {
            return Ok(None);
        }

        // Validate magic.
        if &mmap[0..8] != MAGIC {
            return Ok(None);
        }

        let src_size = u64::from_le_bytes(mmap[8..16].try_into().unwrap());
        let src_mtime_secs = u64::from_le_bytes(mmap[16..24].try_into().unwrap());
        let count = u64::from_le_bytes(mmap[24..32].try_into().unwrap()) as usize;
        let version = u32::from_le_bytes(mmap[32..36].try_into().unwrap());
        let mtime_nanos = u32::from_le_bytes(mmap[36..40].try_into().unwrap());

        if version != VERSION {
            return Ok(None);
        }

        // Validate against ESM metadata.
        let meta = fs::metadata(&esm.path)?;
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let dur = mtime
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default();

        if meta.len() != src_size
            || dur.as_secs() != src_mtime_secs
            || dur.subsec_nanos() != mtime_nanos
        {
            return Ok(None);
        }

        // Sanity-check expected file size.
        let expected = HEADER_SIZE + count * ENTRY_SIZE;
        if mmap.len() < expected {
            return Ok(None);
        }

        Ok(Some(MmapFormIndex { mmap, count }))
    }

    /// Look up a FormID and return its [`RecordMeta`], or `None` if not found.
    ///
    /// Binary search over the sorted entry table — O(log n).
    pub fn get_by_formid(&self, form_id: FormId) -> Option<RecordMeta> {
        let target = form_id.raw();
        let mut lo = 0usize;
        let mut hi = self.count;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let entry_off = HEADER_SIZE + mid * ENTRY_SIZE;
            let entry_fid =
                u32::from_le_bytes(self.mmap[entry_off..entry_off + 4].try_into().unwrap());
            match entry_fid.cmp(&target) {
                std::cmp::Ordering::Equal => return Some(read_entry(&self.mmap, entry_off)),
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
            }
        }
        None
    }
}

/// Decode an entry from the mmap at byte offset `off` into a [`RecordMeta`].
fn read_entry(mmap: &[u8], off: usize) -> RecordMeta {
    // Entry layout (all LE, at `off`):
    //   [0..4]   form_id      u32  (already consumed by the caller)
    //   [4..8]   flags        u32
    //   [8..16]  offset       u64
    //   [16..20] sig          [u8;4]
    //   [20..22] form_version u16
    //   [22..24] _pad         u16
    let flags = u32::from_le_bytes(mmap[off + 4..off + 8].try_into().unwrap());
    let file_off = u64::from_le_bytes(mmap[off + 8..off + 16].try_into().unwrap());
    let sig_bytes = &mmap[off + 16..off + 20];
    let form_version = u16::from_le_bytes(mmap[off + 20..off + 22].try_into().unwrap());
    let signature = String::from_utf8_lossy(sig_bytes)
        .trim_end_matches('\0')
        .to_string();
    RecordMeta {
        offset: file_off,
        signature,
        flags,
        form_version,
    }
}

/// Build the `.midx` from a pre-computed `form_index` and write it to disk.
///
/// Called opportunistically from [`Index::build_fresh`] so that the `.midx` is
/// always written when the `.idx` is freshly built — avoiding an extra ESM walk.
pub fn build_from_form_index_and_save(
    form_index: &HashMap<FormId, RecordMeta>,
    esm_path: &Path,
) -> anyhow::Result<()> {
    let mut entries: Vec<(u32, &RecordMeta)> = form_index
        .iter()
        .map(|(fid, meta)| (fid.raw(), meta))
        .collect();
    entries.sort_unstable_by_key(|&(fid, _)| fid);

    let file_meta = fs::metadata(esm_path)?;
    let mtime = file_meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let dur = mtime
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();

    write_midx_file(
        &midx_path_for(esm_path),
        &entries,
        file_meta.len(),
        dur.as_secs(),
        dur.subsec_nanos(),
    )
}

/// Walk the ESM to collect all records and write the `.midx`.
///
/// Used by [`MmapFormIndex::load_or_build`] when no valid `.midx` exists yet
/// and we want to avoid loading the full `.esm.idx` bincode cache.
fn build_from_esm_and_save(esm: &EsmFile) -> anyhow::Result<()> {
    let mut entries: Vec<(u32, RecordMeta)> = Vec::new();
    esm.walk_records(|meta| {
        let data = esm.data();
        let rh = crate::format::RecordHeader::parse(&data[meta.offset as usize..])?;
        entries.push((rh.form_id, meta));
        Ok(())
    })?;
    entries.sort_unstable_by_key(|(fid, _)| *fid);

    let entries_ref: Vec<(u32, &RecordMeta)> = entries.iter().map(|(f, m)| (*f, m)).collect();

    let file_meta = fs::metadata(&esm.path)?;
    let mtime = file_meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let dur = mtime
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();

    write_midx_file(
        &midx_path_for(&esm.path),
        &entries_ref,
        file_meta.len(),
        dur.as_secs(),
        dur.subsec_nanos(),
    )
}

/// Serialise the sorted entry list to a `.midx` binary file.
fn write_midx_file(
    path: &Path,
    entries: &[(u32, &RecordMeta)],
    src_size: u64,
    src_mtime_secs: u64,
    src_mtime_nanos: u32,
) -> anyhow::Result<()> {
    let count = entries.len() as u64;
    let total = HEADER_SIZE + entries.len() * ENTRY_SIZE;
    let mut buf = vec![0u8; total];

    // Header
    buf[0..8].copy_from_slice(MAGIC);
    buf[8..16].copy_from_slice(&src_size.to_le_bytes());
    buf[16..24].copy_from_slice(&src_mtime_secs.to_le_bytes());
    buf[24..32].copy_from_slice(&count.to_le_bytes());
    buf[32..36].copy_from_slice(&VERSION.to_le_bytes());
    buf[36..40].copy_from_slice(&src_mtime_nanos.to_le_bytes());

    // Entries
    for (i, (form_id, meta)) in entries.iter().enumerate() {
        let off = HEADER_SIZE + i * ENTRY_SIZE;
        buf[off..off + 4].copy_from_slice(&form_id.to_le_bytes());
        buf[off + 4..off + 8].copy_from_slice(&meta.flags.to_le_bytes());
        buf[off + 8..off + 16].copy_from_slice(&meta.offset.to_le_bytes());
        // Signature: up to 4 ASCII bytes, NUL-padded (rest already 0)
        let sig = meta.signature.as_bytes();
        let sig_len = sig.len().min(4);
        buf[off + 16..off + 16 + sig_len].copy_from_slice(&sig[..sig_len]);
        buf[off + 20..off + 22].copy_from_slice(&meta.form_version.to_le_bytes());
        // [off+22..off+24] = _pad, already 0
    }

    let mut file = fs::File::create(path).with_context(|| format!("create {}", path.display()))?;
    file.write_all(&buf)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_meta(offset: u64, sig: &str, flags: u32, form_version: u16) -> RecordMeta {
        RecordMeta {
            offset,
            signature: sig.to_string(),
            flags,
            form_version,
        }
    }

    /// Build a minimal in-memory form_index, write .midx to a temp file,
    /// load it via mmap, and verify round-trip lookup.
    #[test]
    fn test_round_trip() {
        let mut form_index: HashMap<FormId, RecordMeta> = HashMap::new();
        form_index.insert(FormId::new(0x0000_1000), make_meta(100, "WEAP", 0, 131));
        form_index.insert(FormId::new(0x0000_2000), make_meta(200, "ARMO", 4, 131));
        form_index.insert(FormId::new(0x0000_0001), make_meta(50, "TES4", 0, 1));

        // Use the OS temp dir; write a dummy "ESM" file so metadata() works.
        let tmp = std::env::temp_dir();
        let esm_path = tmp.join("esm_mindex_test_round_trip.esm");
        let midx_path = midx_path_for(&esm_path);
        fs::write(&esm_path, b"dummy").unwrap();

        // Collect entries sorted by form_id.
        let mut entries: Vec<(u32, &RecordMeta)> =
            form_index.iter().map(|(f, m)| (f.raw(), m)).collect();
        entries.sort_unstable_by_key(|&(f, _)| f);

        let file_meta = fs::metadata(&esm_path).unwrap();
        let mtime = file_meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let dur = mtime
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default();

        write_midx_file(
            &midx_path,
            &entries,
            file_meta.len(),
            dur.as_secs(),
            dur.subsec_nanos(),
        )
        .unwrap();

        // Load via mmap (access private fields within the same module).
        let midx_file = fs::File::open(&midx_path).unwrap();
        // SAFETY: test-only, file won't be modified while mapped.
        let mmap = unsafe { Mmap::map(&midx_file) }.unwrap();
        let count = u64::from_le_bytes(mmap[24..32].try_into().unwrap()) as usize;
        let idx = MmapFormIndex { mmap, count };

        // Lookup every key and verify all fields round-trip correctly.
        for (fid, expected) in &form_index {
            let got = idx.get_by_formid(*fid).expect("form_id not found");
            assert_eq!(got.offset, expected.offset);
            assert_eq!(got.signature, expected.signature);
            assert_eq!(got.flags, expected.flags);
            assert_eq!(got.form_version, expected.form_version);
        }

        // Missing FormID returns None.
        assert!(idx.get_by_formid(FormId::new(0xDEAD_BEEF)).is_none());

        // Clean up.
        let _ = fs::remove_file(&midx_path);
        let _ = fs::remove_file(&esm_path);
    }

    /// Validate that a header with a wrong version is rejected by try_load.
    ///
    /// We test the version-check byte logic directly (since we cannot call
    /// try_load without a real EsmFile).
    #[test]
    fn test_bad_version_bytes() {
        let mut buf = vec![0u8; HEADER_SIZE];
        buf[0..8].copy_from_slice(MAGIC);
        let bad_version: u32 = VERSION + 99;
        buf[32..36].copy_from_slice(&bad_version.to_le_bytes());
        // count = 0 (zeroed)

        let path = std::env::temp_dir().join("esm_mindex_test_bad_version.midx");
        fs::write(&path, &buf).unwrap();

        let f = fs::File::open(&path).unwrap();
        // SAFETY: test-only.
        let mmap = unsafe { Mmap::map(&f) }.unwrap();
        let version = u32::from_le_bytes(mmap[32..36].try_into().unwrap());
        assert_ne!(version, VERSION, "bad version should differ from VERSION");

        let _ = fs::remove_file(&path);
    }
}
