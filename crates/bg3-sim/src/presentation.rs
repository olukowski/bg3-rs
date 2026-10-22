//! What a live front end needs beyond the story's flags and dialogs: where things are and
//! whether they are on stage, trigger volumes, scripted movement, and the player's own input
//! (moving, clicking someone to talk).
//!
//! The functions here run only in a **live** game ([`Stage::live`], set by
//! [`crate::start::new_game`]): the trace harness keeps answering them from the log, so its
//! numbers don't move. Each cites the engine function it stands for.
//!
//! * Osiris calls: `SetOnStage`, `SetVisible`, `TeleportTo`, `TeleportToPosition`,
//!   `CharacterMoveTo`, `TriggerRegisterForCharacter`, `TriggerUnregisterForCharacter`;
//!   queries: `GetPosition`.
//! * Engine events: `WentOnStage`, `Teleported` / `ItemTeleported`, `EntityEvent` (the
//!   event name a teleport or move was given), `EnteredTrigger` / `LeftTrigger`,
//!   `DialogStartRequested`.
//! * Trigger volumes ([`TriggerDef`]) from the level data, checked whenever a character moves
//!   ([`Sim::update_stage`]).
//!
//! Inferred (marked *inferred* where used) from the trace `full-nautiloid.log` and the data,
//! not read in the binary: the event formats and orders below and which triggers report to
//! whom. The trigger shapes and hit test are the engine's (`bg3_world::trigger`).

use std::collections::{HashMap, HashSet};

use bg3_lsf::Node;
use bg3_osiris::{HostCtx, Value};
use bg3_vfs::Vfs;
use bg3_vfs::modules::BASE_LOAD_ORDER;

use crate::driver::Game;
use crate::entity::EntityKind;
use crate::events::SimValue;
use crate::guid::Guid;
use crate::sim::{NULL_OBJECT, Sim};

/// Movement speeds for scripted moves (`bg3_world::characters` defaults, m/s). Template
/// speeds are not loaded by the sim yet.
const RUN_SPEED: f32 = bg3_world::characters::DEFAULT_MOVEMENT_SPEED_RUN;
const WALK_SPEED: f32 = bg3_world::characters::DEFAULT_MOVEMENT_SPEED_WALK;

/// The volume of a trigger: the shape shared with the lighting/atmosphere triggers, so both
/// use the engine's one hit test (`bg3_world::trigger`).
pub use bg3_world::trigger::TriggerShape;

/// A placed trigger.
#[derive(Clone, Debug)]
pub struct TriggerDef {
    pub id: Guid,
    pub name: String,
    /// `TriggerType` of the object or its root template (`RoomTrigger`, `TriggerArea`,
    /// `TriggerStart`, `EocTriggerArea`, …).
    pub trigger_type: String,
    pub level: String,
    pub position: [f32; 3],
    /// Quaternion `x y z w`.
    pub rotation: [f32; 4],
    pub shape: TriggerShape,
}

impl TriggerDef {
    /// Is `p` (world space) inside the volume?
    // bg3: ls::trigger::HitTest (4.1.1.7631656)
    pub fn contains(&self, p: [f32; 3]) -> bool {
        self.shape.contains(self.position, self.rotation, p)
    }

    /// Does the trigger report every character (not only registered ones)? Room triggers do:
    /// the trace has `EnteredTrigger` for every character standing in a `RoomTrigger_*` when
    /// the level starts and no registration for them (*inferred*); area triggers report the
    /// characters registered with `TriggerRegisterForCharacter`.
    // bg3: esv::trigger::TrackingSystem (CheckTriggers, esv::trigger::RegisteredForComponent) (4.1.1.7631656)
    pub fn reports_everyone(&self) -> bool {
        self.trigger_type == "RoomTrigger"
    }
}

/// Trigger-related fields of a root template (and its parent for inheritance).
#[derive(Clone, Debug, Default)]
struct TriggerTemplate {
    parent: Option<Guid>,
    trigger_type: Option<String>,
    physics_type: Option<i64>,
    radius: Option<f32>,
    extents: Option<[f32; 3]>,
    height: Option<f32>,
}

