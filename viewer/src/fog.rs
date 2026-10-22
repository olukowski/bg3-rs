//! The level's fog volumes as Bevy volumetric fog.
//!
//! The game's fog is a froxel volume: per enabled volume an extinction `Density` (1/m),
//! scattering `Albedo · Density`, a linear fade over `TransitionDistance` from a sphere's
//! surface or a box's faces, lit by the sun, the local lights (Henyey-Greenstein phase, g =
//! `Fog/Phase`) and the probes' spherical harmonics, all pre-exposed. Here each volume becomes a
//! Bevy `FogVolume` (a box; the fade is baked into a small density texture), lit by Bevy's
//! volumetric lighting. Not reproduced (see `docs/research/lighting.md`, "Fog"): volumes of one
//! group combining by max and `Replaces` volumes cutting the fog before them (Bevy adds
//! everything), the density noise, the probes' ambient in-scattering, the phase for local
//! lights (Bevy applies it to directional lights only) and per-light `ScatteringScale`.
//!
//! bg3: VolumetricFogInitialize, VolumetricFogLighting, VolumetricFogGather (Core.metallib,
//! 4.1.1.7631656)

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::light::{FogVolume, VolumetricFog};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bg3_world::fog::FogVolume as Bg3Fog;

use crate::space;

/// Resolution of the baked edge-fade textures.
const DENSITY_RES: u32 = 16;

/// Whether fog is rendered (`BG3_NO_FOG=1` turns it off).
pub fn enabled() -> bool {
    std::env::var_os("BG3_NO_FOG").is_none()
}

#[derive(Resource, Default)]
pub struct LevelFog {
    pub volumes: Vec<Bg3Fog>,
    /// `Fog/Phase` of the active lighting.
    pub phase: f32,
}

impl LevelFog {
    pub fn load(
        vfs: &bg3_vfs::Vfs,
        level: &str,
        templates: &bg3_world::RootTemplates,
        phase: Option<f32>,
    ) -> Self {
        let volumes = match bg3_world::fog::load_fog_volumes(vfs, level, templates) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("fog: {e}");
                Vec::new()
            }
        };
        let total = volumes.len();
        let volumes: Vec<Bg3Fog> = volumes
            .into_iter()
            .filter(|v| v.enabled && v.density > 0.0)
            .collect();
        eprintln!("fog: {} of {total} fog volumes enabled", volumes.len());
        Self {
            volumes,
            // **Assumption**: the shader's phase constant is `Fog/Phase`.
            phase: phase.unwrap_or(0.5),
        }
    }

    pub fn active(&self) -> bool {
        enabled() && !self.volumes.is_empty()
    }
}

/// The volume's edge fade sampled at voxel centres over its bounding box (unit cube → local).
fn density_texture(v: &Bg3Fog, extent: Vec3) -> Image {
    let n = DENSITY_RES;
    let mut data = Vec::with_capacity((n * n * n) as usize);
    for z in 0..n {
        for y in 0..n {
            for x in 0..n {
                let uvw = (Vec3::new(x as f32, y as f32, z as f32) + 0.5) / n as f32;
                let p = (uvw - 0.5) * extent;
                data.push((v.weight(p.to_array()) * 255.0).round() as u8);
            }
        }
    }
    let mut image = Image::new(
        Extent3d {
            width: n,
            height: n,
            depth_or_array_layers: n,
        },
        TextureDimension::D3,
        data,
        TextureFormat::R8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    let clamp = ImageAddressMode::ClampToEdge;
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: clamp,
        address_mode_v: clamp,
        address_mode_w: clamp,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..default()
    });
    image
}

pub fn spawn_fog(mut commands: Commands, fog: Res<LevelFog>, mut images: ResMut<Assets<Image>>) {
    if !fog.active() {
        return;
    }
    for v in &fog.volumes {
        let extent = if v.shape == 1 {
            Vec3::from_array(v.size)
        } else {
            Vec3::splat(2.0 * v.radius)
        };
        let mut transform = space::transform(&v.transform);
        transform.scale *= extent;
        // σs = Albedo·σ, σt = σ: scattering takes the largest albedo channel, the colour the
        // rest; absorption makes up the extinction.
        let scattering = v
            .albedo
            .iter()
            .copied()
            .fold(0.0f32, f32::max)
            .clamp(1e-3, 1.0);
        let [r, g, b] = v.albedo.map(|a| a / scattering);
        commands.spawn((
            Name::new(format!("fog {}", v.name)),
            FogVolume {
                fog_color: Color::linear_rgb(r, g, b),
                density_factor: v.density,
                density_texture: Some(images.add(density_texture(v, extent))),
                absorption: 1.0 - scattering,
                scattering,
                scattering_asymmetry: fog.phase,
                ..default()
            },
            transform,
        ));
    }
}

/// Volumetric fog on the camera. Bevy's fog ambient term isn't exposed and fades with depth, so
/// it is off; the game's comes from the probes' SH (not reproduced).
pub fn apply_fog(
    mut commands: Commands,
    fog: Res<LevelFog>,
    cameras: Query<Entity, Added<Camera3d>>,
) {
    if !fog.active() {
        return;
    }
    for e in &cameras {
        commands.entity(e).insert(VolumetricFog {
            ambient_intensity: 0.0,
            step_count: 64,
            ..default()
        });
    }
}
