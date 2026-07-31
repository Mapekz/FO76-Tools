//! Shared infrastructure for the rkyv-based mmap disk cache that is
//! replacing `bincode` (RUSTSEC-2025-0141 — see `deny.toml` at the repo
//! root) as the format for `esm`'s on-disk caches.
//!
//! A "section" is one memory-mapped file: a fixed 64-byte header
//! ([on-disk layout](#on-disk-header-layout) below) immediately followed by
//! an rkyv archive whose root sits at the *end* of the payload (this is how
//! rkyv lays out archives — see [`Section::get`]'s SAFETY comment). The
//! header carries an identity stamp of the source ESM (size + mtime) plus
//! enough versioning that a stale/foreign/corrupt file is rejected by O(1)
//! header checks alone, so the steady-state read path can hand the mapped
//! bytes straight to [`rkyv::access_unchecked`] with no per-open validation
//! or full-file hashing — the entire point of moving off bincode's
//! multi-second, gigabyte-scale deserialize.
//!
//! # On-disk header layout
//!
//! 64 bytes, little-endian, immediately followed by the rkyv payload:
//!
//! ```text
//! offset  size  field              type       notes
//! ------  ----  -----------------  ---------  ---------------------------------
//! [ 0.. 8)   8  magic              [u8;8]     b"ESMRKYV1"
//! [ 8..12)   4  format_version     u32 LE     header layout version; = 1
//! [12..16)   4  section_kind       u32 LE     SectionKind discriminant
//! [16..24)   8  payload_len        u64 LE     EXACT rkyv payload byte length
//! [24..32)   8  src_size           u64 LE     source ESM byte length
//! [32..40)   8  src_mtime_secs     u64 LE     source ESM mtime, seconds
//! [40..44)   4  src_mtime_nanos    u32 LE     source ESM mtime, nanos
//! [44..48)   4  cache_version      u32 LE     caller's semantic layout version
//! [48..56)   8  layout_fingerprint u64 LE     see `fnv1a_u64`'s doc comment
//! [56..64)   8  reserved           [u8;8]     zero; ignored on read
//! ------  ----  -------------------------------------------------------------
//! [64 .. 64+payload_len)             rkyv archive payload
//! ```
//!
//! # Callers
//!
//! The shared base all five of `Index`'s sections are built on: `tree.rs`'s
//! `TreeIndex` (`tree`) and `index.rs`'s `FormsSection` (`forms`),
//! `EdidSection` (`edid`), `SearchSection` (`search`), and `XrefSection`
//! (`xref`) each call [`write_section`]/[`Section::map`] with their own
//! [`SectionKind`] and layout fingerprint, at the path [`section_path_for`]
//! returns: a fixed-name `esm_cache/` directory, a sibling of the ESM (see
//! [`cache_dir_for`]), holding one file per ESM per section, named
//! `<esm file name>.<section>` (e.g. `esm_cache/SeventySix.esm.forms`) so
//! multiple plugins in one directory never collide.

use anyhow::Context;
use memmap2::Mmap;
use rkyv::ser::Positional;
use std::fs;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::marker::PhantomData;
use std::ops::Range;
use std::path::{Path, PathBuf};

const HEADER_SIZE: usize = 64;
const MAGIC: &[u8; 8] = b"ESMRKYV1";
const FORMAT_VERSION: u32 = 1;

// rkyv's high-level serializer aligns to 16 bytes; the payload must start
// 16-aligned relative to the mmap base (which memmap2 always page-aligns),
// so the header itself must be a multiple of 16.
const _: () = assert!(HEADER_SIZE.is_multiple_of(16));

// `rkyv::primitive::ArchivedUsize` resolves to `ArchivedU16`/`ArchivedU32`/
// `ArchivedU64` depending on which `pointer_width_*` feature is active
// (default, with no feature enabled, is 32-bit). If any workspace crate ever
// turns on rkyv's `pointer_width_64` feature, Cargo feature unification
// silently changes this to 8 bytes crate-wide — every archived `usize` in
// every `Archive`-derived type in this crate would change layout, silently
// invalidating this module's whole "same build, same layout" safety
// argument. Fail the build instead of producing silent UB.
const _: () = assert!(core::mem::size_of::<rkyv::primitive::ArchivedUsize>() == 4);

/// The ESM identity stamp every section header carries, so a section can be
/// validated against the live ESM without opening the ESM itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CacheSig {
    pub size: u64,
    pub mtime_secs: u64,
    pub mtime_nanos: u32,
}

impl CacheSig {
    /// Read the identity stamp from the ESM file at `esm_path` via `fs::metadata`.
    pub(crate) fn read(esm_path: &Path) -> anyhow::Result<Self> {
        let meta =
            fs::metadata(esm_path).with_context(|| format!("stat {}", esm_path.display()))?;
        let mtime = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        let dur = mtime
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap_or_default();
        Ok(CacheSig {
            size: meta.len(),
            mtime_secs: dur.as_secs(),
            mtime_nanos: dur.subsec_nanos(),
        })
    }
}

/// Section kind discriminant stored in the header, so a `.tmp`-renamed file
/// of the wrong kind is rejected rather than misinterpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub(crate) enum SectionKind {
    // Leave room for real variants to be added by later stages without
    // renumbering existing ones.
    Tree = 1,
    Forms = 2,
    Edid = 3,
    Search = 4,
    Xref = 5,
}

impl SectionKind {
    /// File suffix for this section inside [`cache_dir_for`]'s directory
    /// (see [`section_path_for`]). A `match`, not a lookup table, so adding
    /// a variant without naming its suffix here is a compile error rather
    /// than a silent gap.
    const fn file_name(self) -> &'static str {
        match self {
            SectionKind::Tree => "tree",
            SectionKind::Forms => "forms",
            SectionKind::Edid => "edid",
            SectionKind::Search => "search",
            SectionKind::Xref => "xref",
        }
    }
}

/// The shared rkyv cache directory, `esm_cache/`, a sibling of the ESM.
/// Fixed name — does not vary with the ESM's own name — so every ESM in a
/// directory shares one obviously-named cache folder; see
/// [`section_path_for`] for how per-ESM collisions are avoided inside it.
pub(crate) fn cache_dir_for(esm_path: &Path) -> anyhow::Result<PathBuf> {
    let parent = esm_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("esm path has no parent: {}", esm_path.display()))?;
    Ok(parent.join("esm_cache"))
}

