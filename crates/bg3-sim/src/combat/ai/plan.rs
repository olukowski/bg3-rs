//! Candidate actions for one AI decision, their scores, and the choice.
//!
//! The engine's request (`esv::AiHelpers::PrepareRequest` → `UpdateRequest`, 0x1051f94e0,
//! 4.1.1.7631656) gathers the castable spells (`esv::ai::combat::CanAICastSpell`), simulates
//! each on each target from the best tiles around (`esv::AiSpellState::CalculateSpellAi`,
//! `esv::AiTileIteratorHelper::IterateTiles`, top 5 tiles), adds the end position
//! (`CalculatePositionScore`) and the walk there (`CalculateMovementScore`, with attacks of
//! opportunity via `CalculateAiScoreAoO`), scores every `esv::CombatAIAction`
//! (`CombatAIAction::CalculateScore`), sorts them (`LEGACY_Set<CombatAIAction>::Sort`: final
//! score, then raw score, then …) and executes the first. Actions that can't be done this
//! turn stay in the list at `MULTIPLIER_CANNOT_EXECUTE_THIS_TURN`, which moves the character
//! towards them. No randomness is involved in the choice.
//!
//! Our candidates: every usable single-target spell × every participant (and the caster) ×
//! a set of reachable tiles (where the character stands, the point on the route to each
//! participant where each spell comes into range, and 16 points around it), plus the
//! "future" version of each spell on each enemy (walk the movement budget towards it).

use super::archetype::{self as k, Modifiers};
use super::position::{Near, PositionTerms, position_terms};
use super::score::{
    ActorInfo, AiScore, Relation, ScoreBreakdown, TargetInfo, calculate, final_score,
};
use super::simulate::{Planner, simulated_type};
use crate::combat::data::Replenish;
use crate::combat::path::{APPROXIMATE_RADIUS, walk_path};
use crate::combat::{CombatError, distance};
use crate::guid::Guid;
use crate::sim::Sim;

/// Score of an action the engine rejects (`CombatAIAction::CalculateScore`: -1000).
pub const INVALID_SCORE: f32 = -1000.0;

/// Margin inside a spell's range when walking into it (metres).
const RANGE_MARGIN: f32 = 0.05;

/// A known spell the AI may use: now, or once its resources are back (future action).
struct UsableSpell {
    name: String,
    book_index: usize,
    range: f32,
    now: bool,
    costs: Vec<(String, f32)>,
}

/// A place the character can stand this turn and the walk there.
#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    pub position: [f32; 3],
    /// Walked polyline including both endpoints; empty when standing still.
    pub path: Vec<[f32; 3]>,
    /// Path length (metres of `Movement`).
    pub cost: f32,
}

/// One scored candidate.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub spell: String,
    /// Position of the spell in the character's spell list (weapon attacks first).
    pub book_index: usize,
    pub target: Guid,
    /// False: the action can't be done this turn; executing it walks towards it.
    pub this_turn: bool,
    pub tile: Tile,
    /// Chance of the spell roll and as the archetype believes it.
    pub chance: f32,
    pub believed: f32,
    /// Expected effects (spell, position, movement) before scoring.
    pub effects: AiScore,
    pub position: PositionTerms,
    /// Expected attack-of-opportunity damage taken on the walk.
    pub aoo_damage: f32,
    pub breakdown: ScoreBreakdown,
    /// Resource cost part of the final score.
    pub cost: f32,
    /// The value candidates are sorted by.
    pub score: f32,
}

/// Everything the AI considered for one decision.
#[derive(Clone, Debug, PartialEq)]
pub struct AiPlan {
    pub actor: Guid,
    pub archetype: String,
    pub average_damage: f32,
    /// Best first.
    pub candidates: Vec<Candidate>,
}

impl AiPlan {
    /// The action the AI takes: the best candidate, if it scores above zero.
    pub fn chosen(&self) -> Option<&Candidate> {
        self.candidates.first().filter(|c| c.score > 0.0)
    }
}

/// Sort order: final score, raw score, the shorter walk, then spell-book and target order.
// bg3: ls::LEGACY_Set<esv::CombatAIAction>::Sort (0x105213e6c, 4.1.1.7631656) compares the
// final score, the raw score, a flag, whether there is a target, then a distance-like value;
// our walk, book and target keys stand in for the unread rest (ties are common).
fn order(a: &Candidate, b: &Candidate) -> std::cmp::Ordering {
    b.score
        .total_cmp(&a.score)
        .then(b.breakdown.total.total_cmp(&a.breakdown.total))
        .then(a.tile.cost.total_cmp(&b.tile.cost))
        .then(a.book_index.cmp(&b.book_index))
        .then(a.target.0.cmp(&b.target.0))
}

