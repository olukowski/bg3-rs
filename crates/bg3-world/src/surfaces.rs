//! Surface templates (`Type=surface` root templates, `eoc::SurfaceTemplate`): what each surface
//! type looks like. The surfaces themselves live on the AI grid (`bg3_sim::surface`,
//! `docs/research/surfaces.md`).

use std::collections::HashMap;

use bg3_lsf::Node;

use crate::str_attr;
use crate::templates::RootTemplates;

/// The fields of a surface template the renderer needs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SurfaceProps {
    /// `DecalMaterial`: the `MaterialBank` resource the surface renderer draws the type with.
    pub decal_material: Option<String>,
    /// `NormalBlendingFactor`.
    pub normal_blending: Option<f32>,
    /// `FadeOutSpeed`.
    pub fade_out_speed: Option<f32>,
}

impl SurfaceProps {
    // bg3: eoc::SurfaceTemplate::Visit (4.1.1.7631656)
    pub fn read(n: &Node) -> Self {
        let f = |k: &str| n.attr(k).and_then(|v| v.as_f64()).map(|v| v as f32);
        Self {
            decal_material: str_attr(n, "DecalMaterial"),
            normal_blending: f("NormalBlendingFactor"),
            fade_out_speed: f("FadeOutSpeed"),
        }
    }
}

/// Surface templates by `Name` (`SurfaceBlood`, …), the name the surface type enum uses.
pub fn surface_templates(templates: &RootTemplates) -> HashMap<String, SurfaceProps> {
    templates
        .templates
        .values()
        .filter_map(|t| Some((t.name.clone(), t.surface.clone()?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bg3_lsf::Value;

    #[test]
    fn reads_decal_material() {
        let n = Node::new("GameObjects")
            .with_attr("DecalMaterial", Value::FixedString("m".into()))
            .with_attr("NormalBlendingFactor", Value::Float(0.25));
        let p = SurfaceProps::read(&n);
        assert_eq!(p.decal_material.as_deref(), Some("m"));
        assert_eq!(p.normal_blending, Some(0.25));
        assert_eq!(p.fade_out_speed, None);
    }
}