/// Path to one section's file inside [`cache_dir_for`]'s directory, named
/// `<esm file name>.<section>` — the full original file name (extension
/// included) so two different plugins in the same directory never collide,
/// followed by the [`SectionKind`] so the path and the kind stored in the
/// file's own header (checked by [`Section::map`]) can never drift apart.
pub(crate) fn section_path_for(esm_path: &Path, kind: SectionKind) -> anyhow::Result<PathBuf> {
    let file_name = esm_path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("esm path has no file name: {}", esm_path.display()))?;
    let mut name = file_name.to_os_string();
    name.push(".");
    name.push(kind.file_name());
    Ok(cache_dir_for(esm_path)?.join(name))
}

/// One memory-mapped rkyv section: a fixed header followed immediately by
/// an rkyv archive whose root is at the END of the payload (this is how
/// rkyv lays out archives — see [`Section::get`]'s SAFETY comment).
pub(crate) enum Section<A> {
    /// Not present, or present but rejected (stale/corrupt/mismatched) —
    /// callers treat this identically to "not present" and rebuild.
    Absent,
    /// Validated and mapped. Steady state.
    Mapped {
        mmap: Mmap,
        payload: Range<usize>,
        _pd: PhantomData<fn() -> A>,
    },
}

impl<A> Section<A>
where
    A: rkyv::Portable
        + for<'a> rkyv::bytecheck::CheckBytes<rkyv::api::high::HighValidator<'a, rkyv::rancor::Error>>,
{
    /// Open and validate `path` as a section of `kind` matching `sig`.
    ///
    /// Performs ONLY the O(1) header checks (see the module-level layout
    /// table and [`parse_header`]) plus, if the `ESM_CACHE_VERIFY` env var
    /// is set to exactly `"1"`, a full checked [`rkyv::access`] pass. Never
    /// panics. Every rejection reason returns `Ok(Section::Absent)`, not an
    /// `Err` — a missing/corrupt cache is a routine "rebuild" condition, not
    /// a hard failure. Only genuine I/O errors (e.g. permission denied on
    /// `open`) are `Err`.
    pub(crate) fn map(
        path: &Path,
        kind: SectionKind,
        sig: CacheSig,
        cache_version: u32,
        layout_fingerprint: u64,
    ) -> anyhow::Result<Self> {
        // 1. Not present at all is the common case (first run, or a fresh
        // ESM with no cache built yet) — not an error.
        if !path.exists() {
            return Ok(Section::Absent);
        }

        let file = match fs::File::open(path) {
            Ok(f) => f,
            // A file that existed a moment ago but failed to open (removed
            // in a race, permissions changed, etc.) degrades to "rebuild"
            // like any other corrupt-cache case, rather than propagating. We
            // do NOT do this for `File::open` itself failing for a
            // structural reason (e.g. `path`'s parent directory being
            // unreadable) — such errors are rare and usually
            // indicate the caller passed a bad location, not "no cache yet",
            // so we still propagate those. In practice `fs::File::open`'s
            // error kinds are dominated by NotFound (already handled above)
            // and permission errors, both of which are reasonable to treat
            // as "can't use this cache, rebuild" rather than a hard failure.
            Err(_) => return Ok(Section::Absent),
        };

        // 2. `Mmap::map` failing (e.g. a concurrent truncate) is likewise a
        // transient condition — degrade to rebuild, don't crash the caller.
        //
        // SAFETY: this mmap is read-only for its entire lifetime here; the
        // only writer of files at this path is `write_section`, which never
        // mutates a file in place (temp file + atomic rename), so there is
        // no concurrent-mutation hazard for the duration of this mapping.
        let mmap = match unsafe { Mmap::map(&file) } {
            Ok(m) => m,
            Err(_) => return Ok(Section::Absent),
        };

        // 3. Header must physically fit.
        if mmap.len() < HEADER_SIZE {
            return Ok(Section::Absent);
        }

        // 4(a-f). Pure header checks — magic, format_version, section_kind,
        // cache_version, layout_fingerprint, and the ESM identity stamp.
        let Some(header) = parse_header(
            &mmap[..HEADER_SIZE],
            kind,
            sig,
            cache_version,
            layout_fingerprint,
        ) else {
            return Ok(Section::Absent);
        };

        // 4(g). payload must be big enough to hold the archived root type.
        // `parse_header` can't perform this check itself: it is a pure
        // function of just the 64-byte header slice and has no generic type
        // parameter, so it has no way to know `size_of::<A>()`. This is the
        // single most important check in the whole file — rkyv's root
        // position is computed as `payload_len.saturating_sub(size_of::<A>())`,
        // so a payload shorter than the archived root type would saturate to
        // position 0 and let `access_unchecked` read a root that was never
        // written, out of bounds of the actual value.
        let min_len = core::mem::size_of::<A>() as u64;
        if header.payload_len < min_len {
            return Ok(Section::Absent);
        }

        // 4(h). `payload_len` comes from untrusted on-disk bytes and could be
        // anywhere up to `u64::MAX`; use checked arithmetic so a hostile or
        // corrupt file can't wrap this addition into passing a naive `<`/`==`
        // guard the way it would under unchecked arithmetic in a release
        // build. The file must be EXACTLY header + payload_len bytes — not
        // merely "at least" — since that exact-match invariant is what makes
        // treating `HEADER_SIZE..mmap.len()` as the payload slice sound.
        let Some(expected_total) = (HEADER_SIZE as u64).checked_add(header.payload_len) else {
            return Ok(Section::Absent);
        };
        if expected_total != mmap.len() as u64 {
            return Ok(Section::Absent);
        }

        // 5. Alignment: defensive re-check, not expected to ever actually
        // fail. `Mmap` bases are always page-aligned (so at least 4096-byte
        // aligned on every platform this crate targets) and `HEADER_SIZE` is
        // compile-time-asserted to be a multiple of 16 above, so the payload
        // start is 16-aligned in practice by construction. Untrusted bytes
        // drove every check above though, so this is checked rather than
        // assumed — a failure here degrades to `Absent` like everything
        // else, rather than panicking or producing a misaligned access deep
        // inside rkyv.
        if !(mmap.as_ptr() as usize + HEADER_SIZE).is_multiple_of(16) {
            return Ok(Section::Absent);
        }

        let payload = HEADER_SIZE..mmap.len();

        // 6. Optional full bytecheck pass, gated behind an env var so CI can
        // prove the writer's output is sound (see the tests below, which
        // already exercise this path directly) without paying the cost on
        // the hot load-time path. Exact-match "1" only — deliberately not
        // parsing "true"/"yes"/etc., per the spec for this module.
        if std::env::var("ESM_CACHE_VERIFY").as_deref() == Ok("1")
            && let Err(e) = rkyv::access::<A, rkyv::rancor::Error>(&mmap[payload.clone()])
        {
            log::warn!(
                "rkyvcache: {} failed full bytecheck validation under \
                 ESM_CACHE_VERIFY=1 ({e}); treating as absent and \
                 rebuilding. This should never happen — it means the \
                 writer produced a section that fails its own format's \
                 validation.",
                path.display(),
            );
            return Ok(Section::Absent);
        }

        // 7. All checks passed.
        Ok(Section::Mapped {
            mmap,
            payload,
            _pd: PhantomData,
        })
    }

    pub(crate) fn is_mapped(&self) -> bool {
        matches!(self, Section::Mapped { .. })
    }

    /// Borrow the archived root. `None` iff `self` is `Absent`.
    pub(crate) fn get(&self) -> Option<&A> {
        match self {
            Section::Absent => None,
            Section::Mapped { mmap, payload, .. } => {
                // SAFETY: this is the only call to `access_unchecked` in the
                // crate — keep it that way. Its preconditions were
                // established by `Section::map`, above, and hold together as
                // follows:
                //
                // 1. `magic`/`format_version`/`section_kind` were checked
                //    (parse_header 4a-4c), so these bytes are one of *our*
                //    files, written by *this* module's `write_section`, and
                //    not some unrelated file that happens to occupy `path`.
                // 2. `cache_version` and `layout_fingerprint` matched
                //    (parse_header 4d-4e), so the bytes were produced by a
                //    build whose `A` layout is bit-for-bit identical to this
                //    build's `A` layout — the whole reason those two fields
                //    exist is to make this equality checkable in O(1)
                //    without deserializing anything.
                // 3. `mmap.len()` equals `HEADER_SIZE + payload_len` EXACTLY
                //    (map step 4h) and `payload_len >= size_of::<A>()` (map
                //    step 4g), so `rkyv`'s `root_position` computation
                //    (`payload_len.saturating_sub(size_of::<A>())`) lands
                //    inside the payload rather than saturating to a wrong
                //    position — this is the check that keeps the access
                //    below in-bounds.
                // 4. The payload start (`mmap.as_ptr() + HEADER_SIZE`) is
                //    16-aligned (map step 5), matching what rkyv's
                //    high-level serializer assumed when it laid the archive
                //    out.
                // 5. The ESM identity stamp (`src_size`/`src_mtime_secs`/
                //    `src_mtime_nanos`) matched `sig` (parse_header 4f), so
                //    this section was built from the exact ESM the caller is
                //    asking about, not a stale one left over from a prior
                //    game update.
                //
                // What this does NOT prove: that the bytes are not a
                // hand-crafted byte-valid-looking file, or that no single bit
                // got flipped after these header checks ran (e.g. by disk
                // corruption between `map`'s checks and this `get` call —
                // though in practice both happen within the same `mmap`, so
                // there is no window for that specific case). This is
                // accepted for the same reason `deny.toml` accepts
                // RUSTSEC-2025-0141 for bincode in `index.rs`: this cache is
                // written and read by the same binary on the same machine,
                // and never crosses a trust boundary, carries network input,
                // or accepts user-supplied bytes — quoting that rationale,
                // "written and read by the same binary on the same machine,
                // never crosses a trust boundary". A hostile file placed at
                // `path` by something other than this module is exactly the
                // kind of adversarial input the checks above (particularly
                // 4g/4h's checked arithmetic) are hardened against rejecting
                // safely; what's out of scope is a bit-perfect adversarial
                // forgery, which is not this module's threat model.
                Some(unsafe { rkyv::access_unchecked::<A>(&mmap[payload.clone()]) })
            }
        }
    }
}

