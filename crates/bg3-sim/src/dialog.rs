//! Dialogs on the sim: the server's dialog manager.
//!
//! * The story asks for a dialog with `StartDialog_Internal` ([`Sim::request_dialog`]); the
//!   instance gets its id at once (the query's out-parameter) and starts at the next update
//!   ([`Sim::update_dialogs`]), like the engine, which raises `DialogStarted` frames later
//!   (the trace: `DialogAddActorAtReservedSlot` adds an actor between the request and the
//!   start, and the start event already lists it).
//! * A [`bg3_dialog::DialogRunner`] plays it with the sim as its [`DialogWorld`]
//!   ([`SimDialogWorld`]): flag checks and set-flags read and write the sim's flags with the
//!   `scr::SetFlag` holder rules, tag checks see the sim's tags (plus dialog tags), and rolls
//!   go through `bg3_rules` against the speakers' character sheets.
//! * What the runner reports becomes story events in the sim's queue
//!   (`DialogStarted`, `DialogActorJoined`, `FlagSet(…, instance)`, `RollResult`,
//!   `DialogRollResult`, `DialogActorLeft`, `DialogEnded`, …, raised as by
//!   `esv::StoryDialogEventManager`) and presentation [`Notification`]s (lines, choices,
//!   rolls).
//! * Player input: [`Sim::dialog_advance`] (a line finished) and [`Sim::dialog_choose`]. A
//!   [`DialogScript`] (choices and roll outcomes decided in advance) lets headless runs and the
//!   trace harness drive a dialog without a player ([`Sim::dialog_autostep`]).
//!
//! Instance ids are numbered from 1 in request order; dialogs the engine starts on its own
//! (world dialogs) take ids too, so an instance the sim learns from an engine event moves the
//! counter past it.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use bg3_dialog::library::{DialogLibrary, DialogProvider, DialogResource};
use bg3_dialog::{
    Choice, Dialog, DialogEvent, DialogRunner, DialogWorld, FlagChange, FlagType, Line,
    RollOutcome, RollRequest, StartParams, Step,
};
use bg3_rules::context::{Ability, Advantage, EntityId, Skill};
use bg3_rules::dice::DiceRng;
use bg3_rules::rules::{AbilityCheckRoll, CheckKind, Critical, roll_ability_check};

use crate::entity::EntityKind;
use crate::events::SimValue;
use crate::guid::Guid;
use crate::sim::Sim;
use crate::tags::TagSource;

/// A sim UUID as the dialog crate's GUID (same text form).
pub fn to_lsf(g: Guid) -> bg3_lsf::Guid {
    bg3_lsf::Guid(g.0.to_be_bytes())
}

/// The dialog crate's GUID as a sim UUID.
pub fn from_lsf(g: &bg3_lsf::Guid) -> Guid {
    Guid(u128::from_be_bytes(g.0))
}

/// Dialog resources (`DialogBank`) and the compiled dialogs behind them.
pub struct DialogBank {
    resources: HashMap<Guid, DialogResource>,
    provider: Arc<dyn DialogProvider + Send + Sync>,
}

impl std::fmt::Debug for DialogBank {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DialogBank")
            .field("resources", &self.resources.len())
            .finish_non_exhaustive()
    }
}

impl DialogBank {
    /// Resources plus something that loads their dialogs (for tests: a map).
    pub fn new(
        provider: Arc<dyn DialogProvider + Send + Sync>,
        resources: impl IntoIterator<Item = DialogResource>,
    ) -> Self {
        Self {
            resources: resources
                .into_iter()
                .map(|r| (from_lsf(&r.id), r))
                .collect(),
            provider,
        }
    }

    /// Every resource of the install's dialog library.
    pub fn from_library(lib: Arc<DialogLibrary>) -> Self {
        let resources: Vec<DialogResource> = lib.resources().to_vec();
        Self::new(lib, resources)
    }

    pub fn resource(&self, id: Guid) -> Option<&DialogResource> {
        self.resources.get(&id)
    }

    pub fn dialog(&self, id: Guid) -> Option<Arc<Dialog>> {
        self.provider.dialog_by_resource(&to_lsf(id))
    }

