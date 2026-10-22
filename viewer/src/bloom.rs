//! The game's bloom (`shaders/bloom.wesl`), between the exposure histogram and the grading /
//! tonemap pass, as the engine orders its post-process steps. The chain:
//!
//! 1. threshold: scene → 1/2 (`A1`), `sat(Amount · (luma − Threshold))`, min of four taps;
//! 2. downsample `A1` → `A2` (1/4) → `A3` (1/8) → `A4` (1/16) → `A5` (1/32), 3×3 tent;
//! 3. upsample `B4 = A4 + tent(A5)`, `B3 = A3 + tent(B4)`, `B2 = A2 + tent(B3)`, `B1 = A1 + tent(B2)`;
//! 4. combine: scene + 0.2 · `B1`.
//!
//! It runs when the atmosphere's `PostProcessing/Bloom` is set (and the graphics option, on
//! here). Research: `docs/research/lighting.md` ("Bloom").
//!
//! bg3: ls::PostProcessStep_Bloom::ExecuteWTKernel, ::LoadRenderTargets,
//! ::UpdateIsRenderingInternal (4.1.1.7631656)

use bevy::asset::{embedded_asset, load_embedded_asset};
use bevy::core_pipeline::FullscreenShader;
use bevy::image::ToExtents;
use bevy::prelude::*;
use bevy::render::camera::ExtractedCamera;
use bevy::render::extract_component::{ExtractComponent, ExtractComponentPlugin};
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::binding_types::{
    sampler, storage_buffer_read_only, texture_2d, uniform_buffer,
};
use bevy::render::render_resource::*;
use bevy::render::renderer::{RenderContext, RenderDevice, ViewQuery};
use bevy::render::storage::GpuShaderBuffer;
use bevy::render::texture::{CachedTexture, TextureCache};
use bevy::render::view::{ViewTarget, ViewUniform, ViewUniformOffset, ViewUniforms};
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};

use crate::post::{ExposureBuffer, ExposureState, GamePost, GameVignette};

/// Weight of the bloom when added to the scene (`PPBloomUpsampleBlurAndCombine_PS`).
const COMBINE_WEIGHT: f32 = 0.2;
/// Size divisors of the downsample chain (`LoadRenderTargets`: 1/2 … 1/32).
const LEVELS: [u32; 5] = [2, 4, 8, 16, 32];
/// The blur offsets scale with the screen height relative to 1080 lines (`ExecuteWTKernel`).
const REFERENCE_HEIGHT: f32 = 1080.0;
/// Format of the chain's targets (**assumption**: the engine's format code is not decoded; a
/// float format keeps the HDR range).
const FORMAT: TextureFormat = TextureFormat::Rgba16Float;

/// Bloom settings of a camera, from the atmosphere in effect.
#[derive(Component, Clone, Copy, ExtractComponent, Default, Debug)]
#[extract_app(RenderApp)]
pub struct GameBloom {
    pub enabled: bool,
    pub threshold: f32,
    pub amount: f32,
}

/// Whether bloom runs (`BG3_NO_BLOOM=1` turns it off for comparisons).
pub fn enabled() -> bool {
    std::env::var_os("BG3_NO_BLOOM").is_none()
}

pub struct GameBloomPlugin;

impl Plugin for GameBloomPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "shaders/bloom.wesl");
        app.add_plugins(ExtractComponentPlugin::<GameBloom>::default())
            .add_systems(Last, sync_bloom);
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .init_resource::<SpecializedRenderPipelines<BloomPipelines>>()
            .add_systems(RenderStartup, init_pipelines)
            .add_systems(
                Render,
                (
                    prepare_pipelines.in_set(RenderSystems::Prepare),
                    prepare_textures.in_set(RenderSystems::PrepareResources),
                ),
            );
    }
}

type CameraEffects<'a> = (Entity, Option<&'a GameBloom>, Option<&'a GameVignette>);

/// Follow the atmosphere's bloom and vignette settings (they change with the camera's
/// atmosphere trigger).
fn sync_bloom(
    mut commands: Commands,
    lights: Option<Res<crate::lights::LevelLights>>,
    cameras: Query<CameraEffects, With<GamePost>>,
) {
    let Some(lights) = lights else {
        return;
    };
    let p = &lights.lighting.post;
    let mut bloom = GameBloom {
        enabled: p.bloom && enabled(),
        threshold: p.bloom_threshold,
        amount: p.bloom_amount,
    };
    // `BG3_BLOOM=threshold,amount` forces bloom on with other settings (checks only).
    if let Some([t, a]) = pair("BG3_BLOOM") {
        bloom = GameBloom {
            enabled: true,
            threshold: t,
            amount: a,
        };
    }
    let mut vignette = GameVignette {
        color: Vec3::from_array(p.vignette_color).extend(if p.vignette { 1.0 } else { 0.0 }),
        params: Vec4::new(p.vignette_intensity, p.vignette_power, 0.0, 0.0),
    };
    // `BG3_VIGNETTE=intensity,power` forces the vignette on (checks only).
    if let Some([i, pw]) = pair("BG3_VIGNETTE") {
        vignette.color.w = 1.0;
        vignette.params = Vec4::new(i, pw, 0.0, 0.0);
    }
    for (entity, current, current_vignette) in &cameras {
        if current_vignette != Some(&vignette) {
            commands.entity(entity).insert(vignette);
        }
        let changed = current.is_none_or(|c| {
            (c.enabled, c.threshold, c.amount) != (bloom.enabled, bloom.threshold, bloom.amount)
        });
        if changed {
            if current.is_some() {
                eprintln!(
                    "bloom: {} (threshold {}, amount {})",
                    if bloom.enabled { "on" } else { "off" },
                    bloom.threshold,
                    bloom.amount
                );
            }
            commands.entity(entity).insert(bloom);
        }
    }
}

