//! Screen-space subsurface scattering for character skin (`shaders/sss.wesl`).
//!
//! The game lights subsurface pixels (shading model 3) without their albedo and then blurs that
//! irradiance in two separable passes with a 25-tap kernel per diffusion profile, multiplying by
//! the albedo at the end. Here the character shader marks subsurface pixels in the HDR target's
//! alpha (kernel index and amount) and this pass blurs the lit colour of those pixels, after the
//! main passes and before bloom and tonemapping. See `docs/research/character-shaders.md`.
//!
//! bg3: PPSSSS_H, PPSSSS_V (Core.metallib), ls::SubStage_SSSS (4.1.1.7631656)

use bevy::asset::{embedded_asset, load_embedded_asset};
use bevy::core_pipeline::{Core3d, Core3dSystems, FullscreenShader};
use bevy::prelude::*;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::render_resource::binding_types::{
    sampler, texture_2d, texture_depth_2d, texture_depth_2d_multisampled, uniform_buffer,
};
use bevy::render::render_resource::*;
use bevy::render::renderer::{RenderContext, RenderDevice, ViewQuery};
use bevy::render::view::{ExtractedView, Msaa, ViewDepthStencilTexture, ViewTarget};
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};
use bevy::shader::ShaderDefVal;
use bg3_material::character::sss::{KERNEL_TAPS, MAX_PROFILES, SssProfiles};

/// The scattering kernels and widths of the diffusion profiles characters use.
#[derive(Resource, Clone, ShaderType)]
pub struct SssUniform {
    kernel: [[Vec4; KERNEL_TAPS]; MAX_PROFILES],
    profile: [Vec4; MAX_PROFILES],
    camera: Vec4,
}

/// Main-world kernels, extracted every frame.
#[derive(Resource, Clone)]
pub struct SssKernels(pub SssUniform);

impl ExtractResource<RenderApp> for SssKernels {
    type Source = SssKernels;
    fn extract_resource(source: &Self::Source) -> Self {
        source.clone()
    }
}

impl SssKernels {
    pub fn new(profiles: &SssProfiles) -> Self {
        let profiles = profiles.kernels();
        let mut u = SssUniform {
            kernel: [[Vec4::ZERO; KERNEL_TAPS]; MAX_PROFILES],
            profile: [Vec4::ZERO; MAX_PROFILES],
            camera: Vec4::ZERO,
        };
        for (i, p) in profiles.iter().take(MAX_PROFILES).enumerate() {
            for (t, k) in p.kernel.iter().enumerate() {
                u.kernel[i][t] = Vec4::from_array(*k);
            }
            u.profile[i] = Vec4::new(p.scatter_distance, 0.0, 0.0, 0.0);
        }
        Self(u)
    }
}

/// Whether the pass runs (`BG3_NO_SSS=1` turns it off for comparisons).
pub fn enabled() -> bool {
    std::env::var_os("BG3_NO_SSS").is_none()
}

pub struct SssPlugin;

impl Plugin for SssPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "shaders/sss.wesl");
        app.insert_resource(SssKernels::new(&SssProfiles::default()))
            .add_plugins(ExtractResourcePlugin::<SssKernels>::default());
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .init_resource::<SpecializedRenderPipelines<SssPipeline>>()
            .add_systems(RenderStartup, init_pipeline)
            .add_systems(Render, prepare_pipelines.in_set(RenderSystems::Prepare))
            .add_systems(
                Core3d,
                subsurface_scattering
                    .after(Core3dSystems::MainPass)
                    .before(Core3dSystems::EarlyPostProcess),
            );
    }
}

#[derive(Resource)]
struct SssPipeline {
    layout: BindGroupLayoutDescriptor,
    layout_ms: BindGroupLayoutDescriptor,
    sampler: Sampler,
    fullscreen: FullscreenShader,
    shader: Handle<Shader>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct SssKey {
    format: TextureFormat,
    multisampled: bool,
    vertical: bool,
}

fn layout(multisampled: bool) -> BindGroupLayoutDescriptor {
    let depth = if multisampled {
        texture_depth_2d_multisampled()
    } else {
        texture_depth_2d()
    };
    BindGroupLayoutDescriptor::new(
        "bg3 sss layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                depth,
                uniform_buffer::<SssUniform>(false),
            ),
        ),
    )
}