    pub fn provider(&self) -> Arc<dyn DialogProvider + Send + Sync> {
        self.provider.clone()
    }
}

/// Where an instance is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstanceState {
    /// Requested by the story; starts at the next [`Sim::update_dialogs`].
    Requested,
    Running,
    Ended,
}

/// Decisions made in advance: the choices to pick (indices into the offered options, in
/// order) and roll outcomes to force. Used when no player is there to decide.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DialogScript {
    pub choices: VecDeque<usize>,
    pub rolls: VecDeque<bool>,
}

/// A roll a dialog made.
#[derive(Clone, Debug, PartialEq)]
pub struct DialogRollRecord {
    pub node: Guid,
    pub roller: Option<Guid>,
    pub active: bool,
    /// The check through `bg3_rules`, if the roller, ability and DC are known.
    pub check: Option<AbilityCheckRoll>,
    pub success: bool,
    /// The outcome came from a [`DialogScript`].
    pub forced: bool,
    /// For a forced outcome: some natural d20 roll (2–19) gives that outcome with the rules'
    /// modifiers and DC. `false` hints at a modifier or DC the sim gets wrong.
    pub consistent: bool,
}

/// One dialog instance.
#[derive(Clone, Debug)]
pub struct DialogInstance {
    pub id: u32,
    /// The dialog resource.
    pub resource: Guid,
    /// `Name_<uuid>` of the resource, as the story sees it.
    pub name: String,
    /// Actor per speaker slot.
    pub speakers: Vec<Option<Guid>>,
    pub state: InstanceState,
    /// Started by the sim. `false`: learned from an engine event (a dialog the engine started
    /// on its own, or one the sim could not load).
    pub owned: bool,
    pub script: DialogScript,
    /// Rolls made so far.
    pub rolls: Vec<DialogRollRecord>,
    runner: Option<DialogRunner>,
}

impl DialogInstance {
    /// What the instance waits for (`None` before it starts).
    pub fn step(&self) -> Option<&Step> {
        self.runner.as_ref().map(DialogRunner::step)
    }

    pub fn actors(&self) -> impl Iterator<Item = Guid> + '_ {
        self.speakers.iter().flatten().copied()
    }

    /// The runner (for inspection).
    pub fn runner(&self) -> Option<&DialogRunner> {
        self.runner.as_ref()
    }
}

/// The sim's dialog manager state.
#[derive(Clone, Debug, Default)]
pub struct DialogSystem {
    bank: Option<Arc<DialogBank>>,
    pub instances: Vec<DialogInstance>,
    last_id: u32,
}

/// Something the presentation layer shows or reacts to.
#[derive(Clone, Debug, PartialEq)]
pub enum Notification {
    DialogStarted {
        instance: u32,
        dialog: String,
    },
    /// A node plays: show its line; call [`Sim::dialog_advance`] when done.
    DialogLine {
        instance: u32,
        line: Line,
    },
    /// The player picks one: [`Sim::dialog_choose`].
    DialogChoices {
        instance: u32,
        choices: Vec<Choice>,
    },
    DialogRoll {
        instance: u32,
        roll: DialogRollRecord,
    },
    /// A `Trade` node asked to open trading.
    TradeRequested {
        instance: u32,
    },
    DialogEnded {
        instance: u32,
    },
}

/// Why a dialog operation was refused.
#[derive(Debug, thiserror::Error)]
pub enum DialogError {
    #[error("no dialog instance {0}")]
    NoInstance(u32),
    #[error("dialog instance {0} is not running")]
    NotRunning(u32),
    #[error(transparent)]
    Runner(#[from] bg3_dialog::Error),
}

/// A die that always shows the same face (forced roll outcomes).
struct FixedDie(u32);

impl DiceRng for FixedDie {
    fn roll_die(&mut self, faces: u32) -> u32 {
        self.0.clamp(1, faces)
    }
}

/// The sim as the world a dialog runner reads and writes.
pub struct SimDialogWorld<'a> {
    pub sim: &'a mut Sim,
    pub instance: u32,
}

impl SimDialogWorld<'_> {
    /// The objects a per-object flag lives on (`scr::SetFlag` holder rules), or `None` if the
    /// flag can't be used on that object.
    fn holders(&self, flag: Guid, target: Guid) -> Option<Vec<Guid>> {
        self.sim.flag_holders(flag, target).ok().map(|(_, h)| h)
    }
}

