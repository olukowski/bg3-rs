//! Body cut masks (`VertexCut`, `SupportsVertexColorMask`).
//!
//! The vertex shaders of `CHAR_Skin_Body`, `CHAR_Skin_Head_Creature` and the `*_VertCut_*`
//! clothing graphs read the vertex colour's green channel `g` (a byte, `g·255`), point-sample a
//! 16×16 mask texture (`VertexColor_MSK`, bound per character by the engine) at
//! `(frac(16g), floor(16g)/16)` — texel `(byte & 15, byte >> 4)` — and move the vertex to the
//! object origin when the sample's green is ≥ 0.01, which collapses its triangles. The bytes
//! to hide are the `MapValue`s (`VisualSlots.lsx`, `NakedBodySystem`) of the regions worn
//! visuals list in `VertexColorMaskSlots`.
//!
//! bg3: ls::VisualSetSlots::UpdateVertexColorMaskTexture (4.1.1.7631656);
//! Shared_Characters_CHAR_Skin_Body_ST_DEF_vertexMain (Materials.metallib)

/// The texel of the 16×16 mask a vertex-colour green channel selects, as the shader computes
/// it (`frac`/`floor` of `16g` with `g = byte/255`, wrapping at the texture edge).
pub fn mask_texel(green: f32) -> (u32, u32) {
    let x = green * 16.0;
    let col = ((x - x.floor()) * 16.0).floor() as u32 % 16;
    let row = x.floor() as u32 % 16;
    (col, row)
}

/// The 16×16 mask for a set of region bytes: `true` where a vertex is cut.
pub fn mask_texture(bytes: impl IntoIterator<Item = u8>) -> [bool; 256] {
    let mut m = [false; 256];
    for b in bytes {
        m[((b >> 4) as usize) * 16 + (b & 15) as usize] = true;
    }
    m
}

/// Whether a vertex with this green channel is cut by `mask`.
pub fn is_cut(mask: &[bool; 256], green: f32) -> bool {
    let (c, r) = mask_texel(green);
    mask[(r * 16 + c) as usize]
}

/// Remove the triangles the game collapses: those whose three vertices are all cut.
/// Triangles with only some cut vertices would be stretched to the object origin in the
/// game; they are kept (and counted) here. Returns `(removed, partial)`.
pub fn cut_triangles(indices: &mut Vec<u32>, cut: &[bool]) -> (usize, usize) {
    let before = indices.len() / 3;
    let mut partial = 0;
    let mut out = Vec::with_capacity(indices.len());
    for t in indices.as_chunks::<3>().0 {
        let n = t
            .iter()
            .filter(|&&i| cut.get(i as usize).copied().unwrap_or(false))
            .count();
        if n == 3 {
            continue;
        }
        if n > 0 {
            partial += 1;
        }
        out.extend_from_slice(t);
    }
    *indices = out;
    (before - indices.len() / 3, partial)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_byte_selects_its_own_texel() {
        for b in 0u8..=254 {
            let (c, r) = mask_texel(b as f32 / 255.0);
            assert_eq!((c, r), ((b & 15) as u32, (b >> 4) as u32), "byte {b}");
        }
        // 255 wraps to the first texel.
        assert_eq!(mask_texel(1.0), (0, 0));
    }

    #[test]
    fn masks_cut_only_their_regions() {
        let m = mask_texture([50, 185]); // Torso, Thighs
        assert!(is_cut(&m, 50.0 / 255.0));
        assert!(is_cut(&m, 185.0 / 255.0));
        assert!(!is_cut(&m, 0.0));
        assert!(!is_cut(&m, 36.0 / 255.0));
    }

    #[test]
    fn only_fully_cut_triangles_are_removed() {
        let mut idx = vec![0, 1, 2, 2, 3, 4, 5, 6, 7];
        let cut = [true, true, true, true, false, false, false, false];
        assert_eq!(cut_triangles(&mut idx, &cut), (1, 1));
        assert_eq!(idx, [2, 3, 4, 5, 6, 7]);
    }
}
