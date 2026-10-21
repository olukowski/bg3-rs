//! A headless dialog runner: the original's node-selection rules as a step-by-step state
//! machine.
//!
//! ```text
//! start ─▶ start node ─▶ Line ──advance──▶ next node ─▶ … ─▶ Ended
//!                          │                  ▲
//!                          └▶ Choices ─choose─┘
//! ```
//!
//! Behaviour follows `LSDialogs::DefaultDialogNodeLogic` and friends in the reference build
//! (4.1.1.7631656); every rule cites its symbol. Details and open questions:
//! `docs/formats/dialog.md` ("Runtime behaviour").

use std::collections::HashSet;
use std::sync::Arc;

use bg3_lsf::Guid;

use crate::library::DialogProvider;
use crate::model::{
    Dialog, DialogRef, Flag, FlagGroup, FlagKind, NodeId, NodeKind, RuleGroup, TagRule, TextLine,
};
use crate::world::{ActorId, DialogWorld, FlagType, RollOutcome, RollRequest};
use crate::{Error, Result};

/// `GROUP_Players` speaker group.
const GROUP_PLAYERS: &str = "e0d1ff71-04a8-4340-ae64-9684d846eb83";
/// `GROUP_PartyMembers` speaker group.
const GROUP_PARTY_MEMBERS: &str = "02cdc15e-1dc3-5eb4-f73c-9458b4f90b9f";
/// Third fallback group of `Dialog::GetDefaultSpeakerIndex`.
const GROUP_DEFAULT_SPEAKER_3: &str = "e6b3c2c4-e88d-e9e6-ffa1-d49cdfdad411";

/// How a dialog is started.
#[derive(Debug, Clone, Default)]
pub struct StartParams {
    /// Dialog instance id (the game numbers instances from 1; it's the second argument of
    /// `DialogStarted` and the third of dialog-raised `FlagSet` events).
    pub instance: u32,
    /// Actor per speaker slot (index = `speakerlist` index). Missing/`None` slots make nodes
    /// spoken by that slot invalid.
    pub speakers: Vec<Option<ActorId>>,
}

/// What the runner is waiting for.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// A node is playing (a line of text, or a cinematic/staging node without text). Call
    /// [`DialogRunner::advance`] when it's done.
    Line(Line),
    /// The player must pick one of these. Call [`DialogRunner::choose`].
    Choices(Vec<Choice>),
    /// The dialog is over.
    Ended,
}

/// A node being played.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// The dialog the node belongs to (differs from the started one inside nested dialogs).
    pub dialog: DialogRef,
    pub node: NodeId,
    pub node_uuid: Guid,
    /// Constructor of the played node (an alias reports its source's).
    pub constructor: String,
    /// Speaker slot (`-666` narrator, `-1` none).
    pub speaker: Option<i32>,
    /// The speaker's actor, if the slot is filled.
    pub actor: Option<ActorId>,
    /// The chosen text line (loca handle), if the node has one.
    pub text: Option<TextLine>,
}

/// One option offered to the player.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub node: NodeId,
    pub node_uuid: Guid,
    pub constructor: String,
    pub speaker: Option<i32>,
    pub text: Option<TextLine>,
    /// The roll this option starts (`ActiveRoll`), if any.
    pub roll: Option<crate::model::Roll>,
    /// `false` if the option is shown but can't be picked (its `ValidatedFlags` fail).
    pub enabled: bool,
}

/// Something the runner did that the game reports to the outside, in the order the game
/// raises it. Most map 1:1 to Osiris events; see [`DialogEvent::osiris`].
// bg3: esv::StoryDialogEventManager::{OnDialogStarted, OnDialogEnded, OnActorJoined,
//      OnActorLeft, OnInstanceDialogChanged, OnFlagSetEvent} (4.1.1.7631656)
#[derive(Debug, Clone, PartialEq)]
pub enum DialogEvent {
    /// `DialogStarted(dialog, instance)`, or `AutomatedDialogStarted` for automated, world and
    /// behaviour dialogs.
    Started {
        dialog: DialogRef,
        instance: u32,
        automated: bool,
    },
    /// `DialogActorJoined(dialog, instance, actor, speaker index)`, once per filled slot.
    ActorJoined {
        dialog: DialogRef,
        instance: u32,
        actor: ActorId,
        speaker: i32,
    },
    /// `DialogActorLeft(dialog, instance, actor, instance ended)`.
    ActorLeft {
        dialog: DialogRef,
        instance: u32,
        actor: ActorId,
        instance_ended: bool,
    },
    /// `NestedDialogPlayed(dialog, instance)`, raised at the end for each nested dialog that
    /// played.
    NestedDialogPlayed { dialog: DialogRef, instance: u32 },
    /// `DialogEnded(root dialog, instance)` / `AutomatedDialogEnded`.
    Ended {
        dialog: DialogRef,
        instance: u32,
        automated: bool,
    },
    /// `InstanceDialogChanged(instance, old, new, old dialog stopping)`: entering or leaving a
    /// nested dialog.
    InstanceDialogChanged {
        instance: u32,
        old: DialogRef,
        new: DialogRef,
        old_stopping: bool,
    },
    /// `FlagSet(flag, target or NULL, instance)`.
    FlagSet {
        flag: Guid,
        target: Option<ActorId>,
        instance: u32,
    },
    /// `FlagCleared(flag, target or NULL, instance)`.
    FlagCleared {
        flag: Guid,
        target: Option<ActorId>,
        instance: u32,
    },
    /// A dialog tag was added to / removed from a speaker by a `Tag` set-flag (no Osiris
    /// event of its own).
    TagChanged {
        target: ActorId,
        tag: Guid,
        value: bool,
    },
    /// A node started (after its set-flags were applied). Not an Osiris event.
    NodeStarted { dialog: DialogRef, node: Guid },
    /// A roll node rolled: `RollResult("<node uuid>_<instance>", roller, target, success,
    /// is active, criticality)` from the roll system, and for active rolls also
    /// `DialogRollResult(roller, success, dialog, is detect thoughts, criticality)`.
    Roll {
        dialog: DialogRef,
        instance: u32,
        node: Guid,
        roller: Option<ActorId>,
        target: Option<ActorId>,
        active: bool,
        outcome: RollOutcome,
    },
    /// A `Trade` node asked to open the trade screen. Not an Osiris event.
    TradeRequested {
        dialog: DialogRef,
        node: Guid,
        mode: Option<u8>,
    },
}

