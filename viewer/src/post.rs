//! The game's exposure and tonemapping in place of Bevy's (`shaders/post.wesl`).
//!
//! Auto-exposure: a luminance histogram of the HDR target each frame, metered into an EV100
//! that adapts over time and is clamped to the atmosphere's `ExposureMin`/`ExposureMax`. The
//! GPU keeps the adapted EV in a small buffer; it is read back and becomes the camera's
//! `Exposure` (so lights are pre-exposed, as in the game), and the tonemap pass corrects for
//! the frames of readback latency. Tonemapping: see the shader. Everything ported is described
//! in `docs/research/lighting.md` ("Exposure" and "Tonemapping").
//!
//! bg3: ls::PostProcessStep_Exposure, ls::PostProcessStep_ToneMap, PPHistogram, PPCalculateEV,
//! PPToneMap_LUT (Core.metallib) (4.1.1.7631656)

use bevy::asset::{RenderAssetUsages, embedded_asset, load_embedded_asset};
use bevy::camera::Exposure;
use bevy::core_pipeline::tonemapping::{Tonemapping, tonemapping};
use bevy::core_pipeline::{Core3d, Core3dSystems, FullscreenShader};
use bevy::prelude::*;
use bevy::render::extract_component::{ExtractComponent, ExtractComponentPlugin};
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::globals::GlobalsBuffer;
use bevy::render::gpu_readback::{Readback, ReadbackComplete};
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::binding_types::{
    sampler, storage_buffer, storage_buffer_read_only, texture_2d, texture_3d, uniform_buffer,
};
use bevy::render::render_resource::*;
use bevy::render::renderer::{RenderContext, RenderDevice, ViewQuery};
use bevy::render::storage::{GpuShaderBuffer, ShaderBuffer};
use bevy::render::view::{ExtractedView, ViewTarget, ViewUniform, ViewUniformOffset, ViewUniforms};
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};
use bevy::shader::ShaderDefVal;

/// Histogram bins (`PPHistogram`).
pub const BINS: usize = 256;

/// The game's SDR display LUT (64³ `R10G10B10A2`, output sRGB-encoded), read from the install.
pub const DISPLAY_LUT: &str = "Engine/Textures/ZCAM_SDR_4.dds";
const LUT_SIZE: u32 = 64;

/// Exposure and tonemapping settings of a camera, from the level's atmosphere.
#[derive(Component, Clone, ExtractComponent)]
#[extract_app(RenderApp)]
pub struct GamePost {
    pub uniform: PostUniform,
}

/// Parameters shared by the exposure and tonemap shaders (`post.wesl` `PostUniform`).
#[derive(Clone, Copy, ShaderType, Default, Debug)]
pub struct PostUniform {
    /// (ExposureMin, ExposureMax, ExposureCompensation, auto-exposure on (1) / fixed (0)).
    pub exposure: Vec4,
    /// (min percentile, max percentile, metering mode, fixed EV100).
    pub metering: Vec4,
    /// (histogram EV minimum, EV range, EV texture width, height).
    pub histogram: Vec4,
    /// (Contrast, Highlights, Shadows, Pivot).
    pub grading: Vec4,
    /// (Brightness, Saturation, Hue in degrees, 0).
    pub grading2: Vec4,
    /// Global, Dark, Shadow, Midtones, Highlight, Specular range:
    /// (XYOffset.x, XYOffset.y, Saturation, 1.35^ExposureShift).
    pub ranges: [Vec4; 6],
}

/// Adapted exposure state, on the GPU (`post.wesl` `ExposureState`).
#[derive(Clone, Copy, ShaderType, Debug)]
pub struct ExposureState {
    /// EV100 the GPU adapted to.
    pub ev100: f32,
    /// Frames seen (0 = start: jump to the target).
    pub frames: u32,
    pub _pad: Vec2,
    pub histogram: [UVec4; BINS / 4],
}

impl Default for ExposureState {
    fn default() -> Self {
        Self {
            ev100: 0.0,
            frames: 0,
            _pad: Vec2::ZERO,
            histogram: [UVec4::ZERO; BINS / 4],
        }
    }
}

/// The GPU exposure state buffer (main world handle, read back each frame).
#[derive(Resource, Clone, ExtractResource)]
#[extract_app(RenderApp)]
pub struct ExposureBuffer(pub Handle<ShaderBuffer>);