impl Sim {
    /// Walkable route (both endpoints) from `from` towards `to`: the pathfinder's, or a
    /// straight line without one.
    fn ai_route(&self, from: [f32; 3], to: [f32; 3]) -> Option<Vec<[f32; 3]>> {
        match &self.combat.pathfinder {
            Some(p) => p
                .find_path(from, to, APPROXIMATE_RADIUS)
                .filter(|r| r.len() >= 2 && r.first() == Some(&from)),
            None => Some(vec![from, to]),
        }
    }

    /// Walk at most `budget` along the route to `to`, stopping `range` from it.
    fn ai_walk(&self, from: [f32; 3], to: [f32; 3], budget: f32, range: f32) -> Option<Tile> {
        let route = self.ai_route(from, to)?;
        let (path, cost, _) = walk_path(&route, budget, to, range);
        if cost < 1e-3 {
            return None;
        }
        let position = *path.last()?;
        Some(Tile {
            position,
            path,
            cost,
        })
    }

    fn ai_relation(&self, actor: Guid, who: Guid) -> Relation {
        if who == actor {
            Relation::Ally
        } else if self.is_hostile(actor, who) {
            Relation::Enemy
        } else if self.relation(actor, who) >= crate::combat::data::RELATION_ALLIED {
            Relation::Ally
        } else {
            Relation::Neutral
        }
    }

    /// Living participants of `actor`'s fight other than the actor.
    fn ai_others(&self, actor: Guid) -> Vec<Guid> {
        let Some(c) = self.combat_of(actor) else {
            return Vec::new();
        };
        c.participants
            .iter()
            .copied()
            .filter(|&p| p != actor)
            .filter(|&p| self.combatant(p).is_some_and(|x| !x.dead))
            .filter(|&p| self.entity(p).is_some())
            .collect()
    }

    fn ai_target_info(&self, actor: Guid, who: Guid, enemies: &[Guid]) -> Option<TargetInfo> {
        let sheet = self.entity(who)?.sheet.as_ref()?;
        let relation = self.ai_relation(actor, who);
        let mut t = TargetInfo::new(relation, sheet.hp, sheet.max_hp);
        t.is_self = who == actor;
        t.downed = self.combatant(who).is_some_and(|c| c.downed);
        t.out_of_combat = self.combat_of(who).map(|c| c.id) != self.combat_of(actor).map(|c| c.id);
        if relation == Relation::Ally && !t.is_self {
            let at = self.entity(who)?.position;
            t.closest_enemy = enemies
                .iter()
                .filter_map(|e| self.entity(*e).map(|x| distance(at, x.position)))
                .min_by(f32::total_cmp);
        }
        Some(t)
    }

    /// Resource cost score of a spell: per cost, amount / the resource's maximum, weighted
    /// by how the resource replenishes.
    // bg3: esv::AiHelpers::GetResourceCostScore (0x10521cc50, 4.1.1.7631656)
    fn ai_resource_cost(&self, who: Guid, m: &Modifiers, costs: &[(String, f32)]) -> f32 {
        let Some(c) = self.combatant(who) else {
            return 0.0;
        };
        costs
            .iter()
            .map(|(name, amount)| {
                let max = c.resources.max(name);
                let max = if max > 0.0 { max } else { 1.0 };
                let mult = match self.data.combat.resource(name).map(|r| r.replenish) {
                    Some(Replenish::Turn) => m.get(k::MULTIPLIER_RESOURCE_REPLENISH_TYPE_TURN),
                    Some(Replenish::ShortRest) => {
                        m.get(k::MULTIPLIER_RESOURCE_REPLENISH_TYPE_SHORT_REST)
                    }
                    Some(Replenish::Rest) => m.get(k::MULTIPLIER_RESOURCE_REPLENISH_TYPE_REST),
                    Some(Replenish::Never) => m.get(k::MULTIPLIER_RESOURCE_REPLENISH_TYPE_NEVER),
                    Some(Replenish::Other) => m.get(k::MULTIPLIER_RESOURCE_REPLENISH_TYPE_COMBAT),
                    None => 1.0,
                };
                amount / max * mult
            })
            .sum()
    }

    /// The melee attack a character would make as an attack of opportunity.
    fn ai_opportunity_attack(&self, who: Guid) -> Option<(String, f32)> {
        let c = self.combatant(who)?;
        if c.dead || c.downed || c.resources.get("ReactionActionPoint") < 1.0 {
            return None;
        }
        c.spells.iter().find_map(|s| {
            let d = self.data.combat.spells.get(s)?;
            (d.has_flag("IsMelee") && d.has_flag("IsAttack") && simulated_type(&d.spell_type))
                .then(|| Some((s.clone(), self.spell_range(who, s)?)))
                .flatten()
        })
    }

