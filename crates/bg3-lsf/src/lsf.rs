//! LSF ("LSOF") binary resource reader and writer. Layout: `docs/formats/lsf.md`.

use std::collections::HashMap;
use std::io::{Read, Write};

use crate::value::{FsArgument, Guid, TranslatedFSString, TranslatedString};
use crate::{
    Attribute, AttributeType, EngineVersion, Error, Metadata, Node, Region, Resource, Result,
    SourceFormat, Value,
};

pub const MAGIC: &[u8; 4] = b"LSOF";

/// Oldest/newest container versions this reader accepts. BG3 4.1.1 ships only 6 and 7.
pub const MIN_VERSION: u32 = 1;
pub const MAX_VERSION: u32 = 7;

/// First version with a 64-bit engine version in the header.
const VER_EXTENDED_HEADER: u32 = 5;
/// First version with the node-keys section in the header.
const VER_KEYS: u32 = 6;
/// First version whose TranslatedString values carry a u16 version instead of inline text.
const VER_BG3: u32 = 4;
/// First version that compresses node/attribute/value sections as LZ4 *frames*.
const VER_CHUNKED: u32 = 2;

const LZ4_FRAME_MAGIC: [u8; 4] = [0x04, 0x22, 0x4d, 0x18];

/// Container details of an LSF file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LsfInfo {
    /// Container version (header u32 after the magic).
    pub version: u32,
    /// Raw compression flags byte: method in the low nibble (0 none, 1 zlib, 2 LZ4, 3 zstd),
    /// level hint in the high nibble.
    pub compression: u8,
    /// Raw metadata-format word (1 = "keys and adjacency": 16-byte node/attribute records).
    pub metadata_format: u32,
    /// Whether nodes/attributes use the 16-byte records with sibling/next links and offsets.
    pub extended: bool,
}

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
    what: &'static str,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8], what: &'static str) -> Self {
        Cursor { data, pos: 0, what }
    }
    fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }
    fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.data.len())
            .ok_or(Error::Eof(self.what))?;
        let out = &self.data[self.pos..end];
        self.pos = end;
        Ok(out)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.bytes(N)?.try_into().unwrap())
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.array()?))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array()?))
    }
    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.array()?))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.array()?))
    }
}

struct Sections {
    strings: (u32, u32),
    keys: (u32, u32),
    nodes: (u32, u32),
    attributes: (u32, u32),
    values: (u32, u32),
}

fn unpack_engine_version(version: u32, raw: u64) -> EngineVersion {
    if version >= VER_EXTENDED_HEADER {
        EngineVersion {
            major: (raw >> 55) as u32,
            minor: ((raw >> 47) & 0xff) as u32,
            revision: ((raw >> 31) & 0xffff) as u32,
            build: (raw & 0x7fff_ffff) as u32,
        }
    } else {
        let raw = raw as u32;
        EngineVersion {
            major: raw >> 28,
            minor: (raw >> 24) & 0xf,
            revision: (raw >> 16) & 0xff,
            build: raw & 0xffff,
        }
    }
}

fn pack_engine_version(version: u32, v: EngineVersion) -> u64 {
    if version >= VER_EXTENDED_HEADER {
        (u64::from(v.major) << 55)
            | (u64::from(v.minor & 0xff) << 47)
            | (u64::from(v.revision & 0xffff) << 31)
            | u64::from(v.build & 0x7fff_ffff)
    } else {
        u64::from(
            (v.major << 28) | (v.minor & 0xf) << 24 | (v.revision & 0xff) << 16 | v.build & 0xffff,
        )
    }
}

