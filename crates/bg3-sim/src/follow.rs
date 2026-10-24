//! Party members following the controlled character in a live game.
//!
//! In the engine, a follower runs the behaviour state `esv::BSFollowCharacter` (states such as
//! `FollowLeader`, `WaitForLeaderToApproach`, `JumpFollow…`). It walks to a slot of a formation
//! behind the leader (`GetFormationPosition`) along the leader's breadcrumbs, and jumps where
//! the leader jumped (`esv::jumpfollow`).
// bg3: esv::BSFollowCharacter::Update, (anonymous namespace)::GetFormationPosition
// (4.1.1.7631656)
//!
//! The formation (read from `GetFormationPosition`): with one follower its slot is 2 m behind
//! the leader. With two, the slots come from a 2-entry table, otherwise from a 14-entry table by
//! follower index modulo 14. Table entries (local x sideways, z forward) are doubled and turned
//! by the leader's heading; see [`FORMATION`].
//!
//! **Approximate**: the leader's heading is its last movement direction (the sim has no
//! orientation), followers path on the AI grid straight to their slot (no breadcrumbs or
//! jumps) at the default run speed, and start moving once they are more than [`SLACK`] from it.
//! When the leader is in a fight or a dialog, followers stay put.

use std::collections::{HashMap, HashSet, VecDeque};

use bg3_osiris::{HostCtx, Value};

use crate::entity::EntityKind;
use crate::guid::Guid;
use crate::sim::Sim;

/// Formation slots for three or more followers (local x, y, z; z < 0 behind), before doubling.
// bg3: the 14-entry Vector3f table GetFormationPosition indexes (4.1.1.7631656); its first two
// entries are also the two-follower table.
pub const FORMATION: [[f32; 3]; 14] = [
    [-0.5, 0.0, -1.0],
    [0.5, 0.0, -1.0],
    [0.0, 0.0, -2.0],
    [-1.0, 0.0, -2.0],
    [1.0, 0.0, -2.0],
    [-2.0, 0.0, -2.0],
    [2.0, 0.0, -2.0],
    [0.0, 0.0, -3.0],
    [-0.5, 0.0, -3.0],
    [0.5, 0.0, -3.0],
    [-1.5, 0.0, -3.0],
    [1.5, 0.0, -3.0],
    [-2.5, 0.0, -3.0],
    [2.5, 0.0, -3.0],
];

/// A follower closer than this (metres) to its slot doesn't move (*approximate*).
pub const SLACK: f32 = 1.5;
/// A path is planned again once the slot moved this far from its end (*approximate*).
const REPLAN: f32 = 1.0;
const RUN_SPEED: f32 = bg3_world::characters::DEFAULT_MOVEMENT_SPEED_RUN;

/// The local offset of follower `index` of `count`, doubled (see the module docs).
pub fn slot(index: usize, count: usize) -> [f32; 3] {
    let o = match count {
        0 | 1 => [0.0, 0.0, -1.0],
        2 => FORMATION[index & 1],
        _ => FORMATION[index % 14],
    };
    o.map(|v| v * 2.0)
}

#[derive(Clone, Debug, Default)]
pub struct Follow {
    /// `SetNoFollowFlag(x, 1)`.
    pub no_follow: HashSet<Guid>,
    /// The leader's heading on the ground plane (x, z), from its movement.
    heading: Option<[f32; 2]>,
    leader_at: Option<(Guid, [f32; 3])>,
    /// Followers on their way: the rest of the path.
    paths: HashMap<Guid, VecDeque<[f32; 3]>>,
    /// Followers done for now (arrived as close as the path goes, or no path): the slot
    /// then. Planned again once the slot moves [`REPLAN`] away.
    settled: HashMap<Guid, [f32; 3]>,
}

impl Sim {
    /// The character the party follows: the host character if in the party, else the first
    /// party member with a user.
    pub fn follow_leader(&self) -> Option<Guid> {
        self.story
            .host
            .filter(|h| self.party.contains(h))
            .or_else(|| {
                self.party
                    .iter()
                    .copied()
                    .find(|&p| self.entity(p).is_some_and(|e| e.user.is_some()))
            })
    }

    /// Move followers for one frame (called from [`Sim::update_stage`]).
    pub(crate) fn update_followers(&mut self, dt: f32) {
        let Some(leader) = self.follow_leader() else {
            return;
        };
        let Some(lp) = self.entity(leader).map(|e| e.position) else {
            return;
        };
        if let Some((l, prev)) = self.stage.follow.leader_at
            && l == leader
        {
            let (dx, dz) = (lp[0] - prev[0], lp[2] - prev[2]);
            let len = (dx * dx + dz * dz).sqrt();
            if len > 0.05 {
                self.stage.follow.heading = Some([dx / len, dz / len]);
            }
        }
        self.stage.follow.leader_at = Some((leader, lp));
        if self.in_combat(leader) || self.is_speaker_reserved(leader) {
            self.stage.follow.paths.clear();
            return;
        }
        let level = self.entity(leader).and_then(|e| e.level.clone());
        let followers: Vec<Guid> = self
            .party
            .iter()
            .copied()
            .filter(|&p| p != leader)
            .filter(|&p| self.can_follow(p, level.as_deref()))
            .collect();
        let follow = &mut self.stage.follow;
        follow.paths.retain(|id, _| followers.contains(id));
        follow.settled.retain(|id, _| followers.contains(id));
        let [hx, hz] = self.stage.follow.heading.unwrap_or([0.0, 1.0]);
        for (i, &f) in followers.iter().enumerate() {
            let o = slot(i, followers.len());
            // Forward is the heading, sideways is perpendicular to it.
            let target = [
                lp[0] + hz * o[0] + hx * o[2],
                lp[1],
                lp[2] - hx * o[0] + hz * o[2],
            ];
            self.step_follower(f, target, dt);
        }
    }