impl DialogEvent {
    /// The Osiris events this corresponds to, formatted like the engine's runtime log
    /// (`Name(arg, …)`). `name` maps flag/actor UUIDs to their Osiris strings (`Name_uuid`).
    pub fn osiris(&self, name: &dyn Fn(&Guid) -> String) -> Vec<String> {
        let obj = |a: &Option<ActorId>| match a {
            Some(a) => name(a),
            None => "NULL_00000000-0000-0000-0000-000000000000".to_owned(),
        };
        // The story's `CRITICALITYTYPE` enum: NONE 0, SUCCESS 1, FAIL 2.
        let crit = |o: &RollOutcome| match o.critical {
            None => "CRITICALITYTYPE.NONE",
            Some(true) => "CRITICALITYTYPE.SUCCESS",
            Some(false) => "CRITICALITYTYPE.FAIL",
        };
        let started = |automated: bool| {
            if automated {
                "AutomatedDialogStarted"
            } else {
                "DialogStarted"
            }
        };
        let ended = |automated: bool| {
            if automated {
                "AutomatedDialogEnded"
            } else {
                "DialogEnded"
            }
        };
        match self {
            DialogEvent::Started {
                dialog,
                instance,
                automated,
            } => vec![format!("{}({dialog}, {instance})", started(*automated))],
            DialogEvent::Ended {
                dialog,
                instance,
                automated,
            } => vec![format!("{}({dialog}, {instance})", ended(*automated))],
            DialogEvent::ActorJoined {
                dialog,
                instance,
                actor,
                speaker,
            } => vec![format!(
                "DialogActorJoined({dialog}, {instance}, {}, {speaker})",
                name(actor)
            )],
            DialogEvent::ActorLeft {
                dialog,
                instance,
                actor,
                instance_ended,
            } => vec![format!(
                "DialogActorLeft({dialog}, {instance}, {}, {})",
                name(actor),
                *instance_ended as u8
            )],
            DialogEvent::NestedDialogPlayed { dialog, instance } => {
                vec![format!("NestedDialogPlayed({dialog}, {instance})")]
            }
            DialogEvent::InstanceDialogChanged {
                instance,
                old,
                new,
                old_stopping,
            } => vec![format!(
                "InstanceDialogChanged({instance}, {old}, {new}, {})",
                *old_stopping as u8
            )],
            DialogEvent::FlagSet {
                flag,
                target,
                instance,
            } => vec![format!(
                "FlagSet({}, {}, {instance})",
                name(flag),
                obj(target)
            )],
            DialogEvent::FlagCleared {
                flag,
                target,
                instance,
            } => vec![format!(
                "FlagCleared({}, {}, {instance})",
                name(flag),
                obj(target)
            )],
            DialogEvent::Roll {
                dialog,
                instance,
                node,
                roller,
                target,
                active,
                outcome,
            } => {
                let mut v = vec![format!(
                    "RollResult(\"{node}_{instance}\", {}, {}, {}, {}, {})",
                    obj(roller),
                    obj(target),
                    outcome.success as u8,
                    *active as u8,
                    crit(outcome)
                )];
                if *active {
                    v.push(format!(
                        "DialogRollResult({}, {}, {dialog}, 0, {})",
                        obj(roller),
                        outcome.success as u8,
                        crit(outcome)
                    ));
                }
                v
            }
            DialogEvent::TagChanged { .. }
            | DialogEvent::NodeStarted { .. }
            | DialogEvent::TradeRequested { .. } => Vec::new(),
        }
    }
}

/// One dialog on the nesting stack.
#[derive(Debug, Clone)]
struct Frame {
    dialog: Arc<Dialog>,
    /// Actor per speaker slot of this dialog.
    speakers: Vec<Option<ActorId>>,
    /// The `Nested Dialog` node in the parent frame that started this one.
    return_node: Option<NodeId>,
    /// Hubs that presented player choices ("node selection levels"), most recent last.
    selection_levels: Vec<NodeId>,
}

/// The runner. Create with [`DialogRunner::new`], then call [`DialogRunner::start`].
#[derive(Clone)]
pub struct DialogRunner {
    provider: Arc<dyn DialogProvider + Send + Sync>,
    instance: u32,
    frames: Vec<Frame>,
    /// UUIDs of every node started in this instance (`ShowOnce`).
    history: HashSet<Guid>,
    current: Option<NodeId>,
    step: Step,
    events: Vec<DialogEvent>,
    /// Nested dialogs entered, in order (reported as `NestedDialogPlayed` at the end).
    nested_played: Vec<DialogRef>,
    ended: bool,
    /// Result of the last roll, read by `RollResult` children.
    last_roll: Option<bool>,
    /// Application order of set-flag types (see [`FlagSlot`]).
    flag_group_order: Vec<FlagSlot>,
    /// Result of the last question's `ValidatedFlags`, read by `FallibleQuestionResult`.
    last_validation: Option<bool>,
    started: bool,
}

impl std::fmt::Debug for DialogRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DialogRunner")
            .field("instance", &self.instance)
            .field("depth", &self.frames.len())
            .field("current", &self.current)
            .field("step", &self.step)
            .finish_non_exhaustive()
    }
}

/// Outcome of starting a node.
enum Started {
    /// The node plays and waits.
    Wait,
    /// The node finished immediately; step on (with "ended" = true).
    PassThrough,
    /// The node changed the frame stack and already selected what's next.
    Handled,
}

