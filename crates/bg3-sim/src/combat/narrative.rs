//! Narrative combats: fights the story sets up itself (`GLO_NarrativeCombat`), like the
//! Nautiloid's helm fight (`TUT_Helm_FinalCombat`: Zhalk and the mind flayer, then whoever
//! enters the helm's region or is seen by a participant).
//!
//! In the engine `CreateNarrativeCombat` makes a random GUID and queues a create request;
//! `SetInNarrativeCombat(entity, combat, on)` queues a join or leave request; both are
//! carried out by `esv::combat::System::ProcessNarrativeCombatRequests`.
//! `IsInNarrativeCombat` compares the entity's combat participant component with the GUID.
// bg3: esv::OsirisGameFunctions::CreateNarrativeCombat / SetInNarrativeCombat /
// IsInNarrativeCombat (4.1.1.7631656)
//!
//! **Approximate** (the request processing was not read): the GUID is the sim's combat
//! counter; a narrative combat starts once two of its members are hostile to each other, with
//! exactly its members (no allies pulled in); later members join the running fight; leaving
//! only removes a member that has not started fighting. Live games only.

use bg3_osiris::{HostCtx, Value};

use crate::entity::EntityKind;
use crate::guid::Guid;
use crate::sim::Sim;

pub const CALLS: &[(&str, usize)] = &[("SetInNarrativeCombat", 3)];
pub const QUERIES: &[(&str, usize)] = &[("CreateNarrativeCombat", 1), ("IsInNarrativeCombat", 3)];

fn guid(ctx: &HostCtx<'_>, v: &Value) -> Guid {
    ctx.text(v)
        .and_then(Guid::from_osiris)
        .unwrap_or(Guid::NULL)
}

fn int(v: &Value) -> i32 {
    match v.data {
        bg3_osiris::Data::Int(i) => i,
        bg3_osiris::Data::Int64(i) => i as i32,
        _ => 0,
    }
}

pub(crate) fn call(sim: &mut Sim, ctx: &HostCtx<'_>, name: &str, args: &[Value]) -> Option<bool> {
    if !sim.stage.live || !CALLS.contains(&(name, args.len())) {
        return None;
    }
    let (who, cid) = (guid(ctx, &args[0]), guid(ctx, &args[1]));
    Some(sim.set_in_narrative_combat(who, cid, int(&args[2]) != 0))
}

pub(crate) fn query(
    sim: &mut Sim,
    ctx: &mut HostCtx<'_>,
    name: &str,
    args: &mut [Value],
) -> Option<bool> {
    if !sim.stage.live || !QUERIES.contains(&(name, args.len())) {
        return None;
    }
    match name {
        "CreateNarrativeCombat" => {
            let cid = sim.create_narrative_combat();
            args[0] = ctx.guid(&cid.to_string());
        }
        "IsInNarrativeCombat" => {
            let (who, cid) = (guid(ctx, &args[0]), guid(ctx, &args[1]));
            sim.entity(who)?;
            args[2] = Value::int(i32::from(sim.is_in_narrative_combat(who, cid)));
        }
        _ => return None,
    }
    Some(true)
}

impl Sim {
    /// `CreateNarrativeCombat`: a new, empty narrative combat.
    pub fn create_narrative_combat(&mut self) -> Guid {
        let cid = self.new_combat_id();
        self.combat.narrative.push((cid, Vec::new()));
        self.combat.narrative_ids.insert(cid);
        cid
    }

    /// `SetInNarrativeCombat(who, combat, on)`.
    pub fn set_in_narrative_combat(&mut self, who: Guid, cid: Guid, on: bool) -> bool {
        if !self.combat.narrative_ids.contains(&cid)
            || self
                .entity(who)
                .is_none_or(|e| e.kind != EntityKind::Character)
        {
            return false;
        }
        if self.combat_by_id(cid).is_some_and(|c| !c.finished) {
            // Running: join (leaving a running fight is not modelled).
            if on {
                self.join_combat(cid, who);
            }
            return true;
        }
        let Some(i) = self.combat.narrative.iter().position(|(c, _)| *c == cid) else {
            return false;
        };
        let members = &mut self.combat.narrative[i].1;
        if !on {
            members.retain(|m| *m != who);
            return true;
        }
        if !members.contains(&who) {
            members.push(who);
        }
        let members = members.clone();
        let mut fighters: Vec<Guid> = members
            .into_iter()
            .filter(|&m| self.ensure_combatant(m) && self.combatant(m).is_some_and(|c| !c.dead))
            .collect();
        fighters.retain(|&m| self.combat_of(m).is_none_or(|c| c.finished));
        let hostile = fighters
            .iter()
            .any(|&a| fighters.iter().any(|&b| self.is_hostile(a, b)));
        if hostile {
            self.combat.narrative.remove(i);
            self.start_combat(cid, fighters);
        }
        true
    }

    /// `IsInNarrativeCombat(who, combat)`: a member of it, started or not.
    pub fn is_in_narrative_combat(&self, who: Guid, cid: Guid) -> bool {
        self.combat_of(who)
            .is_some_and(|c| c.id == cid && !c.finished)
            || self
                .combat
                .narrative
                .iter()
                .any(|(c, m)| *c == cid && m.contains(&who))
    }
}
