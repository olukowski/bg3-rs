//! Turn-based combat in `--play`: the fights `bg3_sim` runs, presented and played.
//!
//! The game thread (`play.rs`) owns the [`Game`]. Each tick [`CombatThread::tick`] lets the
//! engine start fights (hostiles noticing the party, [`Game::start_combat_on_sight`]), and
//! reports a [`CombatView`] of the running fight (turn order, hit points, the party's
//! resources and actions) plus the sim's [`CombatEvent`]s. The viewer sends [`CombatCmd`]s
//! back: moves (paid with the `bg3-nav` path length), casts, End Turn, and "play the next AI
//! turn", which it then animates (walk the AI's move, then combat clips) before
//! asking for the next one.
//!
//! On screen: a turn-order bar (names, hit points; the acting characters framed), an action
//! bar for the selected party member (its spells and attacks with their costs, action /
//! bonus action / movement left, its items, End Turn) and a combat log with roll breakdowns
//! ("Attack Roll: 12 (d20) + 2 (Proficiency) = 14 vs AC 7: Hit") and damage.
//!
//! Controls in a fight: click an action (or press 1–9) and then a target; click the ground
//! to move (the path stops where the movement runs out); Space or the button ends the turn;
//! Tab switches between party members acting together; Esc drops the chosen action.
//! F toggles camera follow; manual fly-camera input cancels the current turn's pan.
//!
//! Not from the game: the layout and look (portrait-less), generic combat clip selection,
//! straight-line AI moves (the sim's AI walks straight, see `bg3_sim::combat::ai`).

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bg3_loca::Loca;
use bg3_nav::Agent;
use bg3_rules::natives::RollEvent;
use bg3_sim::combat::AiAction;
use bg3_sim::{CombatEvent, Game, Guid};

use crate::characters::{CharacterAnimator, CharacterRoot};
use crate::fly_camera::CombatCamera;
use crate::nav::{Controlled, Navigation};
use crate::space;
use bg3_world::characters::CombatAnimation;

/// A left click within this many pixels of a character's chest picks it as a target.
const PICK_RADIUS_PX: f32 = 45.0;
/// Fallback when the attacker has no matching clip, seconds.
const ATTACK_PAUSE_SECS: f32 = 0.7;
/// Pause between AI turns that did nothing, seconds.
const IDLE_PAUSE_SECS: f32 = 0.35;
/// Combat log lines kept / shown.
const LOG_KEEP: usize = 300;
const LOG_SHOWN: usize = 22;
/// The panels stay this long after a fight ends.
const ENDED_SECS: f32 = 6.0;

// ---- game thread ----------------------------------------------------------------------

/// Player input for the fight.
pub enum CombatCmd {
    /// Move `who` along a path of `distance` metres ending at `to` (BG3 space).
    Move {
        who: Guid,
        distance: f32,
        to: [f32; 3],
    },
    Cast {
        who: Guid,
        spell: String,
        target: Guid,
    },
    EndTurn(Guid),
    /// Play the next AI turn of the current group (party members too with `party`).
    AiStep {
        party: bool,
    },
    /// Use an inventory item ([`use_item`]).
    UseItem {
        user: Guid,
        item: Guid,
    },
}

/// One AI turn, for the animation.
#[derive(Clone, Debug)]
pub struct AiTurn {
    pub who: Guid,
    pub from: [f32; 3],
    pub to: [f32; 3],
    pub path: Vec<[f32; 3]>,
    pub action: AiAction,
}

/// What the game thread reports about combat after a tick.
#[derive(Default)]
pub struct CombatOut {
    /// The fight as it is now (only when it changed).
    pub view: Option<CombatView>,
    pub events: Vec<CombatEvent>,
    pub ai: Vec<AiTurn>,
    /// Answers to [`CombatCmd::AiStep`] (with or without a turn played).
    pub ai_replies: u32,
    /// Refused actions and other notes for the log.
    pub notes: Vec<String>,
    pub animation_kinds: HashMap<String, CombatAnimation>,
}

impl CombatOut {
    pub fn is_empty(&self) -> bool {
        self.view.is_none()
            && self.events.is_empty()
            && self.ai.is_empty()
            && self.ai_replies == 0
            && self.notes.is_empty()
    }
}

/// The fight as the UI shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct CombatView {
    pub id: Guid,
    pub round: i32,
    pub finished: bool,
    /// Turn order: groups of characters that act together.
    pub groups: Vec<Vec<Fighter>>,
}

impl CombatView {
    pub fn fighters(&self) -> impl Iterator<Item = &Fighter> {
        self.groups.iter().flatten()
    }

