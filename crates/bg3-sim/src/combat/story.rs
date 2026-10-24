//! Story queries the fight scripts use (the helm fight, `GLO_` combat helpers), answered from
//! the sim's combat state in a live game ([`crate::presentation::Stage::live`]). The trace
//! harness leaves the state-dependent ones to the trace (hit points and positions in a replay
//! are not the game's); `GetDefaultState`, `IsBoss` and `GetCombatGroupID` come from level
//! data and are answered (and graded) in a replay too.
//!
//! Semantics from the reference build's `esv::Osiris*Functions` (4.1.1.7631656, `objdump` of
//! the arm64 slice); notes in `docs/research/osiris-engine-api.md`, "Fight queries".

use bg3_osiris::{HostCtx, Value};

use super::{data, distance};
use crate::entity::EntityKind;
use crate::guid::Guid;
use crate::sim::Sim;

/// The queries of this module, `(name, arity)`.
pub const QUERIES: &[(&str, usize)] = &[
    ("IsBoss", 2),
    ("GetHitpointsPercentage", 2),
    ("IsOnStage", 2),
    ("GetDistanceTo", 3),
    ("GetCombatGroupID", 2),
    ("GetDefaultState", 2),
    ("IsAlly", 3),
    ("IsControlled", 2),
];

fn guid(ctx: &HostCtx<'_>, v: &Value) -> Guid {
    ctx.text(v)
        .and_then(Guid::from_osiris)
        .unwrap_or(Guid::NULL)
}

/// Answer one of [`QUERIES`] in a live game; `None` otherwise (or when the object is
/// unknown, which fails the query like the engine's missing component does).
pub(crate) fn query(
    sim: &mut Sim,
    ctx: &mut HostCtx<'_>,
    name: &str,
    args: &mut [Value],
) -> Option<bool> {
    if !QUERIES.contains(&(name, args.len())) {
        return None;
    }
    // Answers from level data hold in a replay too (and are graded against the trace);
    // the rest follow live state only.
    if !sim.stage.live && !matches!(name, "GetDefaultState" | "IsBoss" | "GetCombatGroupID") {
        return None;
    }
    let id = guid(ctx, &args[0]);
    let n = args.len();
    let out = match name {
        // bg3: esv::OsirisGameFunctions::IsBoss (4.1.1.7631656): fails without an
        // eoc::combat::ParticipantComponent, else its boss flag.
        "IsBoss" => {
            let c = character_combatant(sim, id)?;
            Value::int(i32::from(c.is_boss))
        }
        // bg3: esv::OsirisGameFunctions::GetHitpointsPercentage (4.1.1.7631656):
        // (hp + temporary hp) × 100 / (max hp + temporary hp) from eoc::HealthComponent, as a
        // real. The sim has no temporary hit points.
        "GetHitpointsPercentage" => {
            let s = sim.entity(id)?.sheet.as_ref()?;
            if s.max_hp <= 0 {
                return None;
            }
            Value::real(s.hp.max(0) as f32 * 100.0 / s.max_hp as f32)
        }
        // bg3: esv::OsirisGameFunctions::IsOnStage (4.1.1.7631656): the game object's
        // on-stage flag; unknown objects fail.
        "IsOnStage" => {
            sim.entity(id)?;
            let on = sim.on_stage(id) && !sim.combat.off_stage.contains(&id);
            Value::int(i32::from(on))
        }
        // bg3: esv::OsirisGameFunctions::GetDistanceTo (4.1.1.7631656): the 3D distance
        // between the two objects' transforms.
        "GetDistanceTo" => {
            let a = sim.entity(id)?.position;
            let b = sim.entity(guid(ctx, &args[1]))?.position;
            Value::real(distance(a, b))
        }
        // bg3: esv::OsirisGameFunctions::GetCombatGroupID (4.1.1.7631656): the participant
        // component's combat group, an empty string without one.
        "GetCombatGroupID" => {
            let g = character_combatant(sim, id)?
                .combat_group
                .clone()
                .unwrap_or_default();
            ctx.string(&g)
        }
        // bg3: esv::OsirisCharacterFunctions::GetDefaultState (4.1.1.7631656): the death type
        // the character was placed with (`Dead_Physical`, …), else an empty string.
        "GetDefaultState" => {
            if sim.entity(id)?.kind != EntityKind::Character {
                return None;
            }
            let s = sim
                .object_combat(id)
                .default_state
                .and_then(super::death_type)
                .unwrap_or_default();
            ctx.string(s)
        }
        // bg3: esv::OsirisGameFunctions::IsAlly (4.1.1.7631656): the relation of the first
        // object towards the second is 100 (eoc::relation::LEGACY_GetRelation).
        "IsAlly" => {
            let other = guid(ctx, &args[1]);
            sim.entity(id)?;
            sim.entity(other)?;
            Value::int(i32::from(sim.relation(id, other) == data::RELATION_ALLIED))
        }
        // bg3: esv::OsirisCharacterFunctions::IsControlled (4.1.1.7631656) compares the
        // character with its user's controlled character. *Approximate*: the sim has one
        // local user whose controlled character is the host character (the avatar).
        "IsControlled" => {
            let e = sim.entity(id)?;
            if e.kind != EntityKind::Character {
                return None;
            }
            Value::int(i32::from(e.user.is_some() && sim.story.host == Some(id)))
        }
        _ => return None,
    };
    args[n - 1] = out;
    Some(true)
}

/// A character's combat state (the engine's participant component), built on first use.
fn character_combatant(sim: &mut Sim, id: Guid) -> Option<&super::Combatant> {
    if sim.entity(id)?.kind != EntityKind::Character || !sim.ensure_combatant(id) {
        return None;
    }
    sim.combatant(id)
}
