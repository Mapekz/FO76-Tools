//! DDS header synthesis and parsing for DX10 (texture) BA2 entries.
//!
//! A DX10 archive entry is not a DDS file on disk — it is a texture header
//! (dimensions, mip count, `DXGI_FORMAT`) plus N compressed chunks, each a
//! contiguous run of mip levels. Extraction must synthesize a standard
//! `DDS_HEADER` (plus the `DDS_HEADER_DXT10` extension for formats that need
//! it) and prepend it to the decompressed, concatenated chunk data; `create`
//! does the inverse — parse a `.dds` file's header back into
//! `(dxgi_format, width, height, mip_count, cubemap)`.
//!
//! Only the 15 `DXGI_FORMAT` values FO76 actually ships are supported
//! (ground-truthed against every DX10 archive in a live game install — see
//! `FORMATS` below); anything else is a hard error naming the unhandled
//! format value rather than a guess. Layout and defaults are ported from
//! xEdit's `TwbDDS.SetUpHeader` (`../TES5Edit/Core/wbDDS.pas`), the reference
//! implementation this crate is validated against.

use anyhow::{Result, bail};

// ── DDS magic / flags / caps (legacy DDS_HEADER, per the DirectX spec) ──────

const MAGIC_DDS: &[u8; 4] = b"DDS ";
const MAGIC_DX10: &[u8; 4] = b"DX10";

const DDSD_CAPS: u32 = 0x0000_0001;
const DDSD_HEIGHT: u32 = 0x0000_0002;
const DDSD_WIDTH: u32 = 0x0000_0004;
const DDSD_PITCH: u32 = 0x0000_0008;
const DDSD_PIXELFORMAT: u32 = 0x0000_1000;
const DDSD_MIPMAPCOUNT: u32 = 0x0002_0000;
const DDSD_LINEARSIZE: u32 = 0x0008_0000;

const DDSCAPS_COMPLEX: u32 = 0x0000_0008;
const DDSCAPS_TEXTURE: u32 = 0x0000_1000;
const DDSCAPS_MIPMAP: u32 = 0x0040_0000;

const DDSCAPS2_CUBEMAP: u32 = 0x0000_0200;
/// `DDSCAPS2_CUBEMAP_{POSITIVE,NEGATIVE}{X,Y,Z}` ORed together.
const DDSCAPS2_CUBEMAP_ALLFACES: u32 = 0x0000_FC00;

const DDPF_ALPHAPIXELS: u32 = 0x0000_0001;
const DDPF_FOURCC: u32 = 0x0000_0004;
const DDPF_RGB: u32 = 0x0000_0040;
const DDPF_LUMINANCE: u32 = 0x0002_0000;

/// `DDS_DIMENSION_TEXTURE2D`, for the DXT10 extension's `resourceDimension`.
const DDS_DIMENSION_TEXTURE2D: u32 = 3;
/// `DDS_RESOURCE_MISC_TEXTURECUBE`, for the DXT10 extension's `miscFlags`.
const DDS_RESOURCE_MISC_TEXTURECUBE: u32 = 0x4;

const DDS_HEADER_SIZE: usize = 128; // magic (4) + the 124-byte DDS_HEADER body
const DDS_HEADER_DXT10_SIZE: usize = 20;

// ── Format table ─────────────────────────────────────────────────────────────

/// How a format's pixel data is described in the legacy `DDS_PIXELFORMAT`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PixelFormat {
    /// A legacy FourCC that fully identifies the format (no DXT10 extension needed).
    FourCc(&'static [u8; 4]),
    /// `R8G8B8A8_UNORM`-style uncompressed RGBA8 masks.
    Rgba8888,
    /// `B8G8R8A8_UNORM`-style uncompressed BGRA8 masks.
    Bgra8888,
    /// `R8_UNORM`-style single-channel luminance mask.
    Luminance8,
    /// No legacy representation — always needs the `DDS_HEADER_DXT10` extension.
    Dxt10,
}

/// Whether `dwPitchOrLinearSize` holds a per-scanline pitch or a whole-top-mip size.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SizeKind {
    Pitch,
    Linear,
}

