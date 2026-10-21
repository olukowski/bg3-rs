//! Where the AI wants to stand: the end-position terms of an action.
//!
//! `esv::AiHelpers::CalculatePositionScore` (0x105215238, 4.1.1.7631656) adds raw terms on
//! the actor for the tile an action ends on. The modelled ones: allies and enemies nearby
//! (`*_NEARBY` with `BASE_NEARBY_SCORE` falling off between the min and max distance) and
//! the distance to the closest enemy beyond `MAX_DISTANCE_TO_CLOSEST_ENEMY`. Raw terms on the
//! actor are scored negated (see `score::score_target`), so a negative raw term is good.
//! Not modelled: height, flanking, AI hints, smoke, surfaces, auras, darkness, invisibility,
//! dangerous items and ledges.

use super::archetype::{self as k, Modifiers};
use crate::combat::distance;

/// One character near a tile: horizontal and full distance, and whether it is the actor's
/// current target (`MULTIPLIER_TARGET_MY_HOSTILE` applies).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Near {
    pub position: [f32; 3],
    pub my_hostile: bool,
}

/// `base` within `min` metres, falling linearly to 0 at `max`.
pub fn nearby_value(d: f32, min: f32, max: f32, base: f32) -> f32 {
    let range = max - min;
    if range == 0.0 {
        if d < min { base } else { 0.0 }
    } else {
        // As the engine: max(d - min, 0), then min(…, range); a max below the min gives 0.
        base * (1.0 - (d - min).max(0.0).min(range) / range)
    }
}

/// The raw position terms for standing at `at`, and their parts for the debug view.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PositionTerms {
    pub allies: f32,
    pub enemies: f32,
    pub far: f32,
}

impl PositionTerms {
    pub fn raw(&self) -> f32 {
        self.allies + self.enemies + self.far
    }
}

/// Position terms. `fallback` uses the `FALLBACK_*` settings (the fallback move) instead of
/// the `ENDPOS_*` ones.
// bg3: esv::AiHelpers::CalculatePositionScore (0x105215238, 4.1.1.7631656)
pub fn position_terms(
    m: &Modifiers,
    at: [f32; 3],
    allies: &[Near],
    enemies: &[Near],
    fallback: bool,
) -> PositionTerms {
    let (ally_mult, ally_min, ally_max, enemy_mult, enemy_min, enemy_max) = if fallback {
        (
            m.get(k::MULTIPLIER_FALLBACK_ALLIES_NEARBY),
            m.get(k::FALLBACK_ALLIES_NEARBY_MIN_DISTANCE),
            m.get(k::FALLBACK_ALLIES_NEARBY_MAX_DISTANCE),
            m.get(k::MULTIPLIER_FALLBACK_ENEMIES_NEARBY),
            m.get(k::FALLBACK_ENEMIES_NEARBY_MIN_DISTANCE),
            m.get(k::FALLBACK_ENEMIES_NEARBY_MAX_DISTANCE),
        )
    } else {
        (
            m.get(k::MULTIPLIER_ENDPOS_ALLIES_NEARBY),
            m.get(k::ENDPOS_ALLIES_NEARBY_MIN_DISTANCE),
            m.get(k::ENDPOS_ALLIES_NEARBY_MAX_DISTANCE),
            m.get(k::MULTIPLIER_ENDPOS_ENEMIES_NEARBY),
            m.get(k::ENDPOS_ENEMIES_NEARBY_MIN_DISTANCE),
            m.get(k::ENDPOS_ENEMIES_NEARBY_MAX_DISTANCE),
        )
    };
    let base = m.get(k::BASE_NEARBY_SCORE);
    let radius = ally_max.max(enemy_max);
    let horizontal = |p: [f32; 3]| ((p[0] - at[0]).powi(2) + (p[2] - at[2]).powi(2)).sqrt();
    let mut closest = f32::MAX;
    let (mut enemy_sum, mut enemy_max_v) = (0.0f32, 0.0f32);
    for e in enemies {
        let h = horizontal(e.position);
        closest = closest.min(h * h);
        if h < radius {
            let mut v = nearby_value(distance(at, e.position), enemy_min, enemy_max, base);
            if e.my_hostile {
                v *= m.get(k::MULTIPLIER_TARGET_MY_HOSTILE);
            }
            enemy_sum += v;
            enemy_max_v = enemy_max_v.max(v);
        }
    }
    let mut ally_sum = 0.0f32;
    for a in allies {
        if horizontal(a.position) < radius {
            ally_sum += nearby_value(distance(at, a.position), ally_min, ally_max, base);
        }
    }
    // A positive multiplier counts the nearest enemy only, a negative one all of them.
    let enemy_v = if enemy_mult <= 0.0 {
        enemy_sum
    } else {
        enemy_max_v
    };
    let mut terms = PositionTerms {
        allies: ally_sum * -ally_mult,
        enemies: enemy_v * -enemy_mult,
        far: 0.0,
    };
    if closest != f32::MAX {
        let max_d = m.get(k::MAX_DISTANCE_TO_CLOSEST_ENEMY);
        if max_d * max_d < closest {
            terms.far = (closest.sqrt() - max_d) * m.get(k::MULTIPLIER_NO_ENEMIES_IN_MAX_DISTANCE);
        }
    }
    terms
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(enemy_mult: f32) -> Modifiers {
        let mut m = Modifiers::neutral();
        m.set(k::BASE_NEARBY_SCORE, 0.2);
        m.set(k::MULTIPLIER_ENDPOS_ENEMIES_NEARBY, enemy_mult);
        m.set(k::ENDPOS_ENEMIES_NEARBY_MIN_DISTANCE, 2.0);
        m.set(k::ENDPOS_ENEMIES_NEARBY_MAX_DISTANCE, 6.0);
        m.set(k::MAX_DISTANCE_TO_CLOSEST_ENEMY, 25.0);
        m.set(k::MULTIPLIER_NO_ENEMIES_IN_MAX_DISTANCE, 0.001);
        m
    }

    fn enemy(x: f32) -> Near {
        Near {
            position: [x, 0.0, 0.0],
            my_hostile: false,
        }
    }

    #[test]
    fn falloff_between_min_and_max() {
        assert_eq!(nearby_value(1.0, 2.0, 6.0, 0.2), 0.2);
        assert!((nearby_value(4.0, 2.0, 6.0, 0.2) - 0.1).abs() < 1e-6);
        assert_eq!(nearby_value(7.0, 2.0, 6.0, 0.2), 0.0);
        assert_eq!(nearby_value(1.0, 2.0, 2.0, 0.2), 0.2);
        assert_eq!(nearby_value(1.0, 1.9, 0.0, 0.2), 0.0, "max below min");
    }

    #[test]
    fn melee_counts_the_nearest_enemy_ranged_counts_all() {
        let at = [0.0; 3];
        let es = [enemy(1.0), enemy(4.0)];
        let melee = position_terms(&mods(0.02), at, &[], &es, false);
        assert!((melee.enemies + 0.2 * 0.02).abs() < 1e-6, "{melee:?}");
        let ranged = position_terms(&mods(-0.5), at, &[], &es, false);
        assert!((ranged.enemies - 0.3 * 0.5).abs() < 1e-6, "{ranged:?}");
    }

    #[test]
    fn far_from_every_enemy_costs() {
        let t = position_terms(&mods(0.02), [0.0; 3], &[], &[enemy(30.0)], false);
        assert!((t.far - 0.005).abs() < 1e-6);
    }
}
