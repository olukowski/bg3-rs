//! Block-compression helpers: decode BC1/BC3/BC4/BC5 blocks and re-pack BG3 maps into the
//! channel layouts a standard PBR renderer expects.
//!
//! Decoding follows the public D3D10+ block-compression specification.

/// Expand an RGB565 colour to 8-bit channels.
fn rgb565(c: u16) -> [u8; 3] {
    let r = (c >> 11) & 31;
    let g = (c >> 5) & 63;
    let b = c & 31;
    [
        ((r << 3) | (r >> 2)) as u8,
        ((g << 2) | (g >> 4)) as u8,
        ((b << 3) | (b >> 2)) as u8,
    ]
}

/// Decode a BC1 colour block. `always_four` forces 4-colour mode (BC2/BC3 colour blocks).
pub fn decode_bc1(block: &[u8], always_four: bool) -> [[u8; 4]; 16] {
    let c0 = u16::from_le_bytes([block[0], block[1]]);
    let c1 = u16::from_le_bytes([block[2], block[3]]);
    let (a, b) = (rgb565(c0), rgb565(c1));
    let mix = |wa: u16, wb: u16, d: u16| -> [u8; 4] {
        let f = |i: usize| ((a[i] as u16 * wa + b[i] as u16 * wb) / d) as u8;
        [f(0), f(1), f(2), 255]
    };
    let palette = if always_four || c0 > c1 {
        [
            [a[0], a[1], a[2], 255],
            [b[0], b[1], b[2], 255],
            mix(2, 1, 3),
            mix(1, 2, 3),
        ]
    } else {
        [
            [a[0], a[1], a[2], 255],
            [b[0], b[1], b[2], 255],
            mix(1, 1, 2),
            [0, 0, 0, 0],
        ]
    };
    let bits = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
    std::array::from_fn(|i| palette[((bits >> (2 * i)) & 3) as usize])
}

/// Decode a BC4 block (also the alpha block of BC3 and each half of BC5).
pub fn decode_bc4(block: &[u8]) -> [u8; 16] {
    let (e0, e1) = (block[0] as u32, block[1] as u32);
    let mut palette = [0u8; 8];
    palette[0] = e0 as u8;
    palette[1] = e1 as u8;
    if e0 > e1 {
        for i in 1..7 {
            palette[i + 1] = (((7 - i as u32) * e0 + i as u32 * e1) / 7) as u8;
        }
    } else {
        for i in 1..5 {
            palette[i + 1] = (((5 - i as u32) * e0 + i as u32 * e1) / 5) as u8;
        }
        palette[6] = 0;
        palette[7] = 255;
    }
    let mut bits = 0u64;
    for (i, &b) in block[2..8].iter().enumerate() {
        bits |= (b as u64) << (8 * i);
    }
    std::array::from_fn(|i| palette[((bits >> (3 * i)) & 7) as usize])
}

/// Decode a BC3 block (BC4-style alpha + 4-colour BC1).
pub fn decode_bc3(block: &[u8]) -> [[u8; 4]; 16] {
    let alpha = decode_bc4(&block[..8]);
    let mut px = decode_bc1(&block[8..16], true);
    for (p, a) in px.iter_mut().zip(alpha) {
        p[3] = a;
    }
    px
}

/// Swap the red and blue channels of a BC1 colour block without re-encoding (exact).
///
/// `always_four` is set for BC2/BC3 colour blocks, whose mode doesn't depend on endpoint order.
pub fn swap_red_blue_bc1(block: &mut [u8], always_four: bool) {
    let swap = |c: u16| ((c & 31) << 11) | (c & 0x07e0) | (c >> 11);
    let c0 = u16::from_le_bytes([block[0], block[1]]);
    let c1 = u16::from_le_bytes([block[2], block[3]]);
    let (mut n0, mut n1) = (swap(c0), swap(c1));
    let mut bits = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
    // The swap can change which endpoint compares greater, i.e. the block mode. Swapping the
    // endpoints and remapping the indices keeps the decoded palette identical.
    if !always_four && (c0 > c1) != (n0 > n1) {
        std::mem::swap(&mut n0, &mut n1);
        let four = c0 > c1;
        let mut out = 0u32;
        for i in 0..16 {
            let idx = (bits >> (2 * i)) & 3;
            let new = match (idx, four) {
                (0, _) => 1,
                (1, _) => 0,
                (2, true) => 3,
                (3, true) => 2,
                (other, false) => other,
                _ => unreachable!(),
            };
            out |= new << (2 * i);
        }
        bits = out;
    }
    block[0..2].copy_from_slice(&n0.to_le_bytes());
    block[2..4].copy_from_slice(&n1.to_le_bytes());
    block[4..8].copy_from_slice(&bits.to_le_bytes());
}

