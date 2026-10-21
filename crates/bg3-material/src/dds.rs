//! Minimal DDS header reader, for surveying and choosing how to upload a texture.
//!
//! Layout per Microsoft's public DDS documentation: `"DDS "`, a 124-byte `DDS_HEADER`, and when
//! the pixel-format FourCC is `DX10`, a 20-byte `DDS_HEADER_DXT10`.

/// Pixel formats we distinguish. Values follow DXGI where a DX10 header is present.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DdsFormat {
    Bc1,
    Bc2,
    Bc3,
    Bc4,
    Bc5,
    Bc6h,
    Bc7,
    Rgba8,
    Bgra8,
    /// Uncompressed with a bit-mask layout we don't name.
    Uncompressed {
        bits: u32,
    },
    Dxgi(u32),
    FourCc([u8; 4]),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DdsHeader {
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub mip_count: u32,
    pub format: DdsFormat,
    /// Whether a `DX10` extension header is present.
    pub dx10: bool,
    /// DXGI says the data is sRGB (`*_UNORM_SRGB`). Legacy headers carry no colour space.
    pub srgb: bool,
    pub cubemap: bool,
    pub array_size: u32,
}

fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(off..off + 4)?.try_into().ok()?))
}

impl DdsHeader {
    pub fn parse(b: &[u8]) -> Option<Self> {
        if b.get(..4)? != b"DDS " || u32_at(b, 4)? != 124 {
            return None;
        }
        let height = u32_at(b, 12)?;
        let width = u32_at(b, 16)?;
        let depth = u32_at(b, 24)?.max(1);
        let mip_count = u32_at(b, 28)?.max(1);
        // DDS_PIXELFORMAT starts at 4 + 72 = 76.
        let pf_flags = u32_at(b, 80)?;
        let fourcc: [u8; 4] = b.get(84..88)?.try_into().ok()?;
        let bits = u32_at(b, 88)?;
        let caps2 = u32_at(b, 112)?;
        let mut cubemap = caps2 & 0x200 != 0;
        let mut array_size = 1;
        let mut srgb = false;
        let dx10 = pf_flags & 0x4 != 0 && &fourcc == b"DX10";
        let format = if dx10 {
            let dxgi = u32_at(b, 128)?;
            cubemap |= u32_at(b, 136)? & 0x4 != 0;
            array_size = u32_at(b, 140)?.max(1);
            srgb = matches!(dxgi, 29 | 72 | 75 | 78 | 91 | 99);
            match dxgi {
                70..=72 => DdsFormat::Bc1,
                73..=75 => DdsFormat::Bc2,
                76..=78 => DdsFormat::Bc3,
                79..=81 => DdsFormat::Bc4,
                82..=84 => DdsFormat::Bc5,
                94..=96 => DdsFormat::Bc6h,
                97..=99 => DdsFormat::Bc7,
                27..=29 => DdsFormat::Rgba8,
                87 | 90 | 91 => DdsFormat::Bgra8,
                n => DdsFormat::Dxgi(n),
            }
        } else if pf_flags & 0x4 != 0 {
            match &fourcc {
                b"DXT1" => DdsFormat::Bc1,
                b"DXT2" | b"DXT3" => DdsFormat::Bc2,
                b"DXT4" | b"DXT5" => DdsFormat::Bc3,
                b"ATI1" | b"BC4U" => DdsFormat::Bc4,
                b"ATI2" | b"BC5U" => DdsFormat::Bc5,
                _ => DdsFormat::FourCc(fourcc),
            }
        } else {
            DdsFormat::Uncompressed { bits }
        };
        Some(Self {
            width,
            height,
            depth,
            mip_count,
            format,
            dx10,
            srgb,
            cubemap,
            array_size,
        })
    }
}

impl DdsHeader {
    /// Byte offset of the first mip level.
    pub fn data_offset(&self) -> usize {
        if self.dx10 { 148 } else { 128 }
    }

    /// `(width, height, byte range)` of every mip level of the first array layer/face.
    /// Only block-compressed formats are supported.
    pub fn mip_levels(&self) -> Option<Vec<(u32, u32, std::ops::Range<usize>)>> {
        let block = crate::bc::block_bytes(self.format)?;
        let mut off = self.data_offset();
        let mut out = Vec::new();
        for level in 0..self.mip_count {
            let (w, h) = ((self.width >> level).max(1), (self.height >> level).max(1));
            let len = crate::bc::mip_bytes(w, h, block);
            out.push((w, h, off..off + len));
            off += len;
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A self-authored header: `w`×`h`, `mips`, FourCC or DX10 with `dxgi`.
    fn header(w: u32, h: u32, mips: u32, fourcc: &[u8; 4], dxgi: Option<u32>) -> Vec<u8> {
        let mut b = vec![0u8; 128];
        b[..4].copy_from_slice(b"DDS ");
        b[4..8].copy_from_slice(&124u32.to_le_bytes());
        b[12..16].copy_from_slice(&h.to_le_bytes());
        b[16..20].copy_from_slice(&w.to_le_bytes());
        b[28..32].copy_from_slice(&mips.to_le_bytes());
        b[80..84].copy_from_slice(&4u32.to_le_bytes());
        b[84..88].copy_from_slice(fourcc);
        if let Some(dxgi) = dxgi {
            let mut ext = vec![0u8; 20];
            ext[..4].copy_from_slice(&dxgi.to_le_bytes());
            ext[4..8].copy_from_slice(&3u32.to_le_bytes());
            ext[12..16].copy_from_slice(&1u32.to_le_bytes());
            b.extend(ext);
        }
        b
    }

    #[test]
    fn legacy_fourcc() {
        let h = DdsHeader::parse(&header(256, 128, 9, b"DXT5", None)).unwrap();
        assert_eq!((h.width, h.height, h.mip_count), (256, 128, 9));
        assert_eq!(h.format, DdsFormat::Bc3);
        assert!(!h.dx10 && !h.srgb && !h.cubemap);
        let h = DdsHeader::parse(&header(4, 4, 1, b"ATI2", None)).unwrap();
        assert_eq!(h.format, DdsFormat::Bc5);
    }

    #[test]
    fn dx10_bc7_srgb() {
        let h = DdsHeader::parse(&header(64, 64, 7, b"DX10", Some(99))).unwrap();
        assert_eq!(h.format, DdsFormat::Bc7);
        assert!(h.dx10 && h.srgb);
        let h = DdsHeader::parse(&header(64, 64, 7, b"DX10", Some(80))).unwrap();
        assert_eq!(h.format, DdsFormat::Bc4);
        assert!(!h.srgb);
    }

    #[test]
    fn rejects_garbage() {
        assert!(DdsHeader::parse(b"not a dds").is_none());
    }
}
