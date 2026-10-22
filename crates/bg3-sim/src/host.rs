//! The engine side of Osiris: story calls and queries answered by the [`Sim`].
//!
//! Functions are registered in the game by `esv::Osiris*Functions` (`OsirisGameFunctions`,
//! `OsirisDialogFunctions`, `OsirisMathFunctions`, …) and identified here by name and arity
//! like in `bg3_osiris`. [`SimFunctions::call`] / [`SimFunctions::query`] return `None` for
//! functions the sim does not implement, so a driver can fall back to something else (the
//! trace replay answers those from the trace). [`SimHost`] is the plain [`Host`]: unknown
//! calls succeed and do nothing, unknown queries fail.

use bg3_osiris::{Data, Host, HostCtx, Value};

use crate::entity::{EntityKind, Origin};
use crate::guid::Guid;
use crate::sim::Sim;

/// Every `(name, arity)` the sim implements, calls first. Kept in sync with the dispatch in
/// [`SimFunctions`] (test `implemented_list_matches_dispatch`).
pub const CALLS: &[(&str, usize)] = &[
    ("SetFlag", 4),
    ("ClearFlag", 4),
    ("SetTag", 2),
    ("ClearTag", 2),
    ("TimerLaunch", 2),
    ("TimerCancel", 1),
    ("ObjectTimerLaunch", 4),
    ("ObjectTimerCancel", 2),
    ("RealtimeObjectTimerLaunch", 3),
    ("RealtimeObjectTimerCancel", 2),
    ("ApplyStatus", 5),
    ("RemoveStatus", 3),
    ("TeleportPartiesToLevelWithMovie", 3),
    ("SetFaction", 2),
    ("DialogRequestStop", 1),
    ("DialogAddActorAtReservedSlot", 5),
    ("EnterCombat", 2),
    ("EndTurn", 1),
    ("SetOnStage", 2),
];

pub const QUERIES: &[(&str, usize)] = &[
    ("GetFlag", 3),
    ("GetFlagName", 2),
    ("IsTagged", 3),
    ("Exists", 2),
    ("IsCharacter", 2),
    ("IsItem", 2),
    ("GetTemplate", 2),
    ("GetRegion", 2),
    ("HasActiveStatus", 3),
    ("HasAppliedStatus", 3),
    ("HasActiveStatusWithGroup", 3),
    ("HasAppliedStatusWithGroup", 3),
    ("IsSummon", 2),
    ("IsWeapon", 2),
    ("IsPartyMember", 3),
    ("GetFaction", 2),
    ("IsGameLevel", 2),
    ("IsCharacterCreationLevel", 2),
    ("GetActiveModStartupLevel", 1),
    ("IsStatusFromGroup", 3),
    ("GetStatusType", 2),
    ("TimerExists", 2),
    ("ObjectTimerExists", 3),
    ("IntegerSum", 3),
    ("IntegerSubtract", 3),
    ("IntegerProduct", 3),
    ("IntegerDivide", 3),
    ("IntegerModulo", 3),
    ("IntegerMax", 3),
    ("IntegerMin", 3),
    ("RealSum", 3),
    ("RealSubtract", 3),
    ("RealProduct", 3),
    ("RealDivide", 3),
    ("Concatenate", 3),
    ("ConcatenateInteger", 3),
    ("GUIDToString", 2),
    ("StringToGUID", 2),
    ("StartDialog_Internal", 11),
    ("DialogIsAutomated", 2),
    ("DialogIsAllowingJoinCombat", 2),
    ("DialogAllowDeadSpeakers", 2),
    ("DialogSpeakerHasLines", 3),
    ("IsSpeakerReserved", 2),
    ("DialogGetInvolvedPlayer", 3),
    ("DialogGetInvolvedNPC", 3),
    ("DialogGetNumberOfInvolvedPlayers", 2),
    ("DialogGetNumberOfInvolvedNPCs", 2),
    ("IsInCombat", 2),
];

/// Dialog functions: answered only when the sim has a dialog bank
/// ([`Sim::set_dialog_bank`]); without one they are left to the caller.
fn is_dialog_function(name: &str) -> bool {
    name.starts_with("Dialog") || name == "StartDialog_Internal" || name == "IsSpeakerReserved"
}

