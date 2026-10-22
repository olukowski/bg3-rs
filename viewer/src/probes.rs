//! The level's baked light probes as Bevy reflection probes (ambient diffuse and specular).
//!
//! Each probe has captures per lighting resource; we take the active lighting's: a 16² BC6H
//! diffuse cube (irradiance/π) and a 128² BC6H specular cube with eight GGX-prefiltered mips.
//! The game multiplies capture texels by `Exposure · 100` (captures are absolute radiance /
//! 100), so the environment map intensity is `100 · Intensity`. Faces are in D3D order and
//! sampled with the BG3 world direction; Bevy samples with `(x, y, −z)` of its own space, which
//! the viewer's z mirror turns back into the BG3 direction, so no face swap is needed.
//!
//! Approximated (see `docs/research/lighting.md`, "Light probes"): Bevy blends overlapping
//! probes as a weighted sum where the game lays them over each other in order (with coverage
//! alpha and a smoothstep fade); spheres become boxes of 2R; Bevy uses the 8 probes nearest the
//! camera; the specular mip is `7 r` here and `8 r` in the game.
//!
//! bg3: ClusteredLighting_* (light probe loop), Integrate_Env_DIFF_PS / Integrate_Env_SPEC_PS
//! (Core.metallib, 4.1.1.7631656)

use bevy::asset::RenderAssetUsages;
use bevy::light::{EnvironmentMapLight, LightProbe, ParallaxCorrection};
use bevy::prelude::*;
use bevy::render::render_resource::{
    Extent3d, TextureDimension, TextureFormat, TextureViewDescriptor, TextureViewDimension,
};
use bg3_material::dds::{DdsFormat, DdsHeader};
use bg3_vfs::Vfs;
use bg3_world::probes::{LightProbe as Bg3Probe, load_level_probes};

use crate::space;

/// Capture texels are radiance in units of 100 cd/m².
const CAPTURE_SCALE: f32 = 100.0;

/// Decoded probe cubemaps, ready to upload.
pub struct ProbeMaps {
    pub name: String,
    pub transform: Transform,
    /// Fade distance as a fraction of the size, per axis.
    pub falloff: Vec3,
    pub intensity: f32,
    pub diffuse: Image,
    pub specular: Image,
}

#[derive(Resource, Default)]
pub struct LevelProbes {
    pub probes: Vec<ProbeMaps>,
    /// Mean radiance (capture units) of what the captures saw where there was no geometry.
    pub sky: Option<Vec3>,
    /// The spherical harmonics of every enabled probe with a capture, in file order.
    pub sh: Vec<ProbeSh>,
    /// The background: the specular capture that saw the most sky (see [`apply_sky`]).
    pub background: Option<(String, Image)>,
}

/// A probe's spherical harmonics (`_SH.dat`): four RGBA coefficient vectors (`Y00`, `Y1−1`,
/// `Y10`, `Y11`), the plain projection of the captured radiance in capture units, with the
/// coverage in alpha; and the volume they apply in.
pub struct ProbeSh {
    pub transform: bg3_world::Transform,
    /// 0 = sphere, 1 = box.
    pub shape: u8,
    pub radius: f32,
    pub size: [f32; 3],
    pub transition_distance: f32,
    pub coefficients: [[f32; 4]; 4],
}

impl ProbeSh {
    /// The probe's weight at a BG3-space point: 0 outside, else smoothstep of the fade (sphere:
    /// `(R − |p|)/TD`; box: 1 − the distance into the `TD` band inside the faces, over `TD`).
    // bg3: VolumetricFogLighting / ClusteredLighting_* probe loop (Core.metallib, 4.1.1.7631656)
    fn fade(&self, p: [f32; 3]) -> f32 {
        let [x, y, z, w] = self.transform.rotation;
        let q = Quat::from_xyzw(x, y, z, w).normalize();
        let local = q.inverse() * (Vec3::from_array(p) - Vec3::from_array(self.transform.position));
        let td = self.transition_distance.max(1e-5);
        let t = if self.shape == 1 {
            let half = Vec3::from_array(self.size) * 0.5;
            if local.abs().cmpgt(half).any() {
                return 0.0;
            }
            1.0 - ((local.abs() - half + td).max(Vec3::ZERO).length() / td).clamp(0.0, 1.0)
        } else {
            let d = local.length();
            if d > self.radius {
                return 0.0;
            }
            ((self.radius - d) / td).clamp(0.0, 1.0)
        };
        t * t * (3.0 - 2.0 * t)
    }
}