impl DialogWorld for SimDialogWorld<'_> {
    fn flag_type(&self, flag: &bg3_lsf::Guid) -> Option<FlagType> {
        let d = self.sim.data.flags.get(from_lsf(flag))?;
        FlagType::from_usage(d.usage.raw())
    }

    // bg3: esv::EoCDialogGameInterface::EvaluateFlag (4.1.1.7631656)
    fn get_flag(&self, flag: &bg3_lsf::Guid, ty: FlagType, target: Option<&bg3_lsf::Guid>) -> bool {
        let f = from_lsf(flag);
        match ty {
            FlagType::Global | FlagType::Local => self.sim.flags.get(Guid::NULL, f),
            _ => {
                let Some(holders) = target.and_then(|t| self.holders(f, from_lsf(t))) else {
                    return false;
                };
                match ty {
                    // Set on any character of the user / any party member.
                    FlagType::User | FlagType::Party => {
                        holders.iter().any(|h| self.sim.flags.get(*h, f))
                    }
                    _ => self.sim.flags.get(holders[0], f),
                }
            }
        }
    }

    // bg3: esv::EoCDialogGameInterface::SetFlag → esv::OsirisVariableHelper::SetIndividualFlag
    // (4.1.1.7631656): only values that change are reported.
    fn set_flag(
        &mut self,
        flag: &bg3_lsf::Guid,
        ty: FlagType,
        target: Option<&bg3_lsf::Guid>,
        value: bool,
        _dialog_instance: u32,
    ) -> Vec<FlagChange> {
        let f = from_lsf(flag);
        let holders = match ty {
            FlagType::Global | FlagType::Local => vec![Guid::NULL],
            _ => match target.and_then(|t| self.holders(f, from_lsf(t))) {
                Some(h) => h,
                None => return Vec::new(),
            },
        };
        holders
            .into_iter()
            .filter(|h| self.sim.flags.put(*h, f, value))
            .map(|h| FlagChange {
                target: (!h.is_null()).then(|| to_lsf(h)),
                value,
            })
            .collect()
    }

    fn has_tag(&self, target: &bg3_lsf::Guid, tag: &bg3_lsf::Guid) -> bool {
        self.sim
            .entity(from_lsf(target))
            .is_some_and(|e| e.tags.is_tagged_for_dialog(from_lsf(tag)))
    }

    fn set_dialog_tag(&mut self, target: &bg3_lsf::Guid, tag: &bg3_lsf::Guid, value: bool) {
        self.sim
            .set_dialog_tag(from_lsf(target), from_lsf(tag), value);
    }

    fn quest_step_unlocked(&self, flag: &bg3_lsf::Guid) -> bool {
        self.sim.quest_steps.contains(&from_lsf(flag))
    }

    fn unlock_quest_step(&mut self, flag: &bg3_lsf::Guid, _target: &bg3_lsf::Guid) {
        self.sim.quest_steps.insert(from_lsf(flag));
    }

    fn roll(&mut self, request: &RollRequest) -> RollOutcome {
        self.sim.dialog_roll(self.instance, request)
    }

    /// `rand() % n` in the game; here the sim's dice.
    fn random(&mut self, n: usize) -> usize {
        self.sim.dice.below(n)
    }
}

/// What a roll node checks: (ability, kind).
fn check_of(r: &bg3_dialog::Roll) -> Option<(Ability, CheckKind)> {
    let skill = Skill::parse(r.skill.trim());
    let ability = Ability::parse(r.ability.trim()).or_else(|| skill.map(Skill::ability))?;
    let kind = match r.roll_type.as_str() {
        "SavingThrow" => CheckKind::SavingThrow,
        "RawAbility" => CheckKind::RawAbility,
        _ => skill.map_or(CheckKind::RawAbility, CheckKind::Skill),
    };
    Some((ability, kind))
}

impl Sim {
    // ---- setup and lookup ----------------------------------------------------------------

