//! Reader for Larian LSPK (`.pak`) archives, header version 18 (Baldur's Gate 3).
//!
//! Layout (all little-endian), see `docs/formats/pak.md`:
//!
//! ```text
//! 0x00  "LSPK"
//! 0x04  u32  version (18)
//! 0x08  u64  file list offset
//! 0x10  u32  file list size
//! 0x14  u8   flags
//! 0x15  u8   priority
//! 0x16  [u8; 16] md5
//! 0x26  u16  number of parts (part 0 = this file; parts N>0 are `<stem>_N.pak`, headerless)
//! ```
//!
//! At the file list offset: `u32 file_count`, `u32 compressed_size`, then an LZ4 block that
//! decompresses to `file_count` entries of 272 bytes each.

use std::fs::File;
use std::path::{Path, PathBuf};

use memmap2::Mmap;

pub const MAGIC: &[u8; 4] = b"LSPK";
pub const SUPPORTED_VERSION: u32 = 18;
const HEADER_SIZE: usize = 0x28;
const ENTRY_SIZE: usize = 272;
const NAME_SIZE: usize = 256;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("not an LSPK archive (bad magic)")]
    BadMagic,
    #[error("unsupported LSPK version {0} (expected {SUPPORTED_VERSION})")]
    UnsupportedVersion(u32),
    #[error("archive is truncated or corrupt: {0}")]
    Corrupt(&'static str),
    #[error("unknown compression method {0}")]
    UnknownCompression(u8),
    #[error("{name}: decompression failed: {reason}")]
    Decompress { name: String, reason: String },
    #[error("{name}: decompressed {actual} bytes, expected {expected}")]
    SizeMismatch {
        name: String,
        expected: usize,
        actual: usize,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

/// Per-file compression method (low nibble of the entry flags).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    None,
    Zlib,
    Lz4,
    Zstd,
}

impl Compression {
    fn from_flags(flags: u8) -> Result<Self> {
        match flags & 0x0f {
            0 => Ok(Self::None),
            1 => Ok(Self::Zlib),
            2 => Ok(Self::Lz4),
            3 => Ok(Self::Zstd),
            n => Err(Error::UnknownCompression(n)),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Header {
    pub version: u32,
    pub file_list_offset: u64,
    pub file_list_size: u32,
    pub flags: u8,
    pub priority: u8,
    pub md5: [u8; 16],
    pub num_parts: u16,
}

#[derive(Debug, Clone)]
pub struct Entry {
    /// Path inside the archive, `/`-separated.
    pub name: String,
    pub offset: u64,
    pub part: u8,
    /// Raw entry flags (compression method in the low nibble, level hint in the high nibble).
    pub flags: u8,
    pub size_on_disk: u32,
    /// Uncompressed size; 0 for stored (uncompressed) entries.
    pub uncompressed_size: u32,
}

impl Entry {
    pub fn compression(&self) -> Result<Compression> {
        Compression::from_flags(self.flags)
    }

    /// Size of the file contents once read.
    pub fn size(&self) -> u64 {
        match self.compression() {
            Ok(Compression::None) | Err(_) => self.size_on_disk as u64,
            Ok(_) => self.uncompressed_size as u64,
        }
    }
}

/// An opened archive with all of its parts memory-mapped.
pub struct Package {
    path: PathBuf,
    header: Header,
    entries: Vec<Entry>,
    parts: Vec<Mmap>,
}

impl std::fmt::Debug for Package {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Package")
            .field("path", &self.path)
            .field("header", &self.header)
            .field("entries", &self.entries.len())
            .finish()
    }
}

impl Package {
    /// Open a primary `.pak` and all of its parts. The files are never written.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_owned();
        let main = map(&path)?;
        let header = parse_header(&main)?;
        let entries = parse_file_list(&main, &header)?;

        let mut parts = vec![main];
        for n in 1..header.num_parts {
            parts.push(map(&part_path(&path, n))?);
        }
        Ok(Self {
            path,
            header,
            entries,
            parts,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Find an entry by exact archive path.
    pub fn find(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// The on-disk (possibly compressed) bytes of an entry, borrowed from the mapping.
    pub fn raw(&self, entry: &Entry) -> Result<&[u8]> {
        let part = self
            .parts
            .get(entry.part as usize)
            .ok_or(Error::Corrupt("entry refers to a missing archive part"))?;
        let start = usize::try_from(entry.offset).map_err(|_| Error::Corrupt("entry offset"))?;
        let end = start
            .checked_add(entry.size_on_disk as usize)
            .ok_or(Error::Corrupt("entry size"))?;
        part.get(start..end)
            .ok_or(Error::Corrupt("entry out of bounds"))
    }

    /// Read and decompress an entry.
    pub fn read(&self, entry: &Entry) -> Result<Vec<u8>> {
        let raw = self.raw(entry)?;
        let expected = entry.uncompressed_size as usize;
        let fail = |reason: String| Error::Decompress {
            name: entry.name.clone(),
            reason,
        };
        let data = match entry.compression()? {
            Compression::None => return Ok(raw.to_vec()),
            Compression::Lz4 => {
                lz4_flex::block::decompress(raw, expected).map_err(|e| fail(e.to_string()))?
            }
            Compression::Zstd => {
                zstd::bulk::decompress(raw, expected).map_err(|e| fail(e.to_string()))?
            }
            Compression::Zlib => {
                use std::io::Read;
                let mut out = Vec::with_capacity(expected);
                flate2::read::ZlibDecoder::new(raw)
                    .read_to_end(&mut out)
                    .map_err(|e| fail(e.to_string()))?;
                out
            }
        };
        if data.len() != expected {
            return Err(Error::SizeMismatch {
                name: entry.name.clone(),
                expected,
                actual: data.len(),
            });
        }
        Ok(data)
    }
}

/// `dir/Foo.pak`, part `n` → `dir/Foo_n.pak`.
pub fn part_path(path: &Path, n: u16) -> PathBuf {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    path.with_file_name(format!("{stem}_{n}.pak"))
}

fn map(path: &Path) -> Result<Mmap> {
    let io = |source| Error::Io {
        path: path.to_owned(),
        source,
    };
    let file = File::open(path).map_err(io)?;
    // SAFETY: the mapping is read-only and we never write to game files. If another process
    // truncates the file while mapped, reads may fault; the game install is treated as immutable.
    #[allow(unsafe_code)]
    unsafe { Mmap::map(&file) }.map_err(io)
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(b[at..at + 2].try_into().unwrap())
}
fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

pub fn parse_header(data: &[u8]) -> Result<Header> {
    if data.len() < HEADER_SIZE {
        return Err(Error::Corrupt("header"));
    }
    if &data[0..4] != MAGIC {
        return Err(Error::BadMagic);
    }
    let version = u32_at(data, 4);
    if version != SUPPORTED_VERSION {
        return Err(Error::UnsupportedVersion(version));
    }
    Ok(Header {
        version,
        file_list_offset: u64_at(data, 0x08),
        file_list_size: u32_at(data, 0x10),
        flags: data[0x14],
        priority: data[0x15],
        md5: data[0x16..0x26].try_into().unwrap(),
        num_parts: u16_at(data, 0x26),
    })
}

fn parse_file_list(data: &[u8], header: &Header) -> Result<Vec<Entry>> {
    let start =
        usize::try_from(header.file_list_offset).map_err(|_| Error::Corrupt("file list offset"))?;
    let list = data
        .get(start..start.saturating_add(header.file_list_size as usize))
        .ok_or(Error::Corrupt("file list out of bounds"))?;
    if list.len() < 8 {
        return Err(Error::Corrupt("file list header"));
    }
    let count = u32_at(list, 0) as usize;
    let compressed_size = u32_at(list, 4) as usize;
    let compressed = list
        .get(8..8 + compressed_size)
        .ok_or(Error::Corrupt("file list data"))?;
    let table_size = count
        .checked_mul(ENTRY_SIZE)
        .ok_or(Error::Corrupt("file count"))?;
    let table =
        lz4_flex::block::decompress(compressed, table_size).map_err(|e| Error::Decompress {
            name: "<file list>".into(),
            reason: e.to_string(),
        })?;
    if table.len() != table_size {
        return Err(Error::Corrupt("file list size"));
    }
    Ok(table
        .as_chunks::<ENTRY_SIZE>()
        .0
        .iter()
        .map(parse_entry)
        .collect())
}

fn parse_entry(e: &[u8; ENTRY_SIZE]) -> Entry {
    let name_bytes = &e[..NAME_SIZE];
    let len = name_bytes.iter().position(|&b| b == 0).unwrap_or(NAME_SIZE);
    let name = String::from_utf8_lossy(&name_bytes[..len]).replace('\\', "/");
    let offset = u32_at(e, 256) as u64 | (u16_at(e, 260) as u64) << 32;
    Entry {
        name,
        offset,
        part: e[262],
        flags: e[263],
        size_on_disk: u32_at(e, 264),
        uncompressed_size: u32_at(e, 268),
    }
}

#[cfg(test)]
mod tests;