/// Reads one section: `(uncompressed size, size on disk)`. A zero on-disk size means the section
/// is stored uncompressed (and occupies `uncompressed` bytes).
fn section(
    c: &mut Cursor<'_>,
    (uncompressed, on_disk): (u32, u32),
    flags: u8,
    chunked: bool,
    what: &'static str,
) -> Result<Vec<u8>> {
    if on_disk == 0 {
        return Ok(c.bytes(uncompressed as usize)?.to_vec());
    }
    let raw = c.bytes(on_disk as usize)?;
    let size = uncompressed as usize;
    let fail = |msg: String| Error::Decompress { section: what, msg };
    let out = match flags & 0x0f {
        0 => raw.to_vec(),
        1 => {
            let mut out = Vec::with_capacity(size);
            flate2::read::ZlibDecoder::new(raw)
                .read_to_end(&mut out)
                .map_err(|e| fail(e.to_string()))?;
            out
        }
        2 if chunked || raw.starts_with(&LZ4_FRAME_MAGIC) => {
            let mut out = Vec::with_capacity(size);
            lz4_flex::frame::FrameDecoder::new(raw)
                .read_to_end(&mut out)
                .map_err(|e| fail(e.to_string()))?;
            out
        }
        2 => lz4_flex::block::decompress(raw, size).map_err(|e| fail(e.to_string()))?,
        3 => {
            let mut out = Vec::with_capacity(size);
            zstd::stream::read::Decoder::new(raw)
                .and_then(|mut d| d.read_to_end(&mut out))
                .map_err(|e| fail(e.to_string()))?;
            out
        }
        m => return Err(fail(format!("unknown compression method {m}"))),
    };
    if out.len() != size {
        return Err(fail(format!("expected {size} bytes, got {}", out.len())));
    }
    Ok(out)
}

/// Parses an LSF file.
pub fn read(data: &[u8]) -> Result<Resource> {
    let mut c = Cursor::new(data, "header");
    if c.bytes(4)? != MAGIC {
        return Err(Error::BadMagic);
    }
    let version = c.u32()?;
    if !(MIN_VERSION..=MAX_VERSION).contains(&version) {
        return Err(Error::UnsupportedVersion(version));
    }
    let engine = if version >= VER_EXTENDED_HEADER {
        c.u64()?
    } else {
        c.u32()?.into()
    };
    let mut pair = || -> Result<(u32, u32)> { Ok((c.u32()?, c.u32()?)) };
    let strings = pair()?;
    let keys = if version >= VER_KEYS { pair()? } else { (0, 0) };
    let sizes = Sections {
        strings,
        keys,
        nodes: pair()?,
        attributes: pair()?,
        values: pair()?,
    };
    let compression = c.u8()?;
    let _unknown = (c.u8()?, c.u16()?);
    let metadata_format = c.u32()?;
    let info = LsfInfo {
        version,
        compression,
        metadata_format,
        extended: metadata_format == 1,
    };

    let chunked = version >= VER_CHUNKED;
    c.what = "sections";
    let strings = section(&mut c, sizes.strings, compression, false, "strings")?;
    let nodes = section(&mut c, sizes.nodes, compression, chunked, "nodes")?;
    let attributes = section(&mut c, sizes.attributes, compression, chunked, "attributes")?;
    let values = section(&mut c, sizes.values, compression, chunked, "values")?;
    let keys = section(&mut c, sizes.keys, compression, chunked, "keys")?;

    let metadata = Metadata {
        format: SourceFormat::Lsf,
        version: unpack_engine_version(version, engine),
        lsf: Some(info),
        ..Default::default()
    };
    let regions = Decoder {
        info,
        names: read_names(&strings)?,
        values: &values,
    }
    .build(&nodes, &attributes, &keys)?;
    Ok(Resource { metadata, regions })
}

fn read_names(data: &[u8]) -> Result<Vec<Vec<String>>> {
    let mut c = Cursor::new(data, "strings");
    let buckets = c.u32()?;
    let mut names = Vec::with_capacity(buckets.min(1 << 16) as usize);
    for _ in 0..buckets {
        let count = c.u16()?;
        let mut bucket = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let len = c.u16()? as usize;
            bucket.push(String::from_utf8_lossy(c.bytes(len)?).into_owned());
        }
        names.push(bucket);
    }
    Ok(names)
}

struct RawNode {
    name: u32,
    parent: i32,
    first_attribute: i32,
}

struct RawAttribute {
    name: u32,
    type_and_length: u32,
    next: i32,
    offset: usize,
}

struct Decoder<'a> {
    info: LsfInfo,
    names: Vec<Vec<String>>,
    values: &'a [u8],
}

