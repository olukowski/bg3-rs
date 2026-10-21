//! Assembling a texture from its tiles.

use std::borrow::Cow;
use std::collections::HashMap;

use crate::bc::BcFormat;
use crate::pagefile::PageFile;
use crate::tileset::{ParameterKind, TextureInfo, TileData, TileRef, TileSet};
use crate::vfs::PageSource;
use crate::{Error, Result, corrupt};

/// What [`TileSet::extract`] should produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    /// Keep the tiles' block compression (no re-encoding; uniform tiles become solid blocks).
    Bc,
    /// Decode to RGBA8.
    Rgba8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Bc(BcFormat),
    Rgba8,
}

/// A contiguous image: BCn blocks row by row, or RGBA8 pixels row by row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub format: ImageFormat,
    /// Colour data is sRGB-encoded (albedo) rather than linear (normal, physical maps).
    pub srgb: bool,
    pub data: Vec<u8>,
}

impl Image {
    /// Decode to RGBA8 (no-op copy for RGBA images).
    pub fn to_rgba8(&self) -> Vec<u8> {
        match self.format {
            ImageFormat::Rgba8 => self.data.clone(),
            ImageFormat::Bc(f) => f.decode_image(&self.data, self.width, self.height),
        }
    }
}

impl TileSet {
    /// The texture's rectangle `(x, y, width, height)` in pixels of mip `level`.
    pub fn texture_rect(&self, tex: &TextureInfo, level: usize) -> (u32, u32, u32, u32) {
        let s = level as u32;
        (
            tex.x >> s,
            tex.y >> s,
            (tex.width >> s).max(1),
            (tex.height >> s).max(1),
        )
    }

    /// Tile range `(x0, y0, x1, y1)` (inclusive) covering the texture at `level`.
    fn tile_range(&self, tex: &TextureInfo, level: usize) -> (u32, u32, u32, u32) {
        let (x, y, w, h) = self.texture_rect(tex, level);
        let (cw, ch) = (self.content_width(), self.content_height());
        (x / cw, y / ch, (x + w - 1) / cw, (y + h - 1) / ch)
    }

    /// Whether every tile of the texture's `layer` is stored at `level` (rather than only at a
    /// coarser level).
    pub fn is_stored(&self, tex: &TextureInfo, layer: usize, level: usize) -> bool {
        if level >= self.levels.len() {
            return false;
        }
        let (x0, y0, x1, y1) = self.tile_range(tex, level);
        (y0..=y1).all(|y| {
            (x0..=x1).all(|x| matches!(self.tile(level, layer, x, y), Some(TileRef::Stored(_))))
        })
    }

    /// The finest mip level at which the texture's `layer` is fully stored. BG3 stores some
    /// layers (e.g. physical maps) only from level 1 down.
    pub fn finest_stored_level(&self, tex: &TextureInfo, layer: usize) -> Option<usize> {
        (0..self.levels.len()).find(|&l| self.is_stored(tex, layer, l))
    }

    /// The BC format used for `layer`'s tiles (from the parameter blocks of its data type).
    pub fn bc_format(&self, layer: usize) -> Option<BcFormat> {
        let dt = self.layers.get(layer)?.data_type;
        let bc = || {
            self.parameter_blocks.iter().filter_map(|p| match &p.kind {
                ParameterKind::Bc(b) => Some(b),
                _ => None,
            })
        };
        bc().find(|b| b.data_type == dt)
            .or_else(|| bc().next())
            .and_then(|b| b.format)
    }

    /// Read and decode the tile described by flat tile `flat`.
    pub fn read_tile(&self, pages: &impl PageSource, flat: u32) -> Result<TileData> {
        let mut cache = HashMap::new();
        self.read_tile_cached(pages, flat, &mut cache)
    }

