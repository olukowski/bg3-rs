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
        for p in probes.iter().filter(|p| p.enabled) {
            match load_probe(vfs, p, lighting) {
                Ok(Some(m)) => out.push(m),
                Ok(None) => {}
                Err(e) => eprintln!("probe {}: {e}", p.name),
            }
            if let Some(c) = p.capture(lighting)
                && let (Some(rgb), Some(alpha)) = (&c.specular, &c.specular_alpha)
                && let Err(e) = sky.add(vfs, rgb, alpha)
            {
                eprintln!("probe {}: sky texels: {e}", p.name);
            }
        }
        let sky = sky.mean();
        eprintln!(
            "probes: {} of {total} with a capture for lighting {lighting}; sky {:?} cd/m²",
            out.len(),
            sky.map(|s| s * CAPTURE_SCALE)
        );
        Self { probes: out, sky }
    }
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
    fn add(&mut self, vfs: &Vfs, rgb: &str, alpha: &str) -> anyhow::Result<()> {
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
            return Ok(());
        };
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
                    if f32::from(a) / 255.0 < SKY_COVERAGE {
                        for c in 0..3 {
                            self.sum[c] += f64::from(texels[i * 3 + c]);
                        }
                        self.count += 1;
                    }
                }
            }
        }
        Ok(())
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

/// A uniform sky behind the level, as a skybox (so it is exposed like the scene).
pub fn apply_sky(
    mut commands: Commands,
    probes: Res<LevelProbes>,
    mut images: ResMut<Assets<Image>>,
    cameras: Query<Entity, Added<Camera3d>>,
) {
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