fn s(node: &Node, name: &str) -> Option<String> {
    node.attr(name)
        .map(ToString::to_string)
        .filter(|s| !s.is_empty())
}

fn vec_attr(node: &Node, name: &str) -> Option<Vec<f32>> {
    Some(
        node.attr(name)?
            .components()?
            .into_iter()
            .map(|x| x as f32)
            .collect(),
    )
}

fn f(node: &Node, name: &str) -> Option<f32> {
    node.attr(name).and_then(|v| v.as_f64()).map(|v| v as f32)
}

fn trigger_template(n: &Node) -> TriggerTemplate {
    TriggerTemplate {
        parent: s(n, "ParentTemplateId").and_then(|p| Guid::parse(&p)),
        trigger_type: s(n, "TriggerType"),
        physics_type: n.attr("PhysicsType").and_then(|v| v.as_i64()),
        radius: f(n, "Radius"),
        extents: vec_attr(n, "Extents")
            .filter(|v| v.len() == 3)
            .map(|v| [v[0], v[1], v[2]]),
        height: f(n, "Height"),
    }
}

fn parse(vfs: &Vfs, path: &str) -> Option<bg3_lsf::Resource> {
    bg3_lsf::parse(&vfs.read(path).ok()?).ok()
}

fn game_objects(res: &bg3_lsf::Resource) -> impl Iterator<Item = &Node> {
    res.region("Templates")
        .into_iter()
        .flat_map(|n| n.children_named("GameObjects"))
}

/// The triggers of a level: its own (`Mods/<M>/Levels/<L>/Triggers`) and its globals
/// (`Mods/<M>/Globals/<L>/Triggers`), a later module replacing an object by id. Shape and type
/// come from the object, else from its root template chain.
pub fn load_triggers(vfs: &Vfs, level: &str) -> Vec<TriggerDef> {
    let mut templates: HashMap<Guid, TriggerTemplate> = HashMap::new();
    for m in BASE_LOAD_ORDER {
        let Some(res) = parse(vfs, &format!("Public/{m}/RootTemplates/_merged.lsf")) else {
            continue;
        };
        for n in game_objects(&res) {
            if s(n, "Type").as_deref() != Some("trigger") {
                continue;
            }
            if let Some(id) = s(n, "MapKey").and_then(|k| Guid::parse(&k)) {
                templates.insert(id, trigger_template(n));
            }
        }
    }
    let mut out: Vec<TriggerDef> = Vec::new();
    let mut index: HashMap<Guid, usize> = HashMap::new();
    for m in BASE_LOAD_ORDER {
        for base in ["Levels", "Globals"] {
            let path = format!("Mods/{m}/{base}/{level}/Triggers/_merged.lsf");
            let Some(res) = parse(vfs, &path) else {
                continue;
            };
            for n in game_objects(&res) {
                let Some(t) = placed_trigger(n, level, &templates) else {
                    continue;
                };
                match index.get(&t.id) {
                    Some(&i) => out[i] = t,
                    None => {
                        index.insert(t.id, out.len());
                        out.push(t);
                    }
                }
            }
        }
    }
    out
}