    fn can_follow(&self, id: Guid, level: Option<&str>) -> bool {
        let Some(e) = self.entity(id) else {
            return false;
        };
        e.kind == EntityKind::Character
            && e.level.as_deref() == level
            && self.on_stage(id)
            && !self.stage.follow.no_follow.contains(&id)
            && !self.in_combat(id)
            && !self.is_speaker_reserved(id)
            && !self.stage.moves.iter().any(|m| m.who == id)
            && self.combatant(id).is_none_or(|c| !c.dead && !c.downed)
    }

    fn step_follower(&mut self, id: Guid, target: [f32; 3], dt: f32) {
        let Some(at) = self.entity(id).map(|e| e.position) else {
            return;
        };
        let path = self.stage.follow.paths.get(&id);
        let planned_end = path.and_then(|p| p.back().copied());
        let settled = self.stage.follow.settled.get(&id).copied();
        let needs_plan = match (planned_end, settled) {
            (Some(end), _) => flat_distance(end, target) > REPLAN,
            (None, Some(s)) => {
                flat_distance(s, target) > REPLAN && flat_distance(at, target) > SLACK
            }
            (None, None) => flat_distance(at, target) > SLACK,
        };
        if needs_plan {
            self.stage.follow.settled.remove(&id);
            let points = match &self.combat.pathfinder {
                Some(nav) => nav
                    .find_path(at, target, crate::combat::path::APPROXIMATE_RADIUS)
                    .unwrap_or_default(),
                None => vec![at, target],
            };
            if points.is_empty() {
                self.stage.follow.paths.remove(&id);
                self.stage.follow.settled.insert(id, target);
                return;
            }
            self.stage.follow.paths.insert(id, points.into());
        }
        let Some(path) = self.stage.follow.paths.get_mut(&id) else {
            return;
        };
        let mut pos = at;
        let mut step = RUN_SPEED * dt;
        while let Some(&next) = path.front() {
            let d = crate::combat::distance(pos, next);
            if d > step {
                let k = step / d;
                pos = std::array::from_fn(|i| pos[i] + (next[i] - pos[i]) * k);
                break;
            }
            step -= d;
            pos = next;
            path.pop_front();
        }
        if path.is_empty() {
            self.stage.follow.paths.remove(&id);
            self.stage.follow.settled.insert(id, target);
        }
        if let Some(e) = self.entities.get_mut(id) {
            e.position = pos;
        }
    }
}

pub const CALLS: &[(&str, usize)] = &[("SetNoFollowFlag", 2)];
pub const QUERIES: &[(&str, usize)] = &[("HasNoFollowFlag", 2)];

fn guid(ctx: &HostCtx<'_>, v: &Value) -> Guid {
    ctx.text(v)
        .and_then(Guid::from_osiris)
        .unwrap_or(Guid::NULL)
}

fn is_character(sim: &Sim, id: Guid) -> bool {
    sim.entity(id)
        .is_some_and(|e| e.kind == EntityKind::Character)
}

// bg3: esv::OsirisCharacterFunctions::SetNoFollowFlag (4.1.1.7631656): raises or clears a
// character flag; fails for anything but a character.
pub(crate) fn call(sim: &mut Sim, ctx: &HostCtx<'_>, name: &str, args: &[Value]) -> Option<bool> {
    if !sim.stage.live || !CALLS.contains(&(name, args.len())) {
        return None;
    }
    let id = guid(ctx, &args[0]);
    if !is_character(sim, id) {
        return Some(false);
    }
    if matches!(args[1].data, bg3_osiris::Data::Int(0)) {
        sim.stage.follow.no_follow.remove(&id);
    } else {
        sim.stage.follow.no_follow.insert(id);
    }
    Some(true)
}

// bg3: esv::OsirisCharacterFunctions::HasNoFollowFlag (4.1.1.7631656)
pub(crate) fn query(
    sim: &mut Sim,
    ctx: &mut HostCtx<'_>,
    name: &str,
    args: &mut [Value],
) -> Option<bool> {
    if !sim.stage.live || !QUERIES.contains(&(name, args.len())) {
        return None;
    }
    let id = guid(ctx, &args[0]);
    if !is_character(sim, id) {
        return Some(false);
    }
    args[1] = Value::int(i32::from(sim.stage.follow.no_follow.contains(&id)));
    Some(true)
}

fn flat_distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formation_slots_are_doubled_table_entries() {
        assert_eq!(slot(0, 1), [0.0, 0.0, -2.0]);
        assert_eq!(slot(1, 2), [1.0, 0.0, -2.0]);
        assert_eq!(slot(2, 3), [0.0, 0.0, -4.0]);
        assert_eq!(slot(15, 16), [1.0, 0.0, -2.0]);
    }
}