struct FormatSpec {
    dxgi: u8,
    name: &'static str,
    /// Bits per pixel-equivalent (block-compressed formats express this per
    /// 4x4 block, e.g. BC1 = 4).
    bits_per_pixel: u32,
    pixel_format: PixelFormat,
    size_kind: SizeKind,
}

/// Every `DXGI_FORMAT` value observed across all DX10 archives shipped with
/// FO76 (verified against a live game install: 31 archives, 250,722 texture
/// entries). Bits-per-pixel values are ported from `TwbDDS.GetBitsPerPixel`.
const FORMATS: &[FormatSpec] = &[
    FormatSpec {
        dxgi: 10,
        name: "R16G16B16A16_FLOAT",
        bits_per_pixel: 64,
        pixel_format: PixelFormat::Dxt10,
        size_kind: SizeKind::Pitch,
    },
    FormatSpec {
        dxgi: 11,
        name: "R16G16B16A16_UNORM",
        bits_per_pixel: 64,
        pixel_format: PixelFormat::Dxt10,
        size_kind: SizeKind::Pitch,
    },
    FormatSpec {
        dxgi: 28,
        name: "R8G8B8A8_UNORM",
        bits_per_pixel: 32,
        pixel_format: PixelFormat::Rgba8888,
        size_kind: SizeKind::Pitch,
    },
    FormatSpec {
        dxgi: 29,
        name: "R8G8B8A8_UNORM_SRGB",
        bits_per_pixel: 32,
        pixel_format: PixelFormat::Dxt10,
        size_kind: SizeKind::Pitch,
    },
    FormatSpec {
        dxgi: 61,
        name: "R8_UNORM",
        bits_per_pixel: 8,
        pixel_format: PixelFormat::Luminance8,
        size_kind: SizeKind::Pitch,
    },
    FormatSpec {
        dxgi: 71,
        name: "BC1_UNORM",
        bits_per_pixel: 4,
        pixel_format: PixelFormat::FourCc(b"DXT1"),
        size_kind: SizeKind::Linear,
    },
    FormatSpec {
        dxgi: 72,
        name: "BC1_UNORM_SRGB",
        bits_per_pixel: 4,
        pixel_format: PixelFormat::Dxt10,
        size_kind: SizeKind::Linear,
    },
    FormatSpec {
        dxgi: 77,
        name: "BC3_UNORM",
        bits_per_pixel: 8,
        pixel_format: PixelFormat::FourCc(b"DXT5"),
        size_kind: SizeKind::Linear,
    },
    FormatSpec {
        dxgi: 78,
        name: "BC3_UNORM_SRGB",
        bits_per_pixel: 8,
        pixel_format: PixelFormat::Dxt10,
        size_kind: SizeKind::Linear,
    },
    FormatSpec {
        dxgi: 80,
        name: "BC4_UNORM",
        bits_per_pixel: 4,
        pixel_format: PixelFormat::FourCc(b"BC4U"),
        size_kind: SizeKind::Linear,
    },
    FormatSpec {
        dxgi: 83,
        name: "BC5_UNORM",
        bits_per_pixel: 8,
        pixel_format: PixelFormat::FourCc(b"BC5U"),
        size_kind: SizeKind::Linear,
    },
    FormatSpec {
        dxgi: 84,
        name: "BC5_SNORM",
        bits_per_pixel: 8,
        pixel_format: PixelFormat::FourCc(b"BC5S"),
        size_kind: SizeKind::Linear,
    },
    FormatSpec {
        dxgi: 87,
        name: "B8G8R8A8_UNORM",
        bits_per_pixel: 32,
        pixel_format: PixelFormat::Bgra8888,
        size_kind: SizeKind::Pitch,
    },
    FormatSpec {
        dxgi: 98,
        name: "BC7_UNORM",
        bits_per_pixel: 8,
        pixel_format: PixelFormat::Dxt10,
        size_kind: SizeKind::Linear,
    },
    FormatSpec {
        dxgi: 99,
        name: "BC7_UNORM_SRGB",
        bits_per_pixel: 8,
        pixel_format: PixelFormat::Dxt10,
        size_kind: SizeKind::Linear,
    },
];

fn spec_for(dxgi_format: u8) -> Result<&'static FormatSpec> {
    FORMATS
        .iter()
        .find(|f| f.dxgi == dxgi_format)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "unsupported DXGI_FORMAT {} — not one of the formats FO76 ships",
                dxgi_format
            )
        })
}

