//! Bounds-checked little-endian reads.

use crate::{Result, corrupt};

/// Borrow `len` bytes at `offset`, or fail with a message naming `what`.
pub(crate) fn slice<'a>(data: &'a [u8], offset: u64, len: u64, what: &str) -> Result<&'a [u8]> {
    let start = usize::try_from(offset).map_err(|_| corrupt(format!("{what}: offset")))?;
    let len = usize::try_from(len).map_err(|_| corrupt(format!("{what}: length")))?;
    start
        .checked_add(len)
        .and_then(|end| data.get(start..end))
        .ok_or_else(|| corrupt(format!("{what} out of bounds ({offset:#x}+{len:#x})")))
}

/// A cursor over a byte slice. Reads past the end fail instead of panicking.
pub(crate) struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
    what: &'static str,
}

impl<'a> Cursor<'a> {
    pub fn new(data: &'a [u8], what: &'static str) -> Self {
        Self { data, pos: 0, what }
    }

    pub fn at(data: &'a [u8], offset: u64, what: &'static str) -> Result<Self> {
        let pos = usize::try_from(offset).map_err(|_| corrupt(format!("{what}: offset")))?;
        if pos > data.len() {
            return Err(corrupt(format!("{what} offset {offset:#x} out of bounds")));
        }
        Ok(Self { data, pos, what })
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.data.len())
            .ok_or_else(|| corrupt(format!("{}: unexpected end of data", self.what)))?;
        let s = &self.data[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.bytes(n).map(|_| ())
    }

    pub fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut a = [0; N];
        a.copy_from_slice(self.bytes(N)?);
        Ok(a)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }

    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    pub fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    /// `count` little-endian u32s.
    pub fn u32s(&mut self, count: usize) -> Result<Vec<u32>> {
        let raw = self.bytes(
            count
                .checked_mul(4)
                .ok_or_else(|| corrupt(format!("{}: count overflow", self.what)))?,
        )?;
        Ok(raw
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| u32::from_le_bytes(*c))
            .collect())
    }
}
