//! Block-compressed (BCn / DXT) helpers: format identification, block decoding to RGBA8, and
//! encoding of solid-colour blocks (for "uniform" tiles).
//!
//! Decoding follows the public BCn specifications (D3D10+ block compression).

/// A block-compression format. Every format uses 4×4 pixel blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BcFormat {
    /// DXT1: RGB + 1-bit alpha, 8 bytes per block.
    Bc1,
    /// DXT5: BC1 colour + interpolated alpha, 16 bytes per block.
    Bc3,
    /// One interpolated channel, 8 bytes per block.
    Bc4,
    /// Two interpolated channels, 16 bytes per block.
    Bc5,
}

impl BcFormat {
    /// From the FourCC stored in a Granite BC parameter block (e.g. `"BC3 "`).
    pub fn from_fourcc(fourcc: [u8; 4]) -> Option<Self> {
        match &fourcc {
            b"BC1 " | b"DXT1" => Some(Self::Bc1),
            b"BC3 " | b"DXT5" => Some(Self::Bc3),
            b"BC4 " | b"ATI1" => Some(Self::Bc4),
            b"BC5 " | b"ATI2" => Some(Self::Bc5),
            _ => None,
        }
    }

    pub fn block_bytes(self) -> usize {
        match self {
            Self::Bc1 | Self::Bc4 => 8,
            Self::Bc3 | Self::Bc5 => 16,
        }
    }

    /// Bytes for a `width × height` image (dimensions rounded up to whole blocks).
    pub fn image_bytes(self, width: u32, height: u32) -> usize {
        width.div_ceil(4) as usize * height.div_ceil(4) as usize * self.block_bytes()
    }

    /// Decode one block into 16 RGBA8 pixels (row-major).
    pub fn decode_block(self, block: &[u8]) -> [[u8; 4]; 16] {
        let mut px = [[0, 0, 0, 255]; 16];
        match self {
            Self::Bc1 => decode_color(&block[0..8], true, &mut px),
            Self::Bc3 => {
                decode_color(&block[8..16], false, &mut px);
                let a = decode_channel(&block[0..8]);
                for (p, a) in px.iter_mut().zip(a) {
                    p[3] = a;
                }
            }
            Self::Bc4 => {
                let r = decode_channel(&block[0..8]);
                for (p, r) in px.iter_mut().zip(r) {
                    *p = [r, r, r, 255];
                }
            }
            Self::Bc5 => {
                let r = decode_channel(&block[0..8]);
                let g = decode_channel(&block[8..16]);
                for i in 0..16 {
                    px[i] = [r[i], g[i], 0, 255];
                }
            }
        }
        px
    }

    /// A block whose 16 pixels all decode to (approximately) `rgba`.
    pub fn solid_block(self, rgba: [u8; 4]) -> Vec<u8> {
        let color = || {
            let c = rgb565(rgba);
            let mut b = Vec::with_capacity(8);
            b.extend_from_slice(&c.to_le_bytes());
            b.extend_from_slice(&c.to_le_bytes());
            b.extend_from_slice(&[0; 4]); // every index selects c0
            b
        };
        let channel = |v: u8| vec![v, v, 0, 0, 0, 0, 0, 0];
        match self {
            Self::Bc1 => color(),
            Self::Bc3 => [channel(rgba[3]), color()].concat(),
            Self::Bc4 => channel(rgba[0]),
            Self::Bc5 => [channel(rgba[0]), channel(rgba[1])].concat(),
        }
    }

    /// Decode a whole `width × height` image into RGBA8.
    pub fn decode_image(self, data: &[u8], width: u32, height: u32) -> Vec<u8> {
        let (w, h) = (width as usize, height as usize);
        let bw = w.div_ceil(4);
        let mut out = vec![0u8; w * h * 4];
        for (i, block) in data
            .chunks_exact(self.block_bytes())
            .take(bw * h.div_ceil(4))
            .enumerate()
        {
            let (bx, by) = (i % bw, i / bw);
            let px = self.decode_block(block);
            for (j, p) in px.iter().enumerate() {
                let (x, y) = (bx * 4 + j % 4, by * 4 + j / 4);
                if x < w && y < h {
                    let o = (y * w + x) * 4;
                    out[o..o + 4].copy_from_slice(p);
                }
            }
        }
        out
    }
}

fn rgb565(c: [u8; 4]) -> u16 {
    let r = (c[0] as u16 * 31 + 127) / 255;
    let g = (c[1] as u16 * 63 + 127) / 255;
    let b = (c[2] as u16 * 31 + 127) / 255;
    (r << 11) | (g << 5) | b
}

fn expand565(c: u16) -> [u8; 3] {
    let r = ((c >> 11) & 31) as u32;
    let g = ((c >> 5) & 63) as u32;
    let b = (c & 31) as u32;
    [
        ((r * 255 + 15) / 31) as u8,
        ((g * 255 + 31) / 63) as u8,
        ((b * 255 + 15) / 31) as u8,
    ]
}

/// BC1-style colour block. `allow_alpha`: BC1's 3-colour + transparent mode when `c0 <= c1`
/// (BC2/BC3 colour blocks always use 4-colour mode).
fn decode_color(block: &[u8], allow_alpha: bool, px: &mut [[u8; 4]; 16]) {
    let c0 = u16::from_le_bytes([block[0], block[1]]);
    let c1 = u16::from_le_bytes([block[2], block[3]]);
    let (a, b) = (expand565(c0), expand565(c1));
    let mix = |wa: u32, wb: u32, d: u32| -> [u8; 4] {
        let f = |i: usize| ((a[i] as u32 * wa + b[i] as u32 * wb) / d) as u8;
        [f(0), f(1), f(2), 255]
    };
    let palette = if c0 > c1 || !allow_alpha {
        [mix(1, 0, 1), mix(0, 1, 1), mix(2, 1, 3), mix(1, 2, 3)]
    } else {
        [mix(1, 0, 1), mix(0, 1, 1), mix(1, 1, 2), [0, 0, 0, 0]]
    };
    let bits = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
    for (i, p) in px.iter_mut().enumerate() {
        let idx = ((bits >> (2 * i)) & 3) as usize;
        p[..3].copy_from_slice(&palette[idx][..3]);
        if allow_alpha {
            p[3] = palette[idx][3];
        }
    }
}

/// BC4-style single interpolated channel (also the BC3 alpha block).
fn decode_channel(block: &[u8]) -> [u8; 16] {
    let (v0, v1) = (block[0] as u32, block[1] as u32);
    let mut pal = [0u32; 8];
    pal[0] = v0;
    pal[1] = v1;
    if v0 > v1 {
        for i in 1..7 {
            pal[i + 1] = ((7 - i) as u32 * v0 + i as u32 * v1) / 7;
        }
    } else {
        for i in 1..5 {
            pal[i + 1] = ((5 - i) as u32 * v0 + i as u32 * v1) / 5;
        }
        pal[6] = 0;
        pal[7] = 255;
    }
    let mut bits = 0u64;
    for (i, &b) in block[2..8].iter().enumerate() {
        bits |= (b as u64) << (8 * i);
    }
    let mut out = [0u8; 16];
    for (i, o) in out.iter_mut().enumerate() {
        *o = pal[((bits >> (3 * i)) & 7) as usize] as u8;
    }
    out
}
