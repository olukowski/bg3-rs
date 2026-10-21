//! BG3 materials → Bevy `StandardMaterial`s with their plain (DDS) textures.
//!
//! Resolution (graph defaults, instance overrides, slots, blend modes) and channel re-packing
//! live in `bg3-material`; this module only loads files, builds `Image`s and materials.
//! Materials whose main maps come from virtual textures get a neutral grey for now, plus their
//! glow map if they have one. See `docs/research/materials.md`.

use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, Face, TextureDimension, TextureFormat};
use bg3_material::convert::{self, TexelFormat, Texture};
use bg3_material::dds::DdsFormat;
use bg3_material::{BlendMode, ResolvedMaterial, ShaderGraph, Slot, resolve};
use bg3_vfs::Vfs;
use bg3_world::ResourceBanks;
use rayon::prelude::*;

/// How a texture is used, which decides its conversion and colour space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Usage {
    /// Colour data; sRGB per the texture resource's `SRGB` flag.
    Color {
        srgb: bool,
    },
    Normal,
    Physical,
}

/// Everything needed to build one `StandardMaterial`, with texture slots as indices into
/// [`PreparedMaterials::images`].
struct Plan {
    material: ResolvedMaterial,
    base: Option<usize>,
    normal: Option<usize>,
    physical: Option<usize>,
    emissive: Option<usize>,
}

/// CPU-side result of resolving and decoding every material a level needs.
#[derive(Resource)]
pub struct PreparedMaterials {
    plans: HashMap<String, Plan>,
    images: Vec<Option<Image>>,
}

/// Material handle per material ID; `None` for helper geometry that isn't drawn.
#[derive(Resource, Default)]
pub struct LevelMaterials {
    pub by_id: HashMap<String, Option<Handle<StandardMaterial>>>,
}

impl PreparedMaterials {
    /// Resolve `ids` and load their textures (in parallel, each file once per usage).
    pub fn load<'a>(vfs: &Vfs, banks: &ResourceBanks, ids: impl Iterator<Item = &'a str>) -> Self {
        let started = std::time::Instant::now();
        let mut ids: Vec<&str> = ids.collect();
        ids.sort_unstable();
        ids.dedup();
        let materials: Vec<_> = ids
            .iter()
            .filter_map(|id| Some((*id, banks.materials.get(*id)?)))
            .collect();

        let mut sources: Vec<&str> = materials
            .iter()
            .filter_map(|(_, m)| m.source_file.as_deref())
            .collect();
        sources.sort_unstable();
        sources.dedup();
        let graphs: HashMap<&str, ShaderGraph> = sources
            .into_par_iter()
            .filter_map(|path| {
                let res = bg3_lsf::parse(&vfs.read(path).ok()?).ok()?;
                Some((path, ShaderGraph::from_resource(&res)?))
            })
            .collect();

        let mut keys: Vec<(String, Usage)> = Vec::new();
        let mut index: HashMap<(String, Usage), usize> = HashMap::new();
        let mut slot = |id: &str, usage: fn(bool) -> Usage| -> Option<usize> {
            let tex = banks.textures.get(id)?;
            let key = (tex.source_file.clone()?, usage(tex.srgb));
            Some(*index.entry(key.clone()).or_insert_with(|| {
                keys.push(key);
                keys.len() - 1
            }))
        };
        let mut plans = HashMap::new();
        for (id, mat) in &materials {
            let path = mat.source_file.as_deref().unwrap_or_default();
            let material = resolve(path, graphs.get(path), &mat.params);
            let tex = |s: Slot| material.texture(s).map(|t| t.id.clone());
            let vt = material.needs_virtual_texture();
            // With a virtual texture, the plain base/normal/physical parameters are graph
            // leftovers (or detail inputs), so only the glow map is used.
            let (base, normal, physical) = if vt {
                (None, None, None)
            } else {
                (
                    tex(Slot::BaseColor).and_then(|t| slot(&t, |s| Usage::Color { srgb: s })),
                    tex(Slot::Normal).and_then(|t| slot(&t, |_| Usage::Normal)),
                    tex(Slot::Physical).and_then(|t| slot(&t, |_| Usage::Physical)),
                )
            };
            let emissive = tex(Slot::Emissive).and_then(|t| slot(&t, |s| Usage::Color { srgb: s }));
            plans.insert(
                (*id).to_owned(),
                Plan {
                    material,
                    base,
                    normal,
                    physical,
                    emissive,
                },
            );
        }

        let decoded: Vec<Option<Image>> = keys
            .par_iter()
            .map(|(path, usage)| match load_texture(vfs, path, *usage) {
                Ok(img) => Some(img),
                Err(e) => {
                    eprintln!("texture {path}: {e}");
                    None
                }
            })
            .collect();
        let bytes: usize = decoded
            .iter()
            .flatten()
            .filter_map(|i| i.data.as_ref().map(Vec::len))
            .sum();
        let vt = plans
            .values()
            .filter(|p| p.material.needs_virtual_texture())
            .count();
        let textured = plans.values().filter(|p| p.base.is_some()).count();
        eprintln!(
            "materials: {} resolved ({textured} with plain base colour, {vt} virtual-textured → grey), {} graphs; {}/{} textures ({:.0} MiB) in {:.2?}",
            plans.len(),
            graphs.len(),
            decoded.iter().flatten().count(),
            keys.len(),
            bytes as f64 / (1024.0 * 1024.0),
            started.elapsed()
        );
        Self {
            plans,
            images: decoded,
        }
    }
}

