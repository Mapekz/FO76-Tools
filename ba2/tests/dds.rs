//! Integration tests for `ba2::dds` — DDS header synthesis and parsing.

use ba2::dds::{format_name, mip0_size, parse_header, synth_header};

const DDPF_FOURCC: u32 = 0x0000_0004;
const DDPF_RGB: u32 = 0x0000_0040;
const DDPF_LUMINANCE: u32 = 0x0002_0000;
const DDSD_LINEARSIZE: u32 = 0x0008_0000;
const DDSD_PITCH: u32 = 0x0000_0008;
const DDSCAPS2_CUBEMAP_ALLFACES: u32 = 0x0000_FE00; // CUBEMAP | all six faces

fn flags(dds: &[u8]) -> u32 {
    u32::from_le_bytes(dds[8..12].try_into().unwrap())
}
fn pitch_or_linear(dds: &[u8]) -> u32 {
    u32::from_le_bytes(dds[20..24].try_into().unwrap())
}
fn pf_flags(dds: &[u8]) -> u32 {
    u32::from_le_bytes(dds[80..84].try_into().unwrap())
}
fn fourcc(dds: &[u8]) -> [u8; 4] {
    dds[84..88].try_into().unwrap()
}
fn caps2(dds: &[u8]) -> u32 {
    u32::from_le_bytes(dds[112..116].try_into().unwrap())
}

// ── Legacy-FourCC formats (no DXT10 extension) ───────────────────────────────

#[test]
fn bc1_unorm_uses_dxt1_fourcc() {
    let h = synth_header(71, 64, 64, 1, false).unwrap();
    assert_eq!(h.len(), 128, "no DXT10 extension expected");
    assert_eq!(&h[0..4], b"DDS ");
    assert_eq!(pf_flags(&h) & DDPF_FOURCC, DDPF_FOURCC);
    assert_eq!(&fourcc(&h), b"DXT1");
    assert_eq!(flags(&h) & DDSD_LINEARSIZE, DDSD_LINEARSIZE);
    assert_eq!(pitch_or_linear(&h), 64 * 64 / 2, "BC1 linear size = w*h/2");
}

#[test]
fn bc3_unorm_uses_dxt5_fourcc() {
    let h = synth_header(77, 32, 32, 1, false).unwrap();
    assert_eq!(&fourcc(&h), b"DXT5");
    assert_eq!(pitch_or_linear(&h), 32 * 32, "BC3 linear size = w*h");
}

#[test]
fn bc4_unorm_uses_bc4u_fourcc() {
    let h = synth_header(80, 16, 16, 1, false).unwrap();
    assert_eq!(&fourcc(&h), b"BC4U");
    assert_eq!(pitch_or_linear(&h), 16 * 16 / 2);
}

#[test]
fn bc5_unorm_uses_bc5u_fourcc() {
    let h = synth_header(83, 16, 16, 1, false).unwrap();
    assert_eq!(&fourcc(&h), b"BC5U");
}

#[test]
fn bc5_snorm_uses_bc5s_fourcc() {
    // The dominant real-world normal-map format — verified against live game
    // archives to be BC5_SNORM (84), not BC5_UNORM (83).
    let h = synth_header(84, 16, 16, 1, false).unwrap();
    assert_eq!(&fourcc(&h), b"BC5S");
}

// ── DXT10-extension formats ───────────────────────────────────────────────────

#[test]
fn bc7_unorm_needs_dxt10_extension() {
    let h = synth_header(98, 64, 64, 1, false).unwrap();
    assert_eq!(h.len(), 148, "DXT10 extension expected (128 + 20)");
    assert_eq!(pf_flags(&h) & DDPF_FOURCC, DDPF_FOURCC);
    assert_eq!(&fourcc(&h), b"DX10");
    let dxgi = u32::from_le_bytes(h[128..132].try_into().unwrap());
    assert_eq!(dxgi, 98);
    let resource_dim = u32::from_le_bytes(h[132..136].try_into().unwrap());
    assert_eq!(resource_dim, 3, "DDS_DIMENSION_TEXTURE2D");
    let array_size = u32::from_le_bytes(h[140..144].try_into().unwrap());
    assert_eq!(array_size, 1);
}

#[test]
fn bc1_unorm_srgb_needs_dxt10_extension() {
    // BC1_UNORM (71) has a legacy FourCC, but its sRGB sibling (72) does not.
    let h = synth_header(72, 64, 64, 1, false).unwrap();
    assert_eq!(h.len(), 148);
    assert_eq!(&fourcc(&h), b"DX10");
}

#[test]
fn r16g16b16a16_needs_dxt10_extension() {
    let h = synth_header(11, 64, 64, 1, false).unwrap();
    assert_eq!(h.len(), 148);
    assert_eq!(flags(&h) & DDSD_PITCH, DDSD_PITCH);
    assert_eq!(pitch_or_linear(&h), 64 * 8, "(w * 64 bpp) >> 3 = w*8");
}

