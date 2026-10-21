//! Re-pack BG3 texture maps into the channel layouts of a glTF-style PBR material.
//!
//! BG3 layouts (see `docs/research/materials.md`):
//! - normal map (`_NM`, BC3): tangent-space X in **A**, Y in **G**, Z in **B**, R unused;
//! - physical map (`_PM` BC1 / `_PMA` BC3): **R** metalness, **G** roughness, **B** occlusion.
//!
//! glTF/Bevy want normal XY in RG (BC5: the BC3 alpha block is copied, green re-encoded) and
//! metallic-roughness as G = roughness, B = metalness with occlusion in R — so the physical map
//! needs only an R↔B swap, done losslessly on the blocks. The virtual textures' normal and
//! physical layers use the same layouts.

use crate::bc;
use crate::dds::{DdsFormat, DdsHeader};

/// Pixel data for an upload: format, size and all mip levels concatenated (largest first).
#[derive(Debug, Clone, PartialEq)]
pub struct Texture {
    pub format: TexelFormat,
    pub width: u32,
    pub height: u32,
    pub mip_count: u32,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TexelFormat {
    /// Block-compressed, uploaded as is.
    Bc(DdsFormat),
    Rg8,
    Rgba8,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConvertError {
    #[error("not a DDS file")]
    NotDds,
    #[error("unsupported DDS format {0:?}")]
    Unsupported(DdsFormat),
    #[error("cube maps, arrays and volumes are not supported")]
    NotPlain2d,
    #[error("truncated DDS data")]
    Truncated,
}

/// `(width, height, bytes)` of one mip level.
type Level<'a> = (u32, u32, &'a [u8]);

/// Parse a 2D block-compressed DDS and return its header and complete mip levels.
fn levels(dds: &[u8]) -> Result<(DdsHeader, Vec<Level<'_>>), ConvertError> {
    let h = DdsHeader::parse(dds).ok_or(ConvertError::NotDds)?;
    if h.cubemap || h.array_size > 1 || h.depth > 1 {
        return Err(ConvertError::NotPlain2d);
    }
    let ranges = h.mip_levels().ok_or(ConvertError::Unsupported(h.format))?;
    // Keep the complete levels only; a short file loses its smallest mips.
    let levels: Vec<_> = ranges
        .into_iter()
        .map_while(|(w, ht, r)| Some((w, ht, dds.get(r)?)))
        .collect();
    if levels.is_empty() {
        return Err(ConvertError::Truncated);
    }
    Ok((h, levels))
}

/// Upload-ready texture with the data unchanged (BC1–BC7). Falls back to RGBA8 when the base
/// level isn't a whole number of blocks (GPU APIs reject that for compressed formats).
pub fn passthrough(dds: &[u8]) -> Result<Texture, ConvertError> {
    let (h, levels) = levels(dds)?;
    if h.width % 4 != 0 || h.height % 4 != 0 {
        return decoded_rgba(&h, &levels[..1]);
    }
    Ok(Texture {
        format: TexelFormat::Bc(h.format),
        width: h.width,
        height: h.height,
        mip_count: levels.len() as u32,
        data: levels.iter().flat_map(|l| l.2).copied().collect(),
    })
}

/// Decode every mip level to RGBA8 (single-channel BC4 becomes grey).
pub fn rgba(dds: &[u8]) -> Result<Texture, ConvertError> {
    let (h, levels) = levels(dds)?;
    decoded_rgba(&h, &levels)
}

fn decoded_rgba(h: &DdsHeader, levels: &[Level<'_>]) -> Result<Texture, ConvertError> {
    let mut data = Vec::new();
    for &(w, ht, bytes) in levels {
        data.extend(
            bc::decode_level(h.format, w, ht, bytes).ok_or(ConvertError::Unsupported(h.format))?,
        );
    }
    Ok(Texture {
        format: TexelFormat::Rgba8,
        width: h.width,
        height: h.height,
        mip_count: levels.len() as u32,
        data,
    })
}

/// BG3 normal map (DDS) → two-channel (X in red, Y in green); see [`normal_texture`].
pub fn normal_map(dds: &[u8]) -> Result<Texture, ConvertError> {
    normal_texture(passthrough(dds)?)
}

/// Re-pack a BG3 normal map (X in alpha, Y in green; also the virtual textures' normal layer)
/// as two channels so the renderer reconstructs Z: BC3 → BC5 block by block, BC5 as is, anything
/// else decoded to RG8.
pub fn normal_texture(mut t: Texture) -> Result<Texture, ConvertError> {
    match t.format {
        TexelFormat::Bc(DdsFormat::Bc3) => {
            for block in t.data.as_chunks_mut::<16>().0 {
                *block = bc::bc3_normal_to_bc5(block);
            }
            t.format = TexelFormat::Bc(DdsFormat::Bc5);
        }
        TexelFormat::Bc(DdsFormat::Bc5) => {}
        TexelFormat::Bc(f @ DdsFormat::Bc1) => {
            // No alpha: X can only be in red.
            t = rg8_from(t, f, |p| [p[0], p[1]])?;
        }
        TexelFormat::Rgba8 => {
            t.data = t
                .data
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| [p[3], p[1]])
                .collect();
            t.format = TexelFormat::Rg8;
        }
        TexelFormat::Bc(f) => return Err(ConvertError::Unsupported(f)),
        TexelFormat::Rg8 => {}
    }
    Ok(t)
}

fn rg8_from(
    t: Texture,
    format: DdsFormat,
    pick: fn(&[u8; 4]) -> [u8; 2],
) -> Result<Texture, ConvertError> {
    let block = bc::block_bytes(format).ok_or(ConvertError::Unsupported(format))?;
    let mut data = Vec::new();
    let mut off = 0;
    for level in 0..t.mip_count {
        let (w, h) = ((t.width >> level).max(1), (t.height >> level).max(1));
        let len = bc::mip_bytes(w, h, block);
        let px = t
            .data
            .get(off..off + len)
            .and_then(|b| bc::decode_level(format, w, h, b))
            .ok_or(ConvertError::Truncated)?;
        data.extend(px.as_chunks::<4>().0.iter().flat_map(pick));
        off += len;
    }
    Ok(Texture {
        format: TexelFormat::Rg8,
        data,
        ..t
    })
}

/// BG3 physical map (DDS) → occlusion (R), roughness (G), metalness (B); see [`physical_texture`].
pub fn physical_map(dds: &[u8]) -> Result<Texture, ConvertError> {
    physical_texture(passthrough(dds)?)
}

/// Swap R and B of a physical map (also the virtual textures' physical layer), exactly.
pub fn physical_texture(mut t: Texture) -> Result<Texture, ConvertError> {
    match t.format {
        TexelFormat::Bc(DdsFormat::Bc1) => t
            .data
            .as_chunks_mut::<8>()
            .0
            .iter_mut()
            .for_each(|b| bc::swap_red_blue_bc1(b, false)),
        TexelFormat::Bc(DdsFormat::Bc3) => t
            .data
            .as_chunks_mut::<16>()
            .0
            .iter_mut()
            .for_each(|b| bc::swap_red_blue_bc1(&mut b[8..], true)),
        TexelFormat::Rgba8 => t
            .data
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .for_each(|p| p.swap(0, 2)),
        TexelFormat::Bc(f) => return Err(ConvertError::Unsupported(f)),
        TexelFormat::Rg8 => return Err(ConvertError::Unsupported(DdsFormat::Bc5)),
    }
    Ok(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Self-authored legacy DDS: `w`×`h`, `fourcc`, one level of `blocks` repeated.
    fn dds(w: u32, h: u32, fourcc: &[u8; 4], block: &[u8]) -> Vec<u8> {
        let mut b = vec![0u8; 128];
        b[..4].copy_from_slice(b"DDS ");
        b[4..8].copy_from_slice(&124u32.to_le_bytes());
        b[12..16].copy_from_slice(&h.to_le_bytes());
        b[16..20].copy_from_slice(&w.to_le_bytes());
        b[28..32].copy_from_slice(&1u32.to_le_bytes());
        b[80..84].copy_from_slice(&4u32.to_le_bytes());
        b[84..88].copy_from_slice(fourcc);
        for _ in 0..(w.div_ceil(4) * h.div_ceil(4)) {
            b.extend_from_slice(block);
        }
        b
    }

    /// A BC3 block of one colour: alpha `a`, RGB565 `c`.
    fn bc3_solid(a: u8, c: u16) -> Vec<u8> {
        let mut b = vec![a, a, 0, 0, 0, 0, 0, 0];
        b.extend_from_slice(&c.to_le_bytes());
        b.extend_from_slice(&c.to_le_bytes());
        b.extend_from_slice(&[0; 4]);
        b
    }

    #[test]
    fn normal_map_takes_x_from_alpha_and_y_from_green() {
        // G = 32/63 → 130, alpha 200, R = 0, B = 31 → 255.
        let colour = (32 << 5) | 31;
        let t = normal_map(&dds(4, 4, b"DXT5", &bc3_solid(200, colour))).unwrap();
        assert_eq!(t.format, TexelFormat::Bc(DdsFormat::Bc5));
        assert_eq!(t.data.len(), 16);
        assert_eq!(bc::decode_bc4(&t.data[..8])[0], 200);
        assert_eq!(bc::decode_bc4(&t.data[8..])[0], 130);
        // Uncompressed fallback keeps the same mapping.
        let rgba = Texture {
            format: TexelFormat::Rgba8,
            width: 1,
            height: 1,
            mip_count: 1,
            data: vec![0, 130, 255, 200],
        };
        assert_eq!(normal_texture(rgba).unwrap().data, [200, 130]);
    }

    #[test]
    fn physical_map_swaps_red_and_blue() {
        // R = 31 (metal 1), G = 0, B = 0 → after swap B = metal.
        let t = physical_map(&dds(4, 4, b"DXT5", &bc3_solid(255, 0xf800))).unwrap();
        let px = bc::decode_bc3(&t.data[..16]);
        assert_eq!(px[0], [0, 0, 255, 255]);
    }

    #[test]
    fn odd_sizes_decode_to_rgba() {
        let t = passthrough(&dds(2, 2, b"DXT5", &bc3_solid(255, 0xffff))).unwrap();
        assert_eq!(t.format, TexelFormat::Rgba8);
        assert_eq!(t.data, vec![255; 16]);
    }

    #[test]
    fn truncated_files_are_rejected() {
        let mut d = dds(8, 8, b"DXT1", &[0; 8]);
        d.truncate(130);
        assert_eq!(passthrough(&d), Err(ConvertError::Truncated));
    }
}