fn load_texture(vfs: &Vfs, path: &str, usage: Usage) -> anyhow::Result<Image> {
    let dds = vfs.read(path)?;
    let (texture, srgb) = match usage {
        Usage::Normal => (convert::normal_map(&dds)?, false),
        Usage::Physical => (convert::physical_map(&dds)?, false),
        Usage::Color { srgb } => {
            let t = convert::passthrough(&dds)?;
            // Single-channel maps would sample as red only; expand them to grey RGBA.
            let t = if t.format == TexelFormat::Bc(DdsFormat::Bc4) {
                convert::rgba(&dds)?
            } else {
                t
            };
            (t, srgb)
        }
    };
    to_image(texture, srgb)
}

fn to_image(t: Texture, srgb: bool) -> anyhow::Result<Image> {
    use TextureFormat as F;
    let format = match (t.format, srgb) {
        (TexelFormat::Bc(DdsFormat::Bc1), false) => F::Bc1RgbaUnorm,
        (TexelFormat::Bc(DdsFormat::Bc1), true) => F::Bc1RgbaUnormSrgb,
        (TexelFormat::Bc(DdsFormat::Bc2), false) => F::Bc2RgbaUnorm,
        (TexelFormat::Bc(DdsFormat::Bc2), true) => F::Bc2RgbaUnormSrgb,
        (TexelFormat::Bc(DdsFormat::Bc3), false) => F::Bc3RgbaUnorm,
        (TexelFormat::Bc(DdsFormat::Bc3), true) => F::Bc3RgbaUnormSrgb,
        (TexelFormat::Bc(DdsFormat::Bc4), _) => F::Bc4RUnorm,
        (TexelFormat::Bc(DdsFormat::Bc5), _) => F::Bc5RgUnorm,
        (TexelFormat::Bc(DdsFormat::Bc7), false) => F::Bc7RgbaUnorm,
        (TexelFormat::Bc(DdsFormat::Bc7), true) => F::Bc7RgbaUnormSrgb,
        (TexelFormat::Rg8, _) => F::Rg8Unorm,
        (TexelFormat::Rgba8, false) => F::Rgba8Unorm,
        (TexelFormat::Rgba8, true) => F::Rgba8UnormSrgb,
        (TexelFormat::Bc(f), _) => anyhow::bail!("unsupported format {f:?}"),
    };
    let mut image = Image::new(
        Extent3d {
            width: t.width,
            height: t.height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        t.data,
        format,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_descriptor.mip_level_count = t.mip_count;
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 8,
        ..default()
    });
    Ok(image)
}

/// Emissive scale: glow colours are ~1, Bevy's default exposure needs a few hundred nits to
/// read as glowing. Tuned by eye; the real value depends on BG3's exposure/tonemapping.
const GLOW_NITS: f32 = 150.0;

/// Neutral stand-in for surfaces whose maps live in virtual textures.
const VT_GREY: Color = Color::srgb(0.45, 0.45, 0.45);

/// Upload the prepared images and build a material per ID.
pub fn upload(
    mut commands: Commands,
    mut prepared: ResMut<PreparedMaterials>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let handles: Vec<Option<Handle<Image>>> = std::mem::take(&mut prepared.images)
        .into_iter()
        .map(|img| img.map(|i| images.add(i)))
        .collect();
    let get = |i: Option<usize>| i.and_then(|i| handles[i].clone());
    let mut out = LevelMaterials::default();
    for (id, plan) in std::mem::take(&mut prepared.plans) {
        let m = &plan.material;
        if m.invisible {
            out.by_id.insert(id, None);
            continue;
        }
        let base_color_texture = get(plan.base);
        let metallic_roughness_texture = get(plan.physical);
        let base_color = match (&base_color_texture, m.flat_color()) {
            (Some(_), _) => Color::WHITE,
            (None, Some([r, g, b])) => Color::linear_rgb(r, g, b),
            (None, None) => VT_GREY,
        };
        let emissive_texture = get(plan.emissive);
        let emissive = match (&emissive_texture, m.glow()) {
            (Some(_), Some([r, g, b])) => LinearRgba::rgb(r, g, b) * GLOW_NITS,
            _ => LinearRgba::BLACK,
        };
        let material = StandardMaterial {
            base_color,
            base_color_texture,
            perceptual_roughness: if metallic_roughness_texture.is_some() {
                1.0
            } else {
                0.8
            },
            metallic: if metallic_roughness_texture.is_some() {
                1.0
            } else {
                0.0
            },
            occlusion_texture: metallic_roughness_texture.clone(),
            metallic_roughness_texture,
            // Green points along +v, as does the mirrored GR2 bitangent: no Y flip. Verified by
            // `cargo run -p bg3x --example normal_convention`.
            normal_map_texture: get(plan.normal),
            flip_normal_map_y: false,
            emissive,
            emissive_texture,
            unlit: m.unlit,
            double_sided: m.two_sided,
            cull_mode: if m.two_sided { None } else { Some(Face::Back) },
            alpha_mode: match m.blend {
                BlendMode::Opaque => AlphaMode::Opaque,
                BlendMode::AlphaTest => AlphaMode::Mask(0.5),
                BlendMode::Blend => AlphaMode::Blend,
            },
            ..default()
        };
        out.by_id.insert(id, Some(materials.add(material)));
    }
    commands.insert_resource(out);
}