/// Parsed header, once the type/version/identity checks (a-f below) have
/// passed. [`Section::map`] still has to check `payload_len` against the
/// caller's `size_of::<A>()` (g) and the mmap's total length (h) itself,
/// since [`parse_header`] is generic-free and never sees more than the fixed
/// 64-byte header slice — see the comments at those call sites in
/// `Section::map` for why those two checks can't live here.
struct Header {
    payload_len: u64,
}

/// Parses and validates the 64-byte header against expectations. Pure — no
/// I/O, no mmap, so it is unit- and Miri-testable directly on a `&[u8]`.
/// Returns `None` on ANY mismatch (never panics on malformed input,
/// including a `bytes` slice shorter than `HEADER_SIZE`).
///
/// Checks, in order, short-circuiting on the first failure:
/// (a) magic, (b) format_version, (c) section_kind, (d) cache_version,
/// (e) layout_fingerprint, (f) the ESM identity stamp (`src_size` /
/// `src_mtime_secs` / `src_mtime_nanos` vs `sig`).
fn parse_header(
    bytes: &[u8],
    kind: SectionKind,
    sig: CacheSig,
    cache_version: u32,
    layout_fingerprint: u64,
) -> Option<Header> {
    if bytes.len() < HEADER_SIZE {
        return None;
    }

    // (a) magic
    if &bytes[0..8] != MAGIC {
        return None;
    }

    // (b) format_version
    let format_version = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    if format_version != FORMAT_VERSION {
        return None;
    }

    // (c) section_kind
    let section_kind = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
    if section_kind != kind as u32 {
        return None;
    }

    let payload_len = u64::from_le_bytes(bytes[16..24].try_into().unwrap());
    let src_size = u64::from_le_bytes(bytes[24..32].try_into().unwrap());
    let src_mtime_secs = u64::from_le_bytes(bytes[32..40].try_into().unwrap());
    let src_mtime_nanos = u32::from_le_bytes(bytes[40..44].try_into().unwrap());
    let file_cache_version = u32::from_le_bytes(bytes[44..48].try_into().unwrap());
    let file_layout_fingerprint = u64::from_le_bytes(bytes[48..56].try_into().unwrap());
    // [56..64) reserved — ignored.

    // (d) cache_version
    if file_cache_version != cache_version {
        return None;
    }

    // (e) layout_fingerprint
    if file_layout_fingerprint != layout_fingerprint {
        return None;
    }

    // (f) ESM identity stamp
    if src_size != sig.size
        || src_mtime_secs != sig.mtime_secs
        || src_mtime_nanos != sig.mtime_nanos
    {
        return None;
    }

    Some(Header { payload_len })
}