fn placed_trigger(
    n: &Node,
    level: &str,
    templates: &HashMap<Guid, TriggerTemplate>,
) -> Option<TriggerDef> {
    let id = Guid::parse(&s(n, "MapKey")?)?;
    let own = trigger_template(n);
    // The object's fields, then its template chain's.
    let mut chain = vec![own.clone()];
    let mut next = s(n, "TemplateName").and_then(|t| Guid::parse(&t));
    for _ in 0..16 {
        let Some(t) = next.and_then(|g| templates.get(&g)) else {
            break;
        };
        chain.push(t.clone());
        next = t.parent;
    }
    let pick = |g: fn(&TriggerTemplate) -> Option<f32>| chain.iter().find_map(g);
    let trigger_type = chain
        .iter()
        .find_map(|t| t.trigger_type.clone())
        .unwrap_or_default();
    let physics = chain.iter().find_map(|t| t.physics_type).unwrap_or(0);
    let shape = TriggerShape::from_physics(
        physics,
        pick(|t| t.radius),
        chain.iter().find_map(|t| t.extents),
        pick(|t| t.height),
        n.children_named("Points")
            .filter_map(|p| vec_attr(p, "Object"))
            .filter(|v| v.len() == 2)
            .map(|v| [v[0], v[1]])
            .collect(),
    );
    let transform = n.child("Transform");
    let position = transform
        .and_then(|t| vec_attr(t, "Position"))
        .filter(|v| v.len() == 3)
        .map_or([0.0; 3], |v| [v[0], v[1], v[2]]);
    let rotation = transform
        .and_then(|t| vec_attr(t, "RotationQuat"))
        .filter(|v| v.len() == 4)
        .map_or([0.0, 0.0, 0.0, 1.0], |v| [v[0], v[1], v[2], v[3]]);
    Some(TriggerDef {
        id,
        name: s(n, "Name").unwrap_or_default(),
        trigger_type,
        level: s(n, "LevelName").unwrap_or_else(|| level.to_owned()),
        position,
        rotation,
        shape,
    })
}

/// A scripted move in progress (`CharacterMoveTo`).
#[derive(Clone, Debug)]
pub struct Move {
    pub who: Guid,
    pub to: [f32; 3],
    /// m/s.
    pub speed: f32,
    /// Raised as `EntityEvent(who, event)` on arrival, if not empty.
    pub event: String,
}

/// Presentation state of the world.
#[derive(Clone, Debug, Default)]
pub struct Stage {
    /// The functions of this module answer the story (a live game, not a trace replay).
    pub live: bool,
    off_stage: HashSet<Guid>,
    invisible: HashSet<Guid>,
    /// Triggers of the loaded level ([`load_triggers`]).
    pub triggers: Vec<TriggerDef>,
    /// `(trigger, character)` registrations.
    registered: HashSet<(Guid, Guid)>,
    /// `(trigger, character)` pairs currently inside.
    inside: HashSet<(Guid, Guid)>,
    /// Position of each character at its last trigger check.
    checked: HashMap<Guid, [f32; 3]>,
    pub moves: Vec<Move>,
    /// Osiris functions (`Name/arity`) the live story used that nothing implements, with
    /// counts: the to-do list of a live game.
    pub missing: std::collections::BTreeMap<String, u64>,
}

/// What the presentation layer shows of an entity.
#[derive(Clone, Debug, PartialEq)]
pub struct Presence {
    pub id: Guid,
    pub name: String,
    pub kind: EntityKind,
    pub template: Option<Guid>,
    pub level: Option<String>,
    pub position: [f32; 3],
    pub on_stage: bool,
    pub visible: bool,
    pub dead: bool,
    /// Moving under script control (`CharacterMoveTo`).
    pub moving: bool,
}

impl Sim {
    /// `SetOnStage` state (objects are on stage unless the story took them off).
    pub fn on_stage(&self, id: Guid) -> bool {
        !self.stage.off_stage.contains(&id)
    }

    pub fn visible(&self, id: Guid) -> bool {
        !self.stage.invisible.contains(&id)
    }

    /// Characters and items of the loaded level and global ones placed in it, plus the party,
    /// as the presentation layer shows them.
    pub fn presences(&self) -> Vec<Presence> {
        let level = self.level.as_deref();
        self.entities
            .iter()
            .filter(|e| matches!(e.kind, EntityKind::Character | EntityKind::Item))
            .filter(|e| e.holder.is_none())
            .filter(|e| e.level.as_deref() == level || self.party.contains(&e.id))
            .map(|e| Presence {
                id: e.id,
                name: e.name.clone(),
                kind: e.kind,
                template: e.template,
                level: e.level.clone(),
                position: e.position,
                on_stage: self.on_stage(e.id),
                visible: self.visible(e.id),
                dead: self
                    .entity_id(e.id)
                    .is_some_and(|h| bg3_rules::RulesContext::is_dead(self, h)),
                moving: self.stage.moves.iter().any(|m| m.who == e.id),
            })
            .collect()
    }