/// Is `(name, arity)` implemented by the sim (call or query)?
pub fn implements(name: &str, arity: usize) -> bool {
    CALLS
        .iter()
        .chain(QUERIES)
        .chain(crate::story_misc::CALLS)
        .chain(crate::story_misc::QUERIES)
        .chain(crate::surface::CALLS)
        .chain(crate::surface::QUERIES)
        .any(|(n, a)| *a == arity && n.eq_ignore_ascii_case(name))
}

/// Argument helpers.
fn guid(ctx: &HostCtx<'_>, v: &Value) -> Guid {
    ctx.text(v)
        .and_then(Guid::from_osiris)
        .unwrap_or(Guid::NULL)
}

fn text(ctx: &HostCtx<'_>, v: &Value) -> String {
    ctx.text(v).unwrap_or_default().to_owned()
}

fn int(v: &Value) -> i32 {
    match v.data {
        Data::Int(i) => i,
        Data::Int64(i) => i as i32,
        Data::Real(r) => r as i32,
        _ => 0,
    }
}

fn real(v: &Value) -> f32 {
    match v.data {
        Data::Int(i) => i as f32,
        Data::Int64(i) => i as f32,
        Data::Real(r) => r,
        _ => 0.0,
    }
}

fn bool_out(b: bool) -> Value {
    Value::int(i32::from(b))
}

/// The sim's Osiris functions over a `&mut Sim`.
pub struct SimFunctions<'a> {
    pub sim: &'a mut Sim,
}

impl<'a> SimFunctions<'a> {
    pub fn new(sim: &'a mut Sim) -> Self {
        Self { sim }
    }

    /// Run a call; `None` if the sim does not implement it, else whether it succeeded.
    pub fn call(&mut self, ctx: &mut HostCtx<'_>, name: &str, args: &[Value]) -> Option<bool> {
        let sim = &mut *self.sim;
        if let Some(ok) = crate::story_misc::call(sim, ctx, name, args) {
            return Some(ok);
        }
        if let Some(ok) = crate::presentation::call(sim, ctx, name, args) {
            return Some(ok);
        }
        if let Some(ok) = crate::party::call(sim, ctx, name, args) {
            return Some(ok);
        }
        if let Some(ok) = crate::surface::call(sim, ctx, name, args) {
            return Some(ok);
        }
        if is_dialog_function(name) && sim.dialog_bank().is_none() {
            return None;
        }
        let g = |i: usize| guid(ctx, &args[i]);
        Some(match (name, args.len()) {
            // bg3: esv::OsirisDialogFunctions::SetFlag / ClearFlag (4.1.1.7631656)
            ("SetFlag" | "ClearFlag", 4) => sim
                .set_flag(
                    g(0),
                    g(1),
                    name == "SetFlag",
                    int(&args[2]),
                    int(&args[3]) != 0,
                )
                .is_ok(),
            // bg3: esv::OsirisGameFunctions::SetTag / ClearTag (4.1.1.7631656)
            ("SetTag", 2) => sim.set_tag(g(0), g(1)),
            ("ClearTag", 2) => sim.clear_tag(g(0), g(1)),
            // bg3: esv::OsirisGameFunctions::TimerLaunch / TimerCancel (4.1.1.7631656)
            ("TimerLaunch", 2) => sim.timer_launch(&text(ctx, &args[0]), int(&args[1])),
            ("TimerCancel", 1) => sim.timer_cancel(&text(ctx, &args[0])),
            // bg3: esv::OsirisGameFunctions::ObjectTimerLaunch (4.1.1.7631656)
            ("ObjectTimerLaunch", 4) => sim.object_timer_launch(
                g(0),
                &text(ctx, &args[1]),
                int(&args[2]),
                int(&args[3]) != 0,
                false,
            ),
            // bg3: esv::OsirisGameFunctions::RealtimeObjectTimerLaunch (4.1.1.7631656)
            ("RealtimeObjectTimerLaunch", 3) => {
                sim.object_timer_launch(g(0), &text(ctx, &args[1]), int(&args[2]), false, true)
            }
            // bg3: esv::OsirisGameFunctions::ObjectTimerCancel / RealtimeObjectTimerCancel
            ("ObjectTimerCancel" | "RealtimeObjectTimerCancel", 2) => {
                sim.object_timer_cancel(g(0), &text(ctx, &args[1]))
            }
            // bg3: esv::OsirisGameFunctions::ApplyStatus (4.1.1.7631656)
            ("ApplyStatus", 5) => {
                sim.apply_status(g(0), &text(ctx, &args[1]), real(&args[2]), g(4))
            }
            // bg3: esv::OsirisGameFunctions::RemoveStatus (4.1.1.7631656)
            ("RemoveStatus", 3) => sim.remove_status(g(0), &text(ctx, &args[1])),
            // bg3: esv::OsirisCharacterFunctions::TeleportPartiesToLevelWithMovie (4.1.1.7631656)
            ("TeleportPartiesToLevelWithMovie", 3) => sim.request_level(&text(ctx, &args[0])),
            // bg3: esv::OsirisGameFunctions::SetFaction (4.1.1.7631656)
            ("SetFaction", 2) => sim.set_faction(g(0), g(1)),
            // bg3: esv::OsirisDialogFunctions::DialogRequestStop (4.1.1.7631656)
            ("DialogRequestStop", 1) => sim.dialog_request_stop(g(0)),
            // bg3: esv::OsirisLSDialogsFunctions::DialogAddActorAtReservedSlot (4.1.1.7631656)
            ("DialogAddActorAtReservedSlot", 5) => sim.dialog_add_actor(int(&args[0]) as u32, g(1)),
            // bg3: esv::OsirisGameFunctions::EnterCombat (4.1.1.7631656); see crate::combat
            ("EnterCombat", 2) => sim.enter_combat(g(0), g(1)).is_some(),
            // bg3: esv::OsirisGameFunctions::EndTurn (4.1.1.7631656)
            ("EndTurn", 1) => sim.combat_end_turn(g(0)).is_ok(),
            // bg3: esv::OsirisGameFunctions::SetOnStage (4.1.1.7631656). Only the off-stage
            // set is kept (used by combat); the engine's WentOnStage/visibility side effects
            // are not modelled.
            ("SetOnStage", 2) => {
                let o = g(0);
                if int(&args[1]) == 0 {
                    sim.combat.off_stage.insert(o);
                } else {
                    sim.combat.off_stage.remove(&o);
                }
                sim.exists(o)
            }
            _ => return None,
        })
    }