impl Decoder<'_> {
    fn name(&self, index: u32) -> Result<&str> {
        self.names
            .get((index >> 16) as usize)
            .and_then(|b| b.get((index & 0xffff) as usize))
            .map(String::as_str)
            .ok_or(Error::BadName(index))
    }

    fn build(&self, nodes: &[u8], attributes: &[u8], keys: &[u8]) -> Result<Vec<Region>> {
        let extended = self.info.extended;
        let record = if extended { 16 } else { 12 };
        if !nodes.len().is_multiple_of(record) || !attributes.len().is_multiple_of(record) {
            return Err(Error::Malformed(format!(
                "node/attribute tables ({} / {} bytes) are not multiples of {record}",
                nodes.len(),
                attributes.len()
            )));
        }

        let mut c = Cursor::new(nodes, "nodes");
        let mut raw_nodes = Vec::with_capacity(nodes.len() / record);
        while c.remaining() > 0 {
            raw_nodes.push(if extended {
                let name = c.u32()?;
                let parent = c.i32()?;
                let _next_sibling = c.i32()?;
                let first_attribute = c.i32()?;
                RawNode {
                    name,
                    parent,
                    first_attribute,
                }
            } else {
                let name = c.u32()?;
                let first_attribute = c.i32()?;
                let parent = c.i32()?;
                RawNode {
                    name,
                    parent,
                    first_attribute,
                }
            });
        }

        let mut c = Cursor::new(attributes, "attributes");
        let mut raw_attrs: Vec<RawAttribute> = Vec::with_capacity(attributes.len() / record);
        // Short records have no next/offset fields: values are packed in table order, and each
        // attribute links to the next one owned by the same node.
        let mut last_of_node: HashMap<i32, usize> = HashMap::new();
        let mut offset = 0usize;
        while c.remaining() > 0 {
            let name = c.u32()?;
            let type_and_length = c.u32()?;
            if extended {
                let next = c.i32()?;
                let offset = c.u32()? as usize;
                raw_attrs.push(RawAttribute {
                    name,
                    type_and_length,
                    next,
                    offset,
                });
            } else {
                let node = c.i32()?;
                let index = raw_attrs.len();
                if let Some(prev) = last_of_node.insert(node, index) {
                    raw_attrs[prev].next = index as i32;
                }
                raw_attrs.push(RawAttribute {
                    name,
                    type_and_length,
                    next: -1,
                    offset,
                });
                offset += (type_and_length >> 6) as usize;
            }
        }

        let mut node_keys = HashMap::new();
        let mut c = Cursor::new(keys, "keys");
        while c.remaining() > 0 {
            let node = c.u32()? as usize;
            node_keys.insert(node, self.name(c.u32()?)?.to_owned());
        }

        let mut built: Vec<Option<Node>> = Vec::with_capacity(raw_nodes.len());
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); raw_nodes.len()];
        let mut roots = Vec::new();
        for (i, raw) in raw_nodes.iter().enumerate() {
            let mut node = Node::new(self.name(raw.name)?);
            node.key = node_keys.remove(&i);
            let mut a = raw.first_attribute;
            while a >= 0 {
                let attr = raw_attrs
                    .get(a as usize)
                    .ok_or_else(|| Error::Malformed(format!("attribute index {a} out of range")))?;
                if node.attributes.len() > raw_attrs.len() {
                    return Err(Error::Malformed("attribute chain loops".into()));
                }
                node.attributes.push(Attribute {
                    name: self.name(attr.name)?.to_owned(),
                    value: self.value(attr)?,
                });
                a = attr.next;
            }
            built.push(Some(node));
            match raw.parent {
                -1 => roots.push(i),
                p if p >= 0 && (p as usize) < raw_nodes.len() && p as usize != i => {
                    children[p as usize].push(i)
                }
                p => return Err(Error::Malformed(format!("node {i} has bad parent {p}"))),
            }
        }

        fn assemble(i: usize, built: &mut [Option<Node>], children: &[Vec<usize>]) -> Result<Node> {
            let mut node = built[i]
                .take()
                .ok_or_else(|| Error::Malformed("node hierarchy has a cycle".into()))?;
            node.children = children[i]
                .iter()
                .map(|&c| assemble(c, built, children))
                .collect::<Result<_>>()?;
            Ok(node)
        }
        let regions = roots
            .into_iter()
            .map(|i| {
                let node = assemble(i, &mut built, &children)?;
                Ok(Region {
                    name: node.name.clone(),
                    node,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        if built.iter().any(Option::is_some) {
            return Err(Error::Malformed("nodes unreachable from any region".into()));
        }
        Ok(regions)
    }

    fn value(&self, attr: &RawAttribute) -> Result<Value> {
        let ty_id = attr.type_and_length & 0x3f;
        let len = (attr.type_and_length >> 6) as usize;
        let ty = AttributeType::from_id(ty_id).ok_or(Error::UnknownType(ty_id.to_string()))?;
        let bytes = attr
            .offset
            .checked_add(len)
            .and_then(|end| self.values.get(attr.offset..end))
            .ok_or(Error::Eof("values"))?;
        let mut c = Cursor::new(bytes, "values");
        let value = decode_value(ty, &mut c, self.info.version)?;
        if c.remaining() != 0 {
            return Err(Error::Malformed(format!(
                "{ty} value has {} trailing bytes (length {len})",
                c.remaining()
            )));
        }
        Ok(value)
    }
}

/// A NUL-terminated string occupying exactly `len` bytes.
fn string(c: &mut Cursor<'_>, len: usize) -> Result<String> {
    let bytes = c.bytes(len)?;
    let bytes = bytes.strip_suffix(&[0]).unwrap_or(bytes);
    Ok(match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => String::from_utf8_lossy(bytes).into_owned(),
    })
}

fn sized_string(c: &mut Cursor<'_>) -> Result<String> {
    let len = c.i32()?;
    if len < 0 {
        return Err(Error::Malformed(format!("negative string length {len}")));
    }
    string(c, len as usize)
}

fn translated_string(c: &mut Cursor<'_>, version: u32) -> Result<TranslatedString> {
    if version >= VER_BG3 {
        let version = c.u16()?;
        let handle = sized_string(c)?;
        Ok(TranslatedString {
            value: None,
            handle,
            version,
        })
    } else {
        let value = sized_string(c)?;
        let handle = sized_string(c)?;
        Ok(TranslatedString {
            value: Some(value),
            handle,
            version: 0,
        })
    }
}

fn translated_fs_string(c: &mut Cursor<'_>, version: u32) -> Result<TranslatedFSString> {
    let string = translated_string(c, version)?;
    let count = c.i32()?;
    let mut arguments = Vec::with_capacity(count.clamp(0, 64) as usize);
    for _ in 0..count {
        let key = sized_string(c)?;
        let string = translated_fs_string(c, version)?;
        let value = sized_string(c)?;
        arguments.push(FsArgument { key, value, string });
    }
    Ok(TranslatedFSString { string, arguments })
}

fn floats<const N: usize>(c: &mut Cursor<'_>) -> Result<[f32; N]> {
    let mut out = [0f32; N];
    for v in &mut out {
        *v = f32::from_le_bytes(c.array()?);
    }
    Ok(out)
}

fn ints<const N: usize>(c: &mut Cursor<'_>) -> Result<[i32; N]> {
    let mut out = [0i32; N];
    for v in &mut out {
        *v = c.i32()?;
    }
    Ok(out)
}

fn decode_value(ty: AttributeType, c: &mut Cursor<'_>, version: u32) -> Result<Value> {
    use AttributeType as T;
    let len = c.remaining();
    Ok(match ty {
        T::None => Value::None,
        T::Byte => Value::Byte(c.u8()?),
        T::Short => Value::Short(c.u16()? as i16),
        T::UShort => Value::UShort(c.u16()?),
        T::Int => Value::Int(c.i32()?),
        T::UInt => Value::UInt(c.u32()?),
        T::Float => Value::Float(f32::from_le_bytes(c.array()?)),
        T::Double => Value::Double(f64::from_le_bytes(c.array()?)),
        T::IVec2 => Value::IVec2(ints(c)?),
        T::IVec3 => Value::IVec3(ints(c)?),
        T::IVec4 => Value::IVec4(ints(c)?),
        T::Vec2 => Value::Vec2(floats(c)?),
        T::Vec3 => Value::Vec3(floats(c)?),
        T::Vec4 => Value::Vec4(floats(c)?),
        T::Mat2 => Value::Mat2(floats(c)?),
        T::Mat3 => Value::Mat3(floats(c)?),
        T::Mat3x4 => Value::Mat3x4(floats(c)?),
        T::Mat4x3 => Value::Mat4x3(floats(c)?),
        T::Mat4 => Value::Mat4(floats(c)?),
        T::Bool => Value::Bool(c.u8()? != 0),
        T::String => Value::String(string(c, len)?),
        T::Path => Value::Path(string(c, len)?),
        T::FixedString => Value::FixedString(string(c, len)?),
        T::LSString => Value::LSString(string(c, len)?),
        T::WString => Value::WString(string(c, len)?),
        T::LSWString => Value::LSWString(string(c, len)?),
        T::ULongLong => Value::ULongLong(c.u64()?),
        T::ScratchBuffer => Value::ScratchBuffer(c.bytes(len)?.to_vec()),
        T::OldInt64 => Value::OldInt64(c.u64()? as i64),
        T::Int8 => Value::Int8(c.u8()? as i8),
        T::TranslatedString => Value::TranslatedString(translated_string(c, version)?),
        T::Uuid => Value::Uuid(Guid::from_lsf_bytes(c.array()?)),
        T::Int64 => Value::Int64(c.u64()? as i64),
        T::TranslatedFSString => Value::TranslatedFSString(translated_fs_string(c, version)?),
    })
}

// ---------------------------------------------------------------------------------------------
// Writer
// ---------------------------------------------------------------------------------------------

/// Section compression for [`write`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Compression {
    #[default]
    None,
    Zlib,
    Lz4,
    Zstd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteOptions {
    /// Container version, [`MIN_VERSION`]..=[`MAX_VERSION`].
    pub version: u32,
    /// 16-byte node/attribute records (required for node keys).
    pub extended: bool,
    pub compression: Compression,
}

impl Default for WriteOptions {
    fn default() -> Self {
        WriteOptions {
            version: MAX_VERSION,
            extended: true,
            compression: Compression::None,
        }
    }
}

#[derive(Default)]
struct Writer {
    buckets: Vec<Vec<String>>,
    lookup: HashMap<String, u32>,
    nodes: Vec<u8>,
    attributes: Vec<u8>,
    values: Vec<u8>,
    keys: Vec<u8>,
    node_count: i32,
    attribute_count: i32,
    parents: Vec<i32>,
}

const BUCKETS: usize = 0x200;

impl Writer {
    fn name(&mut self, s: &str) -> u32 {
        if let Some(&i) = self.lookup.get(s) {
            return i;
        }
        // FNV-1a; any distribution is valid, readers only follow the indices.
        let hash = s.bytes().fold(0x811c_9dc5u32, |h, b| {
            (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
        });
        let b = hash as usize % BUCKETS;
        let index = (b as u32) << 16 | self.buckets[b].len() as u32;
        self.buckets[b].push(s.to_owned());
        self.lookup.insert(s.to_owned(), index);
        index
    }

    fn node(&mut self, node: &Node, name: &str, parent: i32, opts: &WriteOptions) -> Result<()> {
        let index = self.node_count;
        self.node_count += 1;
        let name = self.name(name);
        let first_attribute = if node.attributes.is_empty() {
            -1
        } else {
            self.attribute_count
        };
        self.parents.push(parent);
        if opts.extended {
            for v in [name as i32, parent, -1, first_attribute] {
                self.nodes.extend_from_slice(&v.to_le_bytes());
            }
        } else {
            for v in [name as i32, first_attribute, parent] {
                self.nodes.extend_from_slice(&v.to_le_bytes());
            }
        }
        if let Some(key) = &node.key {
            if opts.version < VER_KEYS || !opts.extended {
                return Err(Error::Malformed(
                    "node keys need an extended LSF v6+ container".into(),
                ));
            }
            let key = self.name(key);
            self.keys.extend_from_slice(&(index as u32).to_le_bytes());
            self.keys.extend_from_slice(&key.to_le_bytes());
        }
        let count = node.attributes.len();
        for (i, attr) in node.attributes.iter().enumerate() {
            let offset = self.values.len();
            encode_value(&attr.value, &mut self.values, opts.version)?;
            let len = self.values.len() - offset;
            if len >= 1 << 26 {
                return Err(Error::Malformed(format!(
                    "value of {} too large",
                    attr.name
                )));
            }
            let type_and_length = attr.value.ty().id() | (len as u32) << 6;
            let name = self.name(&attr.name);
            self.attributes.extend_from_slice(&name.to_le_bytes());
            self.attributes
                .extend_from_slice(&type_and_length.to_le_bytes());
            if opts.extended {
                let next = if i + 1 < count {
                    self.attribute_count + 1
                } else {
                    -1
                };
                self.attributes.extend_from_slice(&next.to_le_bytes());
                self.attributes
                    .extend_from_slice(&(offset as u32).to_le_bytes());
            } else {
                self.attributes.extend_from_slice(&index.to_le_bytes());
            }
            self.attribute_count += 1;
        }
        for child in &node.children {
            self.node(child, &child.name, index, opts)?;
        }
        Ok(())
    }
}

fn put_string(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(s.as_bytes());
    out.push(0);
}

fn put_sized_string(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as i32 + 1).to_le_bytes());
    put_string(out, s);
}