/// Two comma-separated numbers from an environment variable.
fn pair(name: &str) -> Option<[f32; 2]> {
    let v = std::env::var(name).ok()?;
    let x: Vec<f32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
    <[f32; 2]>::try_from(x).ok()
}

/// Parameters of one pass (`bloom.wesl` `BloomUniform`).
#[derive(Clone, Copy, ShaderType)]
struct BloomUniform {
    params: Vec4,
    texel: Vec4,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pass {
    Threshold,
    Downsample,
    Upsample,
    Combine,
}

#[derive(Resource)]
pub struct BloomPipelines {
    layout: BindGroupLayoutDescriptor,
    sampler: Sampler,
    fullscreen: FullscreenShader,
    shader: Handle<Shader>,
}

impl SpecializedRenderPipeline for BloomPipelines {
    type Key = (Pass, TextureFormat);

    fn specialize(&self, (pass, format): Self::Key) -> RenderPipelineDescriptor {
        let entry = match pass {
            Pass::Threshold => "threshold",
            Pass::Downsample => "downsample",
            Pass::Upsample => "upsample",
            Pass::Combine => "combine",
        };
        RenderPipelineDescriptor {
            label: Some(format!("bg3 bloom {entry}").into()),
            layout: vec![self.layout.clone()],
            vertex: self.fullscreen.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                shader_defs: vec![],
                entry_point: Some(entry.into()),
                targets: vec![Some(ColorTargetState {
                    format,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
                ..default()
            }),
            ..default()
        }
    }
}

fn init_pipelines(
    mut commands: Commands,
    device: Res<RenderDevice>,
    fullscreen: Res<FullscreenShader>,
    assets: Res<AssetServer>,
) {
    let layout = BindGroupLayoutDescriptor::new(
        "bg3 bloom layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                uniform_buffer::<ViewUniform>(true),
                uniform_buffer::<BloomUniform>(false),
                storage_buffer_read_only::<ExposureState>(false),
            ),
        ),
    );
    commands.insert_resource(BloomPipelines {
        layout,
        sampler: device.create_sampler(&SamplerDescriptor {
            min_filter: FilterMode::Linear,
            mag_filter: FilterMode::Linear,
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
            ..default()
        }),
        fullscreen: fullscreen.clone(),
        shader: load_embedded_asset!(assets.as_ref(), "shaders/bloom.wesl"),
    });
}

#[derive(Component)]
pub struct BloomPipelineIds {
    threshold: CachedRenderPipelineId,
    downsample: CachedRenderPipelineId,
    upsample: CachedRenderPipelineId,
    combine: CachedRenderPipelineId,
}

fn prepare_pipelines(
    mut commands: Commands,
    cache: Res<PipelineCache>,
    mut specialized: ResMut<SpecializedRenderPipelines<BloomPipelines>>,
    pipelines: Option<Res<BloomPipelines>>,
    views: Query<(Entity, &ViewTarget, &GameBloom)>,
) {
    let Some(p) = pipelines else {
        return;
    };
    for (entity, target, bloom) in &views {
        if !bloom.enabled {
            continue;
        }
        let mut get = |pass, format| specialized.specialize(&cache, &p, (pass, format));
        let ids = BloomPipelineIds {
            threshold: get(Pass::Threshold, FORMAT),
            downsample: get(Pass::Downsample, FORMAT),
            upsample: get(Pass::Upsample, FORMAT),
            combine: get(Pass::Combine, target.main_texture_format()),
        };
        commands.entity(entity).insert(ids);
    }
}

/// The chain's targets: `a[i]` the downsamples (1/2 … 1/32), `b[i]` the upsamples (1/2 … 1/16).
#[derive(Component)]
pub struct BloomTextures {
    a: Vec<CachedTexture>,
    b: Vec<CachedTexture>,
    screen: UVec2,
}

