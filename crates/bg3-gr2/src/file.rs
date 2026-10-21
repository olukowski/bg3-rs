//! Section loading: decompression and pointer relocation.

use crate::header::{Compression, Header, SectionRef, u32_at};
use crate::{Error, Result, bitknit};

/// Size of one relocation record: `u32 offset_in_section, u32 target_section, u32 target_offset`.
const RELOCATION_SIZE: usize = 12;

/// A resolved in-file pointer. `Ptr::NULL` is the null pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Ptr(u64);

impl Ptr {
    pub const NULL: Ptr = Ptr(0);

    pub fn new(section: u32, offset: u32) -> Self {
        Ptr(((u64::from(section) + 1) << 32) | u64::from(offset))
    }

    pub fn is_null(self) -> bool {
        self.0 == 0
    }

    pub fn section(self) -> u32 {
        ((self.0 >> 32) as u32).wrapping_sub(1)
    }

    pub fn offset(self) -> u32 {
        self.0 as u32
    }

    /// Pointer `bytes` further on in the same section.
    pub fn offset_by(self, bytes: usize) -> Self {
        Ptr(self.0 + bytes as u64)
    }

    pub fn from_ref(r: SectionRef) -> Self {
        Self::new(r.section, r.offset)
    }
}

/// A Granny file with all sections decompressed and all pointers relocated.
///
/// Pointer slots (8 bytes in the 64-bit format) are rewritten to encode a [`Ptr`]
/// (`(section + 1) << 32 | offset`); unrelocated slots stay 0 = null.
pub struct Gr2File {
    pub header: Header,
    sections: Vec<Vec<u8>>,
}

impl std::fmt::Debug for Gr2File {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gr2File")
            .field("header", &self.header)
            .field(
                "section_sizes",
                &self.sections.iter().map(Vec::len).collect::<Vec<_>>(),
            )
            .finish()
    }
}

fn slice<'d>(data: &'d [u8], off: u32, len: u32, what: &'static str) -> Result<&'d [u8]> {
    let (off, len) = (off as usize, len as usize);
    data.get(off..off.checked_add(len).ok_or(Error::Truncated(what))?)
        .ok_or(Error::Truncated(what))
}

fn decompress(
    codec: Compression,
    src: &[u8],
    out_len: usize,
    section: usize,
    what: &'static str,
) -> Result<Vec<u8>> {
    if out_len == 0 {
        return Ok(Vec::new());
    }
    match codec {
        Compression::None => {
            let s = src.get(..out_len).ok_or(Error::Truncated(what))?;
            Ok(s.to_vec())
        }
        Compression::BitKnit2 => {
            bitknit::decompress(src, out_len).map_err(|source| Error::Decompress {
                section,
                what,
                source,
            })
        }
        other => Err(Error::UnsupportedCompression(other)),
    }
}

impl Gr2File {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let header = Header::parse(data)?;
        let mut sections = Vec::with_capacity(header.sections.len());
        for (i, s) in header.sections.iter().enumerate() {
            let src = slice(data, s.data_offset, s.data_size, "section data")?;
            let out = decompress(
                s.compression,
                src,
                s.expanded_size as usize,
                i,
                "section data",
            )?;
            sections.push(out);
        }
        let mut file = Self { header, sections };
        file.relocate(data)?;
        Ok(file)
    }

    fn relocate(&mut self, data: &[u8]) -> Result<()> {
        for i in 0..self.sections.len() {
            let s = self.header.sections[i];
            let count = s.relocations_count as usize;
            if count == 0 {
                continue;
            }
            let raw_len = count * RELOCATION_SIZE;
            let table = if s.compression == Compression::BitKnit2 {
                // Compressed relocation tables are prefixed by their compressed size.
                let packed = u32_at(data, s.relocations_offset as usize)
                    .map_err(|_| Error::Truncated("relocations"))?;
                let src = slice(data, s.relocations_offset + 4, packed, "relocations")?;
                decompress(s.compression, src, raw_len, i, "relocations")?
            } else {
                slice(data, s.relocations_offset, raw_len as u32, "relocations")?.to_vec()
            };
            for rec in table.as_chunks::<RELOCATION_SIZE>().0 {
                let at = u32::from_le_bytes(rec[0..4].try_into().unwrap()) as usize;
                let target_section = u32::from_le_bytes(rec[4..8].try_into().unwrap());
                let target_offset = u32::from_le_bytes(rec[8..12].try_into().unwrap());
                let target_len = self
                    .sections
                    .get(target_section as usize)
                    .ok_or(Error::Corrupt("relocation targets missing section"))?
                    .len();
                if target_offset as usize > target_len {
                    return Err(Error::Corrupt("relocation target out of range"));
                }
                let ptr = Ptr::new(target_section, target_offset);
                let slot = self.sections[i]
                    .get_mut(at..at + 8)
                    .ok_or(Error::Corrupt("relocation outside section"))?;
                slot.copy_from_slice(&ptr.0.to_le_bytes());
            }
        }
        Ok(())
    }

    pub fn sections(&self) -> &[Vec<u8>] {
        &self.sections
    }

    pub fn root_type_ptr(&self) -> Ptr {
        Ptr::from_ref(self.header.root_type)
    }

    pub fn root_object_ptr(&self) -> Ptr {
        Ptr::from_ref(self.header.root_object)
    }

    /// `len` bytes at `p`.
    pub fn bytes(&self, p: Ptr, len: usize) -> Result<&[u8]> {
        if p.is_null() {
            return Err(Error::NullPointer);
        }
        let sec = self
            .sections
            .get(p.section() as usize)
            .ok_or(Error::BadPointer(p))?;
        let off = p.offset() as usize;
        sec.get(off..off.checked_add(len).ok_or(Error::BadPointer(p))?)
            .ok_or(Error::BadPointer(p))
    }

    /// Bytes from `p` to the end of its section.
    pub fn tail(&self, p: Ptr) -> Result<&[u8]> {
        if p.is_null() {
            return Err(Error::NullPointer);
        }
        self.sections
            .get(p.section() as usize)
            .and_then(|s| s.get(p.offset() as usize..))
            .ok_or(Error::BadPointer(p))
    }

    pub fn u8(&self, p: Ptr) -> Result<u8> {
        Ok(self.bytes(p, 1)?[0])
    }

    pub fn u16(&self, p: Ptr) -> Result<u16> {
        Ok(u16::from_le_bytes(self.bytes(p, 2)?.try_into().unwrap()))
    }

    pub fn u32(&self, p: Ptr) -> Result<u32> {
        Ok(u32::from_le_bytes(self.bytes(p, 4)?.try_into().unwrap()))
    }

    pub fn i32(&self, p: Ptr) -> Result<i32> {
        Ok(self.u32(p)? as i32)
    }

    pub fn f32(&self, p: Ptr) -> Result<f32> {
        Ok(f32::from_bits(self.u32(p)?))
    }

    /// Read a pointer slot.
    pub fn ptr(&self, p: Ptr) -> Result<Ptr> {
        Ok(Ptr(u64::from_le_bytes(
            self.bytes(p, 8)?.try_into().unwrap(),
        )))
    }

    /// NUL-terminated string at `p` (lossy UTF-8).
    pub fn cstr(&self, p: Ptr) -> Result<String> {
        let tail = self.tail(p)?;
        let end = tail
            .iter()
            .position(|&b| b == 0)
            .ok_or(Error::Corrupt("unterminated string"))?;
        Ok(String::from_utf8_lossy(&tail[..end]).into_owned())
    }
}