impl DialogRunner {
    /// A runner for `dialog`. Nested dialogs are looked up in `provider`.
    pub fn new(
        dialog: Arc<Dialog>,
        provider: Arc<dyn DialogProvider + Send + Sync>,
        params: StartParams,
    ) -> Self {
        DialogRunner {
            provider,
            instance: params.instance,
            frames: vec![Frame {
                dialog,
                speakers: params.speakers,
                return_node: None,
                selection_levels: Vec::new(),
            }],
            history: HashSet::new(),
            current: None,
            step: Step::Ended,
            events: Vec::new(),
            nested_played: Vec::new(),
            ended: false,
            last_roll: None,
            last_validation: None,
            flag_group_order: FlagSlot::OBSERVED_ORDER.to_vec(),
            started: false,
        }
    }

    /// Shorthand: a runner with every speaker slot filled by a distinct placeholder actor
    /// (handy for smoke tests and tools without a game world).
    pub fn with_placeholder_speakers(dialog: Arc<Dialog>, instance: u32) -> Self {
        let speakers = placeholder_speakers(&dialog);
        Self::new(dialog, Arc::new(()), StartParams { instance, speakers })
    }

    /// Sets the order in which a node's set-flags are applied, by resolved type (default:
    /// [`FlagSlot::OBSERVED_ORDER`]).
    pub fn set_flag_group_order(&mut self, order: Vec<FlagSlot>) {
        self.flag_group_order = order;
    }

    /// Replaces the nested-dialog provider.
    pub fn set_provider(&mut self, provider: Arc<dyn DialogProvider + Send + Sync>) {
        self.provider = provider;
    }

    /// The dialog that was started.
    pub fn root_dialog(&self) -> &Arc<Dialog> {
        &self.frames[0].dialog
    }

    /// The dialog currently playing (a nested one while inside it).
    pub fn current_dialog(&self) -> &Arc<Dialog> {
        &self.frame().dialog
    }

    /// The node currently playing or presenting choices.
    pub fn current_node(&self) -> Option<NodeId> {
        self.current
    }

    /// Actor per speaker slot of the current dialog.
    pub fn speakers(&self) -> &[Option<ActorId>] {
        &self.frame().speakers
    }

    pub fn instance(&self) -> u32 {
        self.instance
    }

    /// What the runner is waiting for.
    pub fn step(&self) -> &Step {
        &self.step
    }

    pub fn is_ended(&self) -> bool {
        self.ended
    }

    /// Takes the events produced so far.
    pub fn take_events(&mut self) -> Vec<DialogEvent> {
        std::mem::take(&mut self.events)
    }

    /// Starts the dialog: raises `Started`, then selects the start node.
    // bg3: LSDialogs::DefaultDialogNodeLogic::SelectStartNode (4.1.1.7631656)
    pub fn start(&mut self, world: &mut dyn DialogWorld) -> &Step {
        if self.started {
            return &self.step;
        }
        self.started = true;
        let dialog = self.root_dialog().clone();
        let r = dialog.reference();
        self.events.push(DialogEvent::Started {
            dialog: r.clone(),
            instance: self.instance,
            automated: dialog.is_automated(),
        });
        // Actors join in slot order (the trace shows them right after the start event).
        for (slot, actor) in self.frames[0].speakers.clone().into_iter().enumerate() {
            if let Some(actor) = actor {
                self.events.push(DialogEvent::ActorJoined {
                    dialog: r.clone(),
                    instance: self.instance,
                    actor,
                    speaker: slot as i32,
                });
            }
        }
        let roots: Vec<NodeId> = dialog.roots.clone();
        if !self.select_among(world, None, &roots) {
            self.end();
        }
        &self.step
    }

    /// Continues after a [`Step::Line`].
    pub fn advance(&mut self, world: &mut dyn DialogWorld) -> Result<&Step> {
        match self.step {
            Step::Line(_) => {}
            _ => {
                return Err(Error::InvalidOperation(
                    "advance() without a line playing".into(),
                ));
            }
        }
        let node = self.current.expect("a line has a current node");
        self.step_instance(world, node, true);
        Ok(&self.step)
    }

    /// Picks option `index` of a [`Step::Choices`].
    // bg3: LSDialogs::DefaultDialogNodeLogic::SelectOption (4.1.1.7631656)
    pub fn choose(&mut self, world: &mut dyn DialogWorld, index: usize) -> Result<&Step> {
        let node = match &self.step {
            Step::Choices(c) => {
                c.get(index)
                    .filter(|c| c.enabled)
                    .ok_or_else(|| Error::InvalidOperation(format!("no enabled choice {index}")))?
                    .node
            }
            _ => {
                return Err(Error::InvalidOperation(
                    "choose() without choices on offer".into(),
                ));
            }
        };
        self.start_node(world, node);
        Ok(&self.step)
    }

    /// Stops the instance from outside (`DialogRequestStop` and friends): actors leave and
    /// the dialog ends, as when it runs out of nodes. Does nothing once ended.
    // bg3: esv::OsirisDialogFunctions::DialogRequestStop (4.1.1.7631656)
    pub fn stop(&mut self) {
        if !self.ended {
            self.started = true;
            self.end();
        }
    }

    // ---- frames -----------------------------------------------------------------------

    fn frame(&self) -> &Frame {
        self.frames.last().expect("at least one frame")
    }

    fn frame_mut(&mut self) -> &mut Frame {
        self.frames.last_mut().expect("at least one frame")
    }

    fn dialog(&self) -> Arc<Dialog> {
        self.frame().dialog.clone()
    }

    /// Whether choices are disabled: automated, world and behaviour dialogs never wait for
    /// input.
    // bg3: LSDialogs::DefaultDialogNodeLogic::SelectNextDialogNode, PickNodeToStart
    fn input_disabled(&self) -> bool {
        self.frames[0].dialog.is_automated()
    }

    fn actor(&self, slot: i32) -> Option<ActorId> {
        usize::try_from(slot)
            .ok()
            .and_then(|i| self.frame().speakers.get(i).copied().flatten())
    }

    // ---- node selection ---------------------------------------------------------------

