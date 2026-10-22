//! Fog volumes: local volumetric fog placed in a level (`FogVolumes/_merged.lsf`,
//! `Type=fogVolume`), sparse over their root template (`Shared_FogVolume`). See
//! `docs/research/lighting.md` ("Fog").

use std::collections::HashMap;

use bg3_lsf::Node;
use bg3_vfs::Vfs;
use bg3_vfs::modules::BASE_LOAD_ORDER;

use crate::level::read_transform;
use crate::{Result, RootTemplates, Transform, parse, str_attr};

/// Fog volume fields as stored on one node; `None` = inherit.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FogProps {
    pub enabled: Option<bool>,
    /// 0 = sphere (`Radius`), 1 = box (`Size`).
    pub shape: Option<u8>,
    pub radius: Option<f32>,
    pub size: Option<[f32; 3]>,
    pub transition_distance: Option<f32>,
    pub density: Option<f32>,
    pub albedo: Option<[f32; 3]>,
    pub replaces: Option<bool>,
    pub noise_coverage: Option<f32>,
}

fn vec3(n: &Node, name: &str) -> Option<[f32; 3]> {
    let c = n.attr(name)?.components()?;
    (c.len() == 3).then(|| [c[0] as f32, c[1] as f32, c[2] as f32])
}

impl FogProps {
    pub fn read(n: &Node) -> Self {
        let f = |name| n.attr(name).and_then(|v| v.as_f64()).map(|v| v as f32);
        let b = |name| n.attr(name).and_then(|v| v.as_bool());
        Self {
            enabled: b("Enabled"),
            shape: n
                .attr("FogVolumeShape")
                .and_then(|v| v.as_i64())
                .map(|v| v as u8),
            radius: f("Radius"),
            size: vec3(n, "Size"),
            transition_distance: f("TransitionDistance"),
            density: f("Density"),
            albedo: vec3(n, "Albedo"),
            replaces: b("Replaces"),
            noise_coverage: f("NoiseCoverage"),
        }
    }

    fn inherit(self, p: &Self) -> Self {
        Self {
            enabled: self.enabled.or(p.enabled),
            shape: self.shape.or(p.shape),
            radius: self.radius.or(p.radius),
            size: self.size.or(p.size),
            transition_distance: self.transition_distance.or(p.transition_distance),
            density: self.density.or(p.density),
            albedo: self.albedo.or(p.albedo),
            replaces: self.replaces.or(p.replaces),
            noise_coverage: self.noise_coverage.or(p.noise_coverage),
        }
    }
}

/// A placed fog volume with its fields resolved.
#[derive(Debug, Clone)]
pub struct FogVolume {
    pub id: String,
    pub name: String,
    pub transform: Transform,
    pub enabled: bool,
    /// 0 = sphere, 1 = box.
    pub shape: u8,
    pub radius: f32,
    pub size: [f32; 3],
    /// Fade length at the edge (m).
    pub transition_distance: f32,
    /// Extinction coefficient (1/m).
    pub density: f32,
    /// Scattering / extinction per channel.
    pub albedo: [f32; 3],
    /// Replaces the fog accumulated before it instead of adding to it.
    pub replaces: bool,
    /// Below 1, a 3D noise modulates the density.
    pub noise_coverage: f32,
}

impl FogVolume {
    /// Edge weight at local position `p` (0 outside, 1 deep inside): linear over
    /// `transition_distance` from a sphere's surface or a box's faces.
    // bg3: VolumetricFogInitialize (Core.metallib, 4.1.1.7631656)
    pub fn weight(&self, p: [f32; 3]) -> f32 {
        let td = self.transition_distance.max(1e-4);
        if self.shape == 1 {
            let half = self.size.map(|s| s * 0.5);
            if p.iter().zip(half).any(|(v, h)| v.abs() > h) {
                return 0.0;
            }
            let d: f32 = (0..3)
                .map(|i| (p[i].abs() - (half[i] - td)).max(0.0).powi(2))
                .sum::<f32>()
                .sqrt();
            1.0 - (d / td).clamp(0.0, 1.0)
        } else {
            let r = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
            ((self.radius - r) / td).clamp(0.0, 1.0)
        }
    }
}