/// Encode 16 values as a BC4 block: min/max endpoints (8-value mode), nearest index.
/// Error is at most 1/14 of the block's value range.
pub fn encode_bc4(values: &[u8; 16]) -> [u8; 8] {
    let (lo, hi) = values
        .iter()
        .fold((255u8, 0u8), |(lo, hi), &v| (lo.min(v), hi.max(v)));
    let mut block = [0u8; 8];
    block[0] = hi;
    block[1] = lo;
    if hi == lo {
        return block; // all indices 0 → hi
    }
    let span = (hi - lo) as u32;
    let mut bits = 0u64;
    for (i, &v) in values.iter().enumerate() {
        // Step k = 0 (hi) … 7 (lo) along the 8-value ramp.
        let k = (((hi - v) as u32 * 7 + span / 2) / span).min(7);
        let index = match k {
            0 => 0,
            7 => 1,
            k => k + 1,
        } as u64;
        bits |= index << (3 * i);
    }
    block[2..8].copy_from_slice(&bits.to_le_bytes()[..6]);
    block
}

/// Re-pack a BG3 BC3 normal-map block (X in alpha, Y in green) as a BC5 block (X in red, Y in
/// green). Red is the alpha block copied exactly; green is re-encoded from the decoded values.
pub fn bc3_normal_to_bc5(block: &[u8]) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&block[..8]);
    let green = decode_bc1(&block[8..16], true).map(|p| p[1]);
    out[8..].copy_from_slice(&encode_bc4(&green));
    out
}

/// Bytes per 4×4 block.
pub fn block_bytes(format: crate::dds::DdsFormat) -> Option<usize> {
    use crate::dds::DdsFormat::*;
    match format {
        Bc1 | Bc4 => Some(8),
        Bc2 | Bc3 | Bc5 | Bc6h | Bc6hSigned | Bc7 => Some(16),
        _ => None,
    }
}

/// Size in bytes of one mip level of a block-compressed texture.
pub fn mip_bytes(width: u32, height: u32, block: usize) -> usize {
    (width.div_ceil(4).max(1) * height.div_ceil(4).max(1)) as usize * block
}