/// The display LUT image.
#[derive(Resource, Clone, ExtractResource)]
#[extract_app(RenderApp)]
pub struct DisplayLut(pub Handle<Image>);

/// Load the display LUT (`DISPLAY_LUT`) from the game's files as a 3D texture.
pub fn load_display_lut(vfs: &bg3_vfs::Vfs) -> anyhow::Result<Image> {
    use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
    let dds = vfs.read(DISPLAY_LUT)?;
    let h = bg3_material::dds::DdsHeader::parse(&dds)
        .ok_or_else(|| anyhow::anyhow!("{DISPLAY_LUT}: not a DDS file"))?;
    anyhow::ensure!(
        h.format == bg3_material::dds::DdsFormat::Dxgi(24)
            && (h.width, h.height, h.depth) == (LUT_SIZE, LUT_SIZE, LUT_SIZE),
        "{DISPLAY_LUT}: unexpected layout {h:?}"
    );
    let bytes = (LUT_SIZE * LUT_SIZE * LUT_SIZE * 4) as usize;
    let data = dds
        .get(h.data_offset()..h.data_offset() + bytes)
        .ok_or_else(|| anyhow::anyhow!("{DISPLAY_LUT}: truncated"))?;
    let mut image = Image::new_uninit(
        Extent3d {
            width: LUT_SIZE,
            height: LUT_SIZE,
            depth_or_array_layers: LUT_SIZE,
        },
        TextureDimension::D3,
        TextureFormat::Rgb10a2Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.data = Some(data.to_vec());
    let clamp = ImageAddressMode::ClampToEdge;
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: clamp,
        address_mode_v: clamp,
        address_mode_w: clamp,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..default()
    });
    Ok(image)
}

/// Whether the game's post-processing replaces Bevy's (`BG3_BEVY_POST=1` keeps Bevy's
/// tonemapping and a fixed exposure, for comparisons).
pub fn enabled() -> bool {
    std::env::var_os("BG3_BEVY_POST").is_none()
}

pub struct GamePostPlugin;

impl Plugin for GamePostPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "shaders/post.wesl");
        let state = ExposureState::default();
        let mut bytes = encase::StorageBuffer::new(Vec::<u8>::new());
        bytes.write(&state).expect("exposure state");
        let mut buffer = ShaderBuffer::new(bytes.into_inner(), RenderAssetUsages::RENDER_WORLD);
        buffer.buffer_usage |= BufferUsages::COPY_SRC;
        let handle = app
            .world_mut()
            .resource_mut::<Assets<ShaderBuffer>>()
            .add(buffer);
        app.insert_resource(ExposureBuffer(handle))
            .add_plugins((
                ExtractComponentPlugin::<GamePost>::default(),
                ExtractResourcePlugin::<ExposureBuffer>::default(),
                ExtractResourcePlugin::<DisplayLut>::default(),
            ))
            .add_systems(Startup, (start_readback, upload_display_lut));
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .init_resource::<SpecializedRenderPipelines<PostPipelines>>()
            .add_systems(RenderStartup, init_pipelines)
            .add_systems(Render, prepare_pipelines.in_set(RenderSystems::Prepare))
            .add_systems(
                Core3d,
                (exposure, tonemap)
                    .chain()
                    .before(tonemapping)
                    .in_set(Core3dSystems::PostProcess),
            );
    }
}

/// Read the adapted EV back every frame and give it to the cameras.
fn start_readback(mut commands: Commands, buffer: Res<ExposureBuffer>) {
    commands
        .spawn((
            Name::new("exposure readback"),
            Readback::buffer_range(buffer.0.clone(), 0, 16),
        ))
        .observe(
            |event: On<ReadbackComplete>, mut cameras: Query<&mut Exposure, With<GamePost>>| {
                let word = |i: usize| -> Option<[u8; 4]> { event.get(i..i + 4)?.try_into().ok() };
                let (Some(ev), Some(frames)) = (word(0), word(4)) else {
                    return;
                };
                let (ev, frames) = (f32::from_le_bytes(ev), u32::from_le_bytes(frames));
                if frames == 0 || !ev.is_finite() {
                    return;
                }
                for mut e in &mut cameras {
                    e.ev100 = ev;
                }
            },
        );
}