impl LevelProbes {
    /// Mean ambient radiance (cd/m², linear RGB) the volumetric fog sees at a BG3-space point:
    /// the probes' band-0 harmonics laid over each other in file order, each by its fade times
    /// its coverage, over the distant probe's (taken as 0: not known). The game evaluates band
    /// 1 too, scaled by the phase asymmetry, towards the view ray; Bevy's fog can't take a
    /// direction-dependent ambient, so it is dropped (exact for isotropic fog).
    ///
    /// bg3: VolumetricFogLighting (g_DistantProbeSH, LightProbeDataBuffer SH, × 100)
    /// (Core.metallib, 4.1.1.7631656)
    pub fn fog_ambient_at(&self, p: [f32; 3]) -> Vec3 {
        const Y00: f32 = 0.282_095;
        let mut out = Vec3::ZERO;
        for sh in &self.sh {
            let fade = sh.fade(p);
            if fade <= 0.0 {
                continue;
            }
            let c = sh.coefficients[0];
            let value = (Vec3::new(c[0], c[1], c[2]) * Y00).max(Vec3::ZERO);
            let weight = (c[3] * Y00).clamp(0.0, 1.0) * fade;
            out = out.lerp(value, weight);
        }
        out * CAPTURE_SCALE
    }
}

impl LevelProbes {
    /// Probes of `level` baked with the `lighting` resource.
    pub fn load(
        vfs: &Vfs,
        level: &str,
        templates: &bg3_world::RootTemplates,
        lighting: Option<&str>,
    ) -> Self {
        let Some(lighting) = lighting else {
            return Self::default();
        };
        let probes = match load_level_probes(vfs, level, templates) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("probes: {e}");
                return Self::default();
            }
        };
        let total = probes.len();
        let mut out = Vec::new();
        let mut sky = SkySum::default();
        let mut sh = Vec::new();
        let mut best: Option<(f32, String, String)> = None;
        for p in probes.iter().filter(|p| p.enabled) {
            if let Some(path) = p.capture(lighting).and_then(|c| c.sh.as_deref()) {
                match read_sh(vfs, path) {
                    Ok(coefficients) => sh.push(ProbeSh {
                        transform: p.transform,
                        shape: p.shape,
                        radius: p.radius,
                        size: p.size,
                        transition_distance: p.transition_distance,
                        coefficients,
                    }),
                    Err(e) => eprintln!("probe {}: harmonics: {e}", p.name),
                }
            }
            match load_probe(vfs, p, lighting) {
                Ok(Some(m)) => out.push(m),
                Ok(None) => {}
                Err(e) => eprintln!("probe {}: {e}", p.name),
            }
            if let Some(c) = p.capture(lighting)
                && let (Some(rgb), Some(alpha)) = (&c.specular, &c.specular_alpha)
            {
                match sky.add(vfs, rgb, alpha) {
                    Ok(share) if best.as_ref().is_none_or(|b| share > b.0) => {
                        best = Some((share, p.name.clone(), rgb.clone()));
                    }
                    Ok(_) => {}
                    Err(e) => eprintln!("probe {}: sky texels: {e}", p.name),
                }
            }
        }
        let sky = sky.mean();
        let background = best.and_then(|(share, name, path)| {
            match vfs
                .read(&path)
                .map_err(anyhow::Error::from)
                .and_then(|d| cubemap(&d))
            {
                Ok(image) => {
                    eprintln!(
                        "probes: background from {name} ({:.0} % sky texels)",
                        100.0 * share
                    );
                    Some((name, image))
                }
                Err(e) => {
                    eprintln!("probe {name}: background: {e}");
                    None
                }
            }
        });
        eprintln!(
            "probes: {} of {total} with a capture for lighting {lighting}; sky {:?} cd/m²",
            out.len(),
            sky.map(|s| s * CAPTURE_SCALE)
        );
        Self {
            probes: out,
            sky,
            sh,
            background,
        }
    }
}

