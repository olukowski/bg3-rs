//! Atmosphere and lighting triggers: level volumes that switch the level-wide lighting
//! (`TriggerLighting`) and atmosphere (`TriggerAtmosphere`) resources while the camera/player
//! is inside them. See `docs/research/lighting.md` ("Lighting and atmosphere triggers").
//!
//! bg3: ls::AtmosphereTrigger::SetAtmResourceIDs, ls::LightingTrigger (4.1.1.7631656)

use std::collections::HashMap;

use bg3_lsf::Node;
use bg3_vfs::Vfs;
use bg3_vfs::modules::BASE_LOAD_ORDER;

use crate::trigger::TriggerShape;
use crate::{parse, str_attr};

/// A trigger volume naming lighting or atmosphere resources.
#[derive(Debug, Clone, PartialEq)]
pub struct ResourceZone {
    pub name: String,
    pub position: [f32; 3],
    /// Quaternion `x y z w`.
    pub rotation: [f32; 4],
    /// The volume (a polygon prism for the `AtmosphereTrigger` / `LightingTrigger` templates).
    pub shape: TriggerShape,
    /// `ResourceIDs` in file order. The first is the one active by default (**assumption**:
    /// the others are alternatives that timelines switch to, e.g. the astral-plane vision on
    /// the Nautiloid's helm).
    pub resources: Vec<String>,
    /// Seconds to blend when entering.
    pub fade_time: f32,
}

impl ResourceZone {
    /// Is the BG3 world-space point `p` inside the trigger?
    pub fn contains(&self, p: [f32; 3]) -> bool {
        self.shape.contains(self.position, self.rotation, p)
    }
}

fn components(n: &Node, name: &str) -> Option<Vec<f32>> {
    Some(
        n.attr(name)?
            .components()?
            .into_iter()
            .map(|v| v as f32)
            .collect(),
    )
}

/// Read the triggers of one `Triggers/_merged.lsf` that carry `ResourceIDs` (atmosphere and
/// lighting triggers).
pub fn read_resource_zones(res: &bg3_lsf::Resource) -> Vec<(String, ResourceZone)> {
    res.region("Templates")
        .into_iter()
        .flat_map(|n| n.children_named("GameObjects"))
        .filter_map(|n| {
            let resources: Vec<String> = n
                .child("ResourceIDs")?
                .children_named("ResourceID")
                .filter_map(|r| str_attr(r, "Object"))
                .collect();
            if resources.is_empty() {
                return None;
            }
            let t = n.child("Transform");
            let position = t
                .and_then(|t| components(t, "Position"))
                .filter(|v| v.len() == 3)
                .map_or([0.0; 3], |v| [v[0], v[1], v[2]]);
            let rotation = t
                .and_then(|t| components(t, "RotationQuat"))
                .filter(|v| v.len() == 4)
                .map_or([0.0, 0.0, 0.0, 1.0], |v| [v[0], v[1], v[2], v[3]]);
            let points = n
                .children_named("Points")
                .filter_map(|p| components(p, "Object"))
                .filter(|v| v.len() == 2)
                .map(|v| [v[0], v[1]])
                .collect();
            let f = |name| n.attr(name).and_then(|v| v.as_f64()).map(|v| v as f32);
            // The placed objects carry `Height` and `Points`; `PhysicsType` 3 and `Height` 2
            // are the root templates' (`AtmosphereTrigger`, `LightingTrigger`), which are
            // not loaded here.
            let shape = TriggerShape::from_physics(
                n.attr("PhysicsType").and_then(|v| v.as_i64()).unwrap_or(3),
                f("Radius"),
                components(n, "Extents")
                    .filter(|v| v.len() == 3)
                    .map(|v| [v[0], v[1], v[2]]),
                Some(f("Height").unwrap_or(2.0)),
                points,
            );
            Some((
                str_attr(n, "MapKey")?,
                ResourceZone {
                    name: str_attr(n, "Name").unwrap_or_default(),
                    position,
                    rotation,
                    shape,
                    resources,
                    fade_time: f("FadeTime").unwrap_or(0.0),
                },
            ))
        })
        .collect()
}

/// All atmosphere/lighting triggers of a level (its own and its globals, as for the sim's
/// triggers), merged across modules by `MapKey`.
pub fn load_resource_zones(vfs: &Vfs, level: &str) -> crate::Result<Vec<ResourceZone>> {
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<ResourceZone> = Vec::new();
    for module in BASE_LOAD_ORDER {
        for base in ["Levels", "Globals"] {
            let path = format!("Mods/{module}/{base}/{level}/Triggers/_merged.lsf");
            if !vfs.exists(&path) {
                continue;
            }
            for (id, z) in read_resource_zones(&parse(vfs, &path)?) {
                match index.get(&id) {
                    Some(&i) => out[i] = z,
                    None => {
                        index.insert(id, out.len());
                        out.push(z);
                    }
                }
            }
        }
    }
    Ok(out)
}

/// The zone the engine picks at the camera position `p` among the eligible ones (the
/// lighting or the atmosphere triggers): of those containing `p`, the one with the smallest
/// horizontal bound ([`crate::trigger::preferred`]).
// bg3: ecl::GetLightingTrigger, ecl::CameraSystem::SwitchAtmosphereTrigger (4.1.1.7631656)
pub fn zone_index_at(
    zones: &[ResourceZone],
    p: [f32; 3],
    eligible: impl Fn(usize, &ResourceZone) -> bool,
) -> Option<usize> {
    let hits: Vec<(usize, &ResourceZone)> = zones
        .iter()
        .enumerate()
        .filter(|&(i, z)| eligible(i, z) && z.contains(p))
        .collect();
    crate::trigger::preferred(&hits, |(_, z)| &z.shape).map(|&(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(size: f32) -> ResourceZone {
        ResourceZone {
            name: "z".into(),
            position: [10.0, 0.0, 20.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            shape: TriggerShape::Polygon {
                points: vec![[0.0, 0.0], [size, 0.0], [size, size], [0.0, size]],
                height: 5.0,
            },
            resources: vec!["r".into()],
            fade_time: 2.0,
        }
    }

    #[test]
    fn prism_contains_points_over_the_polygon_and_below_its_height() {
        let z = square(4.0);
        assert!(z.contains([12.0, 1.0, 22.0]));
        assert!(!z.contains([12.0, 6.0, 22.0]));
        assert!(!z.contains([12.0, -1.0, 22.0]));
        assert!(!z.contains([15.0, 1.0, 22.0]));
    }

    #[test]
    fn smallest_zone_wins() {
        let zones = vec![square(4.0), square(2.0)];
        let all = |_: usize, _: &ResourceZone| true;
        assert_eq!(zone_index_at(&zones, [11.0, 1.0, 21.0], all), Some(1));
        assert_eq!(zone_index_at(&zones, [13.0, 1.0, 23.0], all), Some(0));
        assert_eq!(
            zone_index_at(&zones, [11.0, 1.0, 21.0], |i, _| i == 0),
            Some(0)
        );
        assert_eq!(zone_index_at(&zones, [11.0, 9.0, 21.0], all), None);
    }
}