    /// Whether a node waits for player input (questions and active rolls; aliases forward to
    /// their source).
    // bg3: LSDialogs::TaggedQuestionNode::NeedsUserInput, LSDialogs::AliasNode::NeedsUserInput
    fn needs_user_input(&self, d: &Dialog, id: NodeId) -> bool {
        if self.input_disabled() {
            return false;
        }
        match &d.nodes[id].kind {
            NodeKind::Question | NodeKind::ActiveRoll(_) => true,
            NodeKind::Alias { source: Some(s) } => {
                matches!(
                    d.nodes[*s].kind,
                    NodeKind::Question | NodeKind::ActiveRoll(_)
                )
            }
            _ => false,
        }
    }

    /// Expands `Jump` nodes into their targets (point 1: the target itself, point 2: the
    /// target's children), recursively and without duplicates.
    // bg3: LSDialogs::DefaultDialogNodeLogic::ResolveReferenceNodes / DoReferenceNodeResolve,
    //      LSDialogs::JumpNode::GetReferencedNodes (4.1.1.7631656)
    fn resolve_references(d: &Dialog, nodes: &[NodeId]) -> Vec<NodeId> {
        fn go(d: &Dialog, id: NodeId, seen: &mut HashSet<NodeId>, out: &mut Vec<NodeId>) {
            match d.nodes[id].kind {
                NodeKind::Jump {
                    target,
                    target_point,
                } => {
                    if !seen.insert(id) {
                        return;
                    }
                    match (target, target_point) {
                        (Some(t), 1) => go(d, t, seen, out),
                        (Some(t), 2) => {
                            for &c in &d.nodes[t].children {
                                go(d, c, seen, out);
                            }
                        }
                        _ => {}
                    }
                }
                _ => {
                    if !out.contains(&id) {
                        out.push(id);
                    }
                }
            }
        }
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for &n in nodes {
            go(d, n, &mut seen, &mut out);
        }
        out
    }

    /// Evaluates candidate nodes in order and plays/presents the winners. Returns false if
    /// nothing could be selected (the caller ends or returns).
    // bg3: LSDialogs::DefaultDialogNodeLogic::GetPossibleStartNodes / SelectNextDialogNode /
    //      PickNodeToStart (4.1.1.7631656)
    fn select_among(
        &mut self,
        world: &mut dyn DialogWorld,
        from: Option<NodeId>,
        nodes: &[NodeId],
    ) -> bool {
        let d = self.dialog();
        let candidates = Self::resolve_references(&d, nodes);
        let mut picked: Vec<NodeId> = Vec::new();
        let mut player = false;
        let mut exclusive = false;
        let mut fallback: Vec<NodeId> = Vec::new();
        for id in candidates {
            if !self.is_valid(world, &d, id) {
                if d.nodes[id].optional {
                    fallback.push(id);
                }
                continue;
            }
            let input = self.needs_user_input(&d, id);
            if player && !input {
                // Player options beat NPC/automatic nodes.
                continue;
            }
            if input && !player {
                player = true;
                picked.clear();
                exclusive = false;
            }
            let is_exclusive = d.nodes[id].exclusive;
            if exclusive && !is_exclusive {
                continue;
            }
            if is_exclusive && !exclusive {
                exclusive = true;
                picked.clear();
            }
            picked.push(id);
        }

        if picked.is_empty() {
            // Optional nodes that failed their checks are passed through: the first one with
            // children becomes current and its children are evaluated.
            if let Some(&f) = fallback.iter().find(|&&f| !d.nodes[f].children.is_empty()) {
                self.set_current(f);
                let children = d.nodes[f].children.clone();
                return self.select_among(world, Some(f), &children);
            }
            return false;
        }

        if player {
            if let Some(hub) = from {
                self.frame_mut().selection_levels.push(hub);
            }
            self.present(world, &d, &picked);
        } else {
            self.start_node(world, picked[0]);
        }
        true
    }

    fn present(&mut self, world: &mut dyn DialogWorld, d: &Dialog, options: &[NodeId]) {
        let choices = options
            .iter()
            .map(|&id| {
                let source = alias_source(d, id);
                let n = &d.nodes[source];
                Choice {
                    node: id,
                    node_uuid: d.nodes[id].uuid,
                    constructor: n.kind.constructor().to_owned(),
                    speaker: n.speaker,
                    text: self.pick_text(world, d, source),
                    roll: match &n.kind {
                        NodeKind::ActiveRoll(r) => Some(r.clone()),
                        _ => None,
                    },
                    enabled: self.validate_input(world, d, source),
                }
            })
            .collect();
        self.step = Step::Choices(choices);
    }

    /// Whether a presented option can be picked (options failing their `ValidatedFlags` are
    /// shown but disabled, unless the question branches on the outcome).
    // bg3: LSDialogs::TaggedQuestionNode::ValidateInput, eoc::ActiveRollNode::ValidateInput
    fn validate_input(&self, world: &dyn DialogWorld, d: &Dialog, node: NodeId) -> bool {
        let n = &d.nodes[node];
        let Some(v) = &n.validated_flags else {
            return true;
        };
        if !v.has_value {
            return true;
        }
        let fallible =
            |c: &NodeId| matches!(d.nodes[*c].kind, NodeKind::FallibleQuestionResult { .. });
        let branches_on_failure = n.children.iter().any(|c| {
            matches!(
                d.nodes[*c].kind,
                NodeKind::FallibleQuestionResult { result: false }
            )
        });
        let handled = match n.kind {
            NodeKind::ActiveRoll(_) => branches_on_failure,
            _ => n.children.len() == 2 && n.children.iter().all(fallible) && branches_on_failure,
        };
        handled || self.evaluate_flags(world, d, &v.groups)
    }