    /// Expected effects of the attacks of opportunity the walk provokes: enemies whose reach
    /// the path leaves attack the mover.
    // bg3: esv::AiHelpers::CalculateMovementScore (0x10521935c) and CalculateAiScoreAoO
    // (0x10524f0f8, 4.1.1.7631656): the enemy's attack is simulated on the mover with the
    // mover's modifiers. *Approximate*: reach is centre to centre; surfaces and auras on the
    // path are not scored.
    fn ai_aoo(
        &mut self,
        pl: &mut Planner,
        m: &Modifiers,
        mover: Guid,
        path: &[[f32; 3]],
        enemies: &[Guid],
    ) -> AiScore {
        let mut out = AiScore::default();
        if path.len() < 2 {
            return out;
        }
        let mut samples = Vec::new();
        for w in path.windows(2) {
            let n = (distance(w[0], w[1]) / 0.25).ceil().max(1.0) as usize;
            for i in 1..=n {
                let f = i as f32 / n as f32;
                samples.push(std::array::from_fn::<_, 3, _>(|j| {
                    w[0][j] + (w[1][j] - w[0][j]) * f
                }));
            }
        }
        for &e in enemies {
            let Some(at) = self.entity(e).map(|x| x.position) else {
                continue;
            };
            let Some((spell, reach)) = self.ai_opportunity_attack(e) else {
                continue;
            };
            if distance(path[0], at) > reach || samples.iter().all(|p| distance(*p, at) <= reach) {
                continue;
            }
            let score = match pl.opportunity.get(&(e, mover)) {
                Some(s) => s.clone(),
                None => {
                    let s = self.ai_simulate(pl, m, &spell, e, mover).map(|x| x.score);
                    pl.opportunity.insert((e, mover), s.clone());
                    s
                }
            };
            if let Some(score) = score {
                out.add(&score, 1.0);
            }
        }
        out
    }

