//! Integration tests for `ba2::compress` — codec dispatch, round-trips, sniffing.

use ba2::compress::{
    Codec, MAX_DECOMP_SIZE, ReadCodec, compress_entry, compress_lz4, compress_zlib, decompress,
    decompress_lz4, decompress_zlib, is_zlib,
};

fn sample() -> Vec<u8> {
    b"Hello, Fallout 76! Hello, Fallout 76! Hello, Fallout 76! Hello, Fallout 76! "
        .repeat(5)
        .to_vec()
}

// ── Decompression round-trips ─────────────────────────────────────────────

#[test]
fn lz4_round_trip() {
    let data = sample();
    let compressed = compress_lz4(&data);
    let decompressed = decompress_lz4(&compressed, data.len()).unwrap();
    assert_eq!(decompressed, data);
}

#[test]
fn zlib_round_trip() {
    let data = sample();
    let compressed = compress_zlib(&data).unwrap();
    let decompressed = decompress_zlib(&compressed, data.len()).unwrap();
    assert_eq!(decompressed, data);
}

// ── Decompression-bomb cap ────────────────────────────────────────────────

/// A crafted, oversized declared output size must be rejected before any large
/// allocation is attempted, for both codecs.  Exercised through the real
/// dispatch path a corrupt or malicious archive would hit — `decompress()` with
/// an attacker-controlled `unpacked_size` (the on-disk field) — which reaches
/// the same cap in `decompress_lz4` and `decompress_zlib`.
#[test]
fn decompress_rejects_oversized_unpacked_size_via_dispatch() {
    let oversized = (MAX_DECOMP_SIZE + 1) as u32;
    for codec in [ReadCodec::Lz4, ReadCodec::Zlib] {
        let result = decompress(b"", oversized, codec);
        assert!(
            result.is_err(),
            "{codec:?} dispatch must reject oversized unpacked_size"
        );
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("exceeds limit"),
            "{codec:?}: unexpected error message: {msg}"
        );
    }
}

// ── is_zlib / Auto sniffing ───────────────────────────────────────────────

#[test]
fn is_zlib_detects_correctly() {
    let data = sample();
    let zlib = compress_zlib(&data).unwrap();
    assert!(is_zlib(&zlib), "zlib header bytes must be detected");
    let lz4 = compress_lz4(&data);
    assert!(!is_zlib(&lz4), "LZ4 bytes must NOT be detected as zlib");
}

/// The zlib two-byte sniff: first byte == 0x78 and the combined u16 % 31 == 0.
/// Verify common valid headers: 0x789C (default), 0x7801 (no compression),
/// 0x78DA (best compression).
#[test]
fn is_zlib_boundary_valid_headers() {
    // These are the three most common zlib CMF+FLG pairs, all valid.
    assert!(
        is_zlib(&[0x78, 0x9C, 0x00]),
        "0x789C is a valid zlib header"
    );
    assert!(
        is_zlib(&[0x78, 0x01, 0x00]),
        "0x7801 is a valid zlib header"
    );
    assert!(
        is_zlib(&[0x78, 0xDA, 0x00]),
        "0x78DA is a valid zlib header"
    );
}

/// Smaller-window zlib streams have a different first byte (`0x48` is a
/// 4 KiB window, as in FO76's Materials archive) and still sniff as zlib.
#[test]
fn is_zlib_accepts_smaller_windows() {
    assert!(
        is_zlib(&[0x48, 0xC7]),
        "0x48C7 is a valid 4 KiB-window zlib header"
    );
    assert!(
        is_zlib(&[0x08, 0x1D]),
        "0x081D is a valid 256-byte-window zlib header"
    );
    assert!(
        !is_zlib(&[0x88, 0x98]),
        "CINFO 8 is not a valid zlib window"
    );
    assert!(
        !is_zlib(&[0x49, 0xC6]),
        "compression method 9 is not deflate"
    );
}

#[test]
fn auto_decompresses_small_window_zlib() {
    use flate2::Compression;
    use flate2::write::ZlibEncoder;
    use std::io::Write;
    let data = sample();
    // flate2 always writes a 32 KiB window header; rewrite it to the 4 KiB
    // header FO76 uses (the stream itself only needs a window ≤ 32 KiB, and
    // the sample is small enough that 4 KiB covers every back-reference).
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
    enc.write_all(&data).unwrap();
    let mut compressed = enc.finish().unwrap();
    assert!(data.len() <= 4096, "sample must fit a 4 KiB window");
    compressed[0] = 0x48;
    let flg = compressed[1] & 0xE0;
    compressed[1] = flg + (31 - ((0x48u16 << 8 | flg as u16) % 31) as u8) % 31;
    assert!(is_zlib(&compressed));
    let out = decompress(&compressed, data.len() as u32, ReadCodec::Auto).unwrap();
    assert_eq!(out, data);
}