    /// Run a query; `None` if the sim does not implement it or lacks the data to answer (an
    /// object it has not seen, a status without stats entry, …), else whether it succeeded
    /// (with the out-parameters filled in).
    pub fn query(
        &mut self,
        ctx: &mut HostCtx<'_>,
        name: &str,
        args: &mut [Value],
        _out: &[bool],
    ) -> Option<bool> {
        if let Some(ok) = crate::story_misc::query(self.sim, ctx, name, args) {
            return Some(ok);
        }
        if let Some(ok) = crate::presentation::query(self.sim, ctx, name, args) {
            return Some(ok);
        }
        if let Some(ok) = crate::surface::query(self.sim, ctx, name, args) {
            return Some(ok);
        }
        if is_dialog_function(name) && self.sim.dialog_bank().is_none() {
            return None;
        }
        // StartDialog_Internal(dialog, ?, speaker × 6, [out] ok, [out] instance, ?)
        // bg3: esv::OsirisLSDialogsFunctions::StartDialog_Internal (4.1.1.7631656)
        if (name, args.len()) == ("StartDialog_Internal", 11) {
            let dialog = guid(ctx, &args[0]);
            let speakers: Vec<Guid> = (2..8).map(|i| guid(ctx, &args[i])).collect();
            let id = self.sim.request_dialog(dialog, &speakers)?;
            args[8] = Value::int(1);
            args[9] = Value::int(id as i32);
            return Some(true);
        }
        let sim = &*self.sim;
        let n = args.len();
        let g = |ctx: &HostCtx<'_>, i: usize| guid(ctx, &args[i]);
        let result: Option<Value> = match (name, n) {
            // bg3: esv::OsirisDialogFunctions::GetFlag (4.1.1.7631656)
            ("GetFlag", 3) => sim.get_flag(g(ctx, 0), g(ctx, 1)).ok().map(bool_out),
            // bg3: esv::OsirisDialogFunctions::GetFlagName (4.1.1.7631656)
            ("GetFlagName", 2) => {
                let name = sim.data.flags.get(g(ctx, 0))?.name.clone();
                Some(ctx.string(&name))
            }
            // bg3: esv::OsirisGameFunctions::IsTagged (4.1.1.7631656)
            ("IsTagged", 3) => {
                // Objects created outside the sim (character creation, loot) may carry tags
                // the sim cannot know (race, deity, … chosen by the player). In a live game
                // the sim made them itself (`start::new_game`), so it knows their tags.
                let o = g(ctx, 0);
                if !sim.stage.live && sim.entity(o).is_some_and(|e| e.origin == Origin::Observed) {
                    return None;
                }
                Some(bool_out(sim.is_tagged(o, g(ctx, 1))))
            }
            // bg3: esv::OsirisGameFunctions::Exists (4.1.1.7631656)
            ("Exists", 2) => Some(bool_out(sim.exists(g(ctx, 0)))),
            // bg3: esv::OsirisGameFunctions::IsCharacter / IsItem (4.1.1.7631656)
            ("IsCharacter", 2) => {
                Some(bool_out(kind_query(sim, g(ctx, 0), EntityKind::Character)?))
            }
            ("IsItem", 2) => Some(bool_out(kind_query(sim, g(ctx, 0), EntityKind::Item)?)),
            // bg3: esv::OsirisGameFunctions::GetTemplate (4.1.1.7631656)
            ("GetTemplate", 2) => {
                let t = sim
                    .entity(g(ctx, 0))
                    .and_then(|e| e.template)
                    .and_then(|t| sim.data.templates.get(&t))
                    .map(|t| crate::guid::osiris_name(&t.name, t.id))?;
                Some(ctx.guid(&t))
            }
            // bg3: esv::OsirisGameFunctions::GetRegion (4.1.1.7631656)
            ("GetRegion", 2) => {
                let l = sim.region(g(ctx, 0))?.to_owned();
                Some(ctx.string(&l))
            }
            // bg3: esv::OsirisGameFunctions::HasActiveStatus / HasAppliedStatus
            // (4.1.1.7631656). The trace shows `HasAppliedStatus` true right after
            // `ApplyStatus`, before the engine raises `StatusAttempt`/`StatusApplied`.
            ("HasActiveStatus" | "HasAppliedStatus", 3) => {
                let s = text(ctx, &args[1]);
                Some(bool_out(sim.has_active_status(g(ctx, 0), &s)))
            }
            // bg3: esv::OsirisGameFunctions::HasActiveStatusWithGroup /
            // HasAppliedStatusWithGroup (4.1.1.7631656)
            ("HasActiveStatusWithGroup" | "HasAppliedStatusWithGroup", 3) => {
                let grp = text(ctx, &args[1]);
                Some(bool_out(sim.has_status_in_group(g(ctx, 0), &grp)?))
            }
            // bg3: esv::OsirisGameFunctions::IsSummon (4.1.1.7631656).
            // Summons are created by spells, which the sim does not run: no object it loaded
            // is one.
            ("IsSummon", 2) => {
                let e = sim.entity(g(ctx, 0))?;
                if e.origin != Origin::Data {
                    return None;
                }
                Some(bool_out(false))
            }
            // bg3: esv::OsirisItemFunctions::IsWeapon (4.1.1.7631656).
            // An item whose stats entry is of type `Weapon`.
            ("IsWeapon", 2) => {
                let e = sim.entity(g(ctx, 0))?;
                let is = match e.kind {
                    EntityKind::Character | EntityKind::Trigger => false,
                    EntityKind::Item => {
                        let s = e.stats.as_deref()?;
                        sim.data.stats_type.get(s)? == "Weapon"
                    }
                    _ => return None,
                };
                Some(bool_out(is))
            }
            // bg3: esv::OsirisCharacterFunctions::IsPartyMember (4.1.1.7631656).
            // `IsPartyMember(character, include_followers)`: in the party as reported by
            // `CharacterJoinedParty` / `CharacterLeftParty`.
            ("IsPartyMember", 3) => Some(bool_out(sim.party.contains(&g(ctx, 0)))),
            // Module metadata (`Mods/<Module>/meta.lsx`).
            ("IsGameLevel", 2) => {
                let l = text(ctx, &args[0]);
                Some(bool_out(sim.data.module_levels.is_game_level(&l)))
            }
            ("IsCharacterCreationLevel", 2) => {
                let l = text(ctx, &args[0]);
                Some(bool_out(l == sim.data.module_levels.character_creation))
            }
            ("GetActiveModStartupLevel", 1) => {
                let l = sim.data.module_levels.startup.clone();
                if l.is_empty() {
                    return None;
                }
                Some(ctx.string(&l))
            }
            // bg3: esv::OsirisGameFunctions::GetFaction (4.1.1.7631656)
            ("GetFaction", 2) => {
                let f = sim.faction_text(g(ctx, 0))?;
                Some(ctx.guid(&f))
            }

            // bg3: esv::OsirisGameFunctions::IsStatusFromGroup (4.1.1.7631656)
            ("IsStatusFromGroup", 3) => {
                let (s, grp) = (text(ctx, &args[0]), text(ctx, &args[1]));
                Some(bool_out(sim.is_status_from_group(&s, &grp)))
            }
            // bg3: esv::OsirisGameFunctions::GetStatusType (4.1.1.7631656)
            ("GetStatusType", 2) => {
                let s = text(ctx, &args[0]);
                // Engine-defined statuses (`DYING`, `INSURFACE`, …) have no stats entry; the
                // sim does not know their types yet.
                let t = sim.data.statuses.get(&s)?.status_type.clone();
                Some(ctx.string(&t))
            }
            // bg3: esv::OsirisGameFunctions::TimerExists / ObjectTimerExists (4.1.1.7631656)
            ("TimerExists", 2) => {
                let s = text(ctx, &args[0]);
                Some(bool_out(sim.timers.exists(&s, Guid::NULL)))
            }
            ("ObjectTimerExists", 3) => {
                let s = text(ctx, &args[1]);
                Some(bool_out(sim.timers.exists(&s, g(ctx, 0))))
            }
            // bg3: esv::OsirisMathFunctions (4.1.1.7631656)
            ("IntegerSum", 3) => Some(Value::int(int(&args[0]).wrapping_add(int(&args[1])))),
            ("IntegerSubtract", 3) => Some(Value::int(int(&args[0]).wrapping_sub(int(&args[1])))),
            ("IntegerProduct", 3) => Some(Value::int(int(&args[0]).wrapping_mul(int(&args[1])))),
            ("IntegerDivide", 3) => int(&args[0]).checked_div(int(&args[1])).map(Value::int),
            ("IntegerModulo", 3) => int(&args[0]).checked_rem(int(&args[1])).map(Value::int),
            ("IntegerMax", 3) => Some(Value::int(int(&args[0]).max(int(&args[1])))),
            ("IntegerMin", 3) => Some(Value::int(int(&args[0]).min(int(&args[1])))),
            ("RealSum", 3) => Some(Value::real(real(&args[0]) + real(&args[1]))),
            ("RealSubtract", 3) => Some(Value::real(real(&args[0]) - real(&args[1]))),
            ("RealProduct", 3) => Some(Value::real(real(&args[0]) * real(&args[1]))),
            ("RealDivide", 3) => {
                let d = real(&args[1]);
                (d != 0.0).then(|| Value::real(real(&args[0]) / d))
            }
            // bg3: esv::OsirisGeneralFunctions::Concatenate & co. (4.1.1.7631656)
            ("Concatenate", 3) => {
                let s = format!("{}{}", text(ctx, &args[0]), text(ctx, &args[1]));
                Some(ctx.string(&s))
            }
            ("ConcatenateInteger", 3) => {
                let s = format!("{}{}", text(ctx, &args[0]), int(&args[1]));
                Some(ctx.string(&s))
            }
            ("GUIDToString", 2) => {
                let s = text(ctx, &args[0]);
                Some(ctx.string(&s))
            }
            ("StringToGUID", 2) => {
                let s = text(ctx, &args[0]);
                Guid::from_osiris(&s).map(|_| ctx.guid(&s))
            }
            // bg3: esv::OsirisLSDialogsFunctions::DialogIsAutomated / DialogIsAllowingJoinCombat
            // / DialogAllowDeadSpeakers (4.1.1.7631656): the dialog resource's switches.
            ("DialogIsAutomated", 2) => Some(bool_out(sim.dialog_resource(g(ctx, 0))?.automated)),
            ("DialogIsAllowingJoinCombat", 2) => Some(bool_out(
                sim.dialog_resource(g(ctx, 0))?.is_allowing_join_combat,
            )),
            ("DialogAllowDeadSpeakers", 2) => Some(bool_out(
                sim.dialog_resource(g(ctx, 0))?.allow_dead_speakers,
            )),
            // bg3: esv::OsirisLSDialogsFunctions::DialogSpeakerHasLines (4.1.1.7631656)
            ("DialogSpeakerHasLines", 3) => Some(bool_out(
                sim.dialog_speaker_has_lines(g(ctx, 0), int(&args[1]))?,
            )),
            // bg3: esv::OsirisLSDialogsFunctions::IsSpeakerReserved (4.1.1.7631656)
            ("IsSpeakerReserved", 2) => Some(bool_out(sim.is_speaker_reserved(g(ctx, 0)))),
            // bg3: esv::OsirisGameFunctions::IsInCombat (4.1.1.7631656)
            ("IsInCombat", 2) => Some(bool_out(sim.in_combat(g(ctx, 0)))),
            // bg3: esv::OsirisDialogFunctions::DialogGetInvolvedPlayer / DialogGetInvolvedNPC
            // (4.1.1.7631656): the n-th (from 1) player / NPC as a bare UUID; out of range fails.
            ("DialogGetInvolvedPlayer" | "DialogGetInvolvedNPC", 3) => {
                let list =
                    sim.dialog_involved(int(&args[0]) as u32, name == "DialogGetInvolvedPlayer")?;
                let i = usize::try_from(int(&args[1]) - 1).ok();
                i.and_then(|i| list.get(i))
                    .map(|a| ctx.guid(&a.to_string()))
            }
            // bg3: esv::OsirisDialogFunctions::DialogGetNumberOfInvolvedPlayers / …NPCs
            ("DialogGetNumberOfInvolvedPlayers" | "DialogGetNumberOfInvolvedNPCs", 2) => {
                let players = name == "DialogGetNumberOfInvolvedPlayers";
                let list = sim.dialog_involved(int(&args[0]) as u32, players)?;
                Some(Value::int(list.len() as i32))
            }
            _ => return None,
        };
        Some(match result {
            Some(v) => {
                // The last parameter is the out-parameter of every function above.
                args[n - 1] = v;
                true
            }
            None => false,
        })
    }
}