    /// After `node` finished (or was passed through): follow its children, return to a hub,
    /// leave a nested dialog, or end.
    // bg3: LSDialogs::DefaultDialogNodeLogic::StepInstance (4.1.1.7631656)
    fn step_instance(&mut self, world: &mut dyn DialogWorld, node: NodeId, ended: bool) {
        let d = self.dialog();
        let n = &d.nodes[node];
        if ended && n.end_node {
            if n.end_all_dialogs || self.frames.len() == 1 {
                self.end();
            } else {
                self.leave_nested(world);
            }
            return;
        }
        let children = n.children.clone();
        if self.select_among(world, Some(node), &children) {
            return;
        }
        // Nothing to play: go back to the most recent hub and offer its choices again.
        if let Some(hub) = self.frame_mut().selection_levels.pop() {
            self.set_current(hub);
            let children = d.nodes[hub].children.clone();
            if self.select_among(world, Some(hub), &children) {
                return;
            }
            // The hub has nothing left either: keep unwinding.
            return self.step_instance(world, hub, false);
        }
        if self.frames.len() > 1 {
            self.leave_nested(world);
        } else {
            self.end();
        }
    }

    fn set_current(&mut self, node: NodeId) {
        // bg3: LSDialogs::DialogInstance::SetCurrentNode — appends to the visited history.
        let uuid = self.frame().dialog.nodes[node].uuid;
        self.history.insert(uuid);
        self.current = Some(node);
    }

    /// Starts a node: history, set-flags, then the node type's behaviour.
    // bg3: LSDialogs::DefaultDialogNodeLogic::StartDialogNode, LSDialogs::DialogNode::Start
    fn start_node(&mut self, world: &mut dyn DialogWorld, node: NodeId) {
        let d = self.dialog();
        self.set_current(node);
        let n = &d.nodes[node];
        // RollResult / FallibleQuestionResult never push their set-flags.
        // bg3: eoc::RollResultNode::Start, LSDialogs::FallibleQuestionResultNode::Start
        if !matches!(
            n.kind,
            NodeKind::RollResult { .. } | NodeKind::FallibleQuestionResult { .. }
        ) {
            let groups = n.set_flags.clone();
            self.push_flags(world, &d, &groups);
        }
        self.events.push(DialogEvent::NodeStarted {
            dialog: d.reference(),
            node: n.uuid,
        });
        match self.run_node(world, &d, node) {
            Started::Wait => {}
            Started::PassThrough => self.step_instance(world, node, true),
            Started::Handled => {}
        }
    }

    fn run_node(&mut self, world: &mut dyn DialogWorld, d: &Arc<Dialog>, node: NodeId) -> Started {
        let source = alias_source(d, node);
        let n = &d.nodes[source];
        match &n.kind {
            NodeKind::Greeting | NodeKind::Answer | NodeKind::Cinematic { .. } => {
                // A tagged node without a selected text line is done at once.
                // bg3: LSDialogs::BaseTaggedNode::Start
                if !n.texts.is_empty() && self.matching_text(world, d, source).is_none() {
                    return Started::PassThrough;
                }
                let line = self.line(world, d, node);
                self.step = Step::Line(line);
                Started::Wait
            }
            NodeKind::Question => {
                // The chosen option isn't replayed as a line.
                self.validate_question(world, d, source);
                Started::PassThrough
            }
            NodeKind::ActiveRoll(r) | NodeKind::PassiveRoll(r) => {
                let active = matches!(n.kind, NodeKind::ActiveRoll(_));
                if active {
                    // bg3: esv::ActiveRollNode::Start runs TaggedQuestionNode::Start first.
                    self.validate_question(world, d, source);
                }
                let request = RollRequest {
                    dialog: d.reference(),
                    node: d.nodes[node].uuid,
                    passive: !active,
                    roller: n.speaker.and_then(|s| self.actor(s)),
                    target: r.target_speaker.and_then(|s| self.actor(s)),
                    roll: r.clone(),
                };
                let outcome = world.roll(&request);
                self.last_roll = Some(outcome.success);
                self.events.push(DialogEvent::Roll {
                    dialog: request.dialog.clone(),
                    instance: self.instance,
                    node: request.node,
                    roller: request.roller,
                    target: request.target,
                    active,
                    outcome,
                });
                Started::PassThrough
            }
            NodeKind::Pop { level } => {
                // bg3: esv::PopNode::Start → DialogInstance::PopNodeSelectionLevels
                if *level >= 1 {
                    let f = self.frame_mut();
                    let keep = f.selection_levels.len().saturating_sub(*level as usize);
                    f.selection_levels.truncate(keep);
                }
                Started::PassThrough
            }
            NodeKind::NestedDialog {
                dialog,
                speaker_linking,
            } => {
                let Some(nested) = self.provider.dialog_by_resource(dialog) else {
                    // Unknown nested dialog: continue with this node's children.
                    return Started::PassThrough;
                };
                self.enter_nested(world, node, nested, speaker_linking.clone());
                Started::Handled
            }
            NodeKind::Trade { mode, .. } => {
                self.events.push(DialogEvent::TradeRequested {
                    dialog: d.reference(),
                    node: d.nodes[node].uuid,
                    mode: *mode,
                });
                Started::PassThrough
            }
            NodeKind::Jump { .. }
            | NodeKind::Alias { .. }
            | NodeKind::VisualState
            | NodeKind::RollResult { .. }
            | NodeKind::FallibleQuestionResult { .. }
            | NodeKind::Other(_) => Started::PassThrough,
        }
    }

    /// Records the outcome of a question's `ValidatedFlags` (true without them) for
    /// `FallibleQuestionResult` children.
    // bg3: LSDialogs::TaggedQuestionNode::Start (4.1.1.7631656)
    fn validate_question(&mut self, world: &dyn DialogWorld, d: &Dialog, node: NodeId) {
        let outcome = match &d.nodes[node].validated_flags {
            Some(v) if v.has_value => self.evaluate_flags(world, d, &v.groups),
            _ => true,
        };
        self.last_validation = Some(outcome);
    }

    fn line(&self, world: &mut dyn DialogWorld, d: &Dialog, node: NodeId) -> Line {
        let source = alias_source(d, node);
        let n = &d.nodes[source];
        Line {
            dialog: d.reference(),
            node,
            node_uuid: d.nodes[node].uuid,
            constructor: n.kind.constructor().to_owned(),
            speaker: n.speaker,
            actor: n.speaker.and_then(|s| self.actor(s)),
            text: self.pick_text(world, d, source),
        }
    }

