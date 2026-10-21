//! Merge a material instance over its shader graph and classify texture slots.

use crate::graph::{BlendMode, ShaderGraph};
use crate::params::{Param, ParamValue};

/// What a 2D texture parameter feeds, as far as a PBR approximation is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Slot {
    BaseColor,
    Normal,
    /// Physical map: R metalness, G roughness, B occlusion (see [`crate::convert`]).
    Physical,
    /// Glow map, multiplied by the glow colour.
    Emissive,
    /// Anything else (detail maps, masks, noise, gradients, …): not mapped yet.
    Other,
}

/// A resolved 2D texture parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct TextureRef {
    pub param: String,
    /// `TextureBank` ID.
    pub id: String,
    pub slot: Slot,
}

/// A material with instance overrides applied.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedMaterial {
    pub blend: BlendMode,
    pub two_sided: bool,
    pub unlit: bool,
    /// Helper geometry that the game never draws (`Helper_Invisible`, `*_Invisible_*` overlays).
    pub invisible: bool,
    pub textures: Vec<TextureRef>,
    /// `VirtualTextureBank` IDs; the main maps of `*_VT` graphs come from these.
    pub virtual_textures: Vec<String>,
    pub scalars: Vec<(String, f32)>,
    pub vectors: Vec<(String, Vec<f32>)>,
}

impl ResolvedMaterial {
    /// The texture in `slot`, if any (the first one when a graph has several).
    pub fn texture(&self, slot: Slot) -> Option<&TextureRef> {
        self.textures.iter().find(|t| t.slot == slot)
    }

    pub fn scalar(&self, name: &str) -> Option<f32> {
        self.scalars
            .iter()
            .find(|(n, _)| n == name)
            .map(|&(_, v)| v)
    }

    pub fn vector(&self, name: &str) -> Option<&[f32]> {
        self.vectors
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_slice())
    }

    /// Whether the main surface maps come from a virtual texture.
    pub fn needs_virtual_texture(&self) -> bool {
        !self.virtual_textures.is_empty()
    }

    /// A flat colour parameter (`Helper_Color*` graphs), linear RGB.
    pub fn flat_color(&self) -> Option<[f32; 3]> {
        ["Colour", "Color"]
            .iter()
            .find_map(|n| self.vector(n))
            .and_then(rgb)
    }

    /// Glow colour × strength for the glow map, linear RGB. First approximation: the pulse
    /// and gradient animations of the `*_GM_*` graphs are ignored and the peak value is used.
    pub fn glow(&self) -> Option<[f32; 3]> {
        self.texture(Slot::Emissive)?;
        let color = ["GlowColour", "GlowColor", "GlowMap_ColorMult"]
            .iter()
            .find_map(|n| self.vector(n))
            .and_then(rgb)
            .unwrap_or([1.0; 3]);
        let strength = ["GlowMultiplier", "GlowIntensity", "EmissiveColor_Mult"]
            .iter()
            .find_map(|n| self.scalar(n))
            .unwrap_or(1.0)
            * ["MaxGlow", "GlowMax", "GlowMapPulse_MaxGlow"]
                .iter()
                .find_map(|n| self.scalar(n))
                .unwrap_or(1.0);
        Some(color.map(|c| c * strength))
    }
}

fn rgb(v: &[f32]) -> Option<[f32; 3]> {
    Some([*v.first()?, *v.get(1)?, *v.get(2)?])
}

fn same_param(a: &Param, b: &Param) -> bool {
    std::mem::discriminant(&a.value) == std::mem::discriminant(&b.value)
        && if a.name.is_empty() && b.name.is_empty() {
            // Unnamed graph inputs are matched by their default ID.
            a.value == b.value
        } else {
            a.name == b.name
        }
}

/// Apply the instance's enabled parameters over the defaults of the graph at `graph_path`.
///
/// Instance parameters the graph doesn't declare are kept (harmless for classification, handy
/// for diagnostics). Without a graph, every instance parameter is taken as is.
pub fn resolve(
    graph_path: &str,
    graph: Option<&ShaderGraph>,
    instance: &[Param],
) -> ResolvedMaterial {
    let mut params: Vec<Param> = graph.map(|g| g.params.clone()).unwrap_or_default();
    for p in instance {
        match params.iter_mut().find(|q| same_param(q, p)) {
            Some(q) if p.enabled => q.value = p.value.clone(),
            Some(_) => {}
            None => params.push(p.clone()),
        }
    }
    let file = graph_path.rsplit('/').next().unwrap_or(graph_path);
    // Alpha testing: the depth pass samples the base colour (its alpha decides coverage).
    let alpha_tested = params.iter().any(|p| {
        p.depth_pass
            && matches!(p.value, ParamValue::Texture(_))
            && classify(&p.name) == Slot::BaseColor
    });
    let blend = match graph {
        Some(g) if g.alpha_blended() => BlendMode::Blend,
        // Glass graphs are alpha-tested on a mostly-transparent BMA; the game dithers them
        // into see-through glass. Blending is the closer approximation.
        _ if alpha_tested && file.contains("Glass") => BlendMode::Blend,
        _ if alpha_tested => BlendMode::AlphaTest,
        _ => BlendMode::Opaque,
    };
    let mut out = ResolvedMaterial {
        blend,
        two_sided: graph.is_some_and(ShaderGraph::two_sided),
        unlit: graph.is_some_and(ShaderGraph::unlit),
        invisible: file.contains("Invisible"),
        textures: Vec::new(),
        virtual_textures: Vec::new(),
        scalars: Vec::new(),
        vectors: Vec::new(),
    };
    for p in params {
        match p.value {
            ParamValue::Scalar(v) => out.scalars.push((p.name, v)),
            ParamValue::Vector(v) => out.vectors.push((p.name, v)),
            ParamValue::VirtualTexture(id) => out.virtual_textures.push(id),
            ParamValue::Texture(id) => out.textures.push(TextureRef {
                slot: classify(&p.name),
                param: p.name,
                id,
            }),
        }
    }
    out
}