/// Bytes that start with 0x78 but fail the % 31 check must NOT be detected.
#[test]
fn is_zlib_rejects_false_0x78_prefix() {
    // 0x78_00: combined = 0x7800 = 30720; 30720 % 31 = 30720 - 31*990 = 30720 - 30690 = 30 ≠ 0
    assert!(!is_zlib(&[0x78, 0x00]), "0x7800 fails the % 31 check");
}

/// Fewer than 2 bytes must not sniff as zlib.
#[test]
fn is_zlib_too_short() {
    assert!(!is_zlib(&[]), "empty slice is not zlib");
    assert!(!is_zlib(&[0x78]), "one-byte slice is not zlib");
}

#[test]
fn auto_decompresses_zlib() {
    let data = sample();
    let compressed = compress_zlib(&data).unwrap();
    let out = decompress(&compressed, data.len() as u32, ReadCodec::Auto).unwrap();
    assert_eq!(out, data);
}

#[test]
fn auto_decompresses_lz4() {
    let data = sample();
    let compressed = compress_lz4(&data);
    let out = decompress(&compressed, data.len() as u32, ReadCodec::Auto).unwrap();
    assert_eq!(out, data);
}

// ── compress_entry store-fallback ─────────────────────────────────────────

#[test]
fn compress_entry_falls_back_to_store_when_not_smaller() {
    // A single byte cannot compress to fewer bytes.
    let data = vec![0xFFu8; 1];
    let (blob, packed_size) = compress_entry(&data, Codec::Lz4, 1.0).unwrap();
    assert_eq!(packed_size, 0, "packed_size==0 signals 'stored'");
    assert_eq!(blob, data, "stored blob must equal input");
}

#[test]
fn compress_entry_lz4_compresses_repeated_data() {
    let data = sample();
    let (blob, packed_size) = compress_entry(&data, Codec::Lz4, 1.0).unwrap();
    assert!(packed_size > 0, "repeated data should compress");
    assert!(blob.len() < data.len(), "compressed blob must be shorter");
    // Verify round-trip.
    let out = decompress_lz4(&blob, data.len()).unwrap();
    assert_eq!(out, data);
}

/// `Codec::Store` always returns packed_size==0 (stored).
#[test]
fn compress_entry_store_is_uncompressed() {
    let data = sample();
    let (blob, packed_size) = compress_entry(&data, Codec::Store, 1.0).unwrap();
    assert_eq!(packed_size, 0);
    assert_eq!(blob, data);
}

/// `min_shrink_ratio` 0.0 means compression is never accepted, not always:
/// the threshold is `floor(len * 0.0) == 0`, and the accept test is
/// `compressed.len() < threshold`, which no unsigned length can satisfy.
/// Both an incompressible byte and a highly compressible buffer must store.
#[test]
fn compress_entry_shrink_ratio_zero_always_stores() {
    for data in [vec![0xABu8; 1], sample()] {
        let (blob, packed_size) = compress_entry(&data, Codec::Lz4, 0.0).unwrap();
        assert_eq!(
            packed_size,
            0,
            "ratio 0.0 must store (len {}): packed_size==0 signals 'stored'",
            data.len()
        );
        assert_eq!(
            blob,
            data,
            "ratio 0.0 must store (len {}): blob must equal input verbatim",
            data.len()
        );
    }
}

// ── Exact-length decompression ────────────────────────────────────────────

#[test]
fn zlib_output_longer_than_declared_is_rejected() {
    let data = vec![7u8; 4096];
    let packed = compress_zlib(&data).unwrap();
    let err = decompress_zlib(&packed, 100).unwrap_err().to_string();
    assert!(err.contains("expected 100"), "{err}");
    assert_eq!(decompress_zlib(&packed, 4096).unwrap(), data);
}

#[test]
fn zlib_output_shorter_than_declared_is_rejected() {
    let packed = compress_zlib(&[1u8; 10]).unwrap();
    assert!(decompress_zlib(&packed, 11).is_err());
}

#[test]
fn lz4_output_shorter_than_declared_is_rejected() {
    let packed = compress_lz4(&[3u8; 64]);
    assert!(decompress_lz4(&packed, 65).is_err());
    assert_eq!(decompress_lz4(&packed, 64).unwrap(), vec![3u8; 64]);
}
