use anyhow::Context;
use flate2::read::ZlibDecoder;
use std::io::Read;

/// Hard upper bound on any single decompressed output buffer.
///
/// Both LZ4 and zlib decompress calls reject declared output sizes above this
/// limit before allocating, so a malformed record with an unreasonably large
/// declared size cannot cause an unbounded allocation.
///
/// Policy: malformed decompression input → controlled error, never panic or
/// large allocation.  This mirrors the decoder invariant ("unknown/malformed
/// bytes → raw hex fallback, never panic") extended to the compression layer.
pub const MAX_DECOMP_SIZE: usize = 64 * 1024 * 1024; // 64 MiB

/// Decompress an LZ4-block-compressed buffer that must expand to exactly
/// `expected_size` bytes.
///
/// BA2 archives use raw LZ4 blocks (not the LZ4 frame format).
pub fn decompress_lz4(compressed: &[u8], expected_size: usize) -> anyhow::Result<Vec<u8>> {
    check_declared_size("LZ4", expected_size)?;
    let out = lz4_flex::decompress(compressed, expected_size)
        .map_err(|e| anyhow::anyhow!("LZ4 decompress: {}", e))?;
    check_exact_size("LZ4", out, expected_size)
}

/// Decompress a zlib stream that must inflate to exactly `expected_size`
/// bytes. Reading stops one byte past `expected_size`, so a stream that
/// inflates further is rejected without being inflated in full.
pub fn decompress_zlib(compressed: &[u8], expected_size: usize) -> anyhow::Result<Vec<u8>> {
    check_declared_size("zlib", expected_size)?;
    let mut out = Vec::with_capacity(expected_size);
    ZlibDecoder::new(compressed)
        .take(expected_size as u64 + 1)
        .read_to_end(&mut out)
        .context("zlib decompression failed")?;
    check_exact_size("zlib", out, expected_size)
}

fn check_declared_size(codec: &str, expected_size: usize) -> anyhow::Result<()> {
    if expected_size > MAX_DECOMP_SIZE {
        anyhow::bail!(
            "{codec} declared output size {expected_size} exceeds limit of {MAX_DECOMP_SIZE} bytes"
        );
    }
    Ok(())
}

fn check_exact_size(codec: &str, out: Vec<u8>, expected_size: usize) -> anyhow::Result<Vec<u8>> {
    if out.len() != expected_size {
        anyhow::bail!(
            "{codec} output is {} bytes, expected {expected_size}",
            out.len()
        );
    }
    Ok(out)
}

pub fn decompress_record_data(data: &[u8]) -> anyhow::Result<Vec<u8>> {
    if data.len() < 4 {
        anyhow::bail!("compressed record data too short");
    }
    let uncompressed_size = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
    if uncompressed_size == 0 {
        return Ok(Vec::new());
    }
    if uncompressed_size > MAX_DECOMP_SIZE {
        anyhow::bail!(
            "record declared uncompressed size {} exceeds limit of {} bytes",
            uncompressed_size,
            MAX_DECOMP_SIZE
        );
    }
    decompress_zlib(&data[4..], uncompressed_size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decompress_lz4_rejects_oversized_expected_size() {
        let result = decompress_lz4(b"", MAX_DECOMP_SIZE + 1);
        assert!(
            result.is_err(),
            "expected error for oversized expected_size"
        );
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("exceeds limit"),
            "unexpected error message: {msg}"
        );
    }

    #[test]
    fn decompress_zlib_rejects_oversized_expected_size() {
        let result = decompress_zlib(b"", MAX_DECOMP_SIZE + 1);
        assert!(
            result.is_err(),
            "expected error for oversized expected_size"
        );
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("exceeds limit"),
            "unexpected error message: {msg}"
        );
    }

    #[test]
    fn decompress_record_data_rejects_oversized_declared_size() {
        // 4-byte little-endian prefix encoding MAX_DECOMP_SIZE + 1, followed by
        // an empty compressed payload.  The size guard fires before calling zlib.
        let declared = (MAX_DECOMP_SIZE + 1) as u32;
        let mut data = declared.to_le_bytes().to_vec();
        let result = decompress_record_data(&data);
        assert!(
            result.is_err(),
            "expected error for oversized declared size"
        );
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("exceeds limit"),
            "unexpected error message: {msg}"
        );

        // Sanity: a zero declared size returns an empty vec without error.
        data[0..4].copy_from_slice(&0u32.to_le_bytes());
        assert!(decompress_record_data(&data).is_ok());
    }

    #[test]
    fn zlib_output_longer_than_declared_is_rejected() {
        let data = vec![7u8; 4096];
        let packed = flate2_compress(&data);
        let err = decompress_zlib(&packed, 100).unwrap_err().to_string();
        assert!(err.contains("expected 100"), "{err}");
        assert_eq!(decompress_zlib(&packed, 4096).unwrap(), data);
    }

    #[test]
    fn zlib_output_shorter_than_declared_is_rejected() {
        let packed = flate2_compress(&[1u8; 10]);
        assert!(decompress_zlib(&packed, 11).is_err());
    }

    #[test]
    fn lz4_output_shorter_than_declared_is_rejected() {
        let packed = lz4_flex::compress(&[3u8; 64]);
        assert!(decompress_lz4(&packed, 65).is_err());
        assert_eq!(decompress_lz4(&packed, 64).unwrap(), vec![3u8; 64]);
    }

    fn flate2_compress(data: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(data).unwrap();
        enc.finish().unwrap()
    }
}