fn prepare_textures(
    mut commands: Commands,
    mut cache: ResMut<TextureCache>,
    device: Res<RenderDevice>,
    views: Query<(Entity, &ExtractedCamera, &GameBloom)>,
) {
    for (entity, camera, bloom) in &views {
        let Some(screen) = camera.physical_viewport_size.filter(|_| bloom.enabled) else {
            continue;
        };
        let mut texture = |label: &'static str, div: u32| {
            cache.get(
                &device,
                TextureDescriptor {
                    label: Some(label),
                    size: (screen / div).max(UVec2::ONE).to_extents(),
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: TextureDimension::D2,
                    format: FORMAT,
                    usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
            )
        };
        let a = LEVELS
            .iter()
            .map(|&d| texture("bg3 bloom down", d))
            .collect();
        let b = LEVELS[..4]
            .iter()
            .map(|&d| texture("bg3 bloom up", d))
            .collect();
        commands
            .entity(entity)
            .insert(BloomTextures { a, b, screen });
    }
}

/// Run the chain on the view's HDR target.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn bloom(
    view: ViewQuery<(
        &ViewTarget,
        &ViewUniformOffset,
        &GameBloom,
        Option<&BloomPipelineIds>,
        Option<&BloomTextures>,
    )>,
    cache: Res<PipelineCache>,
    pipelines: Option<Res<BloomPipelines>>,
    view_uniforms: Res<ViewUniforms>,
    state: Option<Res<ExposureBuffer>>,
    buffers: Res<RenderAssets<GpuShaderBuffer>>,
    mut ctx: RenderContext,
) {
    let (target, offset, bloom, ids, textures) = view.into_inner();
    let (Some(ids), Some(textures), Some(pipelines)) = (ids, textures, pipelines) else {
        return;
    };
    if !bloom.enabled {
        return;
    }
    let get = |id| cache.get_render_pipeline(id);
    let (Some(threshold), Some(downsample), Some(upsample), Some(combine)) = (
        get(ids.threshold),
        get(ids.downsample),
        get(ids.upsample),
        get(ids.combine),
    ) else {
        return;
    };
    let (Some(state), Some(views)) = (
        state.and_then(|s| buffers.get(&s.0)),
        view_uniforms.uniforms.binding(),
    ) else {
        return;
    };
    let layout = cache.get_bind_group_layout(&pipelines.layout);
    let scale = textures.screen.y as f32 / REFERENCE_HEIGHT;
    let device = ctx.render_device().clone();
    let uniform = |div: u32| {
        let size = (textures.screen / div).max(UVec2::ONE).as_vec2();
        let u = BloomUniform {
            params: Vec4::new(bloom.threshold, bloom.amount, scale, COMBINE_WEIGHT),
            texel: Vec4::new(1.0 / size.x, 1.0 / size.y, 0.0, 0.0),
        };
        let mut bytes = encase::UniformBuffer::new(Vec::<u8>::new());
        bytes.write(&u).expect("bloom uniform");
        device.create_buffer_with_data(&BufferInitDescriptor {
            label: Some("bg3 bloom uniform"),
            contents: bytes.as_ref(),
            usage: BufferUsages::UNIFORM,
        })
    };
    let views_binding = views.clone();
    let draw = |ctx: &mut RenderContext,
                pipeline: &RenderPipeline,
                base: &TextureView,
                base2: &TextureView,
                div: u32,
                dest: &TextureView| {
        let u = uniform(div);
        let bind_group = device.create_bind_group(
            Some("bg3 bloom bind group"),
            &layout,
            &BindGroupEntries::sequential((
                base,
                base2,
                &pipelines.sampler,
                views_binding.clone(),
                u.as_entire_binding(),
                state.buffer.as_entire_buffer_binding(),
            )),
        );
        let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
            label: Some("bg3 bloom"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: dest,
                depth_slice: None,
                resolve_target: None,
                ops: Operations::default(),
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_render_pipeline(pipeline);
        pass.set_bind_group(0, &bind_group, &[offset.offset]);
        pass.draw(0..3, 0..1);
    };
    let a = &textures.a;
    let b = &textures.b;
    let scene = target.main_texture_view();
    draw(
        &mut ctx,
        threshold,
        scene,
        scene,
        LEVELS[0],
        &a[0].default_view,
    );
    for i in 1..LEVELS.len() {
        draw(
            &mut ctx,
            downsample,
            &a[i - 1].default_view,
            &a[i - 1].default_view,
            LEVELS[i],
            &a[i].default_view,
        );
    }
    for i in (0..4).rev() {
        let lower = if i == 3 { &a[4] } else { &b[i + 1] };
        draw(
            &mut ctx,
            upsample,
            &a[i].default_view,
            &lower.default_view,
            LEVELS[i],
            &b[i].default_view,
        );
    }
    let post = target.post_process_write();
    draw(
        &mut ctx,
        combine,
        post.source,
        &b[0].default_view,
        1,
        post.destination,
    );
}
