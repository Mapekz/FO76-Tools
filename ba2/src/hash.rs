//! Bethesda BA2 (BTDX) name/directory hashing.
//!
//! The hash is a standard CRC-32 (poly `0xEDB88320`, reflected) with two
//! non-standard choices: **init value 0** (instead of `0xFFFFFFFF`) and
//! **no final XOR** (instead of the usual invert).  This was verified to
//! produce a 100% match against all 4 507 entries in the two FO76 sample
//! archives (`SeventySix - Localization.ba2` and `SeventySix - Startup.ba2`).

const POLY: u32 = 0xEDB8_8320;

const fn build_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { (c >> 1) ^ POLY } else { c >> 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

static TABLE: [u32; 256] = build_table();

/// Bethesda BA2 hash: CRC-32 table with **init 0** and **no final inversion**.
pub fn beth_crc(bytes: &[u8]) -> u32 {
    let mut crc: u32 = 0;
    for &b in bytes {
        crc = TABLE[((crc ^ b as u32) & 0xff) as usize] ^ (crc >> 8);
    }
    crc
}

/// Derive `(name_hash, dir_hash, ext)` from an archive-internal path.
///
/// The path is lowercased and `/` is converted to `\` before hashing so the
/// result is consistent regardless of OS path conventions.
///
/// * `name_hash` — CRC of the file stem (base name without extension).
/// * `dir_hash`  — CRC of the directory portion; `0` for root-level files.
/// * `ext` — first 4 bytes of the lowercase extension, null-padded
///   (e.g. `"dlstrings"` → `*b"dlst"`).
pub fn hash_path(path: &str) -> (u32, u32, [u8; 4]) {
    let norm = path.to_lowercase().replace('/', "\\");

    // Split into (dir, filename).
    let (dir, file) = match norm.rsplit_once('\\') {
        Some((d, f)) => (d, f),
        None => ("", norm.as_str()),
    };

    // Split filename into (stem, ext).
    let (stem, ext_str) = match file.rsplit_once('.') {
        Some((s, e)) => (s, e),
        None => (file, ""),
    };

    // Extension: first 4 bytes of the lowercased extension, null-padded.
    let mut ext = [0u8; 4];
    let eb = ext_str.as_bytes();
    let n = eb.len().min(4);
    ext[..n].copy_from_slice(&eb[..n]);

    let name_hash = beth_crc(stem.as_bytes());
    let dir_hash = beth_crc(dir.as_bytes());

    (name_hash, dir_hash, ext)
}