    /// The player moved a character (click-to-move): the new position counts for triggers at
    /// the next [`Sim::update_stage`]. No event: the engine reports only what the position
    /// changes cause (triggers, perception).
    pub fn set_position(&mut self, id: Guid, position: [f32; 3]) -> bool {
        match self.entities.get_mut(id) {
            Some(e) => {
                e.position = position;
                true
            }
            None => false,
        }
    }

    /// The player clicked `target` to talk with `player`: the server raises
    /// `DialogStartRequested(target, player)` and the story picks and starts the dialog
    /// (`QRY_SelectAndStartDialog` over `DB_Dialogs`, then `StartDialog_Internal`).
    // bg3: esv::CharacterProtocol::ProcessMsg → esv::StoryDialogEventManager::ThrowDialogRequestEvent (4.1.1.7631656)
    pub fn request_dialog_with(&mut self, target: Guid, player: Guid) -> bool {
        if !self.exists(target) || !self.exists(player) {
            return false;
        }
        let args = vec![
            SimValue::Guid(self.render(target)),
            SimValue::Guid(self.render(player)),
        ];
        self.push_event("DialogStartRequested", args);
        true
    }

    /// Put an object somewhere (teleport) in `level` (`None`: the loaded level). Into another
    /// level: `LeftLevel(object, old)`, `EnteredLevel(object, template, new)`, then
    /// `Teleported(character, NULL, old, new, "RegionSwap")`; within a level `Teleported(…,
    /// "Invalid")`, or `ItemTeleported(item, old, new)` for items; then `EntityEvent(object,
    /// event)` if an event name was given (trace: Shadowheart's `TeleportTo(…,
    /// "TUT_TransformChamber_ShadowheartTeleport", …)` from `WLD_Main_A`; *inferred* order).
    // bg3: esv::OsirisGameFunctions::TeleportTo / TeleportToPosition (4.1.1.7631656)
    pub fn teleport(&mut self, id: Guid, to: [f32; 3], level: Option<String>, event: &str) -> bool {
        let level = level.or_else(|| self.level.clone());
        let Some(e) = self.entities.get_mut(id) else {
            return false;
        };
        let from = e.position;
        let old_level = e.level.clone();
        e.position = to;
        let swap = level.is_some() && old_level != level;
        if level.is_some() {
            e.level = level.clone();
        }
        let (kind, template) = (e.kind, e.template);
        let who = SimValue::Guid(self.render(id));
        if swap && kind == EntityKind::Character {
            if let Some(old) = old_level {
                self.push_event("LeftLevel", vec![who.clone(), SimValue::Str(old)]);
            }
            let template = template.map_or(NULL_OBJECT.to_owned(), |t| t.to_string());
            let new = level.clone().unwrap_or_default();
            self.push_event(
                "EnteredLevel",
                vec![who.clone(), SimValue::Guid(template), SimValue::Str(new)],
            );
        }
        let reals = from.iter().chain(&to).map(|v| SimValue::Real(*v));
        match kind {
            EntityKind::Item => {
                let args = std::iter::once(who.clone()).chain(reals).collect();
                self.push_event("ItemTeleported", args);
            }
            _ => {
                let reason = if swap { "RegionSwap" } else { "Invalid" };
                let args = [who.clone(), SimValue::Guid(NULL_OBJECT.to_owned())]
                    .into_iter()
                    .chain(reals)
                    .chain(std::iter::once(SimValue::Str(reason.to_owned())))
                    .collect();
                self.push_event("Teleported", args);
            }
        }
        self.stage.moves.retain(|m| m.who != id);
        self.stage.checked.remove(&id);
        if !event.is_empty() {
            self.entity_event(who, event);
        }
        true
    }

