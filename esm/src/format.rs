use byteorder::{LittleEndian, ReadBytesExt};
use std::io::{Cursor, Read};

pub const HEADER_SIZE: u64 = 24;
pub const SUBRECORD_HEADER_SIZE: usize = 6;
pub const COMPRESSED_FLAG: u32 = 0x0004_0000;
pub const TES4_SIG: [u8; 4] = *b"TES4";
pub const GRUP_SIG: [u8; 4] = *b"GRUP";
pub const XXXX_SIG: [u8; 4] = *b"XXXX";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Signature(pub [u8; 4]);

impl Signature {
    pub fn from_slice(s: &[u8]) -> Self {
        let mut sig = [0u8; 4];
        let len = s.len().min(4);
        sig[..len].copy_from_slice(&s[..len]);
        Signature(sig)
    }

    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0).unwrap_or("")
    }
}

impl std::fmt::Display for Signature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RecordHeader {
    pub signature: Signature,
    pub data_size: u32,
    pub flags: u32,
    pub form_id: u32,
    pub vcs1: u32,
    pub form_version: u16,
    pub vcs2: u16,
}

impl RecordHeader {
    pub fn parse(data: &[u8]) -> anyhow::Result<Self> {
        let mut cur = Cursor::new(data);
        let mut sig = [0u8; 4];
        cur.read_exact(&mut sig)?;
        Ok(RecordHeader {
            signature: Signature(sig),
            data_size: cur.read_u32::<LittleEndian>()?,
            flags: cur.read_u32::<LittleEndian>()?,
            form_id: cur.read_u32::<LittleEndian>()?,
            vcs1: cur.read_u32::<LittleEndian>()?,
            form_version: cur.read_u16::<LittleEndian>()?,
            vcs2: cur.read_u16::<LittleEndian>()?,
        })
    }

    pub fn is_compressed(&self) -> bool {
        self.flags & COMPRESSED_FLAG != 0
    }

    pub fn total_size(&self) -> u64 {
        // Both operands are u64: HEADER_SIZE is a u64 constant (24) and
        // data_size (u32) is widened to u64.  The maximum value is
        // 24 + u32::MAX ≈ 4 GiB, which is well within u64's range.
        // No overflow is possible with these input types.
        HEADER_SIZE + self.data_size as u64
    }
}

#[derive(Debug, Clone, Copy)]
pub struct GroupHeader {
    pub signature: Signature,
    pub group_size: u32,
    pub label: u32,
    pub group_type: i32,
    pub stamp: u32,
    pub unknown: u32,
}

impl GroupHeader {
    pub fn parse(data: &[u8]) -> anyhow::Result<Self> {
        let mut cur = Cursor::new(data);
        let mut sig = [0u8; 4];
        cur.read_exact(&mut sig)?;
        Ok(GroupHeader {
            signature: Signature(sig),
            group_size: cur.read_u32::<LittleEndian>()?,
            label: cur.read_u32::<LittleEndian>()?,
            group_type: cur.read_i32::<LittleEndian>()?,
            stamp: cur.read_u32::<LittleEndian>()?,
            unknown: cur.read_u32::<LittleEndian>()?,
        })
    }

    pub fn record_signature(&self) -> Option<Signature> {
        if self.group_type == 0 {
            Some(Signature::from_slice(&self.label.to_le_bytes()))
        } else {
            None
        }
    }
}

#[derive(Debug, Clone)]
pub struct SubrecordHeader {
    pub signature: Signature,
    pub size: u16,
}

impl SubrecordHeader {
    pub fn parse(data: &[u8]) -> anyhow::Result<Self> {
        let mut cur = Cursor::new(data);
        let mut sig = [0u8; 4];
        cur.read_exact(&mut sig)?;
        Ok(SubrecordHeader {
            signature: Signature(sig),
            size: cur.read_u16::<LittleEndian>()?,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Subrecord<'a> {
    pub signature: Signature,
    pub data: &'a [u8],
}