/// Serialize the 64-byte header for the given fields, little-endian, per the
/// module-level layout table. Shared by [`write_section`] (real header,
/// written last) and the test suite (building adversarial header buffers).
fn build_header(
    kind: SectionKind,
    sig: CacheSig,
    cache_version: u32,
    layout_fingerprint: u64,
    payload_len: u64,
) -> [u8; HEADER_SIZE] {
    let mut buf = [0u8; HEADER_SIZE];
    buf[0..8].copy_from_slice(MAGIC);
    buf[8..12].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    buf[12..16].copy_from_slice(&(kind as u32).to_le_bytes());
    buf[16..24].copy_from_slice(&payload_len.to_le_bytes());
    buf[24..32].copy_from_slice(&sig.size.to_le_bytes());
    buf[32..40].copy_from_slice(&sig.mtime_secs.to_le_bytes());
    buf[40..44].copy_from_slice(&sig.mtime_nanos.to_le_bytes());
    buf[44..48].copy_from_slice(&cache_version.to_le_bytes());
    buf[48..56].copy_from_slice(&layout_fingerprint.to_le_bytes());
    // [56..64) reserved, left zero.
    buf
}

/// FNV-1a 64-bit offset basis — the conventional starting accumulator for
/// [`fnv1a_u64`].
pub(crate) const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;

/// One step of 64-bit FNV-1a, folding `x` in 8 bytes at a time
/// (little-endian). `acc` should start at [`FNV_OFFSET_BASIS`] for the first
/// call in a chain.
///
/// # Building a real `layout_fingerprint` (Stage 4+)
///
/// `layout_fingerprint` exists so a stale/incompatible on-disk section is
/// rejected by the O(1) header check alone (`parse_header` step e), without
/// ever reaching `access_unchecked` on bytes laid out by a different build.
/// This module has no real cache type yet — it's the reusable building block
/// a later stage plugs real archived types into. The intended pattern, once
/// a section's real `Archive`-derived types exist:
///
/// ```ignore
/// // One `fnv1a_u64` fold per archived type reachable from the section's
/// // root, folding in both `size_of` and `align_of` (a layout change can
/// // alter either independently, e.g. adding a trailing padding field).
/// const LAYOUT_FINGERPRINT: u64 = {
///     let acc = fnv1a_u64(FNV_OFFSET_BASIS, size_of::<Archived<Foo>>() as u64);
///     let acc = fnv1a_u64(acc, align_of::<Archived<Foo>>() as u64);
///     let acc = fnv1a_u64(acc, size_of::<Archived<Bar>>() as u64);
///     fnv1a_u64(acc, align_of::<Archived<Bar>>() as u64)
/// };
/// ```
///
/// then pass `LAYOUT_FINGERPRINT` as the `layout_fingerprint` argument to
/// both `write_section` and `Section::map` for that section kind. See
/// `tests::TEST_LAYOUT_FINGERPRINT` below, which follows exactly this
/// pattern against the test-only `Dummy` type, proving the mechanism works
/// end to end even though no real cache type exists yet.
pub(crate) const fn fnv1a_u64(acc: u64, x: u64) -> u64 {
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let bytes = x.to_le_bytes();
    let mut acc = acc;
    let mut i = 0;
    while i < bytes.len() {
        acc ^= bytes[i] as u64;
        acc = acc.wrapping_mul(FNV_PRIME);
        i += 1;
    }
    acc
}

/// Build a unique temp path next to `base`, e.g.
/// `esm_cache/SeventySix.esm.forms.tmp.<16 hex>`. Used by [`write_section`]
/// so two in-flight writers (or two successive calls) never collide, and so
/// a crash mid-write leaves debris under a random name nothing ever opens —
/// see [`write_section`]'s doc comment for the full write/publish sequence
/// this exists to support.
fn unique_tmp_path(base: &Path) -> anyhow::Result<PathBuf> {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes)?;
    let hex: String = bytes.iter().map(|b| format!("{:02x}", b)).collect();
    let parent = base
        .parent()
        .ok_or_else(|| anyhow::anyhow!("base path has no parent: {}", base.display()))?;
    let mut name = base
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("base path has no file name: {}", base.display()))?
        .to_os_string();
    name.push(".tmp.");
    name.push(hex);
    Ok(parent.join(name))
}