/// `_SH.dat`: four little-endian `float4`s.
fn read_sh(vfs: &Vfs, path: &str) -> anyhow::Result<[[f32; 4]; 4]> {
    let data = vfs.read(path)?;
    anyhow::ensure!(data.len() >= 64, "{} bytes, expected 64", data.len());
    Ok(std::array::from_fn(|i| {
        std::array::from_fn(|c| {
            let o = (i * 4 + c) * 4;
            f32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]])
        })
    }))
}

fn load_probe(vfs: &Vfs, p: &Bg3Probe, lighting: &str) -> anyhow::Result<Option<ProbeMaps>> {
    let Some(c) = p.capture(lighting) else {
        return Ok(None);
    };
    let (Some(diffuse), Some(specular)) = (&c.diffuse, &c.specular) else {
        return Ok(None);
    };
    let diffuse = cubemap(&vfs.read(diffuse)?)?;
    let specular = cubemap(&vfs.read(specular)?)?;
    let mut transform = space::transform(&p.transform);
    // Box probes: `Size` is the full box (**assumption**); spheres become their bounding box.
    let size = if p.shape == 1 {
        Vec3::from_array(p.size)
    } else {
        Vec3::splat(2.0 * p.radius)
    };
    let extent = (size * 0.5).max(Vec3::splat(1e-3));
    let falloff = (Vec3::splat(p.transition_distance) / extent).clamp(Vec3::ZERO, Vec3::ONE);
    transform.scale = size * transform.scale;
    Ok(Some(ProbeMaps {
        name: p.name.clone(),
        transform,
        falloff,
        intensity: CAPTURE_SCALE * p.intensity,
        diffuse,
        specular,
    }))
}

/// Coverage below which a capture texel saw no geometry (sky): the alpha maps hold ≈ 1 on
/// geometry and ≈ 0.05 (the BC4 floor) towards openings.
const SKY_COVERAGE: f32 = 0.5;

/// Mip of the specular captures sampled for the sky estimate (16² of 128²).
const SKY_MIP: usize = 3;

/// The background: the probes' captures are the only record of the sky the game baked them
/// with (no skydome here, `SkydomeEnabled` false, and the flat sky colour and atmospheric
/// scattering are set up on the CPU in a way we haven't traced). We average every specular
/// texel whose coverage says "no geometry", over all probes of the active lighting
/// (**assumption**: a uniform sky of that radiance).
#[derive(Default)]
struct SkySum {
    sum: [f64; 3],
    count: u64,
}

impl SkySum {
    /// Add a capture's sky texels; returns the share of its texels that are sky.
    fn add(&mut self, vfs: &Vfs, rgb: &str, alpha: &str) -> anyhow::Result<f32> {
        let rgb = vfs.read(rgb)?;
        let alpha = vfs.read(alpha)?;
        let (rgb_h, a_h) = (
            DdsHeader::parse(&rgb).ok_or_else(|| anyhow::anyhow!("not a DDS file"))?,
            DdsHeader::parse(&alpha).ok_or_else(|| anyhow::anyhow!("not a DDS file"))?,
        );
        anyhow::ensure!(
            matches!(rgb_h.format, DdsFormat::Bc6h | DdsFormat::Bc6hSigned)
                && a_h.format == DdsFormat::Bc4
                && (rgb_h.width, rgb_h.mip_count) == (a_h.width, a_h.mip_count),
            "unexpected formats {:?} / {:?}",
            rgb_h.format,
            a_h.format
        );
        let signed = rgb_h.format == DdsFormat::Bc6hSigned;
        let (rl, al) = (
            rgb_h.mip_levels().unwrap_or_default(),
            a_h.mip_levels().unwrap_or_default(),
        );
        let mip = SKY_MIP.min(rl.len().saturating_sub(1));
        let (Some(r_last), Some(a_last), Some(r_mip), Some(a_mip)) =
            (rl.last(), al.last(), rl.get(mip), al.get(mip))
        else {
            return Ok(0.0);
        };
        let (mut sky_texels, mut texels_seen) = (0u32, 0u32);
        let r_face = r_last.2.end - rgb_h.data_offset();
        let a_face = a_last.2.end - a_h.data_offset();
        let blocks = r_mip.0.div_ceil(4) as usize * r_mip.1.div_ceil(4) as usize;
        for face in 0..6 {
            for b in 0..blocks {
                let ro = r_mip.2.start + face * r_face + b * 16;
                let ao = a_mip.2.start + face * a_face + b * 8;
                let (Some(rb), Some(ab)) = (rgb.get(ro..ro + 16), alpha.get(ao..ao + 8)) else {
                    anyhow::bail!("truncated cubemap");
                };
                let mut texels = [0.0f32; 48];
                bcdec_rs::bc6h_float(rb, &mut texels, 12, signed);
                let a = bg3_material::bc::decode_bc4(ab);
                for (i, &a) in a.iter().enumerate() {
                    texels_seen += 1;
                    if f32::from(a) / 255.0 < SKY_COVERAGE {
                        sky_texels += 1;
                        for c in 0..3 {
                            self.sum[c] += f64::from(texels[i * 3 + c]);
                        }
                        self.count += 1;
                    }
                }
            }
        }
        Ok(sky_texels as f32 / texels_seen.max(1) as f32)
    }