    // bg3: esv::StoryGameEventManager::ThrowOsirisEntityEvent (4.1.1.7631656)
    fn entity_event(&mut self, who: SimValue, event: &str) {
        self.push_event("EntityEvent", vec![who, SimValue::Str(event.to_owned())]);
    }

    /// `SetOnStage(object, on)`: `WentOnStage(<uuid>, on)` when the state changes, the object
    /// as a bare UUID (trace).
    // bg3: esv::OsirisGameFunctions::SetOnStage (4.1.1.7631656)
    pub fn set_on_stage(&mut self, id: Guid, on: bool) -> bool {
        if !self.exists(id) {
            return false;
        }
        let changed = if on {
            self.stage.off_stage.remove(&id)
        } else {
            self.stage.off_stage.insert(id)
        };
        if changed {
            self.push_event(
                "WentOnStage",
                vec![SimValue::Guid(id.to_string()), SimValue::Int(i32::from(on))],
            );
        }
        true
    }

    /// Load the trigger volumes of the loaded level.
    pub fn set_triggers(&mut self, triggers: Vec<TriggerDef>) {
        self.stage.triggers = triggers;
        self.stage.checked.clear();
    }

    /// One frame of presentation state: scripted moves advance, then characters that moved
    /// (or were registered) are checked against the trigger volumes, raising `EnteredTrigger`
    /// / `LeftTrigger(character, trigger)` on each change.
    pub fn update_stage(&mut self, dt: f64) {
        self.advance_moves(dt as f32);
        self.check_triggers();
    }

    fn advance_moves(&mut self, dt: f32) {
        let mut arrived = Vec::new();
        let mut moves = std::mem::take(&mut self.stage.moves);
        moves.retain(|m| {
            let Some(e) = self.entities.get_mut(m.who) else {
                return false;
            };
            let d = [
                m.to[0] - e.position[0],
                m.to[1] - e.position[1],
                m.to[2] - e.position[2],
            ];
            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            let step = m.speed * dt;
            if len <= step.max(1e-3) {
                e.position = m.to;
                arrived.push((m.who, m.event.clone()));
                false
            } else {
                for (p, d) in e.position.iter_mut().zip(d) {
                    *p += d / len * step;
                }
                true
            }
        });
        self.stage.moves = moves;
        for (who, event) in arrived {
            if !event.is_empty() {
                let who = SimValue::Guid(self.render(who));
                self.entity_event(who, &event);
            }
        }
    }

    // bg3: esv::trigger::TrackingSystem (CheckTriggers, UpdateTracking) (4.1.1.7631656)
    fn check_triggers(&mut self) {
        if self.stage.triggers.is_empty() {
            return;
        }
        let level = self.level.clone();
        let movers: Vec<(Guid, [f32; 3])> = self
            .entities
            .iter()
            .filter(|e| e.kind == EntityKind::Character && e.holder.is_none())
            .filter(|e| e.level == level || self.party.contains(&e.id))
            .filter(|e| self.stage.checked.get(&e.id) != Some(&e.position))
            .map(|e| (e.id, e.position))
            .collect();
        let mut events = Vec::new();
        for (who, p) in movers {
            self.stage.checked.insert(who, p);
            let on_stage = self.on_stage(who);
            for t in &self.stage.triggers {
                if !(t.reports_everyone() || self.stage.registered.contains(&(t.id, who))) {
                    continue;
                }
                let key = (t.id, who);
                let now = on_stage && t.contains(p);
                let was = self.stage.inside.contains(&key);
                if now != was {
                    events.push((now, who, t.id));
                }
            }
        }
        for (entered, who, trigger) in events {
            if entered {
                self.stage.inside.insert((trigger, who));
            } else {
                self.stage.inside.remove(&(trigger, who));
            }
            let name = if entered {
                "EnteredTrigger"
            } else {
                "LeftTrigger"
            };
            let args = vec![
                SimValue::Guid(self.render(who)),
                SimValue::Guid(self.render(trigger)),
            ];
            self.push_event(name, args);
        }
    }
}

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
        bg3_osiris::Data::Int(i) => i,
        bg3_osiris::Data::Int64(i) => i as i32,
        bg3_osiris::Data::Real(r) => r as i32,
        _ => 0,
    }
}