    fn read_tile_cached<'p>(
        &self,
        pages: &'p impl PageSource,
        flat: u32,
        cache: &mut HashMap<u16, Cow<'p, [u8]>>,
    ) -> Result<TileData> {
        let info = *self
            .flat_tiles
            .get(flat as usize)
            .ok_or_else(|| corrupt(format!("flat tile {flat} out of range")))?;
        let bytes = match cache.entry(info.page_file) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => {
                let meta = self
                    .page_files
                    .get(info.page_file as usize)
                    .ok_or_else(|| corrupt(format!("page file {} out of range", info.page_file)))?;
                e.insert(pages.page_file(&meta.name)?)
            }
        };
        let file = PageFile::parse(bytes, self.page_size)?;
        let chunk = file.chunk(info.page as usize, info.chunk as usize)?;
        self.decode_chunk(&chunk)
    }

    /// Stitch the texture's `layer` at mip `level` into one image, tile borders stripped.
    ///
    /// For [`OutputFormat::Bc`] the texture's rectangle at that level must start on a 4-pixel
    /// boundary (true for all levels where the texture is at least a few blocks wide).
    pub fn extract(
        &self,
        pages: &impl PageSource,
        tex: &TextureInfo,
        layer: usize,
        level: usize,
        format: OutputFormat,
    ) -> Result<Image> {
        if layer >= self.layers.len() || level >= self.levels.len() {
            return Err(Error::NotFound(format!(
                "{}: layer {layer} level {level}",
                tex.name
            )));
        }
        let (rx, ry, rw, rh) = self.texture_rect(tex, level);
        let (cw, ch) = (self.content_width(), self.content_height());
        let border = self.tile_border;
        let srgb = self.layers[layer].data_type.is_srgb();

        // Work in "cells": 4×4 blocks for BC output, pixels for RGBA output.
        let (out_format, cell, cell_bytes) = match format {
            OutputFormat::Bc => {
                let f = self
                    .bc_format(layer)
                    .ok_or_else(|| Error::Unsupported("layer has no BC format".into()))?;
                if ![rx, ry, cw, ch, border].iter().all(|v| v.is_multiple_of(4)) {
                    return Err(Error::Unsupported(format!(
                        "{} level {level} is not block aligned",
                        tex.name
                    )));
                }
                (ImageFormat::Bc(f), 4, f.block_bytes())
            }
            OutputFormat::Rgba8 => (ImageFormat::Rgba8, 1, 4),
        };
        let out_w = rw.div_ceil(cell) as usize;
        let out_h = rh.div_ceil(cell) as usize;
        let mut data = vec![0u8; out_w * out_h * cell_bytes];
        let tile_cells_w = (self.tile_width / cell) as usize;

        let (x0, y0, x1, y1) = self.tile_range(tex, level);
        let mut cache = HashMap::new();
        for ty in y0..=y1 {
            for tx in x0..=x1 {
                // Intersection of this tile's content with the texture, in level pixels.
                let ix0 = rx.max(tx * cw);
                let iy0 = ry.max(ty * ch);
                let ix1 = (rx + rw).min((tx + 1) * cw);
                let iy1 = (ry + rh).min((ty + 1) * ch);
                let flat = match self.tile(level, layer, tx, ty) {
                    Some(TileRef::Stored(f)) => f,
                    Some(TileRef::Fallback(_)) if out_format == ImageFormat::Rgba8 => {
                        // Upsample (nearest) the covering tile of the coarser level, as a GPU
                        // sampling the coarser mip would show it.
                        let (anc_level, ax, ay, f) = self
                            .fallback_source(level, layer, tx, ty)
                            .ok_or_else(|| corrupt(format!("{}: dangling fallback", tex.name)))?;
                        let s = (anc_level - level) as u32;
                        let rgba = match self.read_tile_cached(pages, f, &mut cache)? {
                            TileData::Bc { format: bc, data } => {
                                bc.decode_image(&data, self.tile_width, self.tile_height)
                            }
                            TileData::Uniform(c) => {
                                c.repeat((self.tile_width * self.tile_height) as usize)
                            }
                        };
                        let (ox, oy) = ((tx - (ax << s)) * cw, (ty - (ay << s)) * ch);
                        for py in iy0..iy1 {
                            let sy = border + ((oy + py - ty * ch) >> s);
                            for px in ix0..ix1 {
                                let sx = border + ((ox + px - tx * cw) >> s);
                                let si = ((sy * self.tile_width + sx) * 4) as usize;
                                let di = (((py - ry) as usize) * out_w + (px - rx) as usize) * 4;
                                data[di..di + 4].copy_from_slice(&rgba[si..si + 4]);
                            }
                        }
                        continue;
                    }
                    Some(TileRef::Fallback(_)) => {
                        return Err(Error::NotFound(format!(
                            "{} layer {layer} is not stored at level {level} (finest stored: {:?}; \
                             RGBA output upsamples instead)",
                            tex.name,
                            self.finest_stored_level(tex, layer)
                        )));
                    }
                    None => {
                        return Err(corrupt(format!(
                            "{}: no tile at level {level} ({tx}, {ty})",
                            tex.name
                        )));
                    }
                };
                let tile = self.read_tile_cached(pages, flat, &mut cache)?;
                let src_x = ((ix0 - tx * cw + border) / cell) as usize;
                let src_y = ((iy0 - ty * ch + border) / cell) as usize;
                let dst_x = ((ix0 - rx) / cell) as usize;
                let dst_y = ((iy0 - ry) / cell) as usize;
                let n_x = ((ix1 - ix0).div_ceil(cell) as usize).min(out_w - dst_x);
                let n_y = ((iy1 - iy0).div_ceil(cell) as usize).min(out_h - dst_y);

                let src: Cow<'_, [u8]> = match (&tile, out_format) {
                    (TileData::Bc { format: f, data }, ImageFormat::Bc(want)) => {
                        if *f != want {
                            return Err(Error::Unsupported(format!(
                                "mixed BC formats in one layer ({f:?} vs {want:?})"
                            )));
                        }
                        Cow::Borrowed(data)
                    }
                    (TileData::Bc { format: f, data }, ImageFormat::Rgba8) => {
                        Cow::Owned(f.decode_image(data, self.tile_width, self.tile_height))
                    }
                    (TileData::Uniform(c), ImageFormat::Bc(want)) => {
                        let block = want.solid_block(*c);
                        for row in 0..n_y {
                            for col in 0..n_x {
                                let o = ((dst_y + row) * out_w + dst_x + col) * cell_bytes;
                                data[o..o + cell_bytes].copy_from_slice(&block);
                            }
                        }
                        continue;
                    }
                    (TileData::Uniform(c), ImageFormat::Rgba8) => {
                        for row in 0..n_y {
                            for col in 0..n_x {
                                let o = ((dst_y + row) * out_w + dst_x + col) * 4;
                                data[o..o + 4].copy_from_slice(c);
                            }
                        }
                        continue;
                    }
                };
                for row in 0..n_y {
                    let s = ((src_y + row) * tile_cells_w + src_x) * cell_bytes;
                    let d = ((dst_y + row) * out_w + dst_x) * cell_bytes;
                    let n = n_x * cell_bytes;
                    let src_row = src
                        .get(s..s + n)
                        .ok_or_else(|| corrupt("tile smaller than its declared size"))?;
                    data[d..d + n].copy_from_slice(src_row);
                }
            }
        }
        Ok(Image {
            width: rw,
            height: rh,
            format: out_format,
            srgb,
            data,
        })
    }

    /// Extract consecutive mip levels starting at `first_level` (or the finest stored level if
    /// that is coarser), stopping when the texture gets smaller than one 4×4 block or stops being
    /// block aligned. Suitable for uploading as a GPU mip chain.
    pub fn extract_mips(
        &self,
        pages: &impl PageSource,
        tex: &TextureInfo,
        layer: usize,
        first_level: usize,
        format: OutputFormat,
    ) -> Result<Vec<Image>> {
        let start =
            first_level.max(self.finest_stored_level(tex, layer).ok_or_else(|| {
                Error::NotFound(format!("{}: layer {layer} not stored", tex.name))
            })?);
        let mut out = Vec::new();
        for level in start..self.levels.len() {
            let (x, y, w, h) = self.texture_rect(tex, level);
            let aligned = x % 4 == 0 && y % 4 == 0;
            if (w < 4 || h < 4 || !aligned) && !out.is_empty() {
                break;
            }
            if !self.is_stored(tex, layer, level) {
                break;
            }
            out.push(self.extract(pages, tex, layer, level, format)?);
        }
        Ok(out)
    }
}
