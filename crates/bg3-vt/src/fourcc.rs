//! The tile set's FourCC metadata tree.
//!
//! A `.gts` carries a tree of tagged values (a little like IFF/RIFF). Each element is a header
//! `{fourcc: [u8;4], kind: u8, extended: u8, length: u16}`, an extra `u32` holding bits 16.. of
//! the length when `extended == 1`, then the value, padded to a 4-byte boundary (relative to the
//! start of the file). Kinds: 1 = node (value is a list of child elements), 2 = UTF-16LE string
//! with terminating NUL, 3 = u32, 8 = opaque binary, 13 = binary GUID.
//!
//! The texture catalogue lives at `ATLS/TXTS/TXTR`: each texture has `NAME` (the GTex name),
//! `WDTH`, `HGHT`, `XXXX`, `YYYY` (its rectangle in level-0 pixels of the virtual image).

use crate::bytes::Cursor;
use crate::{Result, corrupt};

pub type FourCc = [u8; 4];

#[derive(Debug, Clone, PartialEq)]
pub enum FourCcValue {
    Node(Vec<FourCcNode>),
    String(String),
    U32(u32),
    Binary(Vec<u8>),
    Guid(Vec<u8>),
    /// A kind this reader doesn't interpret; raw bytes kept.
    Other(u8, Vec<u8>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct FourCcNode {
    pub fourcc: FourCc,
    pub value: FourCcValue,
}

impl FourCcNode {
    pub fn tag(&self) -> String {
        String::from_utf8_lossy(&self.fourcc).into_owned()
    }

    pub fn children(&self) -> &[FourCcNode] {
        match &self.value {
            FourCcValue::Node(c) => c,
            _ => &[],
        }
    }

    pub fn child(&self, tag: &[u8; 4]) -> Option<&FourCcNode> {
        self.children().iter().find(|c| &c.fourcc == tag)
    }

    pub fn as_u32(&self) -> Option<u32> {
        match self.value {
            FourCcValue::U32(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match &self.value {
            FourCcValue::String(s) => Some(s),
            _ => None,
        }
    }
}

/// Parse the element list in `file[offset .. offset + len]`. `file` is the whole `.gts` so that
/// padding is computed relative to its start.
pub fn parse(file: &[u8], offset: u64, len: u64) -> Result<Vec<FourCcNode>> {
    let end = offset
        .checked_add(len)
        .filter(|&e| e <= file.len() as u64)
        .ok_or_else(|| corrupt("FourCC list out of bounds"))?;
    let mut cur = Cursor::at(file, offset, "FourCC list")?;
    parse_list(&mut cur, end as usize, 0)
}

fn parse_list(cur: &mut Cursor<'_>, end: usize, depth: u32) -> Result<Vec<FourCcNode>> {
    if depth > 64 {
        return Err(corrupt("FourCC tree too deep"));
    }
    let mut out = Vec::new();
    while cur.pos() < end {
        let fourcc: FourCc = cur.array()?;
        let kind = cur.u8()?;
        let extended = cur.u8()?;
        let mut len = cur.u16()? as usize;
        if extended == 1 {
            len |= (cur.u32()? as usize) << 16;
        }
        let value = match kind {
            1 => {
                let child_end = cur.pos() + len;
                if child_end > end {
                    return Err(corrupt("FourCC node overruns its parent"));
                }
                FourCcValue::Node(parse_list(cur, child_end, depth + 1)?)
            }
            2 => {
                let raw = cur.bytes(len)?;
                let units: Vec<u16> = raw
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_le_bytes(*c))
                    .take_while(|&u| u != 0)
                    .collect();
                FourCcValue::String(String::from_utf16_lossy(&units))
            }
            3 if len == 4 => FourCcValue::U32(cur.u32()?),
            8 => FourCcValue::Binary(cur.bytes(len)?.to_vec()),
            13 => FourCcValue::Guid(cur.bytes(len)?.to_vec()),
            k => FourCcValue::Other(k, cur.bytes(len)?.to_vec()),
        };
        let pad = (4 - cur.pos() % 4) % 4;
        if pad > 0 && cur.pos() + pad <= end {
            cur.skip(pad)?;
        }
        out.push(FourCcNode { fourcc, value });
    }
    if cur.pos() != end {
        return Err(corrupt("FourCC element overruns its list"));
    }
    Ok(out)
}

/// Serialise elements in the same layout (used to build test fixtures). `base` is the absolute
/// file offset the output will be written at (for padding).
pub fn write(nodes: &[FourCcNode], base: usize, out: &mut Vec<u8>) {
    for n in nodes {
        let header_at = out.len();
        out.extend_from_slice(&n.fourcc);
        let (kind, payload): (u8, Vec<u8>) = match &n.value {
            FourCcValue::Node(_) => (1, Vec::new()),
            FourCcValue::String(s) => {
                let mut b: Vec<u8> = s.encode_utf16().flat_map(u16::to_le_bytes).collect();
                b.extend_from_slice(&[0, 0]);
                (2, b)
            }
            FourCcValue::U32(v) => (3, v.to_le_bytes().to_vec()),
            FourCcValue::Binary(b) => (8, b.clone()),
            FourCcValue::Guid(b) => (13, b.clone()),
            FourCcValue::Other(k, b) => (*k, b.clone()),
        };
        out.push(kind);
        // Nodes always use the extended length so it can be patched after writing children.
        let extended = matches!(n.value, FourCcValue::Node(_)) || payload.len() > 0xffff;
        out.push(extended as u8);
        out.extend_from_slice(&((payload.len() & 0xffff) as u16).to_le_bytes());
        if extended {
            out.extend_from_slice(&((payload.len() >> 16) as u32).to_le_bytes());
        }
        if let FourCcValue::Node(children) = &n.value {
            let start = out.len();
            write(children, base, out);
            let len = out.len() - start;
            out[header_at + 6..header_at + 8]
                .copy_from_slice(&((len & 0xffff) as u16).to_le_bytes());
            out[header_at + 8..header_at + 12].copy_from_slice(&((len >> 16) as u32).to_le_bytes());
        } else {
            out.extend_from_slice(&payload);
        }
        while !(base + out.len()).is_multiple_of(4) {
            out.push(0);
        }
    }
}