/// Classify a `Texture2DParameters` name into a [`Slot`].
///
/// Names are free-form per graph; this covers the names used by the graphs the Nautiloid uses
/// (survey in `docs/research/materials.md`).
pub fn classify(param: &str) -> Slot {
    match param.to_ascii_lowercase().as_str() {
        "basecolor" | "basemap" => Slot::BaseColor,
        "normalmap" => Slot::Normal,
        "physicalmap" => Slot::Physical,
        "glowmap" => Slot::Emissive,
        _ => Slot::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(name: &str, value: ParamValue, enabled: bool) -> Param {
        Param {
            name: name.into(),
            value,
            enabled,
            depth_pass: false,
        }
    }

    fn graph(params: Vec<Param>) -> ShaderGraph {
        ShaderGraph {
            shading_model: 0,
            blend_state: 7,
            rasterizer_state: 1,
            depth_state: 5,
            material_type: 0,
            material_usage: 0,
            uv_count: 1,
            discards_pixels: true,
            vertex_color: false,
            params,
        }
    }

    #[test]
    fn enabled_instance_params_override_graph_defaults() {
        let g = graph(vec![
            p("Roughness", ParamValue::Scalar(0.5), false),
            p("Metal", ParamValue::Scalar(0.0), false),
            p("basecolor", ParamValue::Texture("default".into()), false),
        ]);
        let inst = [
            p("Roughness", ParamValue::Scalar(0.9), true),
            p("Metal", ParamValue::Scalar(1.0), false),
            p("basecolor", ParamValue::Texture("mine".into()), true),
        ];
        let m = resolve("Base.lsf", Some(&g), &inst);
        assert_eq!(m.scalar("Roughness"), Some(0.9));
        assert_eq!(m.scalar("Metal"), Some(0.0), "disabled override is ignored");
        assert_eq!(m.texture(Slot::BaseColor).unwrap().id, "mine");
        assert!(!m.needs_virtual_texture());
        assert_eq!(m.blend, BlendMode::Opaque);
        assert!(!m.two_sided && !m.unlit && !m.invisible);
    }

    #[test]
    fn unnamed_textures_match_by_id_and_vt_is_reported() {
        let g = graph(vec![
            p("", ParamValue::Texture("a".into()), false),
            p(
                "virtualtexture",
                ParamValue::VirtualTexture("gvt".into()),
                false,
            ),
        ]);
        let inst = [
            p("", ParamValue::Texture("a".into()), false),
            p(
                "virtualtexture",
                ParamValue::VirtualTexture("vt".into()),
                true,
            ),
        ];
        let m = resolve("Base_VT.lsf", Some(&g), &inst);
        assert_eq!(m.textures.len(), 1);
        assert_eq!(m.virtual_textures, ["vt"]);
        assert!(m.needs_virtual_texture());
    }

    #[test]
    fn blend_modes() {
        let mut base = p("basecolor", ParamValue::Texture("t".into()), false);
        base.depth_pass = true;
        let g = graph(vec![base]);
        assert_eq!(
            resolve("X_AlphaTest.lsf", Some(&g), &[]).blend,
            BlendMode::AlphaTest
        );
        assert_eq!(
            resolve("Base_Glass_Glow.lsf", Some(&g), &[]).blend,
            BlendMode::Blend
        );
        let mut g = graph(vec![]);
        g.blend_state = 0;
        g.rasterizer_state = 0;
        g.shading_model = 1;
        let m = resolve("VFX_Alphablend.lsf", Some(&g), &[]);
        assert_eq!(m.blend, BlendMode::Blend);
        assert!(m.two_sided && m.unlit);
        assert!(resolve("Helper/Helper_Invisible.lsf", None, &[]).invisible);
    }

    #[test]
    fn glow_and_flat_colour() {
        let g = graph(vec![
            p("Glowmap", ParamValue::Texture("gm".into()), false),
            p("GlowColour", ParamValue::Vector(vec![1.0, 0.5, 0.0]), false),
            p("GlowMultiplier", ParamValue::Scalar(2.0), false),
            p("Colour", ParamValue::Vector(vec![0.1, 0.2, 0.3]), false),
        ]);
        let m = resolve("Base_GM_VT.lsf", Some(&g), &[]);
        assert_eq!(m.glow(), Some([2.0, 1.0, 0.0]));
        assert_eq!(m.flat_color(), Some([0.1, 0.2, 0.3]));
        assert_eq!(resolve("x", Some(&graph(vec![])), &[]).glow(), None);
    }

    #[test]
    fn classifies_common_names() {
        assert_eq!(classify("basecolor"), Slot::BaseColor);
        assert_eq!(classify("BaseMap"), Slot::BaseColor);
        assert_eq!(classify("NormalMap"), Slot::Normal);
        assert_eq!(classify("physicalmap"), Slot::Physical);
        assert_eq!(classify("GlowMap"), Slot::Emissive);
        assert_eq!(classify("MicroDetailMap"), Slot::Other);
    }
}