fn real(v: &Value) -> f32 {
    match v.data {
        bg3_osiris::Data::Int(i) => i as f32,
        bg3_osiris::Data::Int64(i) => i as f32,
        bg3_osiris::Data::Real(r) => r,
        _ => 0.0,
    }
}

/// The calls of this module (live games only); `None` for anything else.
pub(crate) fn call(
    sim: &mut Sim,
    ctx: &mut HostCtx<'_>,
    name: &str,
    args: &[Value],
) -> Option<bool> {
    if !sim.stage.live {
        return None;
    }
    let g = |i: usize| guid(ctx, &args[i]);
    Some(match (name, args.len()) {
        ("SetOnStage", 2) => sim.set_on_stage(g(0), int(&args[1]) != 0),
        // bg3: esv::OsirisGameFunctions::SetVisible (4.1.1.7631656)
        ("SetVisible", 2) => {
            let id = g(0);
            if int(&args[1]) != 0 {
                sim.stage.invisible.remove(&id);
            } else {
                sim.stage.invisible.insert(id);
            }
            sim.exists(id)
        }
        // TeleportTo(source, target, event, …): to the target object's position.
        ("TeleportTo", 8) => {
            let target = g(1);
            let Some(to) = sim.entity(target).map(|e| e.position) else {
                return Some(false);
            };
            let level = sim.region(target).map(str::to_owned);
            sim.teleport(g(0), to, level, &text(ctx, &args[2]))
        }
        // TeleportToPosition(object, x, y, z, event, …)
        ("TeleportToPosition", 10) => sim.teleport(
            g(0),
            [real(&args[1]), real(&args[2]), real(&args[3])],
            None,
            &text(ctx, &args[4]),
        ),
        // CharacterMoveTo(character, target, "Run"/"Walk"/…, event, ?): a straight-line move
        // at the default speed (no path finding in the sim yet), `EntityEvent` on arrival
        // (trace: `CharacterMoveTo(…, "TUT_SortingChamber_UsReadyToLeave", 1)` →
        // `EntityEvent(…, "TUT_SortingChamber_UsReadyToLeave")`).
        // bg3: esv::OsirisCharacterFunctions::CharacterMoveTo (4.1.1.7631656)
        ("CharacterMoveTo", 5) => {
            let who = g(0);
            let Some(to) = sim.entity(g(1)).map(|e| e.position) else {
                return Some(false);
            };
            if !sim.exists(who) {
                return Some(false);
            }
            let speed = if text(ctx, &args[2]).eq_ignore_ascii_case("walk") {
                WALK_SPEED
            } else {
                RUN_SPEED
            };
            sim.stage.moves.retain(|m| m.who != who);
            sim.stage.moves.push(Move {
                who,
                to,
                speed,
                event: text(ctx, &args[3]),
            });
            true
        }
        // bg3: esv::OsirisTriggerFunctions::TriggerRegisterForCharacter (4.1.1.7631656)
        ("TriggerRegisterForCharacter", 2) => {
            let (t, c) = (g(0), g(1));
            sim.stage.registered.insert((t, c));
            sim.stage.checked.remove(&c);
            sim.exists(t) && sim.exists(c)
        }
        // bg3: esv::OsirisTriggerFunctions::TriggerUnregisterForCharacter (4.1.1.7631656)
        ("TriggerUnregisterForCharacter", 2) => {
            let (t, c) = (g(0), g(1));
            sim.stage.registered.remove(&(t, c));
            sim.stage.inside.remove(&(t, c));
            true
        }
        _ => {
            note_missing(sim, name, args.len());
            return None;
        }
    })
}

