//! Shader-graph material resources (`Public/**/Assets/Materials/**.lsf`, region `Material`).

use bg3_lsf::{Node, Resource};

use crate::params::{Param, read_params};

/// How a material's pixels combine with what's behind them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlendMode {
    Opaque,
    /// Opaque, but the base colour's alpha decides coverage.
    AlphaTest,
    /// Alpha-blended.
    Blend,
}

/// The render-state flags and parameter defaults of a shader graph.
#[derive(Debug, Clone, PartialEq)]
pub struct ShaderGraph {
    pub shading_model: u8,
    pub blend_state: u8,
    pub rasterizer_state: u8,
    pub depth_state: u8,
    pub material_type: u8,
    pub material_usage: u8,
    pub uv_count: u32,
    pub discards_pixels: bool,
    pub vertex_color: bool,
    pub params: Vec<Param>,
}

impl ShaderGraph {
    /// Read the `Material` region of a shader-graph resource.
    pub fn from_resource(res: &Resource) -> Option<Self> {
        let node = res.region("Material")?;
        let node = if node.name == "Material" {
            node
        } else {
            node.child("Material")?
        };
        Some(Self::from_node(node))
    }

    pub fn from_node(node: &Node) -> Self {
        let int = |name| node.attr(name).and_then(|v| v.as_i64()).unwrap_or(0);
        let flag = |name| node.attr(name).and_then(|v| v.as_bool()).unwrap_or(false);
        Self {
            shading_model: int("ShadingModel") as u8,
            blend_state: int("BlendStateID") as u8,
            rasterizer_state: int("RasterizerStateID") as u8,
            depth_state: int("DepthStateID") as u8,
            material_type: int("MaterialType") as u8,
            material_usage: int("MaterialUsage") as u8,
            uv_count: int("UVCount") as u32,
            discards_pixels: flag("IsDiscardingPixels"),
            vertex_color: flag("VertexColorEnabled"),
            params: read_params(node),
        }
    }

    /// Whether back faces are drawn: rasterizer state 0 is used by every `*_2S`/`*_2s` graph,
    /// state 1 by the one-sided ones.
    pub fn two_sided(&self) -> bool {
        self.rasterizer_state == 0
    }

    /// Whether the graph alpha-blends: blend state 0 is used by the `*Alphablend*` effect
    /// graphs, state 7 by opaque and alpha-tested ones. Other states are not mapped yet.
    pub fn alpha_blended(&self) -> bool {
        self.blend_state == 0
    }

    /// Unlit shading model (`Helper_Color`, effect overlays).
    pub fn unlit(&self) -> bool {
        self.shading_model == 1
    }
}