    /// Give the sim its dialogs (without them, dialog functions are left to the caller).
    pub fn set_dialog_bank(&mut self, bank: Arc<DialogBank>) {
        self.dialogs.bank = Some(bank);
    }

    pub fn dialog_bank(&self) -> Option<&Arc<DialogBank>> {
        self.dialogs.bank.as_ref()
    }

    pub fn dialog(&self, instance: u32) -> Option<&DialogInstance> {
        self.dialogs.instances.iter().find(|i| i.id == instance)
    }

    fn dialog_mut(&mut self, instance: u32) -> Option<&mut DialogInstance> {
        self.dialogs.instances.iter_mut().find(|i| i.id == instance)
    }

    /// The resource of a `DIALOGRESOURCE` value (`Name_<uuid>`).
    pub fn dialog_resource(&self, id: Guid) -> Option<&DialogResource> {
        self.dialogs.bank.as_ref()?.resource(id)
    }

    /// Take what happened for the presentation layer since the last call.
    pub fn take_notifications(&mut self) -> Vec<Notification> {
        std::mem::take(&mut self.notifications)
    }

    /// Decisions for an instance that has no player (headless runs, trace replays).
    pub fn set_dialog_script(&mut self, instance: u32, script: DialogScript) {
        if let Some(i) = self.dialog_mut(instance) {
            i.script = script;
        }
    }

    // ---- Osiris-facing operations ----------------------------------------------------------

    /// `StartDialog_Internal`: a new instance of `resource` with `speakers` in slots 0, 1, …
    /// (null speakers skipped). `None` if the resource is unknown or a speaker doesn't exist.
    /// The instance starts at the next [`Sim::update_dialogs`].
    // bg3: esv::OsirisLSDialogsFunctions::StartDialog_Internal (4.1.1.7631656)
    pub fn request_dialog(&mut self, resource: Guid, speakers: &[Guid]) -> Option<u32> {
        let r = self.dialog_resource(resource)?;
        let name = r.osiris_name();
        let speakers: Vec<Option<Guid>> = speakers
            .iter()
            .filter(|s| !s.is_null())
            .map(|s| Some(*s))
            .collect();
        if speakers.iter().flatten().any(|s| !self.exists(*s)) {
            return None;
        }
        self.dialogs.last_id += 1;
        let id = self.dialogs.last_id;
        self.dialogs.instances.push(DialogInstance {
            id,
            resource,
            name,
            speakers,
            state: InstanceState::Requested,
            owned: true,
            script: DialogScript::default(),
            rolls: Vec::new(),
            runner: None,
        });
        Some(id)
    }

    /// `DialogAddActorAtReservedSlot(instance, actor, …)`: the actor takes the speaker slot
    /// whose speaker list names it, else the next free slot. Only before the instance starts.
    // bg3: esv::OsirisLSDialogsFunctions::DialogAddActorAtReservedSlot (4.1.1.7631656)
    pub fn dialog_add_actor(&mut self, instance: u32, actor: Guid) -> bool {
        if !self.exists(actor) {
            return false;
        }
        let Some(inst) = self.dialog(instance) else {
            return false;
        };
        if inst.state != InstanceState::Requested {
            return false;
        }
        let dialog = self
            .dialogs
            .bank
            .as_ref()
            .and_then(|b| b.dialog(inst.resource));
        let reserved = dialog.and_then(|d| {
            d.speakers
                .iter()
                .find(|s| s.list.contains(&to_lsf(actor)))
                .map(|s| s.index)
        });
        let Some(inst) = self.dialog_mut(instance) else {
            return false;
        };
        let slot = match reserved {
            Some(i) if i >= 0 => i as usize,
            _ => inst.speakers.len(),
        };
        if inst.speakers.len() <= slot {
            inst.speakers.resize(slot + 1, None);
        }
        inst.speakers[slot] = Some(actor);
        true
    }

