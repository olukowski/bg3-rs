//! `NavCloud/navcloud.meta` + `navcloud.data`: the 3D voxel navigation volume of a level.
//!
//! Not used for walking (that is the AI grid). The game's `navcloud::PathfindingSystem` serves
//! path requests from spell/throw previews, spell casting and following (see
//! `docs/formats/navcloud.md`). The volume is split into 6 m cubic regions keyed by
//! `floor(position / 6)`; a region is either coarse (2³ voxels of 3 m) or fine (6³ voxels of 1 m),
//! one byte per voxel.

use std::collections::HashMap;

use crate::bytes::Reader;
use crate::{Error, Result};

/// Edge length of a region in metres.
// bg3: (anonymous)::ToTilePos (navcloud, 4.1.1.7631656)
pub const REGION_SIZE: f32 = 6.0;

/// The only meta version seen in the reference build.
pub const META_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq)]
pub struct Region {
    /// Region coordinate (`floor(world / 6)`).
    pub key: [i32; 3],
    /// Voxel layout: 0 = coarse (2 per axis), 1 = fine (6 per axis).
    pub kind: u32,
    /// `n³` voxel bytes, index `(z * n + y) * n + x`.
    pub voxels: Vec<u8>,
}

impl Region {
    /// Voxels per axis.
    // bg3: navcloud::TryGetTileInfo (4.1.1.7631656)
    pub fn resolution(&self) -> usize {
        match self.kind {
            0 => 2,
            1 => 6,
            _ => 0,
        }
    }

    pub fn voxel_size(&self) -> f32 {
        REGION_SIZE / self.resolution().max(1) as f32
    }

    /// Voxel byte at a world position inside this region.
    pub fn voxel_at(&self, p: [f32; 3]) -> Option<u8> {
        let n = self.resolution();
        let s = self.voxel_size();
        let mut idx = [0usize; 3];
        for a in 0..3 {
            let local = p[a] - self.key[a] as f32 * REGION_SIZE;
            let i = (local / s).floor();
            if !(0.0..n as f32).contains(&i) {
                return None;
            }
            idx[a] = i as usize;
        }
        self.voxels.get((idx[2] * n + idx[1]) * n + idx[0]).copied()
    }
}

#[derive(Debug, Clone, Default)]
pub struct NavCloud {
    pub regions: Vec<Region>,
    by_key: HashMap<[i32; 3], usize>,
}

impl NavCloud {
    /// Parse `navcloud.meta` (region index) and `navcloud.data` (region payloads).
    pub fn parse(meta: &[u8], data: &[u8]) -> Result<Self> {
        let mut m = Reader::new(meta);
        let version = m.u32()?;
        if version != META_VERSION {
            return Err(Error::Version(version));
        }
        let count = m.u32()?;
        let mut regions = Vec::with_capacity(count.min(1 << 20) as usize);
        for _ in 0..count {
            let key = [m.i32()?, m.i32()?, m.i32()?];
            let offset = m.u64()? as usize;
            let size = m.u64()? as usize;
            let bytes = data
                .get(offset..offset.saturating_add(size))
                .ok_or(Error::Truncated(offset))?;
            let mut r = Reader::new(bytes);
            let kind = r.u32()?;
            let len = r.u32()? as usize;
            let voxels = r.take(len)?.to_vec();
            let region = Region { key, kind, voxels };
            let n = region.resolution();
            if n == 0 || len != n * n * n {
                return Err(Error::Size {
                    what: "navcloud region",
                    expected: n * n * n,
                    got: len,
                });
            }
            regions.push(region);
        }
        let by_key = regions
            .iter()
            .enumerate()
            .map(|(i, r)| (r.key, i))
            .collect();
        Ok(Self { regions, by_key })
    }

    pub fn region(&self, key: [i32; 3]) -> Option<&Region> {
        self.by_key.get(&key).map(|&i| &self.regions[i])
    }

    /// Voxel byte at a world position (`None` outside every region).
    pub fn voxel_at(&self, p: [f32; 3]) -> Option<u8> {
        let key = p.map(|v| (v / REGION_SIZE).floor() as i32);
        self.region(key)?.voxel_at(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region_bytes(kind: u32, voxels: &[u8]) -> Vec<u8> {
        let mut b = kind.to_le_bytes().to_vec();
        b.extend((voxels.len() as u32).to_le_bytes());
        b.extend(voxels);
        b
    }

    #[test]
    fn parses_coarse_and_fine_regions() {
        let coarse = region_bytes(0, &[1; 8]);
        let mut fine_vox = vec![0u8; 216];
        fine_vox[(2 * 6 + 1) * 6 + 3] = 7; // x=3, y=1, z=2
        let fine = region_bytes(1, &fine_vox);
        let mut data = coarse.clone();
        data.extend(&fine);
        let mut meta = 1u32.to_le_bytes().to_vec();
        meta.extend(2u32.to_le_bytes());
        for (key, off, size) in [
            ([-1i32, 0, -2], 0u64, coarse.len() as u64),
            ([0, 0, 0], coarse.len() as u64, fine.len() as u64),
        ] {
            for k in key {
                meta.extend(k.to_le_bytes());
            }
            meta.extend(off.to_le_bytes());
            meta.extend(size.to_le_bytes());
        }
        let nc = NavCloud::parse(&meta, &data).unwrap();
        assert_eq!(nc.regions.len(), 2);
        assert_eq!(nc.voxel_at([-0.5, 3.0, -7.0]), Some(1));
        assert_eq!(nc.voxel_at([3.5, 1.2, 2.9]), Some(7));
        assert_eq!(nc.voxel_at([3.5, 1.2, 1.9]), Some(0));
        assert_eq!(nc.voxel_at([100.0, 0.0, 0.0]), None);
    }
}