fn upload_display_lut(
    mut commands: Commands,
    data: Res<crate::GameData>,
    mut images: ResMut<Assets<Image>>,
) {
    match load_display_lut(&data.vfs) {
        Ok(image) => commands.insert_resource(DisplayLut(images.add(image))),
        Err(e) => eprintln!("post: no display LUT ({e}); the image stays black"),
    }
}

/// The game's EV texture size for a screen size (`PostProcessStep_Exposure::OnSizeChanged`).
pub fn ev_texture_size(screen: UVec2) -> Vec2 {
    Vec2::new(
        if screen.x > 2560 { 1280.0 } else { 640.0 },
        if screen.y > 1440 { 768.0 } else { 384.0 },
    )
}

/// Put the game's post-processing on a camera: Bevy's tonemapping off, the initial exposure.
pub fn camera_bundle(uniform: PostUniform) -> impl Bundle {
    (
        GamePost { uniform },
        Tonemapping::None,
        Exposure {
            ev100: uniform.exposure.x,
        },
    )
}

#[derive(Resource)]
struct PostPipelines {
    exposure_layout: BindGroupLayoutDescriptor,
    histogram: CachedComputePipelineId,
    average: CachedComputePipelineId,
    tonemap_layout: BindGroupLayoutDescriptor,
    sampler: Sampler,
    fullscreen: FullscreenShader,
    shader: Handle<Shader>,
}

fn init_pipelines(
    mut commands: Commands,
    device: Res<RenderDevice>,
    fullscreen: Res<FullscreenShader>,
    assets: Res<AssetServer>,
    cache: Res<PipelineCache>,
) {
    let shader: Handle<Shader> = load_embedded_asset!(assets.as_ref(), "shaders/post.wesl");
    let exposure_layout = BindGroupLayoutDescriptor::new(
        "bg3 exposure layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                uniform_buffer::<bevy::render::globals::GlobalsUniform>(false),
                uniform_buffer::<ViewUniform>(true),
                uniform_buffer::<PostUniform>(false),
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                storage_buffer::<ExposureState>(false),
            ),
        ),
    );
    let compute = |entry: &'static str| ComputePipelineDescriptor {
        label: Some(format!("bg3 {entry}").into()),
        layout: vec![exposure_layout.clone()],
        shader: shader.clone(),
        shader_defs: vec![ShaderDefVal::from("EXPOSURE")],
        entry_point: Some(entry.into()),
        ..default()
    };
    let histogram = cache.queue_compute_pipeline(compute("histogram"));
    let average = cache.queue_compute_pipeline(compute("average"));
    let tonemap_layout = BindGroupLayoutDescriptor::new(
        "bg3 tonemap layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                uniform_buffer::<ViewUniform>(true),
                uniform_buffer::<PostUniform>(false),
                storage_buffer_read_only::<ExposureState>(false),
                texture_3d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
            ),
        ),
    );
    commands.insert_resource(PostPipelines {
        exposure_layout,
        histogram,
        average,
        tonemap_layout,
        sampler: device.create_sampler(&SamplerDescriptor {
            min_filter: FilterMode::Linear,
            mag_filter: FilterMode::Linear,
            ..default()
        }),
        fullscreen: fullscreen.clone(),
        shader,
    });
}

impl SpecializedRenderPipeline for PostPipelines {
    type Key = TextureFormat;