/// Decode a whole BC1/BC3/BC4/BC5 mip level to RGBA8 (BC4 → grey; BC5 → RG, B = 0).
pub fn decode_level(
    format: crate::dds::DdsFormat,
    width: u32,
    height: u32,
    data: &[u8],
) -> Option<Vec<u8>> {
    use crate::dds::DdsFormat::*;
    let bb = block_bytes(format)?;
    let (bw, bh) = (width.div_ceil(4) as usize, height.div_ceil(4) as usize);
    if data.len() < bw * bh * bb {
        return None;
    }
    let (w, h) = (width as usize, height as usize);
    let mut out = vec![0u8; w * h * 4];
    for by in 0..bh {
        for bx in 0..bw {
            let block = &data[(by * bw + bx) * bb..][..bb];
            let px: [[u8; 4]; 16] = match format {
                Bc1 => decode_bc1(block, false),
                Bc3 => decode_bc3(block),
                Bc4 => decode_bc4(block).map(|v| [v, v, v, 255]),
                Bc5 => {
                    let (r, g) = (decode_bc4(&block[..8]), decode_bc4(&block[8..]));
                    std::array::from_fn(|i| [r[i], g[i], 0, 255])
                }
                _ => return None,
            };
            for (i, p) in px.iter().enumerate() {
                let (x, y) = (bx * 4 + i % 4, by * 4 + i / 4);
                if x < w && y < h {
                    out[(y * w + x) * 4..][..4].copy_from_slice(p);
                }
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bc1(c0: u16, c1: u16, bits: u32) -> [u8; 8] {
        let mut b = [0u8; 8];
        b[0..2].copy_from_slice(&c0.to_le_bytes());
        b[2..4].copy_from_slice(&c1.to_le_bytes());
        b[4..8].copy_from_slice(&bits.to_le_bytes());
        b
    }

    #[test]
    fn bc1_four_and_three_colour_modes() {
        // Pure red (c0) vs pure blue (c1): 4-colour mode since 0xf800 > 0x001f.
        let px = decode_bc1(&bc1(0xf800, 0x001f, 0b11_10_01_00), false);
        assert_eq!(px[0], [255, 0, 0, 255]);
        assert_eq!(px[1], [0, 0, 255, 255]);
        assert_eq!(px[2], [170, 0, 85, 255]);
        assert_eq!(px[3], [85, 0, 170, 255]);
        // Swapped endpoints: 3-colour mode with transparent black.
        let px = decode_bc1(&bc1(0x001f, 0xf800, 0b11_10), false);
        assert_eq!(px[1], [0, 0, 0, 0]);
        assert_eq!(px[0], [127, 0, 127, 255]);
    }

    #[test]
    fn bc4_eight_and_six_value_modes() {
        let mut b = [0u8; 8];
        b[0] = 255;
        b[1] = 0;
        b[2] = 0b0001_0000; // texel 0 = idx 0, texel 1 = idx 2
        let v = decode_bc4(&b);
        assert_eq!(v[0], 255);
        assert_eq!(v[1], 218); // (6·255 + 0) / 7
        b[0] = 0;
        b[1] = 255;
        b[2] = 0b0011_1111; // texel 0 = idx 7, texel 1 = idx 7 (low bits)
        let v = decode_bc4(&b);
        assert_eq!(v[0], 255, "idx 7 is 255 in 6-value mode");
    }

    #[test]
    fn red_blue_swap_is_exact_in_every_mode() {
        let cases = [
            bc1(0xf800, 0x001f, 0xe4e4_e4e4),
            bc1(0x001f, 0xf800, 0x1b1b_1b1b),
            bc1(0x8410, 0x8410, 0x0123_4567),
            bc1(0x1234, 0xfedc, 0x89ab_cdef),
            bc1(0xa5a5, 0x5a5a, 0xffff_0000),
        ];
        for case in cases {
            for four in [false, true] {
                let before = decode_bc1(&case, four);
                let mut swapped = case;
                swap_red_blue_bc1(&mut swapped, four);
                let after = decode_bc1(&swapped, four);
                for (b, a) in before.iter().zip(after) {
                    assert_eq!([b[2], b[1], b[0], b[3]], a, "{case:?} four={four}");
                }
            }
        }
    }

    #[test]
    fn bc4_encode_round_trip_is_close() {
        let values: [u8; 16] = std::array::from_fn(|i| (i * 13 + 40) as u8);
        let decoded = decode_bc4(&encode_bc4(&values));
        let span = (values[15] - values[0]) as i32;
        for (a, b) in values.iter().zip(decoded) {
            assert!((*a as i32 - b as i32).abs() * 14 <= span + 14, "{a} vs {b}");
        }
        assert_eq!(decode_bc4(&encode_bc4(&[77; 16])), [77; 16]);
    }

    #[test]
    fn bc3_normal_becomes_bc5_with_x_in_red() {
        let mut bc3 = [0u8; 16];
        bc3[0] = 200;
        bc3[1] = 200; // alpha = 200 everywhere
        let green = (32u16 << 5).to_le_bytes(); // G = 130
        bc3[8..10].copy_from_slice(&green);
        bc3[10..12].copy_from_slice(&green);
        let bc5 = bc3_normal_to_bc5(&bc3);
        assert_eq!(decode_bc4(&bc5[..8]), [200; 16]);
        assert_eq!(decode_bc4(&bc5[8..]), [130; 16]);
    }

    #[test]
    fn decodes_partial_blocks() {
        let level =
            decode_level(crate::dds::DdsFormat::Bc1, 2, 2, &bc1(0xffff, 0xffff, 0)).unwrap();
        assert_eq!(level, [255u8; 16]);
    }
}
