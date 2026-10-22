//! Trigger volumes: the shape of an area trigger, the point-in-volume test and the order in
//! which overlapping triggers are preferred. Shared by the sim's trigger events
//! (`bg3_sim::presentation`) and the lighting/atmosphere triggers ([`crate::atmosphere`]), as
//! the engine uses one test for both. See `docs/research/level-structure.md` ("Trigger
//! volumes").
//!
//! bg3: ls::trigger::HitTest, ls::trigger::CalculatePhysicsBounds, ls::Trigger::InitTriggerPhysics_Legacy (4.1.1.7631656)

/// The volume of a trigger in its local frame (origin at the trigger's position, axes turned
/// by its rotation; the trigger's `Scale` is not applied).
#[derive(Clone, Debug, PartialEq)]
pub enum TriggerShape {
    /// `PhysicsType` 1: `Radius`, around the position.
    Sphere { radius: f32 },
    /// `PhysicsType` 2: `Extents`, the half sizes of a box centred on the position.
    Box { extents: [f32; 3] },
    /// `PhysicsType` 3: a prism over the `Points` polygon (local x, z) between local y 0 and
    /// `Height` (so it extends up from the position, or down for a negative height).
    Polygon { points: Vec<[f32; 2]>, height: f32 },
    /// No volume (`PhysicsType` 0: points, splines, …).
    None,
}

impl TriggerShape {
    /// The shape for `PhysicsType` and the fields that go with it (each already resolved
    /// through the template chain). Missing fields fall back to the defaults the sim used
    /// before (**assumption**: the engine's own defaults were not read).
    pub fn from_physics(
        physics_type: i64,
        radius: Option<f32>,
        extents: Option<[f32; 3]>,
        height: Option<f32>,
        points: Vec<[f32; 2]>,
    ) -> Self {
        match physics_type {
            1 => Self::Sphere {
                radius: radius.unwrap_or(1.0),
            },
            2 => Self::Box {
                extents: extents.unwrap_or([1.0; 3]),
            },
            3 => Self::Polygon {
                points,
                height: height.unwrap_or(0.0),
            },
            _ => Self::None,
        }
    }

    /// Is the world-space point `p` inside the shape placed at `position` with `rotation`
    /// (quaternion `x y z w`)?
    // bg3: ls::trigger::HitTest (4.1.1.7631656)
    pub fn contains(&self, position: [f32; 3], rotation: [f32; 4], p: [f32; 3]) -> bool {
        let d = [p[0] - position[0], p[1] - position[1], p[2] - position[2]];
        match self {
            // The sphere ignores the rotation.
            Self::Sphere { radius } => d[0] * d[0] + d[1] * d[1] + d[2] * d[2] <= radius * radius,
            Self::Box { extents } => {
                let l = rotate_inverse(rotation, d);
                (0..3).all(|i| -extents[i] <= l[i] && l[i] <= extents[i])
            }
            Self::Polygon { points, height } => {
                if points.len() < 3 {
                    return false;
                }
                let [x, y, z] = rotate_inverse(rotation, d);
                // The engine first rejects points outside the local bounds (whose y range
                // is spanned by the prism's top and bottom vertices), then tests the polygon.
                height.min(0.0) <= y && y <= height.max(0.0) && point_in_polygon(points, x, z)
            }
            Self::None => false,
        }
    }

    /// The key that orders overlapping lighting and atmosphere triggers: the smallest wins.
    /// It is the radius of the shape's local bounding circle in the horizontal plane: the
    /// sphere's radius, the box's horizontal half diagonal, and for a polygon the largest
    /// distance of a point from the points' mean.
    // bg3: ls::trigger::CalculatePhysicsBounds, ecl::GetLightingTrigger,
    // ecl::CreateAtmosphereInstanceAndGetTrigger (4.1.1.7631656)
    pub fn selection_radius(&self) -> f32 {
        match self {
            Self::Sphere { radius } => *radius,
            Self::Box { extents } => (extents[0] * extents[0] + extents[2] * extents[2]).sqrt(),
            Self::Polygon { points, .. } => {
                if points.is_empty() {
                    return 0.0;
                }
                let n = points.len() as f32;
                let cx = points.iter().map(|p| p[0]).sum::<f32>() / n;
                let cz = points.iter().map(|p| p[1]).sum::<f32>() / n;
                points
                    .iter()
                    .map(|p| (p[0] - cx) * (p[0] - cx) + (p[1] - cz) * (p[1] - cz))
                    .fold(0.0, f32::max)
                    .sqrt()
            }
            Self::None => 0.0,
        }
    }
}

/// Of the triggers `candidates` (already known to contain the point), the one the engine
/// prefers: the smallest [`TriggerShape::selection_radius`]; on a tie the earlier one
/// (**assumption**: the engine's candidate order comes from its trigger tree and was not
/// matched).
pub fn preferred<'a, T>(
    candidates: impl IntoIterator<Item = &'a T>,
    shape: impl Fn(&T) -> &TriggerShape,
) -> Option<&'a T> {
    candidates.into_iter().min_by(|a, b| {
        shape(a)
            .selection_radius()
            .total_cmp(&shape(b).selection_radius())
    })
}