    // ---- nested dialogs ---------------------------------------------------------------

    /// Switches the instance to a nested dialog and starts it from its roots.
    // bg3: LSDialogs::NestedDialogNode::Start, LSDialogs::DialogManager::SetInstanceDialog,
    //      LSDialogs::DialogInstance::ApplySpeakerLinking (4.1.1.7631656)
    fn enter_nested(
        &mut self,
        world: &mut dyn DialogWorld,
        node: NodeId,
        nested: Arc<Dialog>,
        linking: Vec<(i32, i32)>,
    ) {
        // Speaker linking rebuilds the actor list from a snapshot: actors[Key] = old[Value];
        // a slot that is some entry's Value but no entry's Key is cleared.
        let old = self.frame().speakers.clone();
        let mut speakers = old.clone();
        let slots = nested
            .speakers
            .iter()
            .map(|s| s.index + 1)
            .max()
            .unwrap_or(0)
            .max(0) as usize;
        if speakers.len() < slots {
            speakers.resize(slots, None);
        }
        for &(_, value) in &linking {
            if !linking.iter().any(|&(k, _)| k == value)
                && let Some(slot) = usize::try_from(value)
                    .ok()
                    .and_then(|v| speakers.get_mut(v))
            {
                *slot = None;
            }
        }
        for &(key, value) in &linking {
            let Ok(k) = usize::try_from(key) else {
                continue;
            };
            if k >= speakers.len() {
                speakers.resize(k + 1, None);
            }
            speakers[k] = usize::try_from(value)
                .ok()
                .and_then(|v| old.get(v).copied().flatten());
        }
        let parent = self.dialog().reference();
        self.events.push(DialogEvent::InstanceDialogChanged {
            instance: self.instance,
            old: parent,
            new: nested.reference(),
            old_stopping: false,
        });
        self.nested_played.push(nested.reference());
        self.frames.push(Frame {
            dialog: nested.clone(),
            speakers,
            return_node: Some(node),
            selection_levels: Vec::new(),
        });
        let roots = nested.roots.clone();
        if !self.select_among(world, None, &roots) {
            self.leave_nested(world);
        }
    }

    /// Leaves a nested dialog that reached an end node (or ran out of nodes): pops back to the
    /// parent, further while the `Nested Dialog` node returned to is itself an end node, and
    /// continues with that node's children.
    // bg3: LSDialogs::DialogInstance::PopPlayedNestedDialogs,
    //      LSDialogs::DefaultDialogNodeLogic::StepInstance (4.1.1.7631656)
    fn leave_nested(&mut self, world: &mut dyn DialogWorld) {
        loop {
            let frame = self.frames.pop().expect("nested frame");
            let node = frame.return_node.expect("nested frames have a return node");
            self.events.push(DialogEvent::InstanceDialogChanged {
                instance: self.instance,
                old: frame.dialog.reference(),
                new: self.dialog().reference(),
                old_stopping: true,
            });
            let d = self.dialog();
            let n = &d.nodes[node];
            if n.end_node {
                if n.end_all_dialogs || self.frames.len() == 1 {
                    return self.end();
                }
                continue;
            }
            self.current = Some(node);
            let children = n.children.clone();
            if !self.select_among(world, Some(node), &children) {
                self.end();
            }
            return;
        }
    }

    /// Ends the instance: actors leave (slot order), nested dialogs that played are reported,
    /// then the root dialog ends.
    // bg3: esv::StoryDialogEventManager::OnDialogEnded (4.1.1.7631656)
    fn end(&mut self) {
        if self.step == Step::Ended && self.ended {
            return;
        }
        self.frames.truncate(1);
        let d = self.frames[0].dialog.clone();
        let r = d.reference();
        for actor in self.frames[0].speakers.clone().into_iter().flatten() {
            self.events.push(DialogEvent::ActorLeft {
                dialog: r.clone(),
                instance: self.instance,
                actor,
                instance_ended: true,
            });
        }
        for nested in std::mem::take(&mut self.nested_played) {
            if nested != r {
                self.events.push(DialogEvent::NestedDialogPlayed {
                    dialog: nested,
                    instance: self.instance,
                });
            }
        }
        self.events.push(DialogEvent::Ended {
            dialog: r,
            instance: self.instance,
            automated: d.is_automated(),
        });
        self.ended = true;
        self.step = Step::Ended;
        self.current = None;
    }

    // ---- validity ---------------------------------------------------------------------

    /// Whether a node may be selected.
    // bg3: LSDialogs::DefaultDialogNode::IsValid, LSDialogs::BaseTaggedNode::IsValid,
    //      LSDialogs::AliasNode::IsValid, LSDialogs::DialogNode::IsValid,
    //      eoc::RollResultNode::IsValid (4.1.1.7631656)
    fn is_valid(&self, world: &dyn DialogWorld, d: &Dialog, id: NodeId) -> bool {
        let n = &d.nodes[id];
        match &n.kind {
            // Only the roll outcome counts.
            NodeKind::RollResult { success } => return self.last_roll == Some(*success),
            NodeKind::FallibleQuestionResult { result } => {
                return self.last_validation == Some(*result);
            }
            // Plain DialogNode::IsValid: ShowOnce only (no check-flags).
            NodeKind::Jump { .. } | NodeKind::VisualState => {
                return !(n.show_once && self.history.contains(&n.uuid));
            }
            _ => {}
        }
        // The alias's own check-flags and ShowOnce, the source's speaker and tag rules.
        let source = alias_source(d, id);
        let s = &d.nodes[source];
        if let Some(sp) = s.speaker
            && sp >= 0
            && self.actor(sp).is_none()
            && !matches!(s.kind, NodeKind::Cinematic { .. })
        {
            return false;
        }
        if !self.evaluate_flags(world, d, &n.check_flags) {
            return false;
        }
        if n.show_once && self.history.contains(&n.uuid) {
            return false;
        }
        if !s.texts.is_empty() && self.matching_text(world, d, source).is_none() {
            return false;
        }
        true
    }

    // ---- texts ------------------------------------------------------------------------

