//! Light probes: baked image-based lighting placed in a level (`LightProbes/_merged.lsf`,
//! `Type=lightProbe`). Each probe has one capture per lighting resource (`CaptureDescriptions`):
//! diffuse and specular cubemaps (with separate alpha maps) and spherical harmonics. Fields are
//! sparse like lights' (object → root template chain). See `docs/research/lighting.md`.

use std::collections::HashMap;

use bg3_lsf::Node;
use bg3_vfs::Vfs;
use bg3_vfs::modules::BASE_LOAD_ORDER;

use crate::level::read_transform;
use crate::{Result, RootTemplates, Transform, parse, str_attr};

/// Probe fields as stored on one node; `None` = inherit.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProbeProps {
    pub enabled: Option<bool>,
    /// 0 = sphere (`Radius`), 1 = box (`Size`).
    pub shape: Option<u8>,
    pub probe_type: Option<u8>,
    pub radius: Option<f32>,
    pub size: Option<[f32; 3]>,
    pub transition_distance: Option<f32>,
    pub intensity: Option<f32>,
    pub infinite_capture: Option<bool>,
}

impl ProbeProps {
    pub fn read(n: &Node) -> Self {
        let f = |name| n.attr(name).and_then(|v| v.as_f64()).map(|v| v as f32);
        let u = |name| n.attr(name).and_then(|v| v.as_i64()).map(|v| v as u8);
        Self {
            enabled: n.attr("Enabled").and_then(|v| v.as_bool()),
            shape: u("LightProbeShape"),
            probe_type: u("LightProbeType"),
            radius: f("Radius"),
            size: n
                .attr("Size")
                .and_then(|v| v.components())
                .and_then(|c| (c.len() == 3).then(|| [c[0] as f32, c[1] as f32, c[2] as f32])),
            transition_distance: f("TransitionDistance"),
            intensity: f("Intensity"),
            infinite_capture: n.attr("InfiniteCapture").and_then(|v| v.as_bool()),
        }
    }

    fn inherit(self, p: &Self) -> Self {
        Self {
            enabled: self.enabled.or(p.enabled),
            shape: self.shape.or(p.shape),
            probe_type: self.probe_type.or(p.probe_type),
            radius: self.radius.or(p.radius),
            size: self.size.or(p.size),
            transition_distance: self.transition_distance.or(p.transition_distance),
            intensity: self.intensity.or(p.intensity),
            infinite_capture: self.infinite_capture.or(p.infinite_capture),
        }
    }
}

/// One baked capture of a probe, for one lighting resource.
#[derive(Debug, Clone, PartialEq)]
pub struct ProbeCapture {
    /// `LightingBank` resource ID the capture was baked with.
    pub lighting: String,
    pub diffuse: Option<String>,
    pub diffuse_alpha: Option<String>,
    pub specular: Option<String>,
    pub specular_alpha: Option<String>,
    pub sh: Option<String>,
}

/// A placed light probe with its fields resolved.
#[derive(Debug, Clone)]
pub struct LightProbe {
    pub id: String,
    pub name: String,
    pub transform: Transform,
    pub enabled: bool,
    /// 0 = sphere, 1 = box.
    pub shape: u8,
    pub radius: f32,
    pub size: [f32; 3],
    pub transition_distance: f32,
    pub intensity: f32,
    pub captures: Vec<ProbeCapture>,
}

impl LightProbe {
    /// The capture baked for a lighting resource.
    pub fn capture(&self, lighting: &str) -> Option<&ProbeCapture> {
        self.captures.iter().find(|c| c.lighting == lighting)
    }
}

fn template_props(templates: &RootTemplates, id: &str) -> ProbeProps {
    let mut acc = ProbeProps::default();
    let mut current = templates.get(id);
    for _ in 0..32 {
        let Some(t) = current else { break };
        if let Some(p) = &t.probe {
            acc = acc.inherit(p);
        }
        current = t.parent.as_deref().and_then(|p| templates.get(p));
    }
    acc
}