    /// `DialogRequestStop(actor)`: stop every dialog the actor speaks in. A requested dialog
    /// that has not started is dropped.
    // bg3: esv::OsirisDialogFunctions::DialogRequestStop (4.1.1.7631656)
    pub fn dialog_request_stop(&mut self, actor: Guid) -> bool {
        let ids: Vec<(u32, InstanceState)> = self
            .dialogs
            .instances
            .iter()
            .filter(|i| i.owned && i.state != InstanceState::Ended)
            .filter(|i| i.actors().any(|a| a == actor))
            .map(|i| (i.id, i.state))
            .collect();
        for (id, state) in ids {
            match state {
                InstanceState::Requested => self.dialogs.instances.retain(|i| i.id != id),
                _ => {
                    self.with_runner(id, |r, _| r.stop());
                }
            }
        }
        true
    }

    /// `IsSpeakerReserved(actor)`: the actor speaks in a requested or running dialog.
    // bg3: esv::OsirisLSDialogsFunctions::IsSpeakerReserved (4.1.1.7631656)
    pub fn is_speaker_reserved(&self, actor: Guid) -> bool {
        self.dialogs
            .instances
            .iter()
            .filter(|i| i.state != InstanceState::Ended)
            .any(|i| i.actors().any(|a| a == actor))
    }

    /// Whether an actor counts as a player in `DialogGetInvolvedPlayer` & co.: a character
    /// in the party or controlled by a user.
    fn is_dialog_player(&self, actor: Guid) -> bool {
        self.party.contains(&actor)
            || self
                .entity(actor)
                .is_some_and(|e| e.kind == EntityKind::Character && e.user.is_some())
    }

    /// The players (or the NPCs) of an instance, in slot order. Ended instances still
    /// answer (the trace queries them after `DialogEnded`).
    // bg3: esv::OsirisDialogFunctions::DialogGetInvolvedPlayer / DialogGetInvolvedNPC
    // (4.1.1.7631656)
    pub fn dialog_involved(&self, instance: u32, players: bool) -> Option<Vec<Guid>> {
        let inst = self.dialog(instance)?;
        Some(
            inst.actors()
                .filter(|a| self.is_dialog_player(*a) == players)
                .collect(),
        )
    }

    /// `DialogSpeakerHasLines(dialog, n)`: some node of the dialog is spoken by slot `n - 1`,
    /// text or not (*verified by trace*: a slot whose only node is a `TagCinematic` without
    /// text answers 1, a slot without nodes 0).
    // bg3: esv::OsirisLSDialogsFunctions::DialogSpeakerHasLines (4.1.1.7631656)
    pub fn dialog_speaker_has_lines(&self, resource: Guid, n: i32) -> Option<bool> {
        let d = self.dialogs.bank.as_ref()?.dialog(resource)?;
        Some(d.nodes.iter().any(|node| node.speaker == Some(n - 1)))
    }

    /// Add or remove a dialog tag (`Tag` set-flags); the tag merge announces the change.
    pub fn set_dialog_tag(&mut self, object: Guid, tag: Guid, value: bool) {
        let Some(e) = self.entities.get_mut(object) else {
            return;
        };
        let changed = if value {
            e.tags.add(TagSource::Dialog, tag)
        } else {
            e.tags.remove(TagSource::Dialog, tag)
        };
        if changed {
            self.mark_tags_dirty(object);
        }
    }

    // ---- running -------------------------------------------------------------------------

    /// Start every requested dialog (the engine does it in a later frame).
    pub fn update_dialogs(&mut self) {
        let ids: Vec<u32> = self
            .dialogs
            .instances
            .iter()
            .filter(|i| i.state == InstanceState::Requested)
            .map(|i| i.id)
            .collect();
        for id in ids {
            self.start_dialog_instance(id);
        }
    }

    /// Start one requested instance; false if it can't (unknown dialog: the instance ends).
    pub fn start_dialog_instance(&mut self, instance: u32) -> bool {
        let Some(bank) = self.dialogs.bank.clone() else {
            return false;
        };
        let Some(inst) = self.dialog(instance) else {
            return false;
        };
        if inst.state != InstanceState::Requested {
            return false;
        }
        let Some(dialog) = bank.dialog(inst.resource) else {
            if let Some(i) = self.dialog_mut(instance) {
                i.state = InstanceState::Ended;
            }
            return false;
        };
        let speakers = inst.speakers.iter().map(|s| s.map(to_lsf)).collect();
        let name = inst.name.clone();
        let runner = DialogRunner::new(dialog, bank.provider(), StartParams { instance, speakers });
        if let Some(i) = self.dialog_mut(instance) {
            i.runner = Some(runner);
            i.state = InstanceState::Running;
        }
        self.notifications.push(Notification::DialogStarted {
            instance,
            dialog: name,
        });
        self.with_runner(instance, |r, w| {
            r.start(w);
        });
        true
    }