    fn specialize(&self, format: TextureFormat) -> RenderPipelineDescriptor {
        RenderPipelineDescriptor {
            label: Some("bg3 tonemap".into()),
            layout: vec![self.tonemap_layout.clone()],
            vertex: self.fullscreen.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                shader_defs: vec![],
                entry_point: Some("tonemap".into()),
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

#[derive(Component)]
struct TonemapPipelineId(CachedRenderPipelineId);

fn prepare_pipelines(
    mut commands: Commands,
    cache: Res<PipelineCache>,
    mut specialized: ResMut<SpecializedRenderPipelines<PostPipelines>>,
    pipelines: Option<Res<PostPipelines>>,
    views: Query<(Entity, &ExtractedView), With<GamePost>>,
) {
    let Some(p) = pipelines else {
        return;
    };
    for (entity, view) in &views {
        let id = specialized.specialize(&cache, &p, view.target_format);
        commands.entity(entity).insert(TonemapPipelineId(id));
    }
}

fn post_uniform(device: &RenderDevice, u: &PostUniform) -> Option<Buffer> {
    let mut bytes = encase::UniformBuffer::new(Vec::<u8>::new());
    bytes.write(u).ok()?;
    Some(device.create_buffer_with_data(&BufferInitDescriptor {
        label: Some("bg3 post uniform"),
        contents: bytes.as_ref(),
        usage: BufferUsages::UNIFORM,
    }))
}

#[allow(clippy::too_many_arguments)]
fn exposure(
    view: ViewQuery<(&ViewTarget, &ViewUniformOffset, &GamePost)>,
    cache: Res<PipelineCache>,
    pipelines: Res<PostPipelines>,
    view_uniforms: Res<ViewUniforms>,
    globals: Res<GlobalsBuffer>,
    state: Option<Res<ExposureBuffer>>,
    buffers: Res<RenderAssets<GpuShaderBuffer>>,
    mut ctx: RenderContext,
) {
    let (target, offset, post) = view.into_inner();
    let (Some(histogram), Some(average)) = (
        cache.get_compute_pipeline(pipelines.histogram),
        cache.get_compute_pipeline(pipelines.average),
    ) else {
        return;
    };
    let (Some(state), Some(views), Some(globals)) = (
        state.and_then(|s| buffers.get(&s.0)),
        view_uniforms.uniforms.binding(),
        globals.buffer.binding(),
    ) else {
        return;
    };
    let Some(uniform) = post_uniform(ctx.render_device(), &post.uniform) else {
        return;
    };
    let bind_group = ctx.render_device().create_bind_group(
        Some("bg3 exposure bind group"),
        &cache.get_bind_group_layout(&pipelines.exposure_layout),
        &BindGroupEntries::sequential((
            globals,
            views,
            uniform.as_entire_binding(),
            target.main_texture_view(),
            &pipelines.sampler,
            state.buffer.as_entire_buffer_binding(),
        )),
    );
    let size = post.uniform.histogram.zw().as_uvec2();
    let mut pass = ctx
        .command_encoder()
        .begin_compute_pass(&ComputePassDescriptor {
            label: Some("bg3 exposure"),
            timestamp_writes: None,
        });
    pass.set_bind_group(0, &bind_group, &[offset.offset]);
    pass.set_pipeline(histogram);
    pass.dispatch_workgroups(size.x.div_ceil(16), size.y.div_ceil(16), 1);
    pass.set_pipeline(average);
    pass.dispatch_workgroups(1, 1, 1);
}

#[allow(clippy::too_many_arguments)]
fn tonemap(
    view: ViewQuery<(
        &ViewTarget,
        &ViewUniformOffset,
        &GamePost,
        &TonemapPipelineId,
    )>,
    cache: Res<PipelineCache>,
    pipelines: Res<PostPipelines>,
    view_uniforms: Res<ViewUniforms>,
    state: Option<Res<ExposureBuffer>>,
    lut: Option<Res<DisplayLut>>,
    buffers: Res<RenderAssets<GpuShaderBuffer>>,
    images: Res<RenderAssets<bevy::render::texture::GpuImage>>,
    mut ctx: RenderContext,
) {
    let (target, offset, post, id) = view.into_inner();
    let Some(pipeline) = cache.get_render_pipeline(id.0) else {
        return;
    };
    let Some(lut) = lut.and_then(|l| images.get(&l.0)) else {
        return;
    };
    let (Some(state), Some(views)) = (
        state.and_then(|s| buffers.get(&s.0)),
        view_uniforms.uniforms.binding(),
    ) else {
        return;
    };
    let Some(uniform) = post_uniform(ctx.render_device(), &post.uniform) else {
        return;
    };
    let post_write = target.post_process_write();
    let bind_group = ctx.render_device().create_bind_group(
        Some("bg3 tonemap bind group"),
        &cache.get_bind_group_layout(&pipelines.tonemap_layout),
        &BindGroupEntries::sequential((
            post_write.source,
            &pipelines.sampler,
            views,
            uniform.as_entire_binding(),
            state.buffer.as_entire_buffer_binding(),
            &lut.texture_view,
            &lut.sampler,
        )),
    );
    let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
        label: Some("bg3 tonemap"),
        color_attachments: &[Some(RenderPassColorAttachment {
            view: post_write.destination,
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
}
