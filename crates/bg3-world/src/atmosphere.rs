//! Atmosphere and lighting triggers: level volumes that switch the level-wide lighting
//! (`TriggerLighting`) and atmosphere (`TriggerAtmosphere`) resources while the camera/player
//! is inside them. See `docs/research/lighting.md` ("Lighting and atmosphere triggers").
//!
//! bg3: ls::AtmosphereTrigger::SetAtmResourceIDs, ls::LightingTrigger (4.1.1.7631656)

use std::collections::HashMap;

use bg3_lsf::Node;
use bg3_vfs::Vfs;
use bg3_vfs::modules::BASE_LOAD_ORDER;

use crate::{parse, str_attr};

/// A trigger volume naming lighting or atmosphere resources.
#[derive(Debug, Clone, PartialEq)]
pub struct ResourceZone {
    pub name: String,
    pub position: [f32; 3],
    /// Quaternion `x y z w`.
    pub rotation: [f32; 4],
    /// Polygon in the trigger's local (x, z) plane.
    pub points: Vec<[f32; 2]>,
    /// Prism height, up from the position.
    pub height: f32,
    /// `ResourceIDs` in file order. The first is the one active by default (**assumption**:
    /// the others are alternatives that timelines switch to, e.g. the astral-plane vision on
    /// the Nautiloid's helm).
    pub resources: Vec<String>,
    /// Seconds to blend when entering.
    pub fade_time: f32,
}

impl ResourceZone {
    /// Is the BG3 world-space point `p` inside the prism?
    pub fn contains(&self, p: [f32; 3]) -> bool {
        let d = [
            p[0] - self.position[0],
            p[1] - self.position[1],
            p[2] - self.position[2],
        ];
        let [x, y, z] = rotate_inverse(self.rotation, d);
        (0.0..=self.height).contains(&y) && point_in_polygon(&self.points, x, z)
    }

    /// Area of the polygon (to prefer the innermost of nested zones).
    pub fn area(&self) -> f32 {
        let n = self.points.len();
        let mut a = 0.0;
        for i in 0..n {
            let [x0, z0] = self.points[i];
            let [x1, z1] = self.points[(i + 1) % n];
            a += x0 * z1 - x1 * z0;
        }
        (a * 0.5).abs()
    }
}

fn point_in_polygon(points: &[[f32; 2]], x: f32, z: f32) -> bool {
    let mut inside = false;
    let n = points.len();
    let mut j = n.wrapping_sub(1);
    for i in 0..n {
        let [xi, zi] = points[i];
        let [xj, zj] = points[j];
        if (zi > z) != (zj > z) && x < (xj - xi) * (z - zi) / (zj - zi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn rotate_inverse(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let (qx, qy, qz, qw) = (-q[0], -q[1], -q[2], q[3]);
    let c1 = [
        qy * v[2] - qz * v[1],
        qz * v[0] - qx * v[2],
        qx * v[1] - qy * v[0],
    ];
    let c2 = [
        qy * c1[2] - qz * c1[1],
        qz * c1[0] - qx * c1[2],
        qx * c1[1] - qy * c1[0],
    ];
    [
        v[0] + 2.0 * (qw * c1[0] + c2[0]),
        v[1] + 2.0 * (qw * c1[1] + c2[1]),
        v[2] + 2.0 * (qw * c1[2] + c2[2]),
    ]
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
            Some((
                str_attr(n, "MapKey")?,
                ResourceZone {
                    name: str_attr(n, "Name").unwrap_or_default(),
                    position,
                    rotation,
                    points,
                    height: f("Height").unwrap_or(0.0),
                    resources,
                    fade_time: f("FadeTime").unwrap_or(0.0),
                },
            ))
        })
        .collect()
}

/// All atmosphere/lighting triggers of a level, merged across modules by `MapKey`.
pub fn load_resource_zones(vfs: &Vfs, level: &str) -> crate::Result<Vec<ResourceZone>> {
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<ResourceZone> = Vec::new();
    for module in BASE_LOAD_ORDER {
        let path = format!("Mods/{module}/Levels/{level}/Triggers/_merged.lsf");
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
    Ok(out)
}

/// The innermost zone containing `p` whose first resource satisfies `known`.
pub fn zone_at(
    zones: &[ResourceZone],
    p: [f32; 3],
    known: impl Fn(&str) -> bool,
) -> Option<&ResourceZone> {
    zones
        .iter()
        .filter(|z| z.resources.first().is_some_and(|r| known(r)) && z.contains(p))
        .min_by(|a, b| a.area().total_cmp(&b.area()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(size: f32) -> ResourceZone {
        ResourceZone {
            name: "z".into(),
            position: [10.0, 0.0, 20.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            points: vec![[0.0, 0.0], [size, 0.0], [size, size], [0.0, size]],
            height: 5.0,
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
        assert_eq!(z.area(), 16.0);
    }

    #[test]
    fn innermost_zone_wins() {
        let mut inner = square(2.0);
        inner.resources = vec!["inner".into()];
        let zones = vec![square(4.0), inner];
        let at = zone_at(&zones, [11.0, 1.0, 21.0], |_| true).unwrap();
        assert_eq!(at.resources[0], "inner");
        let at = zone_at(&zones, [13.0, 1.0, 23.0], |_| true).unwrap();
        assert_eq!(at.resources[0], "r");
        assert!(zone_at(&zones, [11.0, 1.0, 21.0], |r| r == "none").is_none());
    }
}