    /// A line finished playing: continue.
    pub fn dialog_advance(&mut self, instance: u32) -> Result<(), DialogError> {
        self.with_runner(instance, |r, w| r.advance(w).map(|_| ()))
            .ok_or(DialogError::NotRunning(instance))??;
        Ok(())
    }

    /// The player picked option `index` of the offered choices.
    pub fn dialog_choose(&mut self, instance: u32, index: usize) -> Result<(), DialogError> {
        self.with_runner(instance, |r, w| r.choose(w, index).map(|_| ()))
            .ok_or(DialogError::NotRunning(instance))??;
        Ok(())
    }

    /// One step without a player: start a requested instance, finish a line, or take the
    /// next scripted choice. False when the instance can't go on by itself (choices without a
    /// script, ended, unknown).
    pub fn dialog_autostep(&mut self, instance: u32) -> bool {
        let Some(inst) = self.dialog(instance) else {
            return false;
        };
        match inst.state {
            InstanceState::Requested => self.start_dialog_instance(instance),
            InstanceState::Ended => false,
            InstanceState::Running => match inst.step() {
                Some(Step::Line(_)) => self.dialog_advance(instance).is_ok(),
                Some(Step::Choices(_)) => {
                    let Some(i) = self
                        .dialog_mut(instance)
                        .and_then(|i| i.script.choices.pop_front())
                    else {
                        return false;
                    };
                    self.dialog_choose(instance, i).is_ok()
                }
                _ => false,
            },
        }
    }