fn put_translated_string(out: &mut Vec<u8>, t: &TranslatedString, version: u32) {
    if version >= VER_BG3 {
        out.extend_from_slice(&t.version.to_le_bytes());
    } else {
        put_sized_string(out, t.value.as_deref().unwrap_or(""));
    }
    put_sized_string(out, &t.handle);
}

fn put_translated_fs_string(out: &mut Vec<u8>, t: &TranslatedFSString, version: u32) {
    put_translated_string(out, &t.string, version);
    out.extend_from_slice(&(t.arguments.len() as i32).to_le_bytes());
    for arg in &t.arguments {
        put_sized_string(out, &arg.key);
        put_translated_fs_string(out, &arg.string, version);
        put_sized_string(out, &arg.value);
    }
}

fn encode_value(v: &Value, out: &mut Vec<u8>, version: u32) -> Result<()> {
    fn f(out: &mut Vec<u8>, v: &[f32]) {
        v.iter()
            .for_each(|x| out.extend_from_slice(&x.to_le_bytes()));
    }
    fn i(out: &mut Vec<u8>, v: &[i32]) {
        v.iter()
            .for_each(|x| out.extend_from_slice(&x.to_le_bytes()));
    }
    match v {
        Value::None => {}
        Value::Byte(x) => out.push(*x),
        Value::Short(x) => out.extend_from_slice(&x.to_le_bytes()),
        Value::UShort(x) => out.extend_from_slice(&x.to_le_bytes()),
        Value::Int(x) => out.extend_from_slice(&x.to_le_bytes()),
        Value::UInt(x) => out.extend_from_slice(&x.to_le_bytes()),
        Value::Float(x) => out.extend_from_slice(&x.to_le_bytes()),
        Value::Double(x) => out.extend_from_slice(&x.to_le_bytes()),
        Value::IVec2(x) => i(out, x),
        Value::IVec3(x) => i(out, x),
        Value::IVec4(x) => i(out, x),
        Value::Vec2(x) => f(out, x),
        Value::Vec3(x) => f(out, x),
        Value::Vec4(x) => f(out, x),
        Value::Mat2(x) => f(out, x),
        Value::Mat3(x) => f(out, x),
        Value::Mat3x4(x) | Value::Mat4x3(x) => f(out, x),
        Value::Mat4(x) => f(out, x),
        Value::Bool(x) => out.push(u8::from(*x)),
        Value::String(s)
        | Value::Path(s)
        | Value::FixedString(s)
        | Value::LSString(s)
        | Value::WString(s)
        | Value::LSWString(s) => put_string(out, s),
        Value::ULongLong(x) => out.extend_from_slice(&x.to_le_bytes()),
        Value::ScratchBuffer(b) => out.extend_from_slice(b),
        Value::OldInt64(x) | Value::Int64(x) => out.extend_from_slice(&x.to_le_bytes()),
        Value::Int8(x) => out.push(*x as u8),
        Value::TranslatedString(t) => put_translated_string(out, t, version),
        Value::Uuid(g) => out.extend_from_slice(&g.to_lsf_bytes()),
        Value::TranslatedFSString(t) => put_translated_fs_string(out, t, version),
    }
    Ok(())
}