    fn mean(&self) -> Option<Vec3> {
        (self.count > 0).then(|| {
            let n = self.count as f64;
            Vec3::new(
                (self.sum[0] / n) as f32,
                (self.sum[1] / n) as f32,
                (self.sum[2] / n) as f32,
            )
        })
    }
}

/// The background, as a skybox (so it is exposed like the scene). The game draws its sky,
/// clouds and the atmosphere's environment effect into the emissive buffer, none of which the
/// viewer renders; the probes' specular captures recorded them. **Assumption**: the capture
/// that saw the most sky (texels with no geometry coverage) stands in for the background in
/// every direction, as is (no probe `Intensity`); the geometry it saw near its probe shows
/// wherever the level has none. Without one (or with `BG3_UNIFORM_SKY=1`), a uniform sky of the
/// mean sky texel.
pub fn apply_sky(
    mut commands: Commands,
    mut probes: ResMut<LevelProbes>,
    mut images: ResMut<Assets<Image>>,
    cameras: Query<Entity, Added<Camera3d>>,
    mut background: Local<Option<Handle<Image>>>,
) {
    // `BG3_UNIFORM_SKY=1`: the uniform mean sky (A/B checks only).
    if let Some((_, image)) = probes.background.take()
        && std::env::var_os("BG3_UNIFORM_SKY").is_none()
    {
        *background = Some(images.add(image));
    }
    if let Some(handle) = background.as_ref() {
        for e in &cameras {
            commands.entity(e).insert(bevy::light::Skybox {
                image: Some(handle.clone()),
                brightness: CAPTURE_SCALE,
                ..default()
            });
        }
        return;
    }
    let Some(sky) = probes.sky else {
        return;
    };
    for e in &cameras {
        let texel: Vec<u8> = [sky.x, sky.y, sky.z, 1.0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let mut image = Image::new(
            Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 6,
            },
            TextureDimension::D2,
            texel.repeat(6),
            TextureFormat::Rgba32Float,
            RenderAssetUsages::RENDER_WORLD,
        );
        image.texture_view_descriptor = Some(TextureViewDescriptor {
            dimension: Some(TextureViewDimension::Cube),
            ..default()
        });
        commands.entity(e).insert(bevy::light::Skybox {
            image: Some(images.add(image)),
            brightness: CAPTURE_SCALE,
            ..default()
        });
    }
}