/// Rotate `v` by the inverse (conjugate) of the unit quaternion `q` (`x y z w`). The engine
/// keeps the conjugate of the trigger's rotation for the hit test.
fn rotate_inverse(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let (qx, qy, qz, qw) = (-q[0], -q[1], -q[2], q[3]);
    // v' = v + 2w(q × v) + 2 q × (q × v)
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

/// Even-odd rule: count the edges crossing the line through `z` left of `x`. An edge counts
/// when one end is below `z` and the other at or above it.
fn point_in_polygon(points: &[[f32; 2]], x: f32, z: f32) -> bool {
    let mut inside = false;
    let n = points.len();
    for i in 0..n {
        let [xi, zi] = points[i];
        let [xj, zj] = points[(i + n - 1) % n];
        if (zi < z) != (zj < z) && xi + (z - zi) / (zj - zi) * (xj - xi) < x {
            inside = !inside;
        }
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
    const AT: [f32; 3] = [10.0, 0.0, 10.0];

    fn square(height: f32) -> TriggerShape {
        TriggerShape::Polygon {
            points: vec![[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]],
            height,
        }
    }

    #[test]
    fn shapes_contain_points() {
        let s = TriggerShape::Sphere { radius: 2.0 };
        assert!(s.contains(AT, ID, [11.0, 1.0, 10.0]) && !s.contains(AT, ID, [12.5, 0.0, 10.0]));
        // A box of half sizes 2×1×1 turned 90° about y spans 2 in x and 4 in z.
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let b = TriggerShape::Box {
            extents: [2.0, 1.0, 1.0],
        };
        let turned = [0.0, h, 0.0, h];
        assert!(b.contains(AT, turned, [10.0, 0.0, 11.9]));
        assert!(!b.contains(AT, turned, [11.9, 0.0, 10.0]));
        assert!(b.contains(AT, ID, [8.0, -1.0, 9.0]), "faces are inside");
        let p = square(3.0);
        assert!(p.contains(AT, ID, [10.5, 2.0, 9.5]));
        assert!(!p.contains(AT, ID, [10.5, 3.5, 9.5]) && !p.contains(AT, ID, [12.0, 1.0, 10.0]));
    }

    #[test]
    fn prism_spans_from_the_position_to_its_height() {
        let up = square(3.0);
        assert!(up.contains(AT, ID, [10.0, 0.0, 10.0]) && up.contains(AT, ID, [10.0, 3.0, 10.0]));
        assert!(!up.contains(AT, ID, [10.0, -0.1, 10.0]));
        let down = square(-3.0);
        assert!(down.contains(AT, ID, [10.0, -2.0, 10.0]));
        assert!(!down.contains(AT, ID, [10.0, 1.0, 10.0]));
        // Upside down (180° about x), the prism hangs below the position.
        let flipped = [1.0, 0.0, 0.0, 0.0];
        assert!(up.contains(AT, flipped, [10.0, -2.0, 10.0]));
        assert!(!up.contains(AT, flipped, [10.0, 2.0, 10.0]));
        // Fewer than three points enclose nothing.
        let line = TriggerShape::Polygon {
            points: vec![[-1.0, -1.0], [1.0, 1.0]],
            height: 3.0,
        };
        assert!(!line.contains(AT, ID, [10.0, 1.0, 10.0]));
    }

    #[test]
    fn concave_polygons_use_the_even_odd_rule() {
        // A U open towards +z.
        let u = TriggerShape::Polygon {
            points: vec![
                [0.0, 0.0],
                [3.0, 0.0],
                [3.0, 3.0],
                [2.0, 3.0],
                [2.0, 1.0],
                [1.0, 1.0],
                [1.0, 3.0],
                [0.0, 3.0],
            ],
            height: 1.0,
        };
        let o = [0.0; 3];
        assert!(u.contains(o, ID, [0.5, 0.5, 2.0]) && u.contains(o, ID, [2.5, 0.5, 2.0]));
        assert!(!u.contains(o, ID, [1.5, 0.5, 2.0]));
        assert!(u.contains(o, ID, [1.5, 0.5, 0.5]));
    }

    #[test]
    fn selection_prefers_the_smallest_horizontal_bound() {
        assert_eq!(TriggerShape::Sphere { radius: 2.0 }.selection_radius(), 2.0);
        let b = TriggerShape::Box {
            extents: [3.0, 100.0, 4.0],
        };
        assert_eq!(b.selection_radius(), 5.0);
        // The polygon's bound is centred on the points' mean, not its area centroid, and
        // ignores the height.
        let p = TriggerShape::Polygon {
            points: vec![[0.0, 0.0], [4.0, 0.0], [4.0, 3.0], [0.0, 3.0]],
            height: 1000.0,
        };
        assert_eq!(p.selection_radius(), 2.5);
        let shapes = [b, p, TriggerShape::Sphere { radius: 2.5 }];
        let best = preferred(&shapes, |s| s).unwrap();
        assert_eq!(best, &shapes[1], "ties keep the earlier trigger");
    }
}
