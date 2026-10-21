//! Reader for Larian `.loca` localization files (Baldur's Gate 3).
//!
//! Layout (all little-endian), see `docs/formats/loca.md`:
//!
//! ```text
//! 0x00  "LOCA"
//! 0x04  u32  entry count
//! 0x08  u32  offset of the text area (= 12 + count * 70)
//! 0x0C  count * entry:
//!         [u8; 64] handle, NUL-padded ASCII (e.g. "h0123…g…")
//!         u16      version
//!         u32      text length in bytes, including the trailing NUL
//! text area: the texts, back to back, in entry order (UTF-8, NUL-terminated)
//! ```

use std::collections::HashMap;

pub const MAGIC: &[u8; 4] = b"LOCA";
const HEADER_SIZE: usize = 12;
const KEY_SIZE: usize = 64;
const ENTRY_SIZE: usize = KEY_SIZE + 2 + 4;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not a LOCA file (bad magic)")]
    BadMagic,
    #[error("LOCA file is truncated or corrupt: {0}")]
    Corrupt(&'static str),
    #[error("entry {index}: handle is not valid ASCII/UTF-8")]
    BadHandle { index: usize },
    #[error("entry {index} ({handle}): text is not valid UTF-8")]
    BadText { index: usize, handle: String },
}

pub type Result<T> = std::result::Result<T, Error>;

/// One translated string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocaEntry {
    pub version: u16,
    pub text: String,
}

/// A parsed localization table: handle -> (version, text).
#[derive(Debug, Clone, Default)]
pub struct Loca {
    entries: HashMap<String, LocaEntry>,
    /// Number of entries whose handle was already present when inserted (later one won).
    duplicates: usize,
}

impl Loca {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse a `.loca` file. Entries with a handle seen earlier in the same file replace it.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut loca = Self::new();
        loca.merge_bytes(data)?;
        Ok(loca)
    }

    /// Parse a `.loca` file and add its entries on top of the existing ones (later wins).
    pub fn merge_bytes(&mut self, data: &[u8]) -> Result<()> {
        for item in iter_entries(data)? {
            let (handle, entry) = item?;
            self.insert(handle.to_owned(), entry);
        }
        Ok(())
    }

    /// Add all entries of `other` on top of this table (later wins).
    pub fn merge(&mut self, other: Loca) {
        for (handle, entry) in other.entries {
            self.insert(handle, entry);
        }
    }

    fn insert(&mut self, handle: String, entry: LocaEntry) {
        if self.entries.insert(handle, entry).is_some() {
            self.duplicates += 1;
        }
    }

    /// Look up a handle. Accepts both a bare handle and a `handle;version` reference as used in
    /// stats and LSX data (the version suffix is ignored for lookup).
    pub fn get(&self, handle: &str) -> Option<&LocaEntry> {
        self.entries.get(split_reference(handle).0)
    }

    /// Text for a handle (see [`Loca::get`]).
    pub fn text(&self, handle: &str) -> Option<&str> {
        self.get(handle).map(|e| e.text.as_str())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Number of entries that replaced an earlier entry with the same handle.
    pub fn duplicates(&self) -> usize {
        self.duplicates
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &LocaEntry)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }
}

/// Split a translated-string reference `"h…;3"` into `("h…", Some(3))`.
pub fn split_reference(reference: &str) -> (&str, Option<u16>) {
    match reference.split_once(';') {
        Some((h, v)) => (h, v.trim().parse().ok()),
        None => (reference, None),
    }
}

/// Iterate the raw entries of a `.loca` file in file order without building a map.
pub fn iter_entries(data: &[u8]) -> Result<impl Iterator<Item = Result<(&str, LocaEntry)>> + '_> {
    if data.len() < HEADER_SIZE {
        return Err(Error::Corrupt("shorter than header"));
    }
    if &data[0..4] != MAGIC {
        return Err(Error::BadMagic);
    }
    let count = u32_at(data, 4) as usize;
    let texts_offset = u32_at(data, 8) as usize;
    let table_end = count
        .checked_mul(ENTRY_SIZE)
        .and_then(|n| n.checked_add(HEADER_SIZE))
        .ok_or(Error::Corrupt("entry count overflows"))?;
    if table_end > data.len() || texts_offset < table_end || texts_offset > data.len() {
        return Err(Error::Corrupt("entry table out of bounds"));
    }
    let mut text_pos = texts_offset;
    Ok((0..count).map(move |index| {
        let e = &data[HEADER_SIZE + index * ENTRY_SIZE..][..ENTRY_SIZE];
        let key = &e[..KEY_SIZE];
        let key = &key[..key.iter().position(|&b| b == 0).unwrap_or(KEY_SIZE)];
        let handle = std::str::from_utf8(key).map_err(|_| Error::BadHandle { index })?;
        let version = u16::from_le_bytes([e[KEY_SIZE], e[KEY_SIZE + 1]]);
        let len = u32_at(e, KEY_SIZE + 2) as usize;
        let raw = data
            .get(text_pos..text_pos + len)
            .ok_or(Error::Corrupt("text out of bounds"))?;
        text_pos += len;
        // Length includes the NUL terminator; tolerate its absence.
        let raw = raw.strip_suffix(&[0]).unwrap_or(raw);
        let text = std::str::from_utf8(raw)
            .map_err(|_| Error::BadText {
                index,
                handle: handle.to_owned(),
            })?
            .to_owned();
        Ok((handle, LocaEntry { version, text }))
    }))
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

/// Serialize entries to `.loca` bytes. Used to author test fixtures; mirrors the reader.
pub fn write<'a>(entries: impl IntoIterator<Item = (&'a str, u16, &'a str)>) -> Vec<u8> {
    let entries: Vec<_> = entries.into_iter().collect();
    let texts_offset = HEADER_SIZE + entries.len() * ENTRY_SIZE;
    let mut out = Vec::with_capacity(texts_offset);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    out.extend_from_slice(&(texts_offset as u32).to_le_bytes());
    for (handle, version, text) in &entries {
        let mut key = [0u8; KEY_SIZE];
        let n = handle.len().min(KEY_SIZE - 1);
        key[..n].copy_from_slice(&handle.as_bytes()[..n]);
        out.extend_from_slice(&key);
        out.extend_from_slice(&version.to_le_bytes());
        out.extend_from_slice(&(text.len() as u32 + 1).to_le_bytes());
    }
    for (_, _, text) in &entries {
        out.extend_from_slice(text.as_bytes());
        out.push(0);
    }
    out
}

#[cfg(test)]
mod tests;