    /// The first `TaggedText` whose tag rule holds (one without a rule always holds).
    // bg3: LSDialogs::BaseTaggedNode::OnPresentedForSelection (4.1.1.7631656)
    fn matching_text(&self, world: &dyn DialogWorld, d: &Dialog, node: NodeId) -> Option<usize> {
        d.nodes[node]
            .texts
            .iter()
            .position(|t| !t.has_tag_rule || self.rule_group_holds(world, &t.rule))
    }

    /// The text line a node shows: a random line (`rand() % count` in the game, here
    /// [`DialogWorld::random`]) of the first matching `TaggedText`.
    // bg3: LSDialogs::BaseTaggedNode::OnPresentedForSelection / OnNodeSelected / GetTextLine
    fn pick_text(&self, world: &mut dyn DialogWorld, d: &Dialog, node: NodeId) -> Option<TextLine> {
        let t = &d.nodes[node].texts[self.matching_text(world, d, node)?];
        match t.lines.len() {
            0 => None,
            1 => t.lines.first().cloned(),
            n => t.lines.get(world.random(n)).cloned(),
        }
    }

    /// `RuleGroup`: op 0 = all rules, any other op = any rule; empty = true.
    // bg3: LSDialogs::TagRuleGroup::EvaluateState (4.1.1.7631656)
    fn rule_group_holds(&self, world: &dyn DialogWorld, g: &RuleGroup) -> bool {
        if g.combine == 0 {
            g.rules.iter().all(|r| self.rule_holds(world, r))
        } else {
            g.rules.iter().any(|r| self.rule_holds(world, r))
        }
    }

    /// `Rule`: tags on speaker `speaker`; op 0 = all, 1 = any, 2 = none. No tags = true; an
    /// empty or out-of-range speaker slot fails a rule that has tags. Nested groups
    /// (`HasChildRules`) are ignored by the game.
    // bg3: LSDialogs::TagRule::EvaluateState, LSDialogs::TagRule::HasChildRules
    fn rule_holds(&self, world: &dyn DialogWorld, r: &TagRule) -> bool {
        if r.tags.is_empty() {
            return true;
        }
        let Some(actor) = self.actor(r.speaker) else {
            return false;
        };
        let mut tagged = r.tags.iter().map(|t| world.has_tag(&actor, t));
        match r.combine {
            0 => tagged.all(|x| x),
            1 => tagged.any(|x| x),
            2 => !tagged.any(|x| x),
            _ => false,
        }
    }

    // ---- flags ------------------------------------------------------------------------

    /// The speaker slot the game uses for flags that need a speaker but have none.
    // bg3: LSDialogs::Dialog::GetDefaultSpeakerIndex (4.1.1.7631656)
    fn default_speaker_index(d: &Dialog) -> i32 {
        if let Some(i) = d.options.default_speaker_index
            && i >= 0
        {
            return i;
        }
        let has = |group: &str| {
            let g = Guid::parse(group).expect("constant GUID");
            d.speakers
                .iter()
                .find(|s| s.list.contains(&g))
                .map(|s| s.index)
        };
        has(GROUP_PLAYERS)
            .or_else(|| {
                d.speakers
                    .iter()
                    .find(|s| {
                        [GROUP_PARTY_MEMBERS, GROUP_DEFAULT_SPEAKER_3]
                            .iter()
                            .any(|g| s.list.contains(&Guid::parse(g).unwrap()))
                    })
                    .map(|s| s.index)
            })
            .unwrap_or(-1)
    }

    /// How a flag of a non-tag/script/quest group is stored, and with which speaker slot.
    /// `None` if the flag is dropped (a per-object type without a `paramval`).
    // bg3: LSDialogs::FlagCollection::Visit, LSDialogs::ValueFlagParamInfo<int>::Read
    fn resolve_flag(
        world: &dyn DialogWorld,
        d: &Dialog,
        kind: &FlagKind,
        flag: &Flag,
    ) -> Option<(Option<FlagType>, Option<i32>)> {
        let file_type = match kind {
            FlagKind::Global => Some(FlagType::Global),
            FlagKind::Object => Some(FlagType::Character),
            FlagKind::Local => Some(FlagType::Local),
            FlagKind::Dialog => Some(FlagType::Dialog),
            FlagKind::User => Some(FlagType::User),
            FlagKind::Party => Some(FlagType::Party),
            FlagKind::UserShape => Some(FlagType::UserShape),
            _ => None,
        };
        let resource = world.flag_type(&flag.uuid);
        // Local groups are never retyped; the others take the resource's type.
        let ty = if *kind == FlagKind::Local {
            resource
        } else {
            resource.or(file_type)
        };
        let mut param = flag.param;
        if *kind == FlagKind::Global && resource.is_some_and(|t| t != FlagType::Global) {
            param = Some(Self::default_speaker_index(d));
        }
        if file_type.is_some_and(FlagType::per_object) && param.is_none() {
            return None;
        }
        Some((resource.and(ty), param))
    }

    /// All check-flags hold (AND, stopping at the first failure).
    // bg3: LSDialogs::FlagCollection::EvaluateState, esv::EoCDialogGameInterface::EvaluateFlag
    fn evaluate_flags(&self, world: &dyn DialogWorld, d: &Dialog, groups: &[FlagGroup]) -> bool {
        for g in groups {
            for f in &g.flags {
                let ok = match &g.kind {
                    FlagKind::Tag => match f.param.and_then(|p| self.actor(p)) {
                        Some(a) => world.has_tag(&a, &f.uuid) == f.value,
                        None => !f.value,
                    },
                    FlagKind::Script => {
                        world.script_flag(&f.uuid, &self.frame().speakers) == f.value
                    }
                    FlagKind::Quest => world.quest_step_unlocked(&f.uuid) == f.value,
                    kind => match Self::resolve_flag(world, d, kind, f) {
                        None => true,
                        // No flag resource: the check fails whatever the expected value.
                        Some((None, _)) => false,
                        Some((Some(ty), param)) => {
                            if ty.per_object() {
                                match param.and_then(|p| self.actor(p)) {
                                    Some(a) => world.get_flag(&f.uuid, ty, Some(&a)) == f.value,
                                    None => !f.value,
                                }
                            } else {
                                world.get_flag(&f.uuid, ty, None) == f.value
                            }
                        }
                    },
                };
                if !ok {
                    return false;
                }
            }
        }
        true
    }