/// The queries of this module (live games only); `None` for anything else.
pub(crate) fn query(
    sim: &mut Sim,
    ctx: &mut HostCtx<'_>,
    name: &str,
    args: &mut [Value],
) -> Option<bool> {
    if !sim.stage.live {
        return None;
    }
    match (name, args.len()) {
        // GetPosition(object, [out] x, [out] y, [out] z)
        // bg3: esv::OsirisGameFunctions::GetPosition (4.1.1.7631656)
        ("GetPosition", 4) => {
            let p = sim.entity(guid(ctx, &args[0]))?.position;
            for (i, v) in p.into_iter().enumerate() {
                args[i + 1] = Value::real(v);
            }
            Some(true)
        }
        // IsInTrigger(object, trigger, [out] bool): whether the object stands in the
        // trigger's volume (checked geometrically, registered or not).
        // bg3: esv::OsirisGameFunctions::IsInTrigger (4.1.1.7631656)
        ("IsInTrigger", 3) => {
            let p = sim.entity(guid(ctx, &args[0]))?.position;
            let t = guid(ctx, &args[1]);
            let def = sim.stage.triggers.iter().find(|d| d.id == t)?;
            args[2] = Value::int(i32::from(def.contains(p)));
            Some(true)
        }
        // GetReservedUserID(character, [out] user): the user a character is reserved for
        // (`CharacterReservedUserIDChanged`); fails for characters without one.
        // bg3: esv::OsirisCharacterFunctions::GetReservedUserID (4.1.1.7631656)
        ("GetReservedUserID", 2) => {
            let user = sim.entity(guid(ctx, &args[0]))?.user?;
            args[1] = Value::int(user);
            Some(true)
        }
        // GetUserProfileID(user, [out] profile): the platform profile of a user. A live game
        // has one local user; [`LOCAL_PROFILE`] stands for its profile id.
        // bg3: esv::OsirisGameFunctions::GetUserProfileID (4.1.1.7631656)
        ("GetUserProfileID", 2) => {
            if int(&args[0]) != crate::start::USER_ID {
                return Some(false);
            }
            args[1] = ctx.string(LOCAL_PROFILE);
            Some(true)
        }
        _ => {
            note_missing(sim, name, args.len());
            None
        }
    }
}

/// The local player's profile id (a placeholder; the engine reports the platform's).
pub const LOCAL_PROFILE: &str = "00000000-0000-0000-0000-000000010001";

/// Count a function nothing implements (checked against the sim's own list).
fn note_missing(sim: &mut Sim, name: &str, arity: usize) {
    if crate::host::implements(name, arity) {
        return;
    }
    *sim.stage
        .missing
        .entry(format!("{name}/{arity}"))
        .or_default() += 1;
}

impl Game {
    /// One frame of a live game: presentation state ([`Sim::update_stage`]), then
    /// [`Game::tick`].
    pub fn tick_live(&mut self, dt: f64) -> usize {
        self.sim.update_stage(dt);
        self.tick(dt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trigger(shape: TriggerShape, rotation: [f32; 4]) -> TriggerDef {
        TriggerDef {
            id: Guid::NULL,
            name: String::new(),
            trigger_type: "EocTriggerArea".into(),
            level: String::new(),
            position: [10.0, 0.0, 10.0],
            rotation,
            shape,
        }
    }

    const ID: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

    #[test]
    fn shapes_contain_points() {
        let s = trigger(TriggerShape::Sphere { radius: 2.0 }, ID);
        assert!(s.contains([11.0, 1.0, 10.0]) && !s.contains([12.5, 0.0, 10.0]));
        // A box of half sizes 2×1×1 turned 90° about y spans 2 in x and 4 in z.
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let b = trigger(
            TriggerShape::Box {
                extents: [2.0, 1.0, 1.0],
            },
            [0.0, h, 0.0, h],
        );
        assert!(b.contains([10.0, 0.0, 11.9]) && !b.contains([11.9, 0.0, 10.0]));
        let p = trigger(
            TriggerShape::Polygon {
                points: vec![[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]],
                height: 3.0,
            },
            ID,
        );
        assert!(p.contains([10.5, 2.0, 9.5]));
        assert!(!p.contains([10.5, 3.5, 9.5]) && !p.contains([12.0, 1.0, 10.0]));
    }
}