/// A BC6H cubemap DDS (six faces, each with its whole mip chain) as a Bevy cube image.
fn cubemap(dds: &[u8]) -> anyhow::Result<Image> {
    let h = DdsHeader::parse(dds).ok_or_else(|| anyhow::anyhow!("not a DDS file"))?;
    anyhow::ensure!(h.cubemap, "not a cubemap");
    let format = match h.format {
        DdsFormat::Bc6h => TextureFormat::Bc6hRgbUfloat,
        DdsFormat::Bc6hSigned => TextureFormat::Bc6hRgbFloat,
        f => anyhow::bail!("unexpected format {f:?}"),
    };
    let levels = h
        .mip_levels()
        .ok_or_else(|| anyhow::anyhow!("unsupported layout"))?;
    let start = h.data_offset();
    let face = levels.last().map_or(0, |l| l.2.end) - start;
    let data = dds
        .get(start..start + 6 * face)
        .ok_or_else(|| anyhow::anyhow!("{} bytes, expected 6 × {face}", dds.len() - start))?;
    let mut image = Image::new_uninit(
        Extent3d {
            width: h.width,
            height: h.height,
            depth_or_array_layers: 6,
        },
        TextureDimension::D2,
        format,
        RenderAssetUsages::RENDER_WORLD,
    );
    // DDS stores face after face (layer-major), as Bevy expects by default.
    image.data = Some(data.to_vec());
    image.texture_descriptor.mip_level_count = levels.len() as u32;
    image.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::Cube),
        ..default()
    });
    Ok(image)
}

/// Spawn one reflection probe per loaded probe.
pub fn spawn_probes(
    mut commands: Commands,
    mut probes: ResMut<LevelProbes>,
    mut images: ResMut<Assets<Image>>,
) {
    for p in std::mem::take(&mut probes.probes) {
        let diffuse = images.add(p.diffuse);
        let specular = images.add(p.specular);
        commands.spawn((
            Name::new(format!("probe {}", p.name)),
            LightProbe { falloff: p.falloff },
            EnvironmentMapLight {
                diffuse_map: diffuse,
                specular_map: specular,
                intensity: p.intensity,
                ..default()
            },
            ParallaxCorrection::Auto,
            p.transform,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(shape: u8, position: [f32; 3], value: f32, coverage: f32) -> ProbeSh {
        // Band 0 of a constant radiance L is L / Y00.
        let c = |v: f32| v / 0.282_095;
        ProbeSh {
            transform: bg3_world::Transform {
                position,
                ..Default::default()
            },
            shape,
            radius: 10.0,
            size: [20.0, 10.0, 20.0],
            transition_distance: 2.0,
            coefficients: [
                [c(value), c(value), c(value), c(coverage)],
                [0.0; 4],
                [0.0; 4],
                [0.0; 4],
            ],
        }
    }

    #[test]
    fn sphere_and_box_fades() {
        let s = probe(0, [0.0; 3], 1.0, 1.0);
        assert_eq!(s.fade([0.0, 0.0, 0.0]), 1.0);
        assert_eq!(s.fade([0.0, 11.0, 0.0]), 0.0);
        // Halfway through the 2 m band: smoothstep(0.5).
        assert!((s.fade([9.0, 0.0, 0.0]) - 0.5).abs() < 1e-5);
        let b = probe(1, [0.0; 3], 1.0, 1.0);
        assert_eq!(b.fade([0.0, 0.0, 0.0]), 1.0);
        assert_eq!(b.fade([0.0, 5.5, 0.0]), 0.0);
        // 1 m into the 2 m band below the top face.
        assert!((b.fade([0.0, 4.0, 0.0]) - 0.5).abs() < 1e-5);
    }

    #[test]
    fn fog_ambient_lays_probes_over_in_order() {
        let probes = LevelProbes {
            sh: vec![
                probe(0, [0.0; 3], 0.2, 1.0),
                probe(0, [5.0, 0.0, 0.0], 0.6, 0.5),
            ],
            ..Default::default()
        };
        // Only the first probe: its value × 100.
        assert!((probes.fog_ambient_at([-5.0, 0.0, 0.0]).x - 20.0).abs() < 1e-3);
        // Both: the second over the first by its coverage 0.5.
        assert!((probes.fog_ambient_at([2.0, 0.0, 0.0]).x - 40.0).abs() < 1e-3);
        assert_eq!(probes.fog_ambient_at([50.0, 0.0, 0.0]), Vec3::ZERO);
    }
}