/// Write `value` as a new section file at `path` (via a unique temp file,
/// fsync, then atomic rename). `cache_version` is the CALLER's semantic
/// layout version (e.g. this crate's existing `index::CACHE_VERSION`
/// constant) — stored in the header and checked on load alongside
/// `layout_fingerprint`.
///
/// `path`'s parent directory (the shared `esm_cache/` — see
/// [`section_path_for`]) is created if it doesn't exist yet, since a lazy
/// section (`ensure_edid_index` etc.) can write long after the eager
/// tree/forms pair already created it, or — for a brand-new ESM — before
/// anything has.
///
/// # Write sequence — header written LAST
///
/// 1. Unique temp path next to `path` (getrandom-suffixed, via
///    [`unique_tmp_path`] — never a fixed `.tmp` name, so two in-flight
///    writers, or two successive calls, never collide).
/// 2. Create the temp file and write a 64-byte ALL-ZERO placeholder header.
/// 3. Serialize `value` with rkyv directly into the file through a buffered
///    `IoWriter` (rkyv's streaming writer-based API), so the archive bytes
///    go straight to disk instead of being fully materialized a second time
///    in an in-memory buffer on top of whatever scratch space rkyv's
///    serializer already needs. Record the resulting payload byte length
///    from the writer's own position counter.
/// 4. Flush and `sync_data()` the payload.
/// 5. Seek back to offset 0 and write the REAL 64-byte header, now that
///    `payload_len` is known.
/// 6. `sync_all()`.
/// 7. `fs::rename(tmp, path)` — atomic publish.
/// 8. On ANY error before step 7, best-effort `fs::remove_file(tmp)` and
///    propagate the original error.
///
/// The header is written last, after the payload is already fully flushed
/// and synced, so that a crash or kill at any point before step 5 leaves the
/// temp file's header all-zero. `parse_header` rejects an all-zero header on
/// the very first check (magic mismatch — real magic is `b"ESMRKYV1"`, never
/// all-zero). Combined with the temp-file + atomic-rename publish, this
/// means there is no way for a file to ever exist at the final `path` with a
/// valid-looking header but a truncated or torn payload: either the rename
/// never happened (crash left only the doomed temp file behind, which
/// nothing ever opens by its random name), or it did, in which case the
/// header was written after the fully-synced payload and both are complete.
pub(crate) fn write_section<T>(
    path: &Path,
    kind: SectionKind,
    sig: CacheSig,
    cache_version: u32,
    layout_fingerprint: u64,
    value: &T,
) -> anyhow::Result<()>
where
    T: for<'a> rkyv::Serialize<
            rkyv::api::high::HighSerializer<
                rkyv::ser::writer::IoWriter<BufWriter<fs::File>>,
                rkyv::ser::allocator::ArenaHandle<'a>,
                rkyv::rancor::Error,
            >,
        >,
{
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create cache directory {}", parent.display()))?;
    }
    let tmp_path = unique_tmp_path(path)?;

    let write_result: anyhow::Result<()> = (|| {
        // 2. Placeholder header.
        let file = fs::File::create(&tmp_path)
            .with_context(|| format!("create {}", tmp_path.display()))?;
        let mut writer = BufWriter::new(file);
        writer
            .write_all(&[0u8; HEADER_SIZE])
            .with_context(|| format!("write placeholder header to {}", tmp_path.display()))?;

        // 3. Stream the payload straight into the file via rkyv's IoWriter
        // adapter. `TODO`: `FormsSection` alone is ~200 MiB (5.64M records);
        // `ArenaHandle`'s scratch allocation for a value this size hasn't
        // been profiled. If it turns out to matter, revisit a
        // bounded/streaming allocator here too — this streams the *output*
        // straight to disk already, but `ArenaHandle` is rkyv's *internal*
        // scratch space during serialization, a separate cost.
        let io_writer = rkyv::ser::writer::IoWriter::new(writer);
        let io_writer = rkyv::api::high::to_bytes_in::<_, rkyv::rancor::Error>(value, io_writer)
            .map_err(|e| anyhow::anyhow!("rkyv serialize into {}: {e}", tmp_path.display()))?;
        let payload_len = io_writer.pos() as u64;
        let mut writer = io_writer.into_inner();

        // 4. Flush + sync the payload before the header is touched.
        writer
            .flush()
            .with_context(|| format!("flush {}", tmp_path.display()))?;
        writer
            .get_ref()
            .sync_data()
            .with_context(|| format!("sync payload {}", tmp_path.display()))?;

        // 5. Seek back to 0 and write the real header now that payload_len
        // is known.
        let mut file = writer.into_inner().map_err(|e| {
            anyhow::anyhow!(
                "recover file handle for {}: {}",
                tmp_path.display(),
                e.into_error()
            )
        })?;
        file.seek(SeekFrom::Start(0))
            .with_context(|| format!("seek to header in {}", tmp_path.display()))?;
        let header = build_header(kind, sig, cache_version, layout_fingerprint, payload_len);
        file.write_all(&header)
            .with_context(|| format!("write header to {}", tmp_path.display()))?;

        // 6. Final sync covering the header write.
        file.sync_all()
            .with_context(|| format!("sync {}", tmp_path.display()))?;

        Ok(())
    })();

    match write_result {
        // 7. Atomic publish.
        Ok(()) => fs::rename(&tmp_path, path)
            .with_context(|| format!("rename {} to {}", tmp_path.display(), path.display())),
        // 8. Best-effort cleanup, propagate the original error.
        Err(e) => {
            let _ = fs::remove_file(&tmp_path);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
    struct Dummy {
        a: u32,
        b: String,
        c: Vec<u32>,
    }

    const TEST_KIND: SectionKind = SectionKind::Tree;
    const TEST_CACHE_VERSION: u32 = 42;

    /// Proves the `fnv1a_u64` composition pattern documented on that
    /// function against a real (test-only) `Archive`-derived type, since no
    /// production cache type exists yet for it to be wired to for real.
    const TEST_LAYOUT_FINGERPRINT: u64 = {
        let acc = fnv1a_u64(
            FNV_OFFSET_BASIS,
            core::mem::size_of::<rkyv::Archived<Dummy>>() as u64,
        );
        fnv1a_u64(acc, core::mem::align_of::<rkyv::Archived<Dummy>>() as u64)
    };

    fn test_sig() -> CacheSig {
        CacheSig {
            size: 123_456,
            mtime_secs: 1_700_000_000,
            mtime_nanos: 123_456_789,
        }
    }

    fn dummy_value() -> Dummy {
        Dummy {
            a: 7,
            b: "hello rkyvcache".to_string(),
            c: vec![1, 2, 3, 4, 5],
        }
    }

    fn min_payload_len() -> u64 {
        core::mem::size_of::<rkyv::Archived<Dummy>>() as u64
    }

    /// Distinct, non-colliding temp path per test — each test uses its own
    /// `name`, and cleans its own file up at the end, so sequential or
    /// parallel `cargo test` runs of different tests never interfere.
    fn test_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("esm_rkyvcache_test_{name}.section"))
    }

    fn map_dummy(path: &Path, kind: SectionKind, sig: CacheSig) -> Section<rkyv::Archived<Dummy>> {
        Section::map(path, kind, sig, TEST_CACHE_VERSION, TEST_LAYOUT_FINGERPRINT)
            .expect("Section::map should never return Err for a readable file")
    }

    // ── 1. Round-trip, checked access ───────────────────────────────────────

    #[test]
    fn round_trip_checked_access() {
        let path = test_path("round_trip_checked");
        let sig = test_sig();
        write_section(
            &path,
            TEST_KIND,
            sig,
            TEST_CACHE_VERSION,
            TEST_LAYOUT_FINGERPRINT,
            &dummy_value(),
        )
        .unwrap();

        let bytes = fs::read(&path).unwrap();
        let payload = &bytes[HEADER_SIZE..];
        // Checked access — this is the test that actually justifies using
        // `access_unchecked` in `Section::get`: it proves the writer emits
        // archives that pass full bytecheck validation.
        let archived = rkyv::access::<rkyv::Archived<Dummy>, rkyv::rancor::Error>(payload).unwrap();
        assert_eq!(archived.a, 7);
        assert_eq!(archived.b.as_str(), "hello rkyvcache");
        assert_eq!(archived.c.len(), 5);
        assert_eq!(
            archived
                .c
                .iter()
                .map(|x| x.to_native())
                .collect::<Vec<u32>>(),
            vec![1, 2, 3, 4, 5]
        );

        let _ = fs::remove_file(&path);
    }

    // ── 2. Round-trip, unchecked access via Section::get ────────────────────

    #[test]
    fn round_trip_unchecked_via_section() {
        let path = test_path("round_trip_unchecked");
        let sig = test_sig();
        write_section(
            &path,
            TEST_KIND,
            sig,
            TEST_CACHE_VERSION,
            TEST_LAYOUT_FINGERPRINT,
            &dummy_value(),
        )
        .unwrap();

        let section = map_dummy(&path, TEST_KIND, sig);
        assert!(section.is_mapped());
        let archived = section.get().expect("mapped section must yield Some");
        assert_eq!(archived.a, 7);
        assert_eq!(archived.b.as_str(), "hello rkyvcache");
        assert_eq!(
            archived
                .c
                .iter()
                .map(|x| x.to_native())
                .collect::<Vec<u32>>(),
            vec![1, 2, 3, 4, 5]
        );

        let _ = fs::remove_file(&path);
    }

    // ── 3. Adversarial matrix (mmap-based, through Section::map) ────────────

    #[test]
    fn adversarial_bad_magic() {
        let path = test_path("adv_bad_magic");
        let len = min_payload_len();
        let mut header = build_header(
            TEST_KIND,
            test_sig(),
            TEST_CACHE_VERSION,
            TEST_LAYOUT_FINGERPRINT,
            len,
        );
        header[0] = b'X';
        let mut bytes = header.to_vec();
        bytes.extend(vec![0u8; len as usize]);
        fs::write(&path, &bytes).unwrap();

        assert!(!map_dummy(&path, TEST_KIND, test_sig()).is_mapped());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn adversarial_wrong_format_version() {
        let path = test_path("adv_wrong_format_version");
        let len = min_payload_len();
        let mut header = build_header(
            TEST_KIND,
            test_sig(),
            TEST_CACHE_VERSION,
            TEST_LAYOUT_FINGERPRINT,
            len,
        );
        header[8..12].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
        let mut bytes = header.to_vec();
        bytes.extend(vec![0u8; len as usize]);
        fs::write(&path, &bytes).unwrap();

        assert!(!map_dummy(&path, TEST_KIND, test_sig()).is_mapped());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn adversarial_wrong_section_kind() {
        let path = test_path("adv_wrong_section_kind");
        let len = min_payload_len();
        // Header genuinely built for `Forms`, but we ask `Section::map` for
        // `Tree` — exercises the mismatch from the caller's side.
        let header = build_header(
            SectionKind::Forms,
            test_sig(),
            TEST_CACHE_VERSION,
            TEST_LAYOUT_FINGERPRINT,
            len,
        );
        let mut bytes = header.to_vec();
        bytes.extend(vec![0u8; len as usize]);
        fs::write(&path, &bytes).unwrap();

        assert!(!map_dummy(&path, SectionKind::Tree, test_sig()).is_mapped());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn adversarial_wrong_cache_version() {
        let path = test_path("adv_wrong_cache_version");
        let len = min_payload_len();
        let header = build_header(
            TEST_KIND,
            test_sig(),
            TEST_CACHE_VERSION + 1,
            TEST_LAYOUT_FINGERPRINT,
            len,
        );
        let mut bytes = header.to_vec();
        bytes.extend(vec![0u8; len as usize]);
        fs::write(&path, &bytes).unwrap();

        assert!(!map_dummy(&path, TEST_KIND, test_sig()).is_mapped());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn adversarial_wrong_layout_fingerprint() {
        let path = test_path("adv_wrong_layout_fingerprint");
        let len = min_payload_len();
        let header = build_header(
            TEST_KIND,
            test_sig(),
            TEST_CACHE_VERSION,
            TEST_LAYOUT_FINGERPRINT ^ 1,
            len,
        );
        let mut bytes = header.to_vec();
        bytes.extend(vec![0u8; len as usize]);
        fs::write(&path, &bytes).unwrap();

        assert!(!map_dummy(&path, TEST_KIND, test_sig()).is_mapped());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn adversarial_payload_len_u64_max_does_not_overflow() {
        let path = test_path("adv_payload_len_u64_max");
        // File is header-only on disk (we obviously can't write u64::MAX
        // payload bytes); the point is that `HEADER_SIZE + payload_len` must
        // not overflow/wrap when computing the expected total length.
        let header = build_header(
            TEST_KIND,
            test_sig(),
            TEST_CACHE_VERSION,
            TEST_LAYOUT_FINGERPRINT,
            u64::MAX,
        );
        fs::write(&path, header).unwrap();

        assert!(!map_dummy(&path, TEST_KIND, test_sig()).is_mapped());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn adversarial_payload_len_smaller_than_archived_root() {
        let path = test_path("adv_payload_len_too_small");
        let too_small = min_payload_len() - 1;
        let header = build_header(
            TEST_KIND,
            test_sig(),
            TEST_CACHE_VERSION,
            TEST_LAYOUT_FINGERPRINT,
            too_small,
        );
        let mut bytes = header.to_vec();
        bytes.extend(vec![0u8; too_small as usize]);
        fs::write(&path, &bytes).unwrap();

        assert!(!map_dummy(&path, TEST_KIND, test_sig()).is_mapped());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn adversarial_file_one_byte_short() {
        let path = test_path("adv_one_byte_short");
        let len = min_payload_len();
        let header = build_header(
            TEST_KIND,
            test_sig(),
            TEST_CACHE_VERSION,
            TEST_LAYOUT_FINGERPRINT,
            len,
        );
        let mut bytes = header.to_vec();
        bytes.extend(vec![0u8; len as usize]);
        bytes.pop(); // one byte short of HEADER_SIZE + payload_len
        fs::write(&path, &bytes).unwrap();

        assert!(!map_dummy(&path, TEST_KIND, test_sig()).is_mapped());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn adversarial_file_one_byte_long() {
        let path = test_path("adv_one_byte_long");
        let len = min_payload_len();
        let header = build_header(
            TEST_KIND,
            test_sig(),
            TEST_CACHE_VERSION,
            TEST_LAYOUT_FINGERPRINT,
            len,
        );
        let mut bytes = header.to_vec();
        bytes.extend(vec![0u8; len as usize]);
        bytes.push(0xAA); // one byte longer than HEADER_SIZE + payload_len
        fs::write(&path, &bytes).unwrap();

        assert!(!map_dummy(&path, TEST_KIND, test_sig()).is_mapped());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn adversarial_src_size_mismatch() {
        let path = test_path("adv_src_size_mismatch");
        let len = min_payload_len();
        let sig = test_sig();
        let header = build_header(
            TEST_KIND,
            sig,
            TEST_CACHE_VERSION,
            TEST_LAYOUT_FINGERPRINT,
            len,
        );
        let mut bytes = header.to_vec();
        bytes.extend(vec![0u8; len as usize]);
        fs::write(&path, &bytes).unwrap();

        let mut wrong_sig = sig;
        wrong_sig.size += 1;
        assert!(!map_dummy(&path, TEST_KIND, wrong_sig).is_mapped());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn adversarial_src_mtime_secs_mismatch() {
        let path = test_path("adv_src_mtime_secs_mismatch");
        let len = min_payload_len();
        let sig = test_sig();
        let header = build_header(
            TEST_KIND,
            sig,
            TEST_CACHE_VERSION,
            TEST_LAYOUT_FINGERPRINT,
            len,
        );
        let mut bytes = header.to_vec();
        bytes.extend(vec![0u8; len as usize]);
        fs::write(&path, &bytes).unwrap();

        let mut wrong_sig = sig;
        wrong_sig.mtime_secs += 1;
        assert!(!map_dummy(&path, TEST_KIND, wrong_sig).is_mapped());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn adversarial_src_mtime_nanos_mismatch() {
        let path = test_path("adv_src_mtime_nanos_mismatch");
        let len = min_payload_len();
        let sig = test_sig();
        let header = build_header(
            TEST_KIND,
            sig,
            TEST_CACHE_VERSION,
            TEST_LAYOUT_FINGERPRINT,
            len,
        );
        let mut bytes = header.to_vec();
        bytes.extend(vec![0u8; len as usize]);
        fs::write(&path, &bytes).unwrap();

        let mut wrong_sig = sig;
        wrong_sig.mtime_nanos += 1;
        assert!(!map_dummy(&path, TEST_KIND, wrong_sig).is_mapped());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn adversarial_zero_length_file() {
        let path = test_path("adv_zero_length");
        fs::write(&path, []).unwrap();

        assert!(!map_dummy(&path, TEST_KIND, test_sig()).is_mapped());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn adversarial_header_only_no_payload() {
        let path = test_path("adv_header_only");
        // A structurally valid-looking header (right magic/version/kind/
        // versions/sig) but claiming zero payload bytes — must still be
        // rejected since 0 < size_of::<Archived<Dummy>>().
        let header = build_header(
            TEST_KIND,
            test_sig(),
            TEST_CACHE_VERSION,
            TEST_LAYOUT_FINGERPRINT,
            0,
        );
        fs::write(&path, header).unwrap();

        assert!(!map_dummy(&path, TEST_KIND, test_sig()).is_mapped());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn adversarial_nonexistent_path_is_absent_not_err() {
        let path = test_path("adv_does_not_exist_at_all");
        let _ = fs::remove_file(&path); // ensure it really doesn't exist
        let section = Section::<rkyv::Archived<Dummy>>::map(
            &path,
            TEST_KIND,
            test_sig(),
            TEST_CACHE_VERSION,
            TEST_LAYOUT_FINGERPRINT,
        );
        assert!(section.is_ok(), "missing cache file must not be an Err");
        assert!(!section.unwrap().is_mapped());
    }

    // ── 4. `parse_header` unit tests (pure, no Mmap) ─────────────────────────

    fn ph(bytes: &[u8], kind: SectionKind, sig: CacheSig) -> Option<Header> {
        parse_header(
            bytes,
            kind,
            sig,
            TEST_CACHE_VERSION,
            TEST_LAYOUT_FINGERPRINT,
        )
    }

    #[test]
    fn parse_header_accepts_valid() {
        let len = min_payload_len();
        let header = build_header(
            TEST_KIND,
            test_sig(),
            TEST_CACHE_VERSION,
            TEST_LAYOUT_FINGERPRINT,
            len,
        );
        let parsed = ph(&header, TEST_KIND, test_sig()).expect("valid header must parse");
        assert_eq!(parsed.payload_len, len);
    }

    #[test]
    fn parse_header_rejects_bad_magic() {
        let mut header = build_header(
            TEST_KIND,
            test_sig(),
            TEST_CACHE_VERSION,
            TEST_LAYOUT_FINGERPRINT,
            0,
        );
        header[0] = b'X';
        assert!(ph(&header, TEST_KIND, test_sig()).is_none());
    }

    #[test]
    fn parse_header_rejects_wrong_format_version() {
        let mut header = build_header(
            TEST_KIND,
            test_sig(),
            TEST_CACHE_VERSION,
            TEST_LAYOUT_FINGERPRINT,
            0,
        );
        header[8..12].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
        assert!(ph(&header, TEST_KIND, test_sig()).is_none());
    }

    #[test]
    fn parse_header_rejects_wrong_section_kind() {
        let header = build_header(
            SectionKind::Xref,
            test_sig(),
            TEST_CACHE_VERSION,
            TEST_LAYOUT_FINGERPRINT,
            0,
        );
        assert!(ph(&header, TEST_KIND, test_sig()).is_none());
    }

    #[test]
    fn parse_header_rejects_wrong_cache_version() {
        let header = build_header(
            TEST_KIND,
            test_sig(),
            TEST_CACHE_VERSION,
            TEST_LAYOUT_FINGERPRINT,
            0,
        );
        assert!(
            parse_header(
                &header,
                TEST_KIND,
                test_sig(),
                TEST_CACHE_VERSION + 1,
                TEST_LAYOUT_FINGERPRINT
            )
            .is_none()
        );
    }

    #[test]
    fn parse_header_rejects_wrong_layout_fingerprint() {
        let header = build_header(
            TEST_KIND,
            test_sig(),
            TEST_CACHE_VERSION,
            TEST_LAYOUT_FINGERPRINT,
            0,
        );
        assert!(
            parse_header(
                &header,
                TEST_KIND,
                test_sig(),
                TEST_CACHE_VERSION,
                TEST_LAYOUT_FINGERPRINT ^ 1
            )
            .is_none()
        );
    }

    #[test]
    fn parse_header_rejects_src_size_mismatch() {
        let sig = test_sig();
        let header = build_header(
            TEST_KIND,
            sig,
            TEST_CACHE_VERSION,
            TEST_LAYOUT_FINGERPRINT,
            0,
        );
        let mut wrong = sig;
        wrong.size += 1;
        assert!(ph(&header, TEST_KIND, wrong).is_none());
    }

    #[test]
    fn parse_header_rejects_src_mtime_secs_mismatch() {
        let sig = test_sig();
        let header = build_header(
            TEST_KIND,
            sig,
            TEST_CACHE_VERSION,
            TEST_LAYOUT_FINGERPRINT,
            0,
        );
        let mut wrong = sig;
        wrong.mtime_secs += 1;
        assert!(ph(&header, TEST_KIND, wrong).is_none());
    }

    #[test]
    fn parse_header_rejects_src_mtime_nanos_mismatch() {
        let sig = test_sig();
        let header = build_header(
            TEST_KIND,
            sig,
            TEST_CACHE_VERSION,
            TEST_LAYOUT_FINGERPRINT,
            0,
        );
        let mut wrong = sig;
        wrong.mtime_nanos += 1;
        assert!(ph(&header, TEST_KIND, wrong).is_none());
    }

    #[test]
    fn parse_header_rejects_short_buffer() {
        let header = build_header(
            TEST_KIND,
            test_sig(),
            TEST_CACHE_VERSION,
            TEST_LAYOUT_FINGERPRINT,
            0,
        );
        assert!(ph(&header[..HEADER_SIZE - 1], TEST_KIND, test_sig()).is_none());
    }

    #[test]
    fn parse_header_rejects_empty_buffer() {
        assert!(ph(&[], TEST_KIND, test_sig()).is_none());
    }

    // ── 5. Temp-file naming: two successive writes to the same path ────────

    #[test]
    fn successive_writes_to_same_path_both_succeed() {
        let path = test_path("successive_writes");
        let sig = test_sig();

        write_section(
            &path,
            TEST_KIND,
            sig,
            TEST_CACHE_VERSION,
            TEST_LAYOUT_FINGERPRINT,
            &dummy_value(),
        )
        .expect("first write_section must succeed");
        let first = map_dummy(&path, TEST_KIND, sig);
        assert!(first.is_mapped());
        assert_eq!(first.get().unwrap().a, 7);
        drop(first);

        let second_value = Dummy {
            a: 99,
            b: "second".to_string(),
            c: vec![9],
        };
        write_section(
            &path,
            TEST_KIND,
            sig,
            TEST_CACHE_VERSION,
            TEST_LAYOUT_FINGERPRINT,
            &second_value,
        )
        .expect("second write_section to the same path must also succeed");
        let second = map_dummy(&path, TEST_KIND, sig);
        assert!(second.is_mapped());
        assert_eq!(second.get().unwrap().a, 99);
        assert_eq!(second.get().unwrap().b.as_str(), "second");

        let _ = fs::remove_file(&path);
    }

    // ── cache_dir_for / section_path_for / unique_tmp_path ──────────────────

    #[test]
    fn unique_tmp_path_differs_and_same_parent() -> anyhow::Result<()> {
        let base = PathBuf::from("/tmp/esm_cache/SeventySix.esm.forms");
        let p1 = unique_tmp_path(&base)?;
        let p2 = unique_tmp_path(&base)?;
        assert_ne!(p1, p2);
        assert_eq!(p1.parent(), base.parent());
        Ok(())
    }

    #[test]
    fn cache_dir_for_is_a_fixed_name_sibling_directory() {
        let dir = cache_dir_for(Path::new("/data/SeventySix.esm")).unwrap();
        assert_eq!(dir, PathBuf::from("/data/esm_cache"));
    }

    /// The whole point of the fixed directory name: two different plugins in
    /// the same directory share it (this is deliberate, not a bug —
    /// [`section_path_for`] is what keeps their files from colliding).
    #[test]
    fn cache_dir_for_is_shared_across_different_esms_in_one_directory() {
        let a = cache_dir_for(Path::new("/data/SeventySix.esm")).unwrap();
        let b = cache_dir_for(Path::new("/data/Foo.esp")).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn section_path_for_names_file_by_esm_name_and_kind() {
        let esm = Path::new("/data/SeventySix.esm");
        assert_eq!(
            section_path_for(esm, SectionKind::Tree).unwrap(),
            PathBuf::from("/data/esm_cache/SeventySix.esm.tree")
        );
        assert_eq!(
            section_path_for(esm, SectionKind::Xref).unwrap(),
            PathBuf::from("/data/esm_cache/SeventySix.esm.xref")
        );
    }

    /// Regression guard for the reason [`section_path_for`] prefixes with
    /// the ESM's full file name (extension included) rather than just its
    /// stem: two plugins with the same stem but different extensions must
    /// still land on different files inside the shared directory.
    #[test]
    fn section_path_for_does_not_collide_across_same_stem_different_extension() {
        let esm_file = section_path_for(Path::new("/data/Foo.esm"), SectionKind::Tree).unwrap();
        let esp_file = section_path_for(Path::new("/data/Foo.esp"), SectionKind::Tree).unwrap();
        assert_ne!(esm_file, esp_file);
        assert_eq!(
            esm_file.parent(),
            esp_file.parent(),
            "both still share the one esm_cache/ directory"
        );
    }

    /// `write_section` must create `esm_cache/` on demand — a lazy section
    /// (`ensure_edid_index` etc.) can be the first writer for a brand-new
    /// ESM, with no eager tree/forms write to have created it first. Since
    /// `esm_cache/` is SHARED across every ESM in the parent directory, this
    /// test cannot assert the directory is absent beforehand (another
    /// test/process may have already created it) — only that writing
    /// succeeds and the section reads back correctly either way, and cleans
    /// up only its own file, never the shared directory.
    #[test]
    fn write_section_creates_missing_cache_dir() {
        let pid = std::process::id();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let esm_path = std::env::temp_dir().join(format!(
            "esm_rkyvcache_test_write_creates_dir_{pid}_{nonce}.esm"
        ));

        let path = section_path_for(&esm_path, TEST_KIND).unwrap();
        let sig = test_sig();
        write_section(
            &path,
            TEST_KIND,
            sig,
            TEST_CACHE_VERSION,
            TEST_LAYOUT_FINGERPRINT,
            &dummy_value(),
        )
        .expect("write_section must create the cache dir if missing and succeed either way");

        let section = map_dummy(&path, TEST_KIND, sig);
        assert!(section.is_mapped(), "freshly written section must map back");
        assert_eq!(section.get().unwrap().a, 7);

        let _ = fs::remove_file(&path);
    }
}