/// Read the probes of one `LightProbes/_merged.lsf`.
pub fn read_probes(res: &bg3_lsf::Resource, templates: &RootTemplates) -> Vec<LightProbe> {
    res.region("Templates")
        .into_iter()
        .flat_map(|n| n.children_named("GameObjects"))
        .filter(|n| str_attr(n, "Type").as_deref() == Some("lightProbe"))
        .filter_map(|n| {
            let inherited = str_attr(n, "TemplateName")
                .map(|t| template_props(templates, &t))
                .unwrap_or_default();
            let p = ProbeProps::read(n).inherit(&inherited);
            let captures = n
                .child("CaptureDescriptions")
                .into_iter()
                .flat_map(|c| c.children_named("CaptureDesc"))
                .filter_map(|c| {
                    Some(ProbeCapture {
                        lighting: str_attr(c, "Lighting")?,
                        diffuse: str_attr(c, "DiffuseIBL"),
                        diffuse_alpha: str_attr(c, "DiffuseAlphaIBL"),
                        specular: str_attr(c, "SpecularIBL"),
                        specular_alpha: str_attr(c, "SpecularAlphaIBL"),
                        sh: str_attr(c, "SphericalHarmonicsIBL"),
                    })
                })
                .collect();
            Some(LightProbe {
                id: str_attr(n, "MapKey")?,
                name: str_attr(n, "Name").unwrap_or_default(),
                transform: n.child("Transform").map(read_transform).unwrap_or_default(),
                enabled: p.enabled.unwrap_or(true),
                shape: p.shape.unwrap_or(0),
                radius: p.radius.unwrap_or(5.0),
                size: p.size.unwrap_or([10.0; 3]),
                transition_distance: p.transition_distance.unwrap_or(1.0),
                intensity: p.intensity.unwrap_or(1.0),
                captures,
            })
        })
        .collect()
}

/// All light probes of a level, merged across modules by `MapKey`.
pub fn load_level_probes(
    vfs: &Vfs,
    level: &str,
    templates: &RootTemplates,
) -> Result<Vec<LightProbe>> {
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<LightProbe> = Vec::new();
    for module in BASE_LOAD_ORDER {
        let path = format!("Mods/{module}/Levels/{level}/LightProbes/_merged.lsf");
        if !vfs.exists(&path) {
            continue;
        }
        for p in read_probes(&parse(vfs, &path)?, templates) {
            match index.get(&p.id) {
                Some(&i) => out[i] = p,
                None => {
                    index.insert(p.id.clone(), out.len());
                    out.push(p);
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RootTemplate;
    use bg3_lsf::Value;

    #[test]
    fn probe_fields_inherit_from_the_template_and_captures_are_read() {
        let mut templates = RootTemplates::default();
        let base = Node::new("GameObjects")
            .with_attr("Radius", Value::Float(5.0))
            .with_attr("TransitionDistance", Value::Float(1.0))
            .with_attr("LightProbeShape", Value::Byte(0));
        templates.templates.insert(
            "t".into(),
            RootTemplate {
                id: "t".into(),
                name: "GUS_LightProbe".into(),
                kind: "lightProbe".into(),
                parent: None,
                visual_template: None,
                physics_template: None,
                is_blocker: None,
                equipment_type: None,
                equipment: None,
                light: None,
                character: None,
                probe: Some(ProbeProps::read(&base)),
                fog: None,
                decal: None,
                receive_decal: None,
                surface: None,
            },
        );
        let capture = Node::new("CaptureDesc")
            .with_attr("Lighting", Value::FixedString("L".into()))
            .with_attr("DiffuseIBL", Value::String("d.dds".into()))
            .with_attr("SphericalHarmonicsIBL", Value::String("sh.dat".into()));
        let obj = Node::new("GameObjects")
            .with_attr("MapKey", Value::FixedString("p".into()))
            .with_attr("Type", Value::FixedString("lightProbe".into()))
            .with_attr("TemplateName", Value::FixedString("t".into()))
            .with_attr("Radius", Value::Float(9.0))
            .with_child(Node::new("CaptureDescriptions").with_child(capture));
        let res = bg3_lsf::Resource {
            regions: vec![bg3_lsf::Region {
                name: "Templates".into(),
                node: Node::new("Templates").with_child(obj),
            }],
            ..Default::default()
        };
        let probes = read_probes(&res, &templates);
        assert_eq!(probes.len(), 1);
        let p = &probes[0];
        assert_eq!(p.radius, 9.0);
        assert_eq!(p.transition_distance, 1.0);
        assert_eq!(p.shape, 0);
        let c = p.capture("L").unwrap();
        assert_eq!(c.diffuse.as_deref(), Some("d.dds"));
        assert_eq!(c.sh.as_deref(), Some("sh.dat"));
        assert!(p.capture("other").is_none());
    }
}
