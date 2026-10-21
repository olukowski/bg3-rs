//! File header, file info block and section table.

use crate::{Error, Result};

/// Size of the fixed magic block at the start of every file.
pub const MAGIC_BLOCK_SIZE: usize = 0x20;
/// Size of the file info block that follows the magic block.
pub const FILE_INFO_SIZE: usize = 0x48;
/// Size of one section header.
pub const SECTION_HEADER_SIZE: usize = 44;

/// The 16-byte signatures that open a Granny file. They encode pointer width and byte order.
///
/// Only [`Format::LittleEndian64`] occurs in the BG3 reference install; the others are listed so
/// that we can give precise errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    LittleEndian32,
    BigEndian32,
    LittleEndian64,
    BigEndian64,
    /// A second little-endian 64-bit signature with an identical layout. Seen on 39 files in
    /// `Models.pak` (mostly character proxies) in the reference build.
    LittleEndian64Alt,
}

const MAGICS: [(Format, [u8; 16]); 5] = [
    (
        Format::LittleEndian32,
        [
            0x29, 0xDE, 0x6C, 0xC0, 0xBA, 0xA4, 0x53, 0x2B, 0x25, 0xF5, 0xB7, 0xA5, 0xF6, 0x66,
            0xE2, 0xEE,
        ],
    ),
    (
        Format::BigEndian32,
        [
            0x0E, 0x11, 0x95, 0xB5, 0x6A, 0xA5, 0xB5, 0x4B, 0xEB, 0x28, 0x28, 0x50, 0x25, 0x78,
            0xB3, 0x04,
        ],
    ),
    (
        Format::LittleEndian64,
        [
            0xE5, 0x9B, 0x49, 0x5E, 0x6F, 0x63, 0x1F, 0x14, 0x1E, 0x13, 0xEB, 0xA9, 0x90, 0xBE,
            0xED, 0xC4,
        ],
    ),
    (
        Format::BigEndian64,
        [
            0x31, 0x95, 0xD4, 0xE3, 0x20, 0xDC, 0x4F, 0x62, 0xCC, 0x36, 0xD0, 0x3A, 0xB1, 0x82,
            0xFF, 0x89,
        ],
    ),
    (
        Format::LittleEndian64Alt,
        [
            0xE5, 0x2F, 0x4A, 0xE1, 0x6F, 0xC2, 0x8A, 0xEE, 0x1E, 0xD2, 0xB4, 0x4C, 0x90, 0xD7,
            0x55, 0xAF,
        ],
    ),
];

impl Format {
    pub fn from_magic(magic: &[u8]) -> Option<Self> {
        MAGICS
            .iter()
            .find(|(_, m)| magic.get(..16) == Some(&m[..]))
            .map(|(f, _)| *f)
    }

    pub fn magic(self) -> [u8; 16] {
        MAGICS.iter().find(|(f, _)| *f == self).unwrap().1
    }
}

/// Section compression codec, as stored in the section header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Compression {
    None,
    Oodle0,
    Oodle1,
    BitKnit1,
    BitKnit2,
    Unknown(u32),
}

impl Compression {
    pub fn from_u32(v: u32) -> Self {
        match v {
            0 => Self::None,
            1 => Self::Oodle0,
            2 => Self::Oodle1,
            3 => Self::BitKnit1,
            4 => Self::BitKnit2,
            n => Self::Unknown(n),
        }
    }

    pub fn to_u32(self) -> u32 {
        match self {
            Self::None => 0,
            Self::Oodle0 => 1,
            Self::Oodle1 => 2,
            Self::BitKnit1 => 3,
            Self::BitKnit2 => 4,
            Self::Unknown(n) => n,
        }
    }
}

/// A `(section, offset)` pair as stored in the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct SectionRef {
    pub section: u32,
    pub offset: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct SectionHeader {
    pub compression: Compression,
    /// Absolute file offset of the (possibly compressed) section data.
    pub data_offset: u32,
    /// Bytes on disk.
    pub data_size: u32,
    /// Bytes after decompression.
    pub expanded_size: u32,
    pub alignment: u32,
    /// Oodle-only stop markers (offsets where 16-bit / 8-bit data begins).
    pub first_16bit: u32,
    pub first_8bit: u32,
    pub relocations_offset: u32,
    pub relocations_count: u32,
    pub marshalling_offset: u32,
    pub marshalling_count: u32,
}

#[derive(Debug, Clone)]
pub struct Header {
    pub format: Format,
    /// Total size of magic block + file info + section table.
    pub header_size: u32,
    /// The u32 after `header_size` in the magic block (0 in BG3).
    pub header_format: u32,
    /// File format revision (7 in BG3).
    pub revision: u32,
    pub total_size: u32,
    pub crc32: u32,
    pub root_type: SectionRef,
    pub root_object: SectionRef,
    /// Granny "type tag": identifies the version of the exporter's root type.
    pub type_tag: u32,
    pub extra_tags: [u32; 4],
    pub string_table_crc: u32,
    pub sections: Vec<SectionHeader>,
}

pub(crate) fn u32_at(data: &[u8], off: usize) -> Result<u32> {
    data.get(off..off + 4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        .ok_or(Error::Truncated("header"))
}

impl Header {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let format = Format::from_magic(data).ok_or(Error::BadMagic)?;
        if !matches!(format, Format::LittleEndian64 | Format::LittleEndian64Alt) {
            return Err(Error::UnsupportedFormat(format));
        }
        let header_size = u32_at(data, 0x10)?;
        let header_format = u32_at(data, 0x14)?;
        let fi = MAGIC_BLOCK_SIZE;
        let revision = u32_at(data, fi)?;
        if revision != 7 {
            return Err(Error::UnsupportedRevision(revision));
        }
        let total_size = u32_at(data, fi + 4)?;
        let crc32 = u32_at(data, fi + 8)?;
        let section_table = fi + u32_at(data, fi + 12)? as usize;
        let section_count = u32_at(data, fi + 16)? as usize;
        let sref = |o: usize| -> Result<SectionRef> {
            Ok(SectionRef {
                section: u32_at(data, o)?,
                offset: u32_at(data, o + 4)?,
            })
        };
        let root_type = sref(fi + 20)?;
        let root_object = sref(fi + 28)?;
        let type_tag = u32_at(data, fi + 36)?;
        let mut extra_tags = [0; 4];
        for (i, t) in extra_tags.iter_mut().enumerate() {
            *t = u32_at(data, fi + 40 + 4 * i)?;
        }
        let string_table_crc = u32_at(data, fi + 56)?;
        if section_count > 1024 {
            return Err(Error::Corrupt("implausible section count"));
        }
        let mut sections = Vec::with_capacity(section_count);
        for i in 0..section_count {
            let o = section_table + i * SECTION_HEADER_SIZE;
            let f = |k: usize| u32_at(data, o + 4 * k);
            sections.push(SectionHeader {
                compression: Compression::from_u32(f(0)?),
                data_offset: f(1)?,
                data_size: f(2)?,
                expanded_size: f(3)?,
                alignment: f(4)?,
                first_16bit: f(5)?,
                first_8bit: f(6)?,
                relocations_offset: f(7)?,
                relocations_count: f(8)?,
                marshalling_offset: f(9)?,
                marshalling_count: f(10)?,
            });
        }
        Ok(Self {
            format,
            header_size,
            header_format,
            revision,
            total_size,
            crc32,
            root_type,
            root_object,
            type_tag,
            extra_tags,
            string_table_crc,
            sections,
        })
    }
}