fn template_props(templates: &RootTemplates, id: &str) -> FogProps {
    let mut acc = FogProps::default();
    let mut current = templates.get(id);
    for _ in 0..32 {
        let Some(t) = current else { break };
        if let Some(p) = &t.fog {
            acc = acc.inherit(p);
        }
        current = t.parent.as_deref().and_then(|p| templates.get(p));
    }
    acc
}

/// Read the fog volumes of one `FogVolumes/_merged.lsf`.
pub fn read_fog_volumes(res: &bg3_lsf::Resource, templates: &RootTemplates) -> Vec<FogVolume> {
    res.region("Templates")
        .into_iter()
        .flat_map(|n| n.children_named("GameObjects"))
        .filter(|n| str_attr(n, "Type").as_deref() == Some("fogVolume"))
        .filter_map(|n| {
            let inherited = str_attr(n, "TemplateName")
                .map(|t| template_props(templates, &t))
                .unwrap_or_default();
            let p = FogProps::read(n).inherit(&inherited);
            Some(FogVolume {
                id: str_attr(n, "MapKey")?,
                name: str_attr(n, "Name").unwrap_or_default(),
                transform: n.child("Transform").map(read_transform).unwrap_or_default(),
                enabled: p.enabled.unwrap_or(true),
                shape: p.shape.unwrap_or(0),
                radius: p.radius.unwrap_or(5.0),
                size: p.size.unwrap_or([10.0; 3]),
                transition_distance: p.transition_distance.unwrap_or(1.0),
                density: p.density.unwrap_or(0.01),
                albedo: p.albedo.unwrap_or([1.0; 3]),
                replaces: p.replaces.unwrap_or(false),
                noise_coverage: p.noise_coverage.unwrap_or(1.0),
            })
        })
        .collect()
}

/// All fog volumes of a level, merged across modules by `MapKey`.
pub fn load_fog_volumes(
    vfs: &Vfs,
    level: &str,
    templates: &RootTemplates,
) -> Result<Vec<FogVolume>> {
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<FogVolume> = Vec::new();
    for module in BASE_LOAD_ORDER {
        let path = format!("Mods/{module}/Levels/{level}/FogVolumes/_merged.lsf");
        if !vfs.exists(&path) {
            continue;
        }
        for v in read_fog_volumes(&parse(vfs, &path)?, templates) {
            match index.get(&v.id) {
                Some(&i) => out[i] = v,
                None => {
                    index.insert(v.id.clone(), out.len());
                    out.push(v);
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn volume(shape: u8) -> FogVolume {
        FogVolume {
            id: "f".into(),
            name: "f".into(),
            transform: Transform::default(),
            enabled: true,
            shape,
            radius: 4.0,
            size: [10.0, 4.0, 10.0],
            transition_distance: 2.0,
            density: 0.1,
            albedo: [1.0; 3],
            replaces: false,
            noise_coverage: 1.0,
        }
    }

    #[test]
    fn sphere_weight_fades_linearly_over_the_transition() {
        let v = volume(0);
        assert_eq!(v.weight([0.0; 3]), 1.0);
        assert_eq!(v.weight([3.0, 0.0, 0.0]), 0.5);
        assert_eq!(v.weight([5.0, 0.0, 0.0]), 0.0);
    }

    #[test]
    fn box_weight_fades_from_the_faces() {
        let v = volume(1);
        // Half height 2 = TD: full density only on the mid-plane.
        assert_eq!(v.weight([0.0; 3]), 1.0);
        assert_eq!(v.weight([0.0, 1.0, 0.0]), 0.5);
        let mut tall = v.clone();
        tall.size = [10.0, 10.0, 10.0];
        assert_eq!(tall.weight([0.0; 3]), 1.0);
        assert_eq!(tall.weight([4.0, 0.0, 0.0]), 0.5);
        assert_eq!(tall.weight([6.0, 0.0, 0.0]), 0.0);
    }
}
