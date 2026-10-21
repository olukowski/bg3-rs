//! Optional navigation supplied by the caller; the sim has no grid or renderer dependency.

use std::sync::Arc;

use super::distance;
use crate::Sim;

pub trait Pathfinder: Send + Sync {
    /// Walkable path in BG3 world coordinates from `from` to as close to `to` as reachable,
    /// both endpoints included; None if there is no path.
    fn find_path(&self, from: [f32; 3], to: [f32; 3], radius: f32) -> Option<Vec<[f32; 3]>>;
}

impl std::fmt::Debug for dyn Pathfinder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Pathfinder")
    }
}

/// Approximate footprint until the sim loads character AI bounds from templates.
pub const APPROXIMATE_RADIUS: f32 = 0.5;

impl Sim {
    /// Supply level navigation for AI movement. `None` restores the straight-line fallback.
    pub fn set_pathfinder(&mut self, pathfinder: Option<Arc<dyn Pathfinder>>) {
        self.combat.pathfinder = pathfinder;
    }
}

/// Truncate a polyline at the budget or its first intersection with the stopping sphere.
/// This is our approximate AI's movement policy, not a port of the original engine AI.
pub(super) fn walk_path(
    route: &[[f32; 3]],
    budget: f32,
    target: [f32; 3],
    range: f32,
) -> (Vec<[f32; 3]>, f32, bool) {
    let mut path = vec![route[0]];
    let mut walked = 0.0;
    let mut reached = distance(route[0], target) <= range;
    for w in route.windows(2) {
        if reached || walked >= budget {
            break;
        }
        let len = distance(w[0], w[1]);
        if len <= f32::EPSILON {
            continue;
        }
        let delta = std::array::from_fn::<_, 3, _>(|i| w[1][i] - w[0][i]);
        let offset = std::array::from_fn::<_, 3, _>(|i| w[0][i] - target[i]);
        let dot = |a: [f32; 3], b: [f32; 3]| (0..3).map(|i| a[i] * b[i]).sum::<f32>();
        let b = dot(offset, delta);
        let c = dot(offset, offset) - range * range;
        let discriminant = b * b - len * len * c;
        let hit = if discriminant >= 0.0 {
            let t = (-b - discriminant.sqrt()) / (len * len);
            (0.0..=1.0).contains(&t).then_some(t * len)
        } else {
            None
        };
        let available = (budget - walked).min(len);
        let step = hit.map_or(available, |d| d.min(available));
        reached = hit.is_some_and(|d| d <= available);
        if step > 0.0 {
            let end = if step >= len {
                w[1]
            } else {
                std::array::from_fn(|i| w[0][i] + delta[i] * (step / len))
            };
            path.push(end);
            walked += step;
        }
        if step < len {
            break;
        }
    }
    if walked == 0.0 {
        path.clear();
    }
    (path, walked, reached)
}