/// `IsCharacter` / `IsItem`: 0 for objects of another kind; unknown objects fail.
fn kind_query(sim: &Sim, id: Guid, kind: EntityKind) -> Option<bool> {
    let e = sim.entity(id)?;
    if e.kind == EntityKind::Unknown {
        return None;
    }
    Some(e.kind == kind)
}

/// A plain [`Host`] over the sim: unimplemented calls succeed without effect, unimplemented
/// queries fail.
pub struct SimHost<'a> {
    pub functions: SimFunctions<'a>,
    /// `(name, arity)` of unimplemented functions the story used, with counts.
    pub missing: Vec<(String, usize, u64)>,
}

impl<'a> SimHost<'a> {
    pub fn new(sim: &'a mut Sim) -> Self {
        Self {
            functions: SimFunctions::new(sim),
            missing: Vec::new(),
        }
    }

    fn note_missing(&mut self, name: &str, arity: usize) {
        match self
            .missing
            .iter_mut()
            .find(|(n, a, _)| n == name && *a == arity)
        {
            Some(m) => m.2 += 1,
            None => self.missing.push((name.to_owned(), arity, 1)),
        }
    }
}

impl Host for SimHost<'_> {
    fn call(&mut self, ctx: &mut HostCtx<'_>, name: &str, args: &[Value]) -> bool {
        match self.functions.call(ctx, name, args) {
            Some(ok) => ok,
            None => {
                self.note_missing(name, args.len());
                true
            }
        }
    }

    fn query(
        &mut self,
        ctx: &mut HostCtx<'_>,
        name: &str,
        args: &mut [Value],
        out: &[bool],
    ) -> bool {
        match self.functions.query(ctx, name, args, out) {
            Some(ok) => ok,
            None => {
                self.note_missing(name, args.len());
                false
            }
        }
    }
}