fn init_pipeline(
    mut commands: Commands,
    device: Res<RenderDevice>,
    fullscreen: Res<FullscreenShader>,
    assets: Res<AssetServer>,
) {
    commands.insert_resource(SssPipeline {
        layout: layout(false),
        layout_ms: layout(true),
        sampler: device.create_sampler(&SamplerDescriptor {
            min_filter: FilterMode::Linear,
            mag_filter: FilterMode::Linear,
            ..default()
        }),
        fullscreen: fullscreen.clone(),
        shader: load_embedded_asset!(assets.as_ref(), "shaders/sss.wesl"),
    });
}

impl SpecializedRenderPipeline for SssPipeline {
    type Key = SssKey;

    fn specialize(&self, key: SssKey) -> RenderPipelineDescriptor {
        let mut defs: Vec<ShaderDefVal> = Vec::new();
        if key.multisampled {
            defs.push("MULTISAMPLED".into());
        }
        if key.vertical {
            defs.push("VERTICAL".into());
        }
        RenderPipelineDescriptor {
            label: Some("bg3 sss".into()),
            layout: vec![if key.multisampled {
                self.layout_ms.clone()
            } else {
                self.layout.clone()
            }],
            vertex: self.fullscreen.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                shader_defs: defs,
                targets: vec![Some(ColorTargetState {
                    format: key.format,
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
struct SssPipelineIds {
    horizontal: CachedRenderPipelineId,
    vertical: CachedRenderPipelineId,
}

fn prepare_pipelines(
    mut commands: Commands,
    cache: Res<PipelineCache>,
    mut pipelines: ResMut<SpecializedRenderPipelines<SssPipeline>>,
    pipeline: Option<Res<SssPipeline>>,
    views: Query<(Entity, &ExtractedView, &Msaa)>,
) {
    let Some(pipeline) = pipeline else {
        return;
    };
    for (entity, view, msaa) in &views {
        // The marks live in alpha and the blur needs linear values: HDR targets only.
        if view.target_format != TextureFormat::Rgba16Float || !enabled() {
            continue;
        }
        let key = |vertical| SssKey {
            format: view.target_format,
            multisampled: msaa.samples() > 1,
            vertical,
        };
        commands.entity(entity).insert(SssPipelineIds {
            horizontal: pipelines.specialize(&cache, &pipeline, key(false)),
            vertical: pipelines.specialize(&cache, &pipeline, key(true)),
        });
    }
}

fn subsurface_scattering(
    view: ViewQuery<(
        &ViewTarget,
        &ViewDepthStencilTexture,
        &ExtractedView,
        &SssPipelineIds,
        &Msaa,
    )>,
    cache: Res<PipelineCache>,
    pipeline: Res<SssPipeline>,
    kernels: Res<SssKernels>,
    mut ctx: RenderContext,
) {
    let (target, depth, extracted, ids, msaa) = view.into_inner();
    let (Some(horizontal), Some(vertical)) = (
        cache.get_render_pipeline(ids.horizontal),
        cache.get_render_pipeline(ids.vertical),
    ) else {
        return;
    };
    let mut u = kernels.0.clone();
    let proj = extracted.clip_from_view;
    u.camera = Vec4::new(proj.x_axis.x, proj.y_axis.y, proj.w_axis.z, 0.0);
    let mut buffer = encase::UniformBuffer::new(Vec::<u8>::new());
    if buffer.write(&u).is_err() {
        return;
    }
    let uniform = ctx
        .render_device()
        .create_buffer_with_data(&BufferInitDescriptor {
            label: Some("bg3 sss uniform"),
            contents: buffer.as_ref(),
            usage: BufferUsages::UNIFORM,
        });
    let layout = if msaa.samples() > 1 {
        &pipeline.layout_ms
    } else {
        &pipeline.layout
    };
    let depth_view = &depth.attachment.texture.default_view;
    for pass in [horizontal, vertical] {
        let post = target.post_process_write();
        let bind_group = ctx.render_device().create_bind_group(
            Some("bg3 sss bind group"),
            &cache.get_bind_group_layout(layout),
            &BindGroupEntries::sequential((
                post.source,
                &pipeline.sampler,
                depth_view,
                uniform.as_entire_binding(),
            )),
        );
        let mut render_pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
            label: Some("bg3 sss"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: post.destination,
                depth_slice: None,
                resolve_target: None,
                ops: Operations::default(),
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        render_pass.set_render_pipeline(pass);
        render_pass.set_bind_group(0, &bind_group, &[]);
        render_pass.draw(0..3, 0..1);
    }
}