    /// Tiles the character can reach this turn for its spells' ranges.
    fn ai_tiles(&self, who: Guid, others: &[Guid], ranges: &[f32]) -> Vec<Tile> {
        let Some(from) = self.entity(who).map(|e| e.position) else {
            return Vec::new();
        };
        let budget = self
            .combatant(who)
            .map_or(0.0, |c| c.resources.get("Movement"));
        let mut tiles = vec![Tile {
            position: from,
            path: Vec::new(),
            cost: 0.0,
        }];
        if budget > 0.0 {
            for &o in others {
                let Some(to) = self.entity(o).map(|e| e.position) else {
                    continue;
                };
                for r in ranges {
                    if let Some(t) = self.ai_walk(from, to, budget, (r - RANGE_MARGIN).max(0.0)) {
                        tiles.push(t);
                    }
                }
            }
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::FRAC_PI_4;
                for f in [0.5, 1.0] {
                    let d = budget * f;
                    let to = [from[0] + a.cos() * d, from[1], from[2] + a.sin() * d];
                    if let Some(t) = self.ai_walk(from, to, budget, 0.0) {
                        tiles.push(t);
                    }
                }
            }
        }
        let mut out: Vec<Tile> = Vec::new();
        for t in tiles {
            if t.path.is_empty() && t.cost > 0.0 {
                continue;
            }
            if !out
                .iter()
                .any(|o| distance(o.position, t.position) < 0.1 && o.cost <= t.cost)
            {
                out.push(t);
            }
        }
        out
    }

    /// Score every candidate action for `who` now. Nothing is changed.
    pub fn ai_plan(&mut self, who: Guid) -> Option<AiPlan> {
        let mut pl = self.ai_planner();
        self.ai_plan_with(&mut pl, who)
    }

    pub(super) fn ai_plan_with(&mut self, pl: &mut Planner, who: Guid) -> Option<AiPlan> {
        let c = self.combatant(who)?.clone();
        if c.dead || c.downed {
            return None;
        }
        pl.opportunity.clear();
        let m = self.ai_modifiers(who);
        let actor = ActorInfo {
            average_damage: self.ai_average_damage(pl, who),
            max_hp: self
                .entity(who)
                .and_then(|e| e.sheet.as_ref())
                .map_or(0, |s| s.max_hp),
        };
        let others = self.ai_others(who);
        let enemies: Vec<Guid> = others
            .iter()
            .copied()
            .filter(|o| self.ai_relation(who, *o) == Relation::Enemy)
            .collect();
        let near = |s: &Sim, list: &[Guid]| -> Vec<Near> {
            list.iter()
                .filter_map(|g| {
                    Some(Near {
                        position: s.entity(*g)?.position,
                        my_hostile: false,
                    })
                })
                .collect()
        };
        let (enemy_near, ally_near) = (
            near(self, &enemies),
            near(
                self,
                &others
                    .iter()
                    .copied()
                    .filter(|o| !enemies.contains(o))
                    .collect::<Vec<_>>(),
            ),
        );
        // Spells: castable now, or (future) with full resources.
        let mut spells: Vec<UsableSpell> = Vec::new();
        for (book_index, s) in c.spells.iter().enumerate() {
            let Some(d) = self.data.combat.spells.get(s) else {
                continue;
            };
            if !simulated_type(&d.spell_type) {
                continue;
            }
            let Some(range) = self.spell_range(who, s) else {
                continue;
            };
            let now = c.resources.can_pay(&d.use_costs);
            let ever = d
                .use_costs
                .iter()
                .all(|(n, a)| c.resources.max(n) + 1e-4 >= *a);
            if now || ever {
                spells.push(UsableSpell {
                    name: s.clone(),
                    book_index,
                    range,
                    now,
                    costs: d.use_costs.clone(),
                });
            }
        }
        let mut ranges: Vec<f32> = spells.iter().filter(|s| s.now).map(|s| s.range).collect();
        ranges.sort_by(f32::total_cmp);
        ranges.dedup();
        let tiles = self.ai_tiles(who, &others, &ranges);
        let budget = c.resources.get("Movement");
        let mut targets = others.clone();
        targets.push(who);
        let from = self.entity(who)?.position;
        let mut candidates = Vec::new();
        for UsableSpell {
            name: spell,
            book_index,
            range,
            now,
            costs,
        } in &spells
        {
            let resource_cost = self.ai_resource_cost(who, &m, costs);
            for &t in &targets {
                let Some(at) = self.entity(t).map(|e| e.position) else {
                    continue;
                };
                let Some(sim) = self.ai_simulate(pl, &m, spell, who, t) else {
                    continue;
                };
                let mut places: Vec<(Tile, bool)> = Vec::new();
                if *now {
                    for tile in &tiles {
                        if t == who || distance(tile.position, at) <= range + 1e-3 {
                            places.push((tile.clone(), true));
                        }
                    }
                }
                if places.is_empty() && t != who && enemies.contains(&t) {
                    // Can't be done this turn: walk the budget towards it.
                    let tile = self
                        .ai_walk(from, at, budget, (range - RANGE_MARGIN).max(0.0))
                        .unwrap_or(Tile {
                            position: from,
                            path: Vec::new(),
                            cost: 0.0,
                        });
                    places.push((tile, false));
                }
                for (tile, this_turn) in places {
                    let mut effects = sim.score.clone();
                    let position =
                        position_terms(&m, tile.position, &ally_near, &enemy_near, !this_turn);
                    effects.add_raw(who, position.raw());
                    let aoo = self.ai_aoo(pl, &m, who, &tile.path, &enemies);
                    let aoo_damage = aoo.get(who).damage;
                    effects.add(&aoo, m.get(k::ENABLE_MOVEMENT_AVOID_AOO));
                    let breakdown = calculate(&m, &actor, &effects, |g| {
                        self.ai_target_info(who, g, &enemies)
                    });
                    let (mut score, cost) =
                        final_score(&m, breakdown.total, resource_cost, this_turn);
                    // A move-only action that doesn't move is rejected.
                    if breakdown.invalid || (!this_turn && tile.path.is_empty()) {
                        score = INVALID_SCORE;
                    }
                    candidates.push(Candidate {
                        spell: spell.clone(),
                        book_index: *book_index,
                        target: t,
                        this_turn,
                        tile,
                        chance: sim.chance,
                        believed: sim.believed,
                        effects,
                        position,
                        aoo_damage,
                        breakdown,
                        cost,
                        score,
                    });
                }
            }
        }
        candidates.sort_by(order);
        Some(AiPlan {
            actor: who,
            archetype: m.archetype,
            average_damage: actor.average_damage,
            candidates,
        })
    }

    /// Carry out a candidate: walk, then cast (this turn) or stop (future action).
    pub(super) fn ai_execute(&mut self, who: Guid, c: &Candidate) -> Result<(), CombatError> {
        if let Some(&end) = c.tile.path.last() {
            self.combat_move(who, c.tile.cost, Some(end))?;
        }
        if c.this_turn {
            self.combat_cast(who, &c.spell, c.target)?;
        }
        Ok(())
    }
}