fn compress(data: &[u8], method: Compression, chunked: bool) -> Result<(u32, Vec<u8>)> {
    let fail = |e: std::io::Error| Error::Malformed(format!("compression failed: {e}"));
    let unc = data.len() as u32;
    if data.is_empty() {
        return Ok((0, Vec::new()));
    }
    let packed = match method {
        // Stored sections are written with on-disk size 0.
        Compression::None => return Ok((unc, Vec::new())),
        Compression::Zlib => {
            let mut enc =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            enc.write_all(data).map_err(fail)?;
            enc.finish().map_err(fail)?
        }
        Compression::Lz4 if chunked => {
            let mut enc = lz4_flex::frame::FrameEncoder::new(Vec::new());
            enc.write_all(data).map_err(fail)?;
            enc.finish()
                .map_err(|e| Error::Malformed(format!("compression failed: {e}")))?
        }
        Compression::Lz4 => lz4_flex::block::compress(data),
        Compression::Zstd => zstd::bulk::compress(data, 3).map_err(fail)?,
    };
    Ok((unc, packed))
}

/// Serialises a resource as LSF. Output is a valid LSF file readable by [`read`], but is not
/// byte-identical to Larian's (string hashing and section compression differ).
///
/// Each region is written as a top-level node named after the *region* (in LSF the two names
/// are the same thing).
pub fn write(resource: &Resource, opts: WriteOptions) -> Result<Vec<u8>> {
    let version = opts.version;
    if !(MIN_VERSION..=MAX_VERSION).contains(&version) {
        return Err(Error::UnsupportedVersion(version));
    }
    let mut w = Writer {
        buckets: vec![Vec::new(); BUCKETS],
        ..Default::default()
    };
    for region in &resource.regions {
        w.node(&region.node, &region.name, -1, &opts)?;
    }
    if opts.extended {
        // Next-sibling links: each node points at the next node with the same parent.
        let mut last: HashMap<i32, usize> = HashMap::new();
        for (i, &parent) in w.parents.iter().enumerate() {
            if let Some(prev) = last.insert(parent, i) {
                w.nodes[prev * 16 + 8..prev * 16 + 12].copy_from_slice(&(i as i32).to_le_bytes());
            }
        }
    }
    let mut strings = Vec::new();
    strings.extend_from_slice(&(BUCKETS as u32).to_le_bytes());
    for bucket in &w.buckets {
        strings.extend_from_slice(&(bucket.len() as u16).to_le_bytes());
        for s in bucket {
            strings.extend_from_slice(&(s.len() as u16).to_le_bytes());
            strings.extend_from_slice(s.as_bytes());
        }
    }

    let chunked = version >= VER_CHUNKED;
    let method = opts.compression;
    let sections = [
        compress(&strings, method, false)?,
        compress(&w.nodes, method, chunked)?,
        compress(&w.attributes, method, chunked)?,
        compress(&w.values, method, chunked)?,
        compress(&w.keys, method, chunked)?,
    ];
    let flags: u8 = match method {
        Compression::None => 0,
        Compression::Zlib => 1,
        Compression::Lz4 => 2,
        Compression::Zstd => 3,
    };

    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&version.to_le_bytes());
    let engine = pack_engine_version(version, resource.metadata.version);
    if version >= VER_EXTENDED_HEADER {
        out.extend_from_slice(&engine.to_le_bytes());
    } else {
        out.extend_from_slice(&(engine as u32).to_le_bytes());
    }
    let mut header_pair = |(unc, packed): &(u32, Vec<u8>)| {
        out.extend_from_slice(&unc.to_le_bytes());
        out.extend_from_slice(&(packed.len() as u32).to_le_bytes());
    };
    header_pair(&sections[0]);
    if version >= VER_KEYS {
        header_pair(&sections[4]);
    } else if !w.keys.is_empty() {
        return Err(Error::Malformed("node keys need LSF v6+".into()));
    }
    header_pair(&sections[1]);
    header_pair(&sections[2]);
    header_pair(&sections[3]);
    out.push(flags);
    out.extend_from_slice(&[0, 0, 0]);
    out.extend_from_slice(&u32::from(opts.extended).to_le_bytes());

    let raw = [&strings, &w.nodes, &w.attributes, &w.values, &w.keys];
    for (i, (_, packed)) in sections.iter().enumerate() {
        if packed.is_empty() {
            out.extend_from_slice(raw[i]);
        } else {
            out.extend_from_slice(packed);
        }
    }
    Ok(out)
}
