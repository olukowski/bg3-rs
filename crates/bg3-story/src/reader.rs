//! Byte cursor matching the primitive encodings of Osiris' `COsiSmartBuf`.

use crate::{Error, Result};

/// Little-endian cursor over a whole story file.
///
/// Strings are NUL-terminated and XOR-"scrambled" with a key that becomes `0xAD` once the header
/// has been read (story format 1.4 and later).
// bg3: COsiSmartBuf::read / COsiSmartBuf::AllocAndRead / COsiSmartBuf::endian_read (4.1.1.7631656)
pub(crate) struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    pub(crate) scramble: u8,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            scramble: 0,
        }
    }

    pub(crate) fn pos(&self) -> usize {
        self.pos
    }

    pub(crate) fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    pub(crate) fn bytes(&mut self, n: usize, what: &'static str) -> Result<&'a [u8]> {
        if self.remaining() < n {
            return Err(Error::Eof {
                offset: self.pos,
                what,
            });
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub(crate) fn u8(&mut self, what: &'static str) -> Result<u8> {
        Ok(self.bytes(1, what)?[0])
    }

    pub(crate) fn i8(&mut self, what: &'static str) -> Result<i8> {
        Ok(self.u8(what)? as i8)
    }

    pub(crate) fn bool(&mut self, what: &'static str) -> Result<bool> {
        Ok(self.u8(what)? != 0)
    }

    pub(crate) fn u16(&mut self, what: &'static str) -> Result<u16> {
        let b = self.bytes(2, what)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub(crate) fn u32(&mut self, what: &'static str) -> Result<u32> {
        let b = self.bytes(4, what)?;
        Ok(u32::from_le_bytes(b.try_into().expect("4 bytes")))
    }

    pub(crate) fn i32(&mut self, what: &'static str) -> Result<i32> {
        Ok(self.u32(what)? as i32)
    }

    pub(crate) fn u64(&mut self, what: &'static str) -> Result<u64> {
        let b = self.bytes(8, what)?;
        Ok(u64::from_le_bytes(b.try_into().expect("8 bytes")))
    }

    pub(crate) fn f32(&mut self, what: &'static str) -> Result<f32> {
        Ok(f32::from_bits(self.u32(what)?))
    }

    /// A scrambled, NUL-terminated string.
    pub(crate) fn string(&mut self, what: &'static str) -> Result<String> {
        let start = self.pos;
        let key = self.scramble;
        let rest = &self.data[self.pos..];
        let len = rest.iter().position(|&b| b ^ key == 0).ok_or(Error::Eof {
            offset: start,
            what,
        })?;
        let raw: Vec<u8> = rest[..len].iter().map(|&b| b ^ key).collect();
        self.pos += len + 1;
        String::from_utf8(raw).map_err(|_| Error::Invalid {
            offset: start,
            msg: format!("{what}: string is not valid UTF-8"),
        })
    }

    /// `u32` count followed by that many items.
    pub(crate) fn list<T>(
        &mut self,
        what: &'static str,
        mut item: impl FnMut(&mut Self) -> Result<T>,
    ) -> Result<Vec<T>> {
        let n = self.u32(what)? as usize;
        // Every item is at least one byte; reject absurd counts before allocating.
        if n > self.remaining() {
            return Err(Error::Invalid {
                offset: self.pos - 4,
                msg: format!("{what}: count {n} exceeds remaining bytes"),
            });
        }
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            out.push(item(self)?);
        }
        Ok(out)
    }

    /// `u8` count followed by that many items.
    pub(crate) fn short_list<T>(
        &mut self,
        what: &'static str,
        mut item: impl FnMut(&mut Self) -> Result<T>,
    ) -> Result<Vec<T>> {
        let n = self.u8(what)? as usize;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            out.push(item(self)?);
        }
        Ok(out)
    }
}
