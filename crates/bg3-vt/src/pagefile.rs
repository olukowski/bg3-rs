//! `.gtp` page files.
//!
//! A page file is a sequence of fixed-size pages (`TileSet::page_size`, 1 MiB in BG3). The first
//! page starts with a 24-byte header (`GRAP`, version, GUID). Every page then holds
//! `u32 count, u32 offsets[count]` followed by its chunks; offsets are relative to the page start.
//! A chunk is `{codec: u32, parameter_block: u32, size: u32}` + `size` bytes of payload.

use crate::bytes::Cursor;
use crate::tileset::Codec;
use crate::{Error, Result, corrupt};

pub(crate) const GTP_MAGIC: &[u8; 4] = b"GRAP";
pub(crate) const GTP_HEADER_SIZE: usize = 24;

/// A borrowed view of a page file.
#[derive(Debug, Clone, Copy)]
pub struct PageFile<'a> {
    data: &'a [u8],
    page_size: usize,
    pub version: u32,
    pub guid: [u8; 16],
}

/// One chunk (usually one tile) inside a page.
#[derive(Debug, Clone, Copy)]
pub struct Chunk<'a> {
    pub codec: Codec,
    pub parameter_block: u32,
    pub data: &'a [u8],
}

impl<'a> PageFile<'a> {
    pub fn parse(data: &'a [u8], page_size: u32) -> Result<Self> {
        if data.len() < GTP_HEADER_SIZE || &data[0..4] != GTP_MAGIC {
            return Err(corrupt("not a Granite page file (missing GRAP magic)"));
        }
        if page_size == 0 {
            return Err(corrupt("zero page size"));
        }
        let mut c = Cursor::new(data, "GTP header");
        c.skip(4)?;
        let version = c.u32()?;
        if version != 4 {
            return Err(Error::Unsupported(format!("GTP version {version}")));
        }
        let guid = c.array()?;
        Ok(Self {
            data,
            page_size: page_size as usize,
            version,
            guid,
        })
    }

    pub fn num_pages(&self) -> usize {
        self.data.len().div_ceil(self.page_size)
    }

    /// Chunk offsets of `page`, relative to the page start.
    pub fn chunk_offsets(&self, page: usize) -> Result<Vec<u32>> {
        let (start, page_data) = self.page(page)?;
        let mut c = Cursor::at(page_data, start as u64, "page chunk table")?;
        let n = c.u32()? as usize;
        if n > self.page_size / 4 {
            return Err(corrupt(format!("page {page}: implausible chunk count {n}")));
        }
        c.u32s(n)
    }

    /// Read chunk `index` of `page`.
    pub fn chunk(&self, page: usize, index: usize) -> Result<Chunk<'a>> {
        let offsets = self.chunk_offsets(page)?;
        let offset = *offsets
            .get(index)
            .ok_or_else(|| corrupt(format!("page {page} has no chunk {index}")))?;
        let (_, page_data) = self.page(page)?;
        let mut c = Cursor::at(page_data, offset as u64, "chunk")?;
        let codec = Codec::from_raw(c.u32()?);
        let parameter_block = c.u32()?;
        let size = c.u32()? as usize;
        Ok(Chunk {
            codec,
            parameter_block,
            data: c.bytes(size)?,
        })
    }

    /// (table offset within the page, page bytes).
    fn page(&self, page: usize) -> Result<(usize, &'a [u8])> {
        let start = page
            .checked_mul(self.page_size)
            .filter(|&s| s < self.data.len())
            .ok_or_else(|| corrupt(format!("page {page} out of range")))?;
        let end = (start + self.page_size).min(self.data.len());
        let table = if page == 0 { GTP_HEADER_SIZE } else { 0 };
        Ok((table, &self.data[start..end]))
    }
}