/// Human-readable name for a `dxgi_format` value, if it is one FO76 ships.
pub fn format_name(dxgi_format: u8) -> Option<&'static str> {
    FORMATS
        .iter()
        .find(|f| f.dxgi == dxgi_format)
        .map(|f| f.name)
}

/// Bits-per-pixel for a `dxgi_format` value FO76 ships. Used by the mip-chunk
/// size arithmetic in `writer.rs`.
pub fn bits_per_pixel(dxgi_format: u8) -> Result<u32> {
    Ok(spec_for(dxgi_format)?.bits_per_pixel)
}

/// Size (in bytes) of the top-level (mip 0) image for `dxgi_format` at `width`
/// x `height`, per `TwbBSArchive.Pack`'s `MipSize` formula
/// (`(width * height * bits_per_pixel) >> 3`).
pub fn mip0_size(dxgi_format: u8, width: u32, height: u32) -> Result<u32> {
    let bpp = bits_per_pixel(dxgi_format)?;
    Ok(((width as u64 * height as u64 * bpp as u64) >> 3) as u32)
}

// ── Header synthesis (extract) ───────────────────────────────────────────────

/// Synthesize a DDS file header (`DDS_HEADER`, plus `DDS_HEADER_DXT10` when
/// the format needs it) for a texture entry. The caller appends the
/// decompressed, concatenated mip chunk data after this header to produce a
/// complete `.dds` file.
pub fn synth_header(
    dxgi_format: u8,
    width: u16,
    height: u16,
    mip_count: u8,
    cubemap: bool,
) -> Result<Vec<u8>> {
    let spec = spec_for(dxgi_format)?;
    // DirectXTex/DecodeDDSHeader: a stored mip_count of 0 means 1 (single top mip).
    let mip_count = if mip_count == 0 {
        1u32
    } else {
        mip_count as u32
    };
    let width = width as u32;
    let height = height as u32;

    let needs_dxt10 = spec.pixel_format == PixelFormat::Dxt10;
    let mut buf = vec![
        0u8;
        DDS_HEADER_SIZE
            + if needs_dxt10 {
                DDS_HEADER_DXT10_SIZE
            } else {
                0
            }
    ];

    let mut flags = DDSD_CAPS | DDSD_PIXELFORMAT | DDSD_WIDTH | DDSD_HEIGHT | DDSD_MIPMAPCOUNT;
    flags |= match spec.size_kind {
        SizeKind::Linear => DDSD_LINEARSIZE,
        SizeKind::Pitch => DDSD_PITCH,
    };
    let pitch_or_linear_size: u32 = match spec.size_kind {
        SizeKind::Linear => {
            ((width as u64 * height as u64 * spec.bits_per_pixel as u64) >> 3) as u32
        }
        SizeKind::Pitch => ((width as u64 * spec.bits_per_pixel as u64) >> 3) as u32,
    };

    let mut caps = DDSCAPS_TEXTURE;
    if mip_count > 1 {
        caps |= DDSCAPS_MIPMAP | DDSCAPS_COMPLEX;
    }
    let mut caps2 = 0u32;
    if cubemap {
        caps |= DDSCAPS_COMPLEX;
        caps2 = DDSCAPS2_CUBEMAP | DDSCAPS2_CUBEMAP_ALLFACES;
    }

    buf[0..4].copy_from_slice(MAGIC_DDS);
    buf[4..8].copy_from_slice(&124u32.to_le_bytes()); // dwSize
    buf[8..12].copy_from_slice(&flags.to_le_bytes());
    buf[12..16].copy_from_slice(&height.to_le_bytes());
    buf[16..20].copy_from_slice(&width.to_le_bytes());
    buf[20..24].copy_from_slice(&pitch_or_linear_size.to_le_bytes());
    buf[24..28].copy_from_slice(&1u32.to_le_bytes()); // dwDepth
    buf[28..32].copy_from_slice(&mip_count.to_le_bytes());
    // buf[32..76] dwReserved1[11] stays zeroed.
    buf[76..80].copy_from_slice(&32u32.to_le_bytes()); // ddspf.dwSize

    match spec.pixel_format {
        PixelFormat::FourCc(fourcc) => {
            buf[80..84].copy_from_slice(&DDPF_FOURCC.to_le_bytes());
            buf[84..88].copy_from_slice(fourcc);
        }
        PixelFormat::Dxt10 => {
            buf[80..84].copy_from_slice(&DDPF_FOURCC.to_le_bytes());
            buf[84..88].copy_from_slice(MAGIC_DX10);
        }
        PixelFormat::Rgba8888 => {
            buf[80..84].copy_from_slice(&(DDPF_RGB | DDPF_ALPHAPIXELS).to_le_bytes());
            buf[88..92].copy_from_slice(&32u32.to_le_bytes()); // dwRGBBitCount
            buf[92..96].copy_from_slice(&0x0000_00FFu32.to_le_bytes()); // R
            buf[96..100].copy_from_slice(&0x0000_FF00u32.to_le_bytes()); // G
            buf[100..104].copy_from_slice(&0x00FF_0000u32.to_le_bytes()); // B
            buf[104..108].copy_from_slice(&0xFF00_0000u32.to_le_bytes()); // A
        }
        PixelFormat::Bgra8888 => {
            buf[80..84].copy_from_slice(&(DDPF_RGB | DDPF_ALPHAPIXELS).to_le_bytes());
            buf[88..92].copy_from_slice(&32u32.to_le_bytes());
            buf[92..96].copy_from_slice(&0x00FF_0000u32.to_le_bytes()); // R
            buf[96..100].copy_from_slice(&0x0000_FF00u32.to_le_bytes()); // G
            buf[100..104].copy_from_slice(&0x0000_00FFu32.to_le_bytes()); // B
            buf[104..108].copy_from_slice(&0xFF00_0000u32.to_le_bytes()); // A
        }
        PixelFormat::Luminance8 => {
            buf[80..84].copy_from_slice(&DDPF_LUMINANCE.to_le_bytes());
            buf[88..92].copy_from_slice(&8u32.to_le_bytes());
            buf[92..96].copy_from_slice(&0x0000_00FFu32.to_le_bytes()); // R
        }
    }

    buf[108..112].copy_from_slice(&caps.to_le_bytes());
    buf[112..116].copy_from_slice(&caps2.to_le_bytes());
    // buf[116..128] dwCaps3, dwCaps4, dwReserved2 stay zeroed.

    if needs_dxt10 {
        let misc_flags = if cubemap {
            DDS_RESOURCE_MISC_TEXTURECUBE
        } else {
            0
        };
        buf[128..132].copy_from_slice(&(dxgi_format as u32).to_le_bytes());
        buf[132..136].copy_from_slice(&DDS_DIMENSION_TEXTURE2D.to_le_bytes());
        buf[136..140].copy_from_slice(&misc_flags.to_le_bytes());
        buf[140..144].copy_from_slice(&1u32.to_le_bytes()); // arraySize
        // buf[144..148] miscFlags2 stays zeroed.
    }

    Ok(buf)
}