// ── Uncompressed RGB/luminance formats ───────────────────────────────────────

#[test]
fn r8g8b8a8_unorm_uses_rgba_masks() {
    let h = synth_header(28, 16, 16, 1, false).unwrap();
    assert_eq!(h.len(), 128);
    assert_eq!(pf_flags(&h) & DDPF_RGB, DDPF_RGB);
    let r_mask = u32::from_le_bytes(h[92..96].try_into().unwrap());
    let a_mask = u32::from_le_bytes(h[104..108].try_into().unwrap());
    assert_eq!(r_mask, 0x0000_00FF);
    assert_eq!(a_mask, 0xFF00_0000);
    assert_eq!(pitch_or_linear(&h), 16 * 4);
}

#[test]
fn b8g8r8a8_unorm_uses_bgra_masks() {
    let h = synth_header(87, 16, 16, 1, false).unwrap();
    let r_mask = u32::from_le_bytes(h[92..96].try_into().unwrap());
    assert_eq!(r_mask, 0x00FF_0000, "BGRA: red is the third byte");
}

#[test]
fn r8_unorm_uses_luminance_mask() {
    let h = synth_header(61, 16, 16, 1, false).unwrap();
    assert_eq!(pf_flags(&h) & DDPF_LUMINANCE, DDPF_LUMINANCE);
    assert_eq!(pitch_or_linear(&h), 16);
}

// ── mip_count / cubemap edge cases ───────────────────────────────────────────

#[test]
fn zero_mip_count_treated_as_one() {
    let h0 = synth_header(71, 32, 32, 0, false).unwrap();
    let h1 = synth_header(71, 32, 32, 1, false).unwrap();
    assert_eq!(h0, h1, "mip_count 0 must synthesize identically to 1");
}

#[test]
fn cubemap_sets_caps2_allfaces() {
    let h = synth_header(71, 32, 32, 1, true).unwrap();
    assert_eq!(caps2(&h), DDSCAPS2_CUBEMAP_ALLFACES);
}

#[test]
fn cubemap_dxt10_sets_misc_flags() {
    let h = synth_header(98, 32, 32, 1, true).unwrap();
    let misc_flags = u32::from_le_bytes(h[136..140].try_into().unwrap());
    assert_eq!(misc_flags, 0x4, "DDS_RESOURCE_MISC_TEXTURECUBE");
}

// ── Unknown formats ───────────────────────────────────────────────────────────

#[test]
fn unknown_dxgi_format_errors_naming_the_value() {
    let err = synth_header(200, 32, 32, 1, false).unwrap_err();
    assert!(
        err.to_string().contains("200"),
        "error must name the unhandled format value: {}",
        err
    );
}

#[test]
fn format_name_covers_every_shipped_format() {
    for dxgi in [10, 11, 28, 29, 61, 71, 72, 77, 78, 80, 83, 84, 87, 98, 99] {
        assert!(
            format_name(dxgi).is_some(),
            "missing name for format {}",
            dxgi
        );
    }
    assert!(format_name(200).is_none());
}

#[test]
fn mip0_size_matches_bits_per_pixel_formula() {
    // Ground-truthed against a real archive entry.
    assert_eq!(mip0_size(71, 1024, 1024).unwrap(), 524_288);
}

// ── parse_header round-trips synth_header ────────────────────────────────────

#[test]
fn parse_header_round_trips_legacy_fourcc() {
    let h = synth_header(71, 128, 64, 5, false).unwrap();
    let meta = parse_header(&h).unwrap();
    assert_eq!(meta.dxgi_format, 71);
    assert_eq!(meta.width, 128);
    assert_eq!(meta.height, 64);
    assert_eq!(meta.mip_count, 5);
    assert!(!meta.cubemap);
    assert_eq!(meta.header_len, 128);
}

#[test]
fn parse_header_round_trips_dxt10_extension() {
    let h = synth_header(98, 32, 32, 3, false).unwrap();
    let meta = parse_header(&h).unwrap();
    assert_eq!(meta.dxgi_format, 98);
    assert_eq!(meta.header_len, 148);
}

#[test]
fn parse_header_round_trips_cubemap() {
    let h = synth_header(71, 32, 32, 1, true).unwrap();
    let meta = parse_header(&h).unwrap();
    assert!(meta.cubemap);
}

#[test]
fn parse_header_round_trips_rgba_masks() {
    let h = synth_header(28, 16, 16, 1, false).unwrap();
    let meta = parse_header(&h).unwrap();
    assert_eq!(meta.dxgi_format, 28);
}

#[test]
fn parse_header_rejects_bad_magic() {
    let mut h = synth_header(71, 16, 16, 1, false).unwrap();
    h[0] = b'X';
    assert!(parse_header(&h).is_err());
}

#[test]
fn parse_header_rejects_too_small() {
    assert!(parse_header(&[0u8; 16]).is_err());
}