    /// Applies set-flags in file order and records the resulting events.
    // bg3: LSDialogs::FlagCollection::PushState, esv::EoCDialogGameInterface::SetFlag,
    //      esv::OsirisVariableHelper::SetIndividualFlag (4.1.1.7631656)
    fn push_flags(&mut self, world: &mut dyn DialogWorld, d: &Dialog, groups: &[FlagGroup]) {
        // The game keeps a node's flags in a hash map keyed by the *resolved* flag type and
        // applies the types in bucket order (runtime-dependent, see `FlagSlot`); flags of one
        // type keep file order.
        let mut entries: Vec<(FlagSlot, &FlagKind, &Flag)> = Vec::new();
        for g in groups {
            for f in &g.flags {
                let slot = match &g.kind {
                    FlagKind::Tag => FlagSlot::Tag,
                    FlagKind::Script => FlagSlot::Script,
                    FlagKind::Quest => FlagSlot::Quest,
                    kind => match Self::resolve_flag(world, d, kind, f) {
                        Some((Some(ty), _)) => FlagSlot::Typed(ty),
                        _ => FlagSlot::Unresolved,
                    },
                };
                entries.push((slot, &g.kind, f));
            }
        }
        let mut slots: Vec<FlagSlot> = Vec::new();
        for (slot, ..) in &entries {
            if !slots.contains(slot) {
                slots.push(*slot);
            }
        }
        // Stable: types missing from the order table keep their first-appearance order.
        slots.sort_by_key(|s| {
            self.flag_group_order
                .iter()
                .position(|o| o == s)
                .unwrap_or(usize::MAX)
        });
        for slot in slots {
            for &(_, kind, f) in entries.iter().filter(|e| e.0 == slot) {
                self.push_flag(world, d, kind, f);
            }
        }
    }

    // bg3: LSDialogs::FlagCollection::PushState, esv::EoCDialogGameInterface::SetFlag,
    //      esv::OsirisVariableHelper::SetIndividualFlag (4.1.1.7631656)
    fn push_flag(&mut self, world: &mut dyn DialogWorld, d: &Dialog, kind: &FlagKind, f: &Flag) {
        match kind {
            FlagKind::Tag => {
                if let Some(a) = f.param.and_then(|p| self.actor(p)) {
                    world.set_dialog_tag(&a, &f.uuid, f.value);
                    self.events.push(DialogEvent::TagChanged {
                        target: a,
                        tag: f.uuid,
                        value: f.value,
                    });
                }
            }
            FlagKind::Script => {}
            FlagKind::Quest => {
                if let Some(a) = f.param.and_then(|p| self.actor(p)) {
                    world.unlock_quest_step(&f.uuid, &a);
                }
            }
            kind => {
                let Some((Some(ty), param)) = Self::resolve_flag(world, d, kind, f) else {
                    return;
                };
                let target = if ty.per_object() {
                    match param.and_then(|p| self.actor(p)) {
                        Some(a) => Some(a),
                        None => return,
                    }
                } else {
                    None
                };
                let changes = world.set_flag(&f.uuid, ty, target.as_ref(), f.value, self.instance);
                if ty.raises_events() {
                    for c in changes {
                        self.events.push(if c.value {
                            DialogEvent::FlagSet {
                                flag: f.uuid,
                                target: c.target,
                                instance: self.instance,
                            }
                        } else {
                            DialogEvent::FlagCleared {
                                flag: f.uuid,
                                target: c.target,
                                instance: self.instance,
                            }
                        });
                    }
                }
            }
        }
    }
}

/// The key a set-flag is filed under in the game's per-node flag map: its resolved type.
///
/// The game applies a node's set-flags type by type, in the order of
/// `(string-table handle of the type name) % 37`, which depends on string interning at run
/// time and can't be derived from the binary (`docs/formats/dialog.md`, "Order of set-flags").
/// [`FlagSlot::OBSERVED_ORDER`] holds what a reference trace showed; types it doesn't list keep
/// their order of first appearance in the node.
// bg3: ls::DEPRECATED_RefMapImpl<…FixedString, ObjectSet<LSDialogs::Flag*>…>::GetOrAdd,
//      LSDialogs::FlagCollection::FlagCollection (37 buckets) (4.1.1.7631656)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FlagSlot {
    Tag,
    Script,
    Quest,
    /// A flag resolved through its resource's `Usage`.
    Typed(FlagType),
    /// No flag resource: the game neither checks nor sets it meaningfully.
    Unresolved,
}

impl FlagSlot {
    /// Order seen in the reference trace (`full-nautiloid.log`, `TUT_Lab_TrappedDevourer`):
    /// `Object` flags before `Global` flags.
    pub const OBSERVED_ORDER: &'static [FlagSlot] = &[
        FlagSlot::Typed(FlagType::Character),
        FlagSlot::Typed(FlagType::Global),
    ];
}

/// An alias's source node (or the node itself).
fn alias_source(d: &Dialog, id: NodeId) -> NodeId {
    match d.nodes[id].kind {
        NodeKind::Alias { source: Some(s) } => s,
        _ => id,
    }
}

/// One distinct placeholder actor per speaker slot (`00000000-0000-0000-0000-0000000000NN`,
/// NN = slot + 1).
pub fn placeholder_speakers(d: &Dialog) -> Vec<Option<ActorId>> {
    let slots = d
        .speakers
        .iter()
        .map(|s| s.index + 1)
        .max()
        .unwrap_or(0)
        .max(0) as usize;
    (0..slots)
        .map(|i| {
            let mut b = [0u8; 16];
            b[12..16].copy_from_slice(&(i as u32 + 1).to_be_bytes());
            Some(Guid(b))
        })
        .collect()
}