// ── Header parsing (create) ──────────────────────────────────────────────────

/// Metadata parsed from a `.dds` file's header, sufficient to write it back
/// as a DX10 archive entry.
pub struct TextureMeta {
    pub dxgi_format: u8,
    pub width: u16,
    pub height: u16,
    pub mip_count: u8,
    pub cubemap: bool,
    /// Byte length of the header — mip data starts immediately after it.
    pub header_len: usize,
}

/// Parse a `.dds` file's header into the fields needed to write a DX10
/// archive entry. Rejects anything not expressible as one of the 15 formats
/// FO76 ships.
pub fn parse_header(dds: &[u8]) -> Result<TextureMeta> {
    if dds.len() < DDS_HEADER_SIZE {
        bail!(
            "DDS file too small to contain a header ({} bytes)",
            dds.len()
        );
    }
    if &dds[0..4] != MAGIC_DDS {
        bail!("not a DDS file (bad magic {:?})", &dds[0..4]);
    }
    let dw_size = u32::from_le_bytes(dds[4..8].try_into().unwrap());
    if dw_size != 124 {
        bail!("unexpected DDS_HEADER dwSize {} (expected 124)", dw_size);
    }
    let height = u32::from_le_bytes(dds[12..16].try_into().unwrap());
    let width = u32::from_le_bytes(dds[16..20].try_into().unwrap());
    let mut mip_count = u32::from_le_bytes(dds[28..32].try_into().unwrap());
    if mip_count == 0 {
        mip_count = 1;
    }
    let pf_flags = u32::from_le_bytes(dds[80..84].try_into().unwrap());
    let mut fourcc = [0u8; 4];
    fourcc.copy_from_slice(&dds[84..88]);
    let rgb_bit_count = u32::from_le_bytes(dds[88..92].try_into().unwrap());
    let r_mask = u32::from_le_bytes(dds[92..96].try_into().unwrap());
    let g_mask = u32::from_le_bytes(dds[96..100].try_into().unwrap());
    let b_mask = u32::from_le_bytes(dds[100..104].try_into().unwrap());
    let a_mask = u32::from_le_bytes(dds[104..108].try_into().unwrap());
    let caps2 = u32::from_le_bytes(dds[112..116].try_into().unwrap());

    let (dxgi_format, header_len, ext_cubemap) =
        if pf_flags & DDPF_FOURCC != 0 && &fourcc == MAGIC_DX10 {
            if dds.len() < DDS_HEADER_SIZE + DDS_HEADER_DXT10_SIZE {
                bail!("DDS file too small to contain the DXT10 extension header");
            }
            let dxgi = u32::from_le_bytes(dds[128..132].try_into().unwrap());
            if dxgi > u8::MAX as u32 {
                bail!("DXT10 dxgiFormat {} out of range", dxgi);
            }
            let misc_flags = u32::from_le_bytes(dds[136..140].try_into().unwrap());
            (
                dxgi as u8,
                DDS_HEADER_SIZE + DDS_HEADER_DXT10_SIZE,
                misc_flags & DDS_RESOURCE_MISC_TEXTURECUBE != 0,
            )
        } else if pf_flags & DDPF_FOURCC != 0 && &fourcc == b"DXT1" {
            (71, DDS_HEADER_SIZE, false)
        } else if pf_flags & DDPF_FOURCC != 0 && &fourcc == b"DXT5" {
            (77, DDS_HEADER_SIZE, false)
        } else if pf_flags & DDPF_FOURCC != 0 && &fourcc == b"BC4U" {
            (80, DDS_HEADER_SIZE, false)
        } else if pf_flags & DDPF_FOURCC != 0 && &fourcc == b"BC5U" {
            (83, DDS_HEADER_SIZE, false)
        } else if pf_flags & DDPF_FOURCC != 0 && &fourcc == b"BC5S" {
            (84, DDS_HEADER_SIZE, false)
        } else if pf_flags & DDPF_RGB != 0
            && rgb_bit_count == 32
            && r_mask == 0x0000_00FF
            && g_mask == 0x0000_FF00
            && b_mask == 0x00FF_0000
            && a_mask == 0xFF00_0000
        {
            (28, DDS_HEADER_SIZE, false)
        } else if pf_flags & DDPF_RGB != 0
            && rgb_bit_count == 32
            && r_mask == 0x00FF_0000
            && g_mask == 0x0000_FF00
            && b_mask == 0x0000_00FF
            && a_mask == 0xFF00_0000
        {
            (87, DDS_HEADER_SIZE, false)
        } else if pf_flags & DDPF_LUMINANCE != 0 && rgb_bit_count == 8 && r_mask == 0x0000_00FF {
            (61, DDS_HEADER_SIZE, false)
        } else {
            bail!(
                "unrecognized DDS pixel format (flags {:#x}, fourcc {:?}, rgb_bit_count {})",
                pf_flags,
                fourcc,
                rgb_bit_count
            );
        };

    // spec_for validates dxgi_format is one of the 15 formats FO76 ships.
    spec_for(dxgi_format)?;

    if width == 0 || height == 0 || width > u16::MAX as u32 || height > u16::MAX as u32 {
        bail!("DDS dimensions {}x{} out of range", width, height);
    }
    if mip_count > u8::MAX as u32 {
        bail!("DDS mip count {} out of range", mip_count);
    }

    let cubemap = ext_cubemap || caps2 & DDSCAPS2_CUBEMAP != 0;

    Ok(TextureMeta {
        dxgi_format,
        width: width as u16,
        height: height as u16,
        mip_count: mip_count as u8,
        cubemap,
        header_len,
    })
}