    /// Run `f` on an instance's runner with the sim as its world, then turn what the runner
    /// reported into story events and notifications.
    fn with_runner<R>(
        &mut self,
        instance: u32,
        f: impl FnOnce(&mut DialogRunner, &mut SimDialogWorld<'_>) -> R,
    ) -> Option<R> {
        let mut runner = self.dialog_mut(instance)?.runner.take()?;
        let out = {
            let mut world = SimDialogWorld {
                sim: self,
                instance,
            };
            f(&mut runner, &mut world)
        };
        let events = runner.take_events();
        let step = runner.step().clone();
        let ended = runner.is_ended();
        if let Some(i) = self.dialog_mut(instance) {
            i.runner = Some(runner);
            if ended {
                i.state = InstanceState::Ended;
            }
        }
        self.raise_dialog_events(instance, events);
        self.notifications.push(match step {
            Step::Line(line) => Notification::DialogLine { instance, line },
            Step::Choices(choices) => Notification::DialogChoices { instance, choices },
            Step::Ended => Notification::DialogEnded { instance },
        });
        Some(out)
    }

    /// The story events for what a runner reported.
    // bg3: esv::StoryDialogEventManager::{OnDialogStarted, OnActorJoined, OnActorLeft,
    //      OnDialogEnded, OnInstanceDialogChanged, OnFlagSetEvent, OnFlagClearedEvent},
    //      esv::DialogRollListener (4.1.1.7631656)
    fn raise_dialog_events(&mut self, instance: u32, events: Vec<DialogEvent>) {
        let obj = |sim: &Sim, a: &Option<bg3_lsf::Guid>| {
            SimValue::Guid(sim.render(a.as_ref().map_or(Guid::NULL, from_lsf)))
        };
        let crit = |o: &RollOutcome| {
            SimValue::Enum(
                "CRITICALITYTYPE".to_owned(),
                match o.critical {
                    None => "NONE",
                    Some(true) => "SUCCESS",
                    Some(false) => "FAIL",
                }
                .to_owned(),
            )
        };
        for e in events {
            match e {
                DialogEvent::Started {
                    dialog,
                    instance,
                    automated,
                } => {
                    let name = if automated {
                        "AutomatedDialogStarted"
                    } else {
                        "DialogStarted"
                    };
                    let args = vec![
                        SimValue::Guid(dialog.osiris_name()),
                        SimValue::Int(instance as i32),
                    ];
                    self.push_event(name, args);
                }
                DialogEvent::ActorJoined {
                    dialog,
                    instance,
                    actor,
                    speaker,
                } => {
                    let args = vec![
                        SimValue::Guid(dialog.osiris_name()),
                        SimValue::Int(instance as i32),
                        SimValue::Guid(self.render(from_lsf(&actor))),
                        SimValue::Int(speaker),
                    ];
                    self.push_event("DialogActorJoined", args);
                }
                DialogEvent::ActorLeft {
                    dialog,
                    instance,
                    actor,
                    instance_ended,
                } => {
                    let args = vec![
                        SimValue::Guid(dialog.osiris_name()),
                        SimValue::Int(instance as i32),
                        SimValue::Guid(self.render(from_lsf(&actor))),
                        SimValue::Int(i32::from(instance_ended)),
                    ];
                    self.push_event("DialogActorLeft", args);
                }
                DialogEvent::NestedDialogPlayed { dialog, instance } => {
                    let args = vec![
                        SimValue::Guid(dialog.osiris_name()),
                        SimValue::Int(instance as i32),
                    ];
                    self.push_event("NestedDialogPlayed", args);
                }
                DialogEvent::Ended {
                    dialog,
                    instance,
                    automated,
                } => {
                    let name = if automated {
                        "AutomatedDialogEnded"
                    } else {
                        "DialogEnded"
                    };
                    let args = vec![
                        SimValue::Guid(dialog.osiris_name()),
                        SimValue::Int(instance as i32),
                    ];
                    self.push_event(name, args);
                }
                DialogEvent::InstanceDialogChanged {
                    instance,
                    old,
                    new,
                    old_stopping,
                } => {
                    let args = vec![
                        SimValue::Int(instance as i32),
                        SimValue::Guid(old.osiris_name()),
                        SimValue::Guid(new.osiris_name()),
                        SimValue::Int(i32::from(old_stopping)),
                    ];
                    self.push_event("InstanceDialogChanged", args);
                }
                DialogEvent::FlagSet {
                    flag,
                    target,
                    instance,
                } => {
                    let holder = target.as_ref().map_or(Guid::NULL, from_lsf);
                    self.push_flag_event(from_lsf(&flag), holder, true, instance as i32);
                }
                DialogEvent::FlagCleared {
                    flag,
                    target,
                    instance,
                } => {
                    let holder = target.as_ref().map_or(Guid::NULL, from_lsf);
                    self.push_flag_event(from_lsf(&flag), holder, false, instance as i32);
                }
                DialogEvent::Roll {
                    dialog,
                    instance,
                    node,
                    roller,
                    target,
                    active,
                    outcome,
                } => {
                    let args = vec![
                        SimValue::Str(format!("{node}_{instance}")),
                        obj(self, &roller),
                        obj(self, &target),
                        SimValue::Int(i32::from(outcome.success)),
                        SimValue::Int(i32::from(active)),
                        crit(&outcome),
                    ];
                    self.push_event("RollResult", args);
                    if active {
                        // The last-but-one argument is "detect thoughts" (not modelled).
                        let args = vec![
                            obj(self, &roller),
                            SimValue::Int(i32::from(outcome.success)),
                            SimValue::Guid(dialog.osiris_name()),
                            SimValue::Int(0),
                            crit(&outcome),
                        ];
                        self.push_event("DialogRollResult", args);
                    }
                }
                DialogEvent::TradeRequested { .. } => {
                    self.notifications
                        .push(Notification::TradeRequested { instance });
                }
                // Dialog tags are announced by the tag merge; node starts are internal.
                DialogEvent::TagChanged { .. } | DialogEvent::NodeStarted { .. } => {}
            }
        }
    }

    /// A roll node rolls: through `bg3_rules` (ability check of the roller against the
    /// difficulty class's DC), or the next scripted outcome.
    // bg3: eoc::ActiveRollNode::Start, esv::PassiveRollNode::Start (4.1.1.7631656)
    fn dialog_roll(&mut self, instance: u32, req: &RollRequest) -> RollOutcome {
        let forced = self
            .dialog_mut(instance)
            .and_then(|i| i.script.rolls.pop_front());
        let roller = req.roller.as_ref().map(from_lsf);
        // A roller the sim doesn't know rolls with neutral stats.
        let entity = roller
            .and_then(|g| self.entity_id(g))
            .unwrap_or(EntityId(u32::MAX));
        // Missing difficulty class: Act1_Medium's 10 (*fallback*, never needed in the
        // reference build's opening).
        let dc = req
            .roll
            .difficulty_class
            .as_ref()
            .and_then(|d| self.data.difficulty_classes.get(&from_lsf(d)).copied())
            .unwrap_or(10);
        let advantage = Advantage {
            advantage: req.roll.advantage == 1,
            disadvantage: req.roll.advantage == 2,
        };
        let check = check_of(&req.roll);
        let mut dice = std::mem::take(&mut self.dice);
        let (check, success, consistent) = match (check, forced) {
            (Some((ability, kind)), None) => {
                let c = roll_ability_check(self, &mut dice, entity, ability, kind, dc, advantage);
                let s = c.success;
                (Some(c), s, true)
            }
            (Some((ability, kind)), Some(want)) => {
                let fitting = (2..=19).find_map(|n| {
                    let c = roll_ability_check(
                        self,
                        &mut FixedDie(n),
                        entity,
                        ability,
                        kind,
                        dc,
                        advantage,
                    );
                    (c.success == want).then_some(c)
                });
                let ok = fitting.is_some();
                (fitting, want, ok)
            }
            (None, forced) => (None, forced.unwrap_or(false), forced.is_some()),
        };
        self.dice = dice;
        let critical = match check.as_ref().map(|c| c.critical) {
            Some(Critical::Success) => Some(true),
            Some(Critical::Fail) => Some(false),
            _ => None,
        };
        let record = DialogRollRecord {
            node: from_lsf(&req.node),
            roller,
            active: !req.passive,
            check,
            success,
            forced: forced.is_some(),
            consistent,
        };
        if let Some(i) = self.dialog_mut(instance) {
            i.rolls.push(record.clone());
        }
        self.notifications.push(Notification::DialogRoll {
            instance,
            roll: record,
        });
        RollOutcome { success, critical }
    }

    // ---- dialogs the sim did not start -----------------------------------------------------

    /// Follow a dialog event the engine raised that the sim did not produce (a world dialog
    /// the engine started itself, or any dialog when the sim has no dialog bank): remember
    /// the instance and its actors so ids and the involved-actor queries stay right.
    pub(crate) fn observe_dialog_event(&mut self, name: &str, texts: &[&str]) {
        let int = |i: usize| texts.get(i).and_then(|t| t.parse::<u32>().ok());
        match name {
            "DialogStarted" | "AutomatedDialogStarted" => {
                let (Some(dialog), Some(id)) = (texts.first(), int(1)) else {
                    return;
                };
                self.dialogs.last_id = self.dialogs.last_id.max(id);
                if self.dialog(id).is_some() {
                    return;
                }
                self.dialogs.instances.push(DialogInstance {
                    id,
                    resource: Guid::from_osiris(dialog).unwrap_or(Guid::NULL),
                    name: (*dialog).to_owned(),
                    speakers: Vec::new(),
                    state: InstanceState::Running,
                    owned: false,
                    script: DialogScript::default(),
                    rolls: Vec::new(),
                    runner: None,
                });
            }
            "DialogActorJoined" => {
                let (Some(id), Some(actor), Some(slot)) = (
                    int(1),
                    texts.get(2).and_then(|t| Guid::from_osiris(t)),
                    int(3),
                ) else {
                    return;
                };
                if let Some(inst) = self.dialog_mut(id).filter(|i| !i.owned) {
                    let slot = slot as usize;
                    if inst.speakers.len() <= slot {
                        inst.speakers.resize(slot + 1, None);
                    }
                    inst.speakers[slot] = Some(actor);
                }
            }
            "DialogEnded" | "AutomatedDialogEnded" => {
                if let Some(inst) = int(1)
                    .and_then(|id| self.dialog_mut(id))
                    .filter(|i| !i.owned)
                {
                    inst.state = InstanceState::Ended;
                }
            }
            _ => {}
        }
    }
}