    pub fn fighter(&self, id: Guid) -> Option<&Fighter> {
        self.fighters().find(|f| f.id == id)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Fighter {
    pub id: Guid,
    pub hp: i32,
    pub max_hp: i32,
    pub position: [f32; 3],
    /// Controlled by the player (party member).
    pub party: bool,
    /// May act now.
    pub active: bool,
    /// Its turn this round is over.
    pub done: bool,
    pub downed: bool,
    pub dead: bool,
    /// `(name, amount, max)` (party members only).
    pub resources: Vec<(String, f32, f32)>,
    /// Spells and attacks (party members only).
    pub actions: Vec<ActionInfo>,
    /// Carried items `(id, name)` (party members only).
    pub items: Vec<(Guid, String)>,
}

impl Fighter {
    pub fn resource(&self, name: &str) -> f32 {
        self.resources
            .iter()
            .find(|r| r.0 == name)
            .map_or(0.0, |r| r.1)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActionInfo {
    pub spell: String,
    /// Loca handle of `DisplayName`.
    pub display: Option<String>,
    pub costs: Vec<(String, f32)>,
    /// Metres; `None` if unknown.
    pub range: Option<f32>,
    /// Cast on oneself (`Shout` spells), no target to pick.
    pub on_self: bool,
    pub affordable: bool,
}

/// Spell types the action bar offers (a target or oneself; no ground targeting, throwing or
/// summoning yet).
const OFFERED_TYPES: &[&str] = &["Target", "Projectile", "Shout"];
/// Spells the action bar leaves out: they target the ground (`Projectile_Jump`), need
/// an item (`Target_Dip`) or the cursor (`Target_MageHand…`) — not supported by the viewer yet.
const HIDDEN_SPELLS: &[&str] = &["Projectile_Jump", "Target_Dip"];

/// Combat state kept on the game thread.
#[derive(Default)]
pub struct CombatThread {
    /// The level's AI grid, for line of sight when hostiles notice the party.
    grid: Option<Arc<bg3_nav::NavGrid>>,
    /// The fight being shown.
    current: Option<Guid>,
    last: Option<CombatView>,
    out: CombatOut,
}

impl CombatThread {
    pub fn new(grid: Option<Arc<bg3_nav::NavGrid>>) -> Self {
        Self {
            grid,
            ..Self::default()
        }
    }

    /// After the game's tick: start fights the engine would start, follow the party's fight
    /// and hand out what changed.
    pub fn tick(&mut self, game: &mut Game) -> CombatOut {
        if self.current.is_none() {
            let grid = self.grid.clone();
            let can_see =
                |a: [f32; 3], b: [f32; 3]| grid.as_ref().is_none_or(|g| grid_sight(g, a, b));
            if let Some(cid) = game.start_combat_on_sight_with(can_see) {
                eprintln!("combat: hostiles noticed the party, fight {cid}");
            }
            self.current = game
                .sim
                .party
                .clone()
                .into_iter()
                .find_map(|p| game.sim.combat_of(p).filter(|c| !c.finished).map(|c| c.id));
        }
        self.out.events.extend(game.take_combat_events());
        for event in &self.out.events {
            if let CombatEvent::SpellCast { spell, .. } = event {
                let kind =
                    game.sim
                        .data
                        .combat
                        .spells
                        .get(spell)
                        .map_or(CombatAnimation::Cast, |d| {
                            // Presentation classification from install stats, not spell-name matching.
                            if d.has_flag("IsSpell") {
                                CombatAnimation::Cast
                            } else if d.has_flag("IsMelee") {
                                CombatAnimation::Melee
                            } else if d.has_flag("IsAttack") && d.spell_type == "Projectile" {
                                CombatAnimation::Ranged
                            } else {
                                CombatAnimation::Cast
                            }
                        });
                self.out.animation_kinds.insert(spell.clone(), kind);
            }
        }
        if let Some(cid) = self.current {
            let view = snapshot(game, cid);
            if view != self.last {
                self.out.view.clone_from(&view);
                self.last = view;
            }
            if self.last.as_ref().is_none_or(|v| v.finished) {
                self.current = None;
                self.last = None;
            }
        }
        std::mem::take(&mut self.out)
    }

    /// Carry out the player's (or the AI step's) command; results show at the next tick.
    pub fn apply(&mut self, game: &mut Game, cmd: CombatCmd) {
        let name = |game: &Game, id: Guid| game.sim.render(id);
        match cmd {
            CombatCmd::Move { who, distance, to } => {
                if let Err(e) = game.combat_move(who, distance, Some(to)) {
                    self.out.notes.push(format!("Can't move: {e}"));
                }
            }
            CombatCmd::Cast { who, spell, target } => {
                if let Err(e) = game.combat_cast(who, &spell, target) {
                    self.out.notes.push(format!("Can't use {spell}: {e}"));
                }
            }
            CombatCmd::EndTurn(who) => {
                if let Err(e) = game.combat_end_turn(who) {
                    self.out
                        .notes
                        .push(format!("Can't end {}'s turn: {e}", name(game, who)));
                }
            }
            CombatCmd::AiStep { party } => {
                self.out.ai_replies += 1;
                if let Some(t) = self.current.and_then(|cid| ai_step(game, cid, party)) {
                    self.out.ai.push(t);
                }
            }
            CombatCmd::UseItem { user, item } => {
                if let Some(note) = use_item(game, user, item) {
                    self.out.notes.push(note);
                }
            }
        }
    }
}

/// Use an item from the action bar: `Game::use_item` (the item's use actions; the story
/// reacts to the start and completion events on the next ticks).
pub fn use_item(game: &mut Game, user: Guid, item: Guid) -> Option<String> {
    let ok = game.use_item(user, item);
    let what = game.sim.render(item);
    (!ok).then(|| format!("Can't use {what}"))
}

/// Line of sight on the AI grid (*approximate*, the viewer's stand-in for the engine's sight
/// checks): every half metre of the straight line between the two characters' feet must be
/// over a walkable tile near the line's height (walls and floors between decks block).
fn grid_sight(grid: &bg3_nav::NavGrid, a: [f32; 3], b: [f32; 3]) -> bool {
    let agent = Agent::humanoid();
    let d = bg3_sim::combat::distance(a, b);
    let steps = (d / 0.5).ceil().max(1.0) as usize;
    (0..=steps).all(|i| {
        let k = i as f32 / steps as f32;
        let p = [
            a[0] + (b[0] - a[0]) * k,
            a[1] + (b[1] - a[1]) * k,
            a[2] + (b[2] - a[2]) * k,
        ];
        grid.closest_in_column(bg3_nav::grid::column_of(p), p[1])
            .is_some_and(|t| (grid.height(t) - p[1]).abs() < 2.5 && !grid.is_colliding(t, &agent))
    })
}

/// One AI turn of the current group: the first member still to act (a non-player one, or
/// anyone with `party`). Like `Sim::combat_run_ai` with one turn, plus where it walked.
fn ai_step(game: &mut Game, cid: Guid, party: bool) -> Option<AiTurn> {
    let c = game.sim.combat_by_id(cid).filter(|c| !c.finished)?;
    let who = c
        .current_group()?
        .members
        .iter()
        .copied()
        .find(|&m| c.is_active(m) && (party || !game.sim.is_player_controlled(m)))?;
    let from = game.sim.entity(who)?.position;
    let action = game.sim.ai_turn(who).unwrap_or(AiAction::Nothing);
    if game.sim.combat_of(who).is_some_and(|c| c.is_active(who)) {
        game.sim.combat_end_turn(who).ok();
    }
    // Deliver the story events the turn raised.
    game.tick(0.0);
    let to = game.sim.entity(who).map_or(from, |e| e.position);
    Some(AiTurn {
        who,
        from,
        to,
        path: action.path().to_vec(),
        action,
    })
}

fn snapshot(game: &mut Game, cid: Guid) -> Option<CombatView> {
    let c = game.sim.combat_by_id(cid)?.clone();
    let mut groups = Vec::new();
    for (gi, g) in c.order.iter().enumerate() {
        let mut out = Vec::new();
        for &m in &g.members {
            game.sim.ensure_combatant(m);
            let sim = &game.sim;
            let Some(e) = sim.entity(m) else {
                continue;
            };
            let (hp, max_hp) = e.sheet.as_ref().map_or((0, 0), |s| (s.hp, s.max_hp));
            let cb = sim.combatant(m);
            let party = sim.is_player_controlled(m);
            let active = c.is_active(m);
            let mut f = Fighter {
                id: m,
                hp,
                max_hp,
                position: e.position,
                party,
                active,
                done: gi < c.current || (gi == c.current && c.ended.contains(&m)),
                downed: cb.is_some_and(|x| x.downed),
                dead: cb.is_some_and(|x| x.dead),
                resources: Vec::new(),
                actions: Vec::new(),
                items: Vec::new(),
            };
            if party && let Some(cb) = cb {
                f.resources = cb
                    .resources
                    .entries
                    .iter()
                    .map(|r| (r.name.clone(), r.amount, r.max))
                    .collect();
                let can_act = active && !f.downed && !f.dead;
                for s in &cb.spells {
                    let Some(d) = sim.data.combat.spells.get(s) else {
                        continue;
                    };
                    if !OFFERED_TYPES.contains(&d.spell_type.as_str())
                        || HIDDEN_SPELLS.contains(&s.as_str())
                    {
                        continue;
                    }
                    f.actions.push(ActionInfo {
                        spell: s.clone(),
                        display: d
                            .field("DisplayName")
                            .map(|h| h.split(';').next().unwrap_or(h).to_owned()),
                        costs: d.use_costs.clone(),
                        range: sim.spell_range(m, s),
                        on_self: d.spell_type == "Shout",
                        affordable: can_act && cb.resources.can_pay(&d.use_costs),
                    });
                }
                f.items = sim
                    .entities
                    .iter()
                    .filter(|i| i.holder == Some(m))
                    .take(12)
                    .map(|i| (i.id, i.name.clone()))
                    .collect();
            }
            out.push(f);
        }
        groups.push(out);
    }
    Some(CombatView {
        id: cid,
        round: c.round,
        finished: c.finished,
        groups,
    })
}

/// `--combat-test`: jump to the deck fight's set-up the way the install test does it —
/// Lae'zel recruited (the story's `MakePlayer` / `RegisterAsCompanion` and her companion
/// faction), the avatar and her where the story puts the party
/// (`TUT_ImpRoom_CombatPlayerPosition_001`); the engine's noticing then starts the fight.
pub fn setup_test_fight(game: &mut Game, avatar: Guid) {
    game.tick_live(0.0);
    game.sim.dialog_request_stop(avatar);
    let find = |game: &Game, name: &str| {
        game.sim
            .entities
            .iter()
            .find(|e| e.name == name)
            .map(|e| (e.id, e.position))
    };
    let Some((laezel, _)) = find(game, "S_Player_Laezel") else {
        eprintln!("combat-test: no S_Player_Laezel");
        return;
    };
    let spot = find(game, "TUT_ImpRoom_CombatPlayerPosition_001")
        .map(|(_, p)| p)
        .or_else(|| {
            find(game, "S_FirstFight_Imp_Melee_02").map(|(_, p)| [p[0] + 5.0, p[1], p[2] + 5.0])
        });
    // The story keeps the guide off stage until her dialog (`TUT_LowerDeck`).
    game.sim.set_on_stage(laezel, true);
    game.sim.make_player(laezel, avatar);
    game.sim.register_as_companion(laezel);
    game.set_companion_factions(&[laezel]);
    // The opening's dialogs (and the lower deck's guide dialog, whose trigger the spot is
    // in; the story teleports the guide there) would start now: skip them, then place the
    // party.
    if let Some(p) = spot {
        game.sim.set_position(avatar, p);
    }
    for _ in 0..10 {
        game.tick_live(0.1);
        game.sim.dialog_request_stop(avatar);
        game.sim.dialog_request_stop(laezel);
    }
    if let Some(p) = spot {
        game.sim.set_position(avatar, p);
        game.sim
            .set_position(laezel, [p[0] + 1.2, p[1], p[2] + 0.6]);
    }
    // Entering the guide's trigger made the story hide her and teleport her in
    // (`SetVisible(guide, 0)`); her dialog, skipped here, shows her again by setting this
    // global flag (nodes of `TUT_LowerDeck_AfterImpFight`). Set it so the story's own rule
    // (`QRY_OnlyOnce("TUT_LowerDeck_GuideAppears")` → `SetVisible(guide, 1)`) runs.
    if let Some(flag) = Guid::parse("5b044d7c-ba8d-47f5-b056-dbe941c9cf72") {
        // TUT_LowerDeck_Event_SetGuideVisible
        game.sim.set_flag(flag, Guid::NULL, true, 0, true).ok();
    }
    game.tick_live(0.0);
    game.take_notifications();
    if !game.sim.visible(laezel) {
        eprintln!("combat-test: S_Player_Laezel is still invisible");
    }
    eprintln!("combat-test: party placed at {spot:?}");
}

// ---- viewer ----------------------------------------------------------------------------

/// Combat output received from the game thread this frame (filled by `play::receive`).
#[derive(Resource, Default)]
pub struct CombatInbox(pub Vec<CombatOut>);

/// Commands for the game thread (sent by `play::send_combat`).
#[derive(Resource, Default)]
pub struct CombatOutbox(pub Vec<CombatCmd>);

/// A character walking a combat move (player or AI); `play.rs` leaves its transform alone
/// meanwhile.
#[derive(Component)]
pub struct CombatWalk {
    /// Bevy space, nearest first.
    pub waypoints: Vec<Vec3>,
    pub speed: f32,
    /// Where it ends (BG3 space), for `play.rs` once it arrives.
    pub end: [f32; 3],
}

#[derive(Clone, Debug, PartialEq)]
enum LogKind {
    Turn,
    Action,
    Roll,
    Damage,
    Death,
    Note,
}

/// What the AI animation is doing.
enum Anim {
    Walking(Entity),
    Pause(f32),
}

enum Presentation {
    Walk(AiTurn),
    Event(CombatEvent),
}

#[derive(Resource)]
pub struct CombatUi {
    loca: Arc<Loca>,
    names: HashMap<Guid, String>,
    view: Option<CombatView>,
    /// The party member the action bar is for.
    selected: Option<Guid>,
    /// The action waiting for a target.
    spell: Option<String>,
    log: VecDeque<(String, LogKind)>,
    last_cast: Option<(Guid, Guid)>,
    waiting_ai: bool,
    anims: VecDeque<Presentation>,
    animation_kinds: HashMap<String, CombatAnimation>,
    anim: Option<Anim>,
    /// Party turns played by the AI too (`--combat-autoplay`).
    autoplay: bool,
    ended: Option<f32>,
    dirty: bool,
}

impl CombatUi {
    fn name(&self, id: Guid) -> String {
        self.names
            .get(&id)
            .cloned()
            .unwrap_or_else(|| format!("{id}"))
    }

    fn spell_name(&self, a: &ActionInfo) -> String {
        a.display
            .as_deref()
            .and_then(|h| self.loca.text(h))
            .map(crate::dialog::clean_markup)
            .unwrap_or_else(|| a.spell.clone())
    }

    fn push(&mut self, mut text: String, kind: LogKind) {
        // The avatar is "You": "You use …", not "You uses …".
        for (third, second) in [
            ("You uses ", "You use "),
            ("You moves ", "You move "),
            ("You deals ", "You deal "),
            ("You heals ", "You heal "),
            ("You gains ", "You gain "),
            ("You joins ", "You join "),
            ("You is ", "You are "),
            ("You dies", "You die"),
        ] {
            if let Some(rest) = text.strip_prefix(third) {
                text = format!("{second}{rest}");
            }
        }
        self.log.push_back((text, kind));
        while self.log.len() > LOG_KEEP {
            self.log.pop_front();
        }
        self.dirty = true;
    }

    fn in_fight(&self) -> bool {
        self.view.as_ref().is_some_and(|v| !v.finished)
    }

    fn selected_fighter(&self) -> Option<&Fighter> {
        let v = self.view.as_ref()?;
        v.fighter(self.selected?)
    }

    /// Party members acting now, in turn order.
    fn acting_party(&self) -> Vec<Guid> {
        self.view
            .as_ref()
            .map(|v| {
                v.fighters()
                    .filter(|f| f.party && f.active && !f.dead && !f.downed)
                    .map(|f| f.id)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn log_event(&mut self, e: CombatEvent) {
        let n = |s: &Self, id: Guid| s.name(id);
        match e {
            CombatEvent::CombatStarted { .. } => {
                self.push("Combat started".into(), LogKind::Turn);
            }
            CombatEvent::Initiative {
                entity,
                die,
                faces,
                bonus,
                total,
            } => {
                let t = format!(
                    "{}: Initiative {die} (d{faces}) {} = {total}",
                    n(self, entity),
                    signed(bonus, "Bonus")
                );
                self.push(t, LogKind::Roll);
            }
            CombatEvent::RoundStarted { round, .. } => {
                self.push(format!("-- Round {round} --"), LogKind::Turn);
            }
            CombatEvent::TurnStarted { entity } => {
                let t = turn_label(&n(self, entity));
                self.push(t, LogKind::Turn);
            }
            CombatEvent::Moved {
                entity,
                distance,
                movement_left,
            } => {
                let t = format!(
                    "{} moves {distance:.1} m ({movement_left:.1} m left)",
                    n(self, entity)
                );
                self.push(t, LogKind::Action);
            }
            CombatEvent::SpellCast {
                caster,
                spell,
                target,
            } => {
                self.last_cast = Some((caster, target));
                let label = self
                    .view
                    .as_ref()
                    .and_then(|v| v.fighter(caster))
                    .and_then(|f| f.actions.iter().find(|a| a.spell == spell))
                    .map(|a| self.spell_name(a))
                    .unwrap_or(spell);
                let t = if caster == target {
                    format!("{} uses {label}", n(self, caster))
                } else {
                    format!("{} uses {label} on {}", n(self, caster), n(self, target))
                };
                self.push(t, LogKind::Action);
            }
            CombatEvent::Roll { roll, .. } => {
                let t = roll_text(&roll);
                self.push(t, LogKind::Roll);
            }
            CombatEvent::Damage {
                source,
                target,
                amount,
                damage_type,
                critical,
                hp,
                max_hp,
                ..
            } => {
                let crit = if critical { " (critical)" } else { "" };
                let t = format!(
                    "{} deals {amount} {damage_type} damage{crit} to {} ({hp}/{max_hp} HP)",
                    n(self, source),
                    n(self, target)
                );
                self.push(t, LogKind::Damage);
            }
            CombatEvent::Healed { target, amount, hp } => {
                let t = format!("{} heals {amount} ({hp} HP)", n(self, target));
                self.push(t, LogKind::Damage);
            }
            CombatEvent::StatusApplied { target, status } => {
                if !status.starts_with("TUT_") {
                    let t = format!("{} gains {status}", n(self, target));
                    self.push(t, LogKind::Note);
                }
            }
            CombatEvent::Downed { entity } => {
                let t = format!("{} is downed", n(self, entity));
                self.push(t, LogKind::Death);
            }
            CombatEvent::DeathSave {
                entity,
                roll,
                successes,
                failures,
            } => {
                let t = format!(
                    "{}: Death Saving Throw {roll} (d20): {successes} successes, {failures} failures",
                    n(self, entity)
                );
                self.push(t, LogKind::Roll);
            }
            CombatEvent::Died { entity } => {
                let t = format!("{} dies", n(self, entity));
                self.push(t, LogKind::Death);
            }
            CombatEvent::CombatEnded { .. } => {
                self.push("Combat ended".into(), LogKind::Turn);
            }
            CombatEvent::Joined { entity, .. } => {
                let t = format!("{} joins the fight", n(self, entity));
                self.push(t, LogKind::Turn);
            }
            CombatEvent::TurnEnded { .. } | CombatEvent::Log(_) => {}
        }
    }
}

/// "Lae'zel's turn"; the avatar is "You".
fn turn_label(name: &str) -> String {
    if name == "You" {
        "Your turn".to_owned()
    } else {
        format!("{name}'s turn")
    }
}

fn signed(v: i32, label: &str) -> String {
    if v < 0 {
        format!("- {} ({label})", -v)
    } else {
        format!("+ {v} ({label})")
    }
}

fn capitalised(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// A roll as the game's combat log shows it: "Attack Roll: 12 (d20) + 2 (Proficiency) = 14".
fn roll_text(r: &RollEvent) -> String {
    let terms = |d20: &bg3_rules::dice::D20, terms: &[bg3_rules::rules::Term]| {
        let mut s = format!("{} (d20)", d20.natural);
        if let Some(x) = d20.discarded {
            s.push_str(&format!(" [other d20: {x}]"));
        }
        for t in terms {
            s.push(' ');
            s.push_str(&signed(t.value, &capitalised(&t.label)));
        }
        s
    };
    match r {
        RollEvent::Attack(a) => {
            let crit = match a.critical {
                bg3_rules::rules::Critical::Success => " (Critical Hit)",
                bg3_rules::rules::Critical::Fail => " (Critical Miss)",
                bg3_rules::rules::Critical::None => "",
            };
            format!(
                "Attack Roll: {} = {} vs AC {}: {}{crit}",
                terms(&a.d20, &a.terms),
                a.total,
                a.armor_class,
                if a.hit { "Hit" } else { "Miss" }
            )
        }
        RollEvent::SavingThrow(s) => format!(
            "{} Saving Throw: {} = {} vs DC {}: {}",
            s.ability,
            terms(&s.d20, &s.terms),
            s.total,
            s.dc,
            if s.success { "Success" } else { "Failure" }
        ),
        RollEvent::Die { die, value, dc } => format!("Roll: {value} ({die:?}) vs DC {dc}"),
    }
}

pub struct CombatPlugin {
    pub loca: Arc<Loca>,
    pub names: HashMap<Guid, String>,
    pub autoplay: bool,
}

#[derive(Component)]
struct TurnBar;
#[derive(Component)]
struct ActionBar;
#[derive(Component)]
struct LogPanel;
/// Clicks on this node don't reach the world.
#[derive(Component)]
struct UiBlocker;

#[derive(Component, Clone, Debug)]
enum UiButton {
    Action(usize),
    EndTurn,
    Select(Guid),
    Item(Guid),
}

impl Plugin for CombatPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(CombatUi {
            loca: self.loca.clone(),
            names: self.names.clone(),
            view: None,
            selected: None,
            spell: None,
            log: VecDeque::new(),
            last_cast: None,
            waiting_ai: false,
            anims: VecDeque::new(),
            anim: None,
            autoplay: self.autoplay,
            ended: None,
            dirty: true,
            animation_kinds: HashMap::new(),
        })
        .insert_resource(CombatInbox::default())
        .insert_resource(CombatOutbox::default())
        .add_systems(Startup, spawn_panels)
        .add_systems(
            Update,
            (
                process_inbox
                    .after(crate::play::receive)
                    .before(crate::play::sync_characters),
                (combat_clicks, combat_keys)
                    .before(crate::play::talk_click)
                    .after(process_inbox),
                (animate, drive_ai, refresh_panels, draw_rings)
                    .chain()
                    .after(crate::play::sync_characters)
                    .before(crate::characters::animate_characters),
            ),
        );
    }
}

fn to_bevy(p: [f32; 3]) -> Vec3 {
    Vec3::from_array(space::vec3(p))
}

fn to_bg3(v: Vec3) -> [f32; 3] {
    space::vec3(v.to_array())
}

/// Root entities by object id.
fn roots_by_id<'a>(
    roots: impl Iterator<Item = (Entity, &'a CharacterRoot)>,
) -> HashMap<Guid, Entity> {
    roots
        .filter_map(|(e, r)| Guid::parse(&r.id).map(|g| (g, e)))
        .collect()
}

fn process_inbox(mut inbox: ResMut<CombatInbox>, mut ui: ResMut<CombatUi>) {
    for out in inbox.0.drain(..) {
        if let Some(v) = out.view {
            if !v.finished {
                ui.ended = None;
            }
            ui.view = Some(v);
            ui.dirty = true;
        }
        ui.animation_kinds.extend(out.animation_kinds);
        // A game-thread AI step is already complete. Present its route before its cast,
        // damage and next-turn notifications, including the camera focus change.
        ui.anims.extend(out.ai.into_iter().map(Presentation::Walk));
        ui.anims
            .extend(out.events.into_iter().map(Presentation::Event));
        for n in out.notes {
            ui.push(n, LogKind::Note);
        }
        if out.ai_replies > 0 {
            ui.waiting_ai = false;
        }
    }
    // Keep the selection on a party member who can act.
    let acting = ui.acting_party();
    if ui.selected.is_none_or(|s| !acting.contains(&s)) {
        let next = acting.first().copied();
        if ui.selected != next {
            ui.selected = next;
            ui.spell = None;
            ui.dirty = true;
        }
    }
}

/// Present walks and combat events in order; clip durations gate the next AI request.
#[allow(clippy::too_many_arguments)]
pub(crate) fn animate(
    mut commands: Commands,
    time: Res<Time>,
    mut ui: ResMut<CombatUi>,
    mut state: ResMut<crate::play::SimState>,
    mut camera: ResMut<CombatCamera>,
    mut walkers: Query<(Entity, &mut CombatWalk, &mut Transform, &CharacterRoot)>,
    roots: Query<(Entity, &CharacterRoot)>,
    mut animators: Query<(&CharacterRoot, &mut CharacterAnimator)>,
    controlled: Query<(), With<Controlled>>,
) {
    let dt = time.delta_secs();
    // Walks (AI and player).
    for (entity, mut w, mut t, root) in &mut walkers {
        let mut budget = w.speed * dt;
        while budget > 0.0 {
            let Some(&next) = w.waypoints.first() else {
                break;
            };
            let to = next - t.translation;
            let flat = Vec3::new(to.x, 0.0, to.z);
            if flat.length() > 1e-4 {
                t.rotation = Quat::from_rotation_y(flat.x.atan2(flat.z));
            }
            let d = to.length();
            if d <= budget {
                t.translation = next;
                budget -= d;
                w.waypoints.remove(0);
            } else {
                t.translation += to / d * budget;
                budget = 0.0;
            }
        }
        if w.waypoints.is_empty() {
            t.translation = to_bevy(w.end);
            if controlled.contains(entity) {
                // The avatar's position as the sim has it: no write-back, no teleport.
                state.sent = Some(w.end);
            }
            commands.entity(entity).remove::<CombatWalk>();
            set_clip(&mut animators, root, false);
        } else {
            set_clip(&mut animators, root, true);
        }
    }
    match ui.anim.take() {
        Some(Anim::Walking(e)) if walkers.contains(e) => {
            ui.anim = Some(Anim::Walking(e));
            return;
        }
        Some(Anim::Pause(left)) if left > dt => {
            ui.anim = Some(Anim::Pause(left - dt));
            return;
        }
        _ => {}
    }
    let by_id = roots_by_id(roots.iter());
    // Non-visual events need no frame of their own. Stop at the first walk/clip.
    while let Some(next) = ui.anims.pop_front() {
        match next {
            Presentation::Walk(turn) => {
                camera.actor_started(turn.who);
                if turn.path.len() >= 2
                    && let Some(&e) = by_id.get(&turn.who)
                {
                    let from = turn.from;
                    commands.queue(move |world: &mut World| {
                        if let Some(mut transform) = world.get_mut::<Transform>(e) {
                            transform.translation = to_bevy(from);
                        }
                    });
                    let speed = roots.get(e).map_or(4.0, |(_, r)| r.run_speed.max(2.0));
                    commands.entity(e).insert(CombatWalk {
                        waypoints: turn.path.iter().skip(1).copied().map(to_bevy).collect(),
                        speed,
                        end: turn.to,
                    });
                    ui.anim = Some(Anim::Walking(e));
                    return;
                }
                if !matches!(turn.action, AiAction::Cast { .. }) {
                    ui.anim = Some(Anim::Pause(IDLE_PAUSE_SECS));
                    return;
                }
            }
            Presentation::Event(event) => {
                if matches!(event, CombatEvent::CombatEnded { .. }) {
                    ui.ended = Some(time.elapsed_secs());
                }
                let pause = present_event(
                    &event,
                    &ui.animation_kinds,
                    &by_id,
                    &mut animators,
                    &mut commands,
                    &mut camera,
                );
                ui.log_event(event);
                if pause > 0.0 {
                    ui.anim = Some(Anim::Pause(pause));
                    return;
                }
            }
        }
    }
}

/// Viewer approximation: generic clips, without the original blueprint's blending, animation
/// event markers, weapon subsets or directional hit selection. See the resource survey.
fn present_event(
    event: &CombatEvent,
    kinds: &HashMap<String, CombatAnimation>,
    by_id: &HashMap<Guid, Entity>,
    animators: &mut Query<(&CharacterRoot, &mut CharacterAnimator)>,
    commands: &mut Commands,
    camera: &mut CombatCamera,
) -> f32 {
    let (who, kind) = match event {
        CombatEvent::TurnStarted { entity } => {
            camera.turn_started(*entity);
            return 0.0;
        }
        CombatEvent::CombatEnded { .. } => {
            camera.stop();
            return 0.0;
        }
        CombatEvent::SpellCast {
            caster,
            spell,
            target,
        } => {
            if let (Some(&actor), Some(&target)) = (by_id.get(caster), by_id.get(target)) {
                commands.queue(move |world: &mut World| {
                    let target = world.get::<Transform>(target).map(|t| t.translation);
                    if let Some(target) = target
                        && let Some(mut t) = world.get_mut::<Transform>(actor)
                    {
                        let delta = target - t.translation;
                        if delta.x * delta.x + delta.z * delta.z > 1e-6 {
                            t.rotation = Quat::from_rotation_y(delta.x.atan2(delta.z));
                        }
                    }
                });
            }
            (
                *caster,
                kinds.get(spell).copied().unwrap_or(CombatAnimation::Cast),
            )
        }
        CombatEvent::Damage { target, amount, .. } if *amount > 0 => {
            (*target, CombatAnimation::Hit)
        }
        CombatEvent::Died { entity } => (*entity, CombatAnimation::Death),
        CombatEvent::Downed { entity } => (*entity, CombatAnimation::Downed),
        CombatEvent::Healed { target, hp, .. } if *hp > 0 => {
            if let Some((root, mut a)) = animators
                .iter_mut()
                .find(|(r, _)| Guid::parse(&r.id) == Some(*target))
                && a.incapacitated
            {
                a.revive(&root.idle);
            }
            return 0.0;
        }
        _ => return 0.0,
    };
    let fallback = if matches!(
        kind,
        CombatAnimation::Melee | CombatAnimation::Ranged | CombatAnimation::Cast
    ) {
        ATTACK_PAUSE_SECS
    } else {
        0.0
    };
    if let Some((root, mut a)) = animators
        .iter_mut()
        .find(|(r, _)| Guid::parse(&r.id) == Some(who))
    {
        if a.incapacitated && !matches!(kind, CombatAnimation::Death | CombatAnimation::Downed) {
            return 0.0;
        }
        if let Some(clip) = root.combat.get(&kind) {
            eprintln!(
                "combat animation {} {kind:?}: {} ({:.3}s)",
                root.name, clip.0.name, clip.0.duration
            );
            match kind {
                CombatAnimation::Death => {
                    a.incapacitated = true;
                    return a.once(clip, None); // hold the exact final death pose
                }
                CombatAnimation::Downed => {
                    a.downed(clip);
                    return 0.0;
                }
                _ => return a.once(clip, root.idle.clone()),
            }
        }
        if kind == CombatAnimation::Downed {
            if let Some(clip) = root.combat.get(&CombatAnimation::Dead) {
                a.incapacitated = true;
                return a.once(clip, None);
            }
            a.freeze();
        }
        if kind == CombatAnimation::Death {
            a.freeze();
        }
    }
    if kind == CombatAnimation::Death
        && let Some(&entity) = by_id.get(&who)
    {
        commands.queue(move |world: &mut World| {
            if let Some(mut t) = world.get_mut::<Transform>(entity) {
                t.rotation *= Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
            }
        });
    }
    fallback
}

/// Run while walking a combat move, idle otherwise (nav.rs does the same for click-to-move).
fn set_clip(
    animators: &mut Query<(&CharacterRoot, &mut CharacterAnimator)>,
    root: &CharacterRoot,
    running: bool,
) {
    let Some((_, mut anim)) = animators.iter_mut().find(|(r, _)| r.id == root.id) else {
        return;
    };
    let want = if running { &root.run } else { &root.idle };
    anim.locomotion(want);
}

/// Ask for the next AI turn when nobody in the party is to act (or always, with autoplay) and
/// the last one has been shown.
fn drive_ai(mut ui: ResMut<CombatUi>, mut outbox: ResMut<CombatOutbox>) {
    if !ui.in_fight() || ui.waiting_ai || ui.anim.is_some() || !ui.anims.is_empty() {
        return;
    }
    let Some(v) = &ui.view else {
        return;
    };
    let autoplay = ui.autoplay;
    let ai_due = v
        .fighters()
        .any(|f| f.active && (autoplay || !f.party || f.downed));
    let party_due = v.fighters().any(|f| f.active && f.party && !f.downed);
    if ai_due && (autoplay || !party_due) {
        outbox.0.push(CombatCmd::AiStep { party: autoplay });
        ui.waiting_ai = true;
    }
}

/// Physical cursor position over a node?
fn node_hit(
    node: &ComputedNode,
    t: &UiGlobalTransform,
    vis: &InheritedVisibility,
    p: Vec2,
) -> bool {
    vis.get() && node.contains_point(*t, p)
}

/// Clicks in a fight: the UI's buttons, else a target for the chosen action, else a move.
/// The click doesn't go on to talking or click-to-move.
#[allow(clippy::too_many_arguments)]
fn combat_clicks(
    mut buttons: ResMut<ButtonInput<MouseButton>>,
    mut ui: ResMut<CombatUi>,
    mut outbox: ResMut<CombatOutbox>,
    nav: Option<Res<Navigation>>,
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform)>,
    ui_buttons: Query<(
        &UiButton,
        &ComputedNode,
        &UiGlobalTransform,
        &InheritedVisibility,
    )>,
    blockers: Query<(&ComputedNode, &UiGlobalTransform, &InheritedVisibility), With<UiBlocker>>,
    roots: Query<(
        Entity,
        &CharacterRoot,
        &GlobalTransform,
        &InheritedVisibility,
    )>,
    walking: Query<(), With<CombatWalk>>,
    mut commands: Commands,
) {
    if !buttons.just_pressed(MouseButton::Left) || !ui.in_fight() {
        return;
    }
    let Some(window) = windows.iter().next() else {
        return;
    };
    let (Some(cursor), Some(physical)) =
        (window.cursor_position(), window.physical_cursor_position())
    else {
        return;
    };
    buttons.clear_just_pressed(MouseButton::Left);
    // UI first.
    if let Some((b, ..)) = ui_buttons
        .iter()
        .find(|(_, n, t, v)| node_hit(n, t, v, physical))
    {
        press(&mut ui, &mut outbox, b.clone());
        return;
    }
    if blockers.iter().any(|(n, t, v)| node_hit(n, t, v, physical)) {
        return;
    }
    let Some(me) = ui.selected_fighter().cloned() else {
        ui.push("Not your turn".into(), LogKind::Note);
        return;
    };
    let Some((camera, cam_t)) = cameras.iter().next() else {
        return;
    };
    // A target for the chosen action.
    if let Some(spell) = ui.spell.clone() {
        let mut best: Option<(f32, Guid)> = None;
        for (_, root, t, vis) in &roots {
            let Some(id) = Guid::parse(&root.id) else {
                continue;
            };
            if !vis.get() || ui.view.as_ref().and_then(|v| v.fighter(id)).is_none() {
                continue;
            }
            let chest = t.translation() + Vec3::Y * 1.0;
            let Ok(screen) = camera.world_to_viewport(cam_t, chest) else {
                continue;
            };
            let d = screen.distance(cursor);
            if d < PICK_RADIUS_PX && best.is_none_or(|b| d < b.0) {
                best = Some((d, id));
            }
        }
        match best {
            Some((_, target)) => {
                outbox.0.push(CombatCmd::Cast {
                    who: me.id,
                    spell,
                    target,
                });
                ui.spell = None;
                ui.dirty = true;
            }
            None => ui.push("Pick a target (Esc cancels)".into(), LogKind::Note),
        }
        return;
    }
    // A move along the AI grid, as far as the movement left goes.
    let Some(nav) = nav else {
        return;
    };
    let Some(entity) = roots_by_id(roots.iter().map(|(e, r, _, _)| (e, r)))
        .get(&me.id)
        .copied()
    else {
        return;
    };
    if walking.contains(entity) {
        return;
    }
    let Ok(ray) = camera.viewport_to_world(cam_t, cursor) else {
        return;
    };
    let Some((_, hit)) = nav
        .grid
        .raycast(to_bg3(ray.origin), to_bg3(*ray.direction), 500.0)
    else {
        return;
    };
    let left = me.resource("Movement");
    if left < 0.05 {
        ui.push("No movement left".into(), LogKind::Note);
        return;
    }
    let step = roots.get(entity).map_or(0.4, |(_, r, _, _)| r.step_height);
    let agent = Agent {
        step_height: step,
        ..Agent::humanoid()
    };
    let path = match nav.grid.find_path(me.position, hit, &agent) {
        Ok(p) => p,
        Err(e) => {
            ui.push(format!("Can't go there: {e}"), LogKind::Note);
            return;
        }
    };
    let (points, length) = truncate_path(&path.waypoints, left);
    if points.len() < 2 || length < 0.05 {
        return;
    }
    let end = *points.last().expect("two points");
    outbox.0.push(CombatCmd::Move {
        who: me.id,
        distance: length,
        to: end,
    });
    let speed = roots
        .get(entity)
        .map_or(4.0, |(_, r, _, _)| r.run_speed.max(2.0));
    commands.entity(entity).insert(CombatWalk {
        waypoints: points.iter().skip(1).map(|&p| to_bevy(p)).collect(),
        speed,
        end,
    });
}

/// The path cut where `budget` metres run out (horizontal length, as `bg3_nav::Path::length`
/// measures it); returns the points and the length kept.
fn truncate_path(points: &[[f32; 3]], budget: f32) -> (Vec<[f32; 3]>, f32) {
    let mut out = Vec::new();
    let mut used = 0.0;
    for (i, &p) in points.iter().enumerate() {
        if i == 0 {
            out.push(p);
            continue;
        }
        let a = points[i - 1];
        let seg = ((p[0] - a[0]).powi(2) + (p[2] - a[2]).powi(2)).sqrt();
        if used + seg <= budget {
            used += seg;
            out.push(p);
            continue;
        }
        let k = ((budget - used) / seg).clamp(0.0, 1.0);
        out.push([
            a[0] + (p[0] - a[0]) * k,
            a[1] + (p[1] - a[1]) * k,
            a[2] + (p[2] - a[2]) * k,
        ]);
        used = budget;
        break;
    }
    (out, used)
}

fn press(ui: &mut CombatUi, outbox: &mut CombatOutbox, b: UiButton) {
    let Some(me) = ui.selected_fighter().cloned() else {
        return;
    };
    match b {
        UiButton::Action(i) => {
            let Some(a) = me.actions.get(i).cloned() else {
                return;
            };
            if !a.affordable {
                let t = format!("{}: not enough resources", ui.spell_name(&a));
                ui.push(t, LogKind::Note);
            } else if a.on_self {
                outbox.0.push(CombatCmd::Cast {
                    who: me.id,
                    spell: a.spell,
                    target: me.id,
                });
            } else {
                ui.spell = Some(a.spell);
            }
        }
        UiButton::EndTurn => {
            outbox.0.push(CombatCmd::EndTurn(me.id));
            ui.spell = None;
        }
        UiButton::Select(id) => {
            if ui.acting_party().contains(&id) {
                ui.selected = Some(id);
                ui.spell = None;
            }
        }
        UiButton::Item(item) => {
            // Item use goes through the hook on the game thread (`use_item`).
            outbox.0.push(CombatCmd::UseItem { user: me.id, item });
        }
    }
    ui.dirty = true;
}

const DIGITS: [KeyCode; 9] = [
    KeyCode::Digit1,
    KeyCode::Digit2,
    KeyCode::Digit3,
    KeyCode::Digit4,
    KeyCode::Digit5,
    KeyCode::Digit6,
    KeyCode::Digit7,
    KeyCode::Digit8,
    KeyCode::Digit9,
];

/// Keys in a fight (only while no dialog shows, whose keys they share).
fn combat_keys(
    keys: Res<ButtonInput<KeyCode>>,
    mut ui: ResMut<CombatUi>,
    mut outbox: ResMut<CombatOutbox>,
    dialogs: Query<&Visibility, With<crate::play::PlayPanel>>,
) {
    if !ui.in_fight() || dialogs.iter().any(|v| *v != Visibility::Hidden) {
        return;
    }
    if keys.just_pressed(KeyCode::Space) {
        press(&mut ui, &mut outbox, UiButton::EndTurn);
    }
    if keys.just_pressed(KeyCode::Escape) && ui.spell.take().is_some() {
        ui.dirty = true;
    }
    if keys.just_pressed(KeyCode::Tab) {
        let acting = ui.acting_party();
        if let Some(i) = ui
            .selected
            .and_then(|s| acting.iter().position(|a| *a == s))
        {
            let next = acting[(i + 1) % acting.len()];
            press(&mut ui, &mut outbox, UiButton::Select(next));
        }
    }
    if let Some(i) = DIGITS.iter().position(|k| keys.just_pressed(*k)) {
        press(&mut ui, &mut outbox, UiButton::Action(i));
    }
}

/// Rings on the ground: the acting characters (gold), the selected one (bright), the party
/// (blue) and enemies (red); the chosen action's range around the selected character.
fn draw_rings(
    mut gizmos: Gizmos,
    ui: Res<CombatUi>,
    roots: Query<(&CharacterRoot, &GlobalTransform, &InheritedVisibility)>,
) {
    let Some(v) = ui.view.as_ref().filter(|v| !v.finished) else {
        return;
    };
    let flat = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
    for (root, t, vis) in &roots {
        let Some(f) = Guid::parse(&root.id).and_then(|g| v.fighter(g)) else {
            continue;
        };
        if f.dead || !vis.get() {
            continue;
        }
        let p = t.translation() + Vec3::Y * 0.05;
        let color = match (f.active, f.party) {
            (true, _) => Color::srgb(1.0, 0.8, 0.25),
            (false, true) => Color::srgb(0.35, 0.6, 1.0),
            (false, false) => Color::srgb(0.95, 0.25, 0.2),
        };
        gizmos.circle(Isometry3d::new(p, flat), 0.6, color);
        if ui.selected == Some(f.id) {
            gizmos.circle(Isometry3d::new(p, flat), 0.7, Color::WHITE);
            let range = ui
                .spell
                .as_ref()
                .and_then(|s| f.actions.iter().find(|a| &a.spell == s))
                .and_then(|a| a.range);
            if let Some(r) = range {
                gizmos.circle(
                    Isometry3d::new(p, flat),
                    r,
                    Color::srgba(1.0, 0.5, 0.2, 0.8),
                );
            }
        }
    }
}

fn spawn_panels(mut commands: Commands) {
    commands.spawn((
        TurnBar,
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(8.0),
            width: Val::Percent(100.0),
            justify_content: JustifyContent::Center,
            ..default()
        },
        Visibility::Hidden,
    ));
    commands.spawn((
        ActionBar,
        UiBlocker,
        Node {
            position_type: PositionType::Absolute,
            left: Val::Percent(1.0),
            width: Val::Percent(58.0),
            bottom: Val::Px(8.0),
            padding: UiRect::all(Val::Px(10.0)),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(6.0),
            ..default()
        },
        BackgroundColor(Color::srgba(0.02, 0.02, 0.03, 0.85)),
        Visibility::Hidden,
    ));
    commands.spawn((
        LogPanel,
        UiBlocker,
        Node {
            position_type: PositionType::Absolute,
            right: Val::Px(8.0),
            top: Val::Px(96.0),
            width: Val::Px(440.0),
            padding: UiRect::all(Val::Px(8.0)),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(2.0),
            ..default()
        },
        BackgroundColor(Color::srgba(0.02, 0.02, 0.03, 0.78)),
        Visibility::Hidden,
    ));
}

fn font(size: f32) -> TextFont {
    TextFont {
        font_size: bevy::text::FontSize::Px(size),
        ..default()
    }
}

fn short_resource(name: &str) -> &str {
    match name {
        "ActionPoint" => "Action",
        "BonusActionPoint" => "Bonus Action",
        "ReactionActionPoint" => "Reaction",
        "Movement" => "Movement",
        "SpellSlot" => "Spell Slot",
        other => other,
    }
}

fn cost_text(costs: &[(String, f32)]) -> String {
    costs
        .iter()
        .map(|(n, a)| {
            if (*a - 1.0).abs() < 1e-3 {
                short_resource(n).to_owned()
            } else {
                format!("{a} {}", short_resource(n))
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[allow(clippy::type_complexity)]
fn refresh_panels(
    mut commands: Commands,
    time: Res<Time>,
    mut ui: ResMut<CombatUi>,
    mut panels: ParamSet<(
        Query<(Entity, &mut Visibility), With<TurnBar>>,
        Query<(Entity, &mut Visibility), With<ActionBar>>,
        Query<(Entity, &mut Visibility), With<LogPanel>>,
    )>,
) {
    let expired = ui
        .ended
        .is_some_and(|t| time.elapsed_secs() - t > ENDED_SECS);
    if expired && ui.view.is_some() {
        ui.view = None;
        ui.dirty = true;
    }
    if !ui.dirty {
        return;
    }
    ui.dirty = false;
    let ui = &*ui;
    let shown = ui.view.is_some();
    let fight = ui.in_fight();

    let gold = Color::srgb(0.93, 0.78, 0.45);
    let white = Color::srgb(0.92, 0.92, 0.9);
    let grey = Color::srgb(0.55, 0.55, 0.55);

    // Turn order.
    if let Ok((bar, mut vis)) = panels.p0().single_mut() {
        commands.entity(bar).despawn_related::<Children>();
        *vis = if fight {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if let Some(v) = ui.view.as_ref().filter(|_| fight) {
            commands.entity(bar).with_children(|row| {
                row.spawn((
                    Node {
                        padding: UiRect::all(Val::Px(6.0)),
                        margin: UiRect::horizontal(Val::Px(3.0)),
                        align_items: AlignItems::Center,
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.7)),
                ))
                .with_child((
                    Text::new(format!("Round {}", v.round)),
                    font(15.0),
                    TextColor(gold),
                ));
                for g in &v.groups {
                    let members: Vec<&Fighter> = g.iter().filter(|f| !f.dead).collect();
                    if members.is_empty() {
                        continue;
                    }
                    // A shared turn: one frame around the group.
                    row.spawn((Node {
                        margin: UiRect::horizontal(Val::Px(3.0)),
                        column_gap: Val::Px(2.0),
                        ..default()
                    },))
                        .with_children(|grp| {
                            for f in members {
                                fighter_box(grp, ui, f);
                            }
                        });
                }
            });
        }
    }

    // Action bar.
    if let Ok((bar, mut vis)) = panels.p1().single_mut() {
        commands.entity(bar).despawn_related::<Children>();
        let me = ui.selected_fighter().filter(|_| fight);
        *vis = if fight {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        commands.entity(bar).with_children(|p| {
            let Some(me) = me else {
                let who = ui
                    .view
                    .as_ref()
                    .and_then(|v| v.fighters().find(|f| f.active))
                    .map(|f| turn_label(&ui.name(f.id)))
                    .unwrap_or_default();
                p.spawn((Text::new(who), font(18.0), TextColor(grey)));
                return;
            };
            // Header: who, hit points, resources.
            let res = |n: &str| {
                me.resources
                    .iter()
                    .find(|r| r.0 == n)
                    .map(|r| (r.1, r.2))
                    .unwrap_or((0.0, 0.0))
            };
            let (a, am) = res("ActionPoint");
            let (b, bm) = res("BonusActionPoint");
            let (m, mm) = res("Movement");
            let mut header = format!(
                "{}   HP {}/{}   Action {}/{}   Bonus Action {}/{}   Movement {m:.1}/{mm:.1} m",
                ui.name(me.id),
                me.hp,
                me.max_hp,
                a,
                am,
                b,
                bm
            );
            let slots = res("SpellSlot");
            if slots.1 > 0.0 {
                header.push_str(&format!("   Spell Slots {}/{}", slots.0, slots.1));
            }
            p.spawn((Text::new(header), font(15.0), TextColor(gold)));
            if let Some(s) = &ui.spell {
                let a = me.actions.iter().find(|a| &a.spell == s);
                let name = a.map(|a| ui.spell_name(a)).unwrap_or_else(|| s.clone());
                let range = a
                    .and_then(|a| a.range)
                    .map(|r| format!(" (range {r:.1} m)"))
                    .unwrap_or_default();
                p.spawn((
                    Text::new(format!("{name}{range}: click a target, Esc to cancel")),
                    font(15.0),
                    TextColor(Color::srgb(1.0, 0.6, 0.3)),
                ));
            }
            // Actions.
            p.spawn(Node {
                flex_wrap: FlexWrap::Wrap,
                column_gap: Val::Px(5.0),
                row_gap: Val::Px(5.0),
                ..default()
            })
            .with_children(|row| {
                for (i, a) in me.actions.iter().enumerate() {
                    let chosen = ui.spell.as_deref() == Some(a.spell.as_str());
                    let label = if i < 9 {
                        format!("{} {}", i + 1, ui.spell_name(a))
                    } else {
                        ui.spell_name(a)
                    };
                    let cost = cost_text(&a.costs);
                    let bg = if chosen {
                        Color::srgba(0.55, 0.35, 0.1, 0.95)
                    } else if a.affordable {
                        Color::srgba(0.16, 0.16, 0.2, 0.95)
                    } else {
                        Color::srgba(0.08, 0.08, 0.09, 0.9)
                    };
                    let fg = if a.affordable { white } else { grey };
                    row.spawn((
                        UiButton::Action(i),
                        Node {
                            padding: UiRect::axes(Val::Px(6.0), Val::Px(3.0)),
                            flex_direction: FlexDirection::Column,
                            ..default()
                        },
                        BackgroundColor(bg),
                    ))
                    .with_children(|b| {
                        b.spawn((Text::new(label), font(13.0), TextColor(fg)));
                        if !cost.is_empty() {
                            b.spawn((Text::new(cost), font(10.0), TextColor(grey)));
                        }
                    });
                }
            });
            // Items (the use hook) and End Turn.
            p.spawn(Node {
                column_gap: Val::Px(5.0),
                align_items: AlignItems::Center,
                ..default()
            })
            .with_children(|row| {
                if !me.items.is_empty() {
                    row.spawn((Text::new("Items:"), font(13.0), TextColor(grey)));
                }
                for (id, name) in &me.items {
                    row.spawn((
                        UiButton::Item(*id),
                        Node {
                            padding: UiRect::axes(Val::Px(6.0), Val::Px(3.0)),
                            ..default()
                        },
                        BackgroundColor(Color::srgba(0.12, 0.14, 0.12, 0.95)),
                    ))
                    .with_child((
                        Text::new(name.clone()),
                        font(12.0),
                        TextColor(white),
                    ));
                }
                row.spawn(Node {
                    flex_grow: 1.0,
                    ..default()
                });
                row.spawn((
                    UiButton::EndTurn,
                    Node {
                        padding: UiRect::axes(Val::Px(14.0), Val::Px(6.0)),
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.45, 0.12, 0.1, 0.95)),
                ))
                .with_child((
                    Text::new("End Turn (Space)"),
                    font(15.0),
                    TextColor(white),
                ));
            });
        });
    }

    // Log.
    if let Ok((panel, mut vis)) = panels.p2().single_mut() {
        commands.entity(panel).despawn_related::<Children>();
        *vis = if shown {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        commands.entity(panel).with_children(|p| {
            p.spawn((Text::new("Combat Log"), font(15.0), TextColor(gold)));
            let skip = ui.log.len().saturating_sub(LOG_SHOWN);
            for (text, kind) in ui.log.iter().skip(skip) {
                let (color, size) = match kind {
                    LogKind::Turn => (gold, 14.0),
                    LogKind::Action => (white, 13.0),
                    LogKind::Roll => (Color::srgb(0.6, 0.78, 0.98), 12.5),
                    LogKind::Damage => (Color::srgb(1.0, 0.55, 0.45), 13.0),
                    LogKind::Death => (Color::srgb(1.0, 0.3, 0.25), 14.0),
                    LogKind::Note => (grey, 12.5),
                };
                p.spawn((Text::new(text.clone()), font(size), TextColor(color)));
            }
        });
    }
}

/// One entry of the turn-order bar.
fn fighter_box(parent: &mut ChildSpawnerCommands, ui: &CombatUi, f: &Fighter) {
    let frame = if f.active {
        Color::srgb(1.0, 0.8, 0.25)
    } else {
        Color::srgba(0.0, 0.0, 0.0, 0.0)
    };
    let bg = match (f.party, f.done) {
        (true, false) => Color::srgba(0.1, 0.2, 0.4, 0.88),
        (true, true) => Color::srgba(0.06, 0.1, 0.18, 0.75),
        (false, false) => Color::srgba(0.4, 0.08, 0.06, 0.88),
        (false, true) => Color::srgba(0.18, 0.05, 0.04, 0.75),
    };
    let text = if f.done && !f.active {
        Color::srgb(0.6, 0.6, 0.6)
    } else {
        Color::srgb(0.95, 0.95, 0.92)
    };
    let pct = if f.max_hp > 0 {
        (f.hp.max(0) as f32 / f.max_hp as f32 * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };
    let hp = if f.downed {
        "Downed".to_owned()
    } else {
        format!("{}/{} HP", f.hp, f.max_hp)
    };
    let mut b = parent.spawn((
        Node {
            min_width: Val::Px(96.0),
            padding: UiRect::all(Val::Px(5.0)),
            border: UiRect::all(Val::Px(2.0)),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(2.0),
            ..default()
        },
        BorderColor::all(frame),
        BackgroundColor(bg),
    ));
    if f.party {
        b.insert(UiButton::Select(f.id));
    }
    b.with_children(|c| {
        c.spawn((Text::new(ui.name(f.id)), font(14.0), TextColor(text)));
        c.spawn((Text::new(hp), font(12.0), TextColor(text)));
        c.spawn((
            Node {
                height: Val::Px(4.0),
                width: Val::Percent(100.0),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.6)),
        ))
        .with_child((
            Node {
                height: Val::Px(4.0),
                width: Val::Percent(pct),
                ..default()
            },
            BackgroundColor(Color::srgb(0.75, 0.15, 0.12)),
        ));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_stop_where_movement_runs_out() {
        let pts = [[0.0, 0.0, 0.0], [3.0, 0.0, 0.0], [3.0, 1.0, 4.0]];
        let (all, len) = truncate_path(&pts, 9.0);
        assert_eq!((all.len(), len), (3, 7.0));
        let (cut, len) = truncate_path(&pts, 5.0);
        assert_eq!(len, 5.0);
        assert_eq!(cut.len(), 3);
        let end = cut[2];
        assert!(
            (end[0] - 3.0).abs() < 1e-5 && (end[2] - 2.0).abs() < 1e-5,
            "{end:?}"
        );
    }

    #[test]
    fn rolls_read_like_the_game_log() {
        let r = RollEvent::Attack(bg3_rules::rules::AttackRoll {
            attacker: bg3_rules::EntityId(1),
            target: bg3_rules::EntityId(2),
            attack_type: bg3_rules::context::AttackType::MeleeWeaponAttack,
            advantage: bg3_rules::context::Advantage::default(),
            d20: bg3_rules::dice::D20 {
                natural: 12,
                discarded: None,
            },
            terms: vec![bg3_rules::rules::Term {
                label: "proficiency".into(),
                value: 2,
            }],
            total: 14,
            armor_class: 7,
            critical: bg3_rules::rules::Critical::None,
            hit: true,
        });
        assert_eq!(
            roll_text(&r),
            "Attack Roll: 12 (d20) + 2 (Proficiency) = 14 vs AC 7: Hit"
        );
    }
}
