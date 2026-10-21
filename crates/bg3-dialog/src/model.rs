//! In-memory model of a compiled dialog (`Story/DialogsBinary/**/*.lsf`) and the loader from a
//! parsed [`bg3_lsf::Resource`]. Field names follow the resource's attribute names; see
//! `docs/formats/dialog.md`.

use std::collections::HashMap;

use bg3_lsf::{Guid, Node as LsfNode, Resource, Value};

use crate::{Error, Result};

/// Index of a node in [`Dialog::nodes`].
pub type NodeId = usize;

/// Speaker index used by nodes for "no speaker" (`-1`), e.g. aliases, jumps and most cinematics.
pub const SPEAKER_NONE: i32 = -1;
/// Speaker index the editor uses for the narrator (`-666`).
pub const SPEAKER_NARRATOR: i32 = -666;

/// A compiled dialog resource.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Dialog {
    /// The dialog's own UUID (`dialog.UUID`). Nested-dialog nodes refer to dialogs by this id.
    pub uuid: Guid,
    /// The timeline (cinematic) resource that stages this dialog, if any (`TimelineId`).
    pub timeline_id: Option<Guid>,
    /// Editor category, e.g. `Generic NPC Dialog`, `Voice bark`, `Companion Dialog`.
    pub category: String,
    pub options: DialogOptions,
    /// Speaker slots, in `speakerlist` order (normally sorted by `index`).
    pub speakers: Vec<Speaker>,
    /// `DefaultAddressedSpeakers`: speaker slot → slot it addresses by default (`-1` = none).
    pub default_addressed_speakers: Vec<(i32, i32)>,
    /// Root nodes in `RootNodes` order (the order in which start nodes are tried).
    pub roots: Vec<NodeId>,
    /// All nodes, in file order.
    pub nodes: Vec<Node>,
    by_uuid: HashMap<Guid, NodeId>,
}

/// Dialog-level switches (attributes of the `dialog` node). Absent attributes are `false`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DialogOptions {
    /// `automated`: plays without player input (ambient dialog, "AD"); reported to Osiris as
    /// `AutomatedDialogStarted` / `AutomatedDialogEnded`.
    pub automated: bool,
    pub allow_dead_speakers: bool,
    pub is_allowing_join_combat: bool,
    pub is_behaviour: bool,
    pub is_private_dialog: bool,
    pub is_subbed_dialog: bool,
    pub is_subs_anonymous: Option<bool>,
    pub is_world: bool,
    pub is_sfx_dialog: bool,
    /// `DefaultSpeakerIndex`, if set.
    pub default_speaker_index: Option<i32>,
}

/// One entry of the speaker list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Speaker {
    /// Speaker slot index (`index`, stored as a string in the file).
    pub index: i32,
    /// Candidate actors for the slot (`list`, `;`-separated UUIDs; character or tag ids).
    pub list: Vec<Guid>,
    pub mapping_id: Option<Guid>,
    /// `IsPeanutSpeaker`: a minor/background speaker.
    pub is_peanut: bool,
    /// `SpeakerTagsIndex`, if present.
    pub tags_index: Option<String>,
}

/// One dialog node.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub uuid: Guid,
    /// The `constructor` with its type-specific data.
    pub kind: NodeKind,
    /// Speaker slot (`speaker`); `-1` none, `-666` narrator. `None` if the attribute is absent.
    pub speaker: Option<i32>,
    /// `addressedspeaker`, if set.
    pub addressed_speaker: Option<i32>,
    /// `Root`: the editor marked this node as a root.
    pub root: bool,
    /// `Greeting`.
    pub greeting: bool,
    /// `endnode`: the dialog ends after this node.
    pub end_node: bool,
    /// `endAllDialogs`.
    pub end_all_dialogs: bool,
    /// `ShowOnce`: hidden once it has been played.
    pub show_once: bool,
    pub optional: bool,
    pub exclusive: bool,
    pub stub: bool,
    pub suppress_subtitle: bool,
    pub gameplay_node: bool,
    /// `AllowNodeGrouping` when present.
    pub allow_node_grouping: Option<bool>,
    /// `GroupID` / `GroupIndex`: grouped (randomised or sequenced) siblings.
    pub group: Option<NodeGroup>,
    pub transition_mode: Option<u8>,
    pub wait_time: Option<f32>,
    pub approval_rating_id: Option<Guid>,
    pub override_approval_rating_id: bool,
    /// Children in file order (the order the runner evaluates them in).
    pub children: Vec<NodeId>,
    pub check_flags: Vec<FlagGroup>,
    pub set_flags: Vec<FlagGroup>,
    /// `ValidatedFlags` (questions and active rolls): flags checked when the choice is picked.
    pub validated_flags: Option<ValidatedFlags>,
    /// `Tags`: dialog tags on the node (e.g. the visual tag shown next to a choice).
    pub tags: Vec<Guid>,
    /// `TaggedTexts`: the node's lines, each guarded by a tag rule.
    pub texts: Vec<TaggedText>,
    /// Editor notes (`editorData` → `CinematicNodeContext`), when compiled in.
    pub cinematic_context: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeGroup {
    pub id: String,
    pub index: Option<i32>,
}

/// The node `constructor` and its type-specific attributes.
#[derive(Debug, Clone, PartialEq)]
pub enum NodeKind {
    /// `TagGreeting`: an NPC line, typically a root.
    Greeting,
    /// `TagAnswer`: an NPC (or narrator) line.
    Answer,
    /// `TagQuestion`: a player choice.
    Question,
    /// `TagCinematic`: a staged moment without text.
    Cinematic { behaviour_node_type: Option<u8> },
    /// `Alias`: stands in for another node (`SourceNode`).
    Alias { source: Option<NodeId> },
    /// `Jump`: continue at another node (`jumptarget`); `jumptargetpoint` is 1 or 2.
    Jump {
        target: Option<NodeId>,
        target_point: u8,
    },
    /// `Pop`: leave `PopLevel` levels of nested dialogs.
    Pop { level: i32 },
    /// `Nested Dialog`: play another dialog (by its dialog UUID), then continue here.
    NestedDialog {
        dialog: Guid,
        /// `SpeakerLinking`: nested speaker slot (`Key`) → this dialog's slot (`Value`).
        speaker_linking: Vec<(i32, i32)>,
    },
    /// `Visual State`: a staging marker; passes straight through.
    VisualState,
    /// `ActiveRoll`: a player-initiated ability check.
    ActiveRoll(Roll),
    /// `PassiveRoll`: an automatic check.
    PassiveRoll(Roll),
    /// `RollResult`: child of a roll, taken when the roll's outcome equals `success`.
    RollResult { success: bool },
    /// `FallibleQuestionResult`: child of a question whose validation can fail.
    FallibleQuestionResult { result: bool },
    /// `Trade`: opens the trade screen.
    Trade {
        mode: Option<u8>,
        items_tag_filter: Option<String>,
        player_override: Option<i32>,
    },
    /// A constructor this crate doesn't know.
    Other(String),
}

impl NodeKind {
    /// The constructor name as stored in the file.
    pub fn constructor(&self) -> &str {
        match self {
            NodeKind::Greeting => "TagGreeting",
            NodeKind::Answer => "TagAnswer",
            NodeKind::Question => "TagQuestion",
            NodeKind::Cinematic { .. } => "TagCinematic",
            NodeKind::Alias { .. } => "Alias",
            NodeKind::Jump { .. } => "Jump",
            NodeKind::Pop { .. } => "Pop",
            NodeKind::NestedDialog { .. } => "Nested Dialog",
            NodeKind::VisualState => "Visual State",
            NodeKind::ActiveRoll(_) => "ActiveRoll",
            NodeKind::PassiveRoll(_) => "PassiveRoll",
            NodeKind::RollResult { .. } => "RollResult",
            NodeKind::FallibleQuestionResult { .. } => "FallibleQuestionResult",
            NodeKind::Trade { .. } => "Trade",
            NodeKind::Other(s) => s,
        }
    }
}

/// Data of an `ActiveRoll` / `PassiveRoll` node.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Roll {
    /// `Ability` (e.g. `Charisma`; may be empty).
    pub ability: String,
    /// `Skill` (e.g. `Persuasion`; empty for raw ability checks and saves).
    pub skill: String,
    /// `RollType`: `SkillCheck`, `RawAbility`, `SavingThrow`, `Damage`, `Melee…Attack`, or empty.
    pub roll_type: String,
    /// `DifficultyClassID`: a `DifficultyClasses` resource.
    pub difficulty_class: Option<Guid>,
    /// `Advantage` (0, 1 or 2).
    pub advantage: u8,
    /// `AdvantageReason`: loca handle explaining the advantage.
    pub advantage_reason: Option<String>,
    /// `RollTargetSpeaker`: speaker slot the roll is made against.
    pub target_speaker: Option<i32>,
    pub exclude_companions_optional_bonuses: bool,
    pub exclude_speaker_optional_bonuses: bool,
    /// `ExcludedBonusesTags`.
    pub excluded_bonuses_tags: Vec<Guid>,
    /// `IsFlatSkillCheck` (passive rolls only).
    pub flat_skill_check: bool,
    /// `DetectThoughts`.
    pub detect_thoughts: bool,
}

/// A flag group: flags of one type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlagGroup {
    pub kind: FlagKind,
    pub flags: Vec<Flag>,
}

/// A flag reference in a dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Flag {
    /// Flag (or tag, quest step, script flag) UUID.
    pub uuid: Guid,
    /// For checks: the value the flag must have. For sets: the value to set.
    pub value: bool,
    /// `paramval`: the speaker slot whose object the flag applies to (types that need one).
    pub param: Option<i32>,
}

/// `flaggroup.type`. The [`FlagDefinition::usage`](crate::defs::FlagDefinition::usage) byte of
/// a flag resource matches the group type it is used with (see the docs).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FlagKind {
    Global,
    Object,
    Local,
    Dialog,
    User,
    Party,
    UserShape,
    /// Checks/sets a tag on a speaker.
    Tag,
    /// A `ScriptFlags` condition script.
    Script,
    /// A quest step.
    Quest,
    Other(String),
}

impl FlagKind {
    pub fn parse(s: &str) -> Self {
        match s {
            "Global" => Self::Global,
            "Object" => Self::Object,
            "Local" => Self::Local,
            "Dialog" => Self::Dialog,
            "User" => Self::User,
            "Party" => Self::Party,
            "UserShape" => Self::UserShape,
            "Tag" => Self::Tag,
            "Script" => Self::Script,
            "Quest" => Self::Quest,
            other => Self::Other(other.to_owned()),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Global => "Global",
            Self::Object => "Object",
            Self::Local => "Local",
            Self::Dialog => "Dialog",
            Self::User => "User",
            Self::Party => "Party",
            Self::UserShape => "UserShape",
            Self::Tag => "Tag",
            Self::Script => "Script",
            Self::Quest => "Quest",
            Self::Other(s) => s,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedFlags {
    /// `ValidatedHasValue`.
    pub has_value: bool,
    pub groups: Vec<FlagGroup>,
}

/// One `TaggedText`: lines guarded by a tag rule.
#[derive(Debug, Clone, PartialEq)]
pub struct TaggedText {
    pub has_tag_rule: bool,
    pub rule: RuleGroup,
    pub lines: Vec<TextLine>,
}

/// `RuleGroup`: rules combined with [`TagCombineOp`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RuleGroup {
    pub combine: u8,
    pub rules: Vec<TagRule>,
}

/// `Rule`: tags checked on one speaker.
#[derive(Debug, Clone, PartialEq)]
pub struct TagRule {
    /// `TagCombineOp` for the tags of this rule.
    pub combine: u8,
    /// Speaker slot whose tags are checked (`-1` = …, see docs).
    pub speaker: i32,
    pub tags: Vec<Guid>,
    /// Nested rule group (`HasChildRules`; seen once in the reference build).
    pub child: Option<Box<RuleGroup>>,
}

/// `TagText`: one localised line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextLine {
    /// Loca handle of the text (`TagText`).
    pub handle: String,
    pub version: u16,
    /// `LineId`: voice/timeline line id.
    pub line_id: Option<Guid>,
    pub stub: bool,
    pub custom_sequence_id: Option<Guid>,
    /// `DialogVariables` referenced by the text (variable UUIDs).
    pub dialog_variables: Option<String>,
}

impl Dialog {
    /// Builds the model from a parsed compiled-dialog resource (LSF/LSX/LSJ).
    pub fn from_resource(res: &Resource) -> Result<Self> {
        let root = res
            .region("dialog")
            .ok_or_else(|| Error::Malformed("no `dialog` region".into()))?;
        let uuid = guid_attr(root, "UUID")?.ok_or_else(|| missing("dialog", "UUID"))?;
        let mut dialog = Dialog {
            uuid,
            timeline_id: guid_attr(root, "TimelineId")?,
            category: root.attr_str("category").unwrap_or_default().to_owned(),
            options: DialogOptions {
                automated: flag(root, "automated"),
                allow_dead_speakers: flag(root, "AllowDeadSpeakers"),
                is_allowing_join_combat: flag(root, "IsAllowingJoinCombat"),
                is_behaviour: flag(root, "IsBehaviour"),
                is_private_dialog: flag(root, "IsPrivateDialog"),
                is_subbed_dialog: flag(root, "IsSubbedDialog"),
                is_subs_anonymous: root.attr("IsSubsAnonymous").and_then(Value::as_bool),
                is_world: flag(root, "IsWorld"),
                is_sfx_dialog: flag(root, "issfxdialog"),
                default_speaker_index: int(root, "DefaultSpeakerIndex"),
            },
            ..Default::default()
        };

        if let Some(list) = root.child("speakerlist") {
            for s in list.children_named("speaker") {
                let index = s
                    .attr_str("index")
                    .and_then(|i| i.trim().parse().ok())
                    .ok_or_else(|| missing("speaker", "index"))?;
                let list = match s.attr_str("list") {
                    Some(l) => l
                        .split(';')
                        .map(str::trim)
                        .filter(|p| !p.is_empty())
                        .map(|p| parse_guid(p, "speaker list"))
                        .collect::<Result<_>>()?,
                    None => Vec::new(),
                };
                dialog.speakers.push(Speaker {
                    index,
                    list,
                    mapping_id: guid_attr(s, "SpeakerMappingId")?,
                    is_peanut: flag(s, "IsPeanutSpeaker"),
                    tags_index: s.attr_str("SpeakerTagsIndex").map(str::to_owned),
                });
            }
        }
        if let Some(das) = root.child("DefaultAddressedSpeakers") {
            for o in das.children_named("Object") {
                if let (Some(k), Some(v)) = (int(o, "MapKey"), int(o, "MapValue")) {
                    dialog.default_addressed_speakers.push((k, v));
                }
            }
        }

        let nodes = root
            .child("nodes")
            .ok_or_else(|| Error::Malformed("no `nodes`".into()))?;
        // First pass: assign ids so children/jumps/aliases can be resolved in the second.
        let raw: Vec<&LsfNode> = nodes.children_named("node").collect();
        for (i, n) in raw.iter().enumerate() {
            let id = guid_attr(n, "UUID")?.ok_or_else(|| missing("node", "UUID"))?;
            if dialog.by_uuid.insert(id, i).is_some() {
                return Err(Error::Malformed(format!("duplicate node {id}")));
            }
        }
        for n in raw {
            let node = dialog.load_node(n)?;
            dialog.nodes.push(node);
        }
        for r in nodes.children_named("RootNodes") {
            let id = guid_attr(r, "RootNodes")?.ok_or_else(|| missing("RootNodes", "RootNodes"))?;
            dialog.roots.push(dialog.resolve(id, "root")?);
        }
        Ok(dialog)
    }

    /// Parses compiled dialog bytes (LSF, or LSX/LSJ with the same layout).
    pub fn parse(data: &[u8]) -> Result<Self> {
        Self::from_resource(&bg3_lsf::parse(data)?)
    }

    /// The node with the given UUID.
    pub fn node_by_uuid(&self, uuid: &Guid) -> Option<NodeId> {
        self.by_uuid.get(uuid).copied()
    }

    /// The speaker-list entry for a slot index.
    pub fn speaker(&self, index: i32) -> Option<&Speaker> {
        self.speakers.iter().find(|s| s.index == index)
    }

    fn resolve(&self, id: Guid, what: &str) -> Result<NodeId> {
        self.by_uuid
            .get(&id)
            .copied()
            .ok_or_else(|| Error::Malformed(format!("{what} refers to unknown node {id}")))
    }

    fn load_node(&self, n: &LsfNode) -> Result<Node> {
        let uuid = guid_attr(n, "UUID")?.ok_or_else(|| missing("node", "UUID"))?;
        let constructor = n
            .attr_str("constructor")
            .ok_or_else(|| missing("node", "constructor"))?;
        let kind = match constructor {
            "TagGreeting" => NodeKind::Greeting,
            "TagAnswer" => NodeKind::Answer,
            "TagQuestion" => NodeKind::Question,
            "TagCinematic" => NodeKind::Cinematic {
                behaviour_node_type: int(n, "BehaviourNodeType").map(|v| v as u8),
            },
            "Alias" => NodeKind::Alias {
                source: match guid_attr(n, "SourceNode")? {
                    Some(id) => Some(self.resolve(id, "alias")?),
                    None => None,
                },
            },
            "Jump" => NodeKind::Jump {
                target: match guid_attr(n, "jumptarget")? {
                    Some(id) => Some(self.resolve(id, "jump")?),
                    None => None,
                },
                target_point: int(n, "jumptargetpoint").unwrap_or(1) as u8,
            },
            "Pop" => NodeKind::Pop {
                level: int(n, "PopLevel").unwrap_or(1),
            },
            "Nested Dialog" => NodeKind::NestedDialog {
                dialog: guid_attr(n, "NestedDialogNodeUUID")?
                    .ok_or_else(|| missing("Nested Dialog", "NestedDialogNodeUUID"))?,
                speaker_linking: n
                    .child("SpeakerLinking")
                    .map(|l| {
                        l.children_named("SpeakerLinkingEntry")
                            .filter_map(|e| Some((int(e, "Key")?, int(e, "Value")?)))
                            .collect()
                    })
                    .unwrap_or_default(),
            },
            "Visual State" => NodeKind::VisualState,
            "ActiveRoll" => NodeKind::ActiveRoll(load_roll(n)?),
            "PassiveRoll" => NodeKind::PassiveRoll(load_roll(n)?),
            "RollResult" => NodeKind::RollResult {
                success: flag(n, "Success"),
            },
            "FallibleQuestionResult" => NodeKind::FallibleQuestionResult {
                result: flag(n, "Result"),
            },
            "Trade" => NodeKind::Trade {
                mode: int(n, "TradeMode").map(|v| v as u8),
                items_tag_filter: n.attr_str("ItemsTagFilter").map(str::to_owned),
                player_override: int(n, "PlayerOverride"),
            },
            other => NodeKind::Other(other.to_owned()),
        };

        let mut children = Vec::new();
        if let Some(c) = n.child("children") {
            for child in c.children_named("child") {
                let id = guid_attr(child, "UUID")?.ok_or_else(|| missing("child", "UUID"))?;
                children.push(self.resolve(id, "child")?);
            }
        }

        let group = n.attr_str("GroupID").map(|id| NodeGroup {
            id: id.to_owned(),
            index: int(n, "GroupIndex"),
        });

        let validated_flags = match n.child("ValidatedFlags") {
            Some(v) => Some(ValidatedFlags {
                has_value: flag(v, "ValidatedHasValue"),
                groups: load_flag_groups(Some(v))?,
            }),
            None => None,
        };

        let mut texts = Vec::new();
        if let Some(tt) = n.child("TaggedTexts") {
            for t in tt.children_named("TaggedText") {
                texts.push(load_tagged_text(t)?);
            }
        }

        let tags = match n.child("Tags") {
            Some(t) => t
                .children_named("Tag")
                .filter_map(|t| t.attr("Tag").and_then(Value::as_guid))
                .collect(),
            None => Vec::new(),
        };

        let cinematic_context = n.child("editorData").and_then(|e| {
            e.children_named("data")
                .find(|d| d.attr_str("key") == Some("CinematicNodeContext"))
                .and_then(|d| d.attr_str("val"))
                .map(str::to_owned)
        });

        Ok(Node {
            uuid,
            kind,
            speaker: int(n, "speaker"),
            addressed_speaker: int(n, "addressedspeaker"),
            root: flag(n, "Root"),
            greeting: flag(n, "Greeting"),
            end_node: flag(n, "endnode"),
            end_all_dialogs: flag(n, "endAllDialogs"),
            show_once: flag(n, "ShowOnce"),
            optional: flag(n, "optional"),
            exclusive: flag(n, "exclusive"),
            stub: flag(n, "stub"),
            suppress_subtitle: flag(n, "suppresssubtitle"),
            gameplay_node: flag(n, "gameplaynode"),
            allow_node_grouping: n.attr("AllowNodeGrouping").and_then(Value::as_bool),
            group,
            transition_mode: int(n, "transitionmode").map(|v| v as u8),
            wait_time: n.attr("waittime").and_then(Value::as_f64).map(|v| v as f32),
            approval_rating_id: n.attr("ApprovalRatingID").and_then(Value::as_guid),
            override_approval_rating_id: flag(n, "OverrideApprovalRatingID"),
            children,
            check_flags: load_flag_groups(n.child("checkflags"))?,
            set_flags: load_flag_groups(n.child("setflags"))?,
            validated_flags,
            tags,
            texts,
            cinematic_context,
        })
    }
}

fn load_roll(n: &LsfNode) -> Result<Roll> {
    Ok(Roll {
        ability: n.attr_str("Ability").unwrap_or_default().to_owned(),
        skill: n.attr_str("Skill").unwrap_or_default().to_owned(),
        roll_type: n.attr_str("RollType").unwrap_or_default().to_owned(),
        difficulty_class: n.attr("DifficultyClassID").and_then(Value::as_guid),
        advantage: int(n, "Advantage").unwrap_or(0) as u8,
        advantage_reason: n.attr_str("AdvantageReason").map(str::to_owned),
        target_speaker: int(n, "RollTargetSpeaker"),
        exclude_companions_optional_bonuses: flag(n, "ExcludeCompanionsOptionalBonuses"),
        exclude_speaker_optional_bonuses: flag(n, "ExcludeSpeakerOptionalBonuses"),
        excluded_bonuses_tags: n
            .children_named("ExcludedBonusesTags")
            .filter_map(|t| t.attr("Object").and_then(Value::as_guid))
            .collect(),
        flat_skill_check: flag(n, "IsFlatSkillCheck"),
        detect_thoughts: flag(n, "DetectThoughts"),
    })
}

fn load_flag_groups(n: Option<&LsfNode>) -> Result<Vec<FlagGroup>> {
    let Some(n) = n else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for g in n.children_named("flaggroup") {
        let kind = FlagKind::parse(
            g.attr_str("type")
                .ok_or_else(|| missing("flaggroup", "type"))?,
        );
        let mut flags = Vec::new();
        for f in g.children_named("flag") {
            flags.push(Flag {
                uuid: guid_attr(f, "UUID")?.ok_or_else(|| missing("flag", "UUID"))?,
                value: flag(f, "value"),
                param: int(f, "paramval"),
            });
        }
        out.push(FlagGroup { kind, flags });
    }
    Ok(out)
}

fn load_rule_group(g: &LsfNode) -> Result<RuleGroup> {
    let mut rules = Vec::new();
    if let Some(rs) = g.child("Rules") {
        for r in rs.children_named("Rule") {
            let tags = match r.child("Tags") {
                Some(t) => t
                    .children_named("Tag")
                    .map(|t| {
                        t.attr("Object")
                            .and_then(Value::as_guid)
                            .ok_or_else(|| missing("Tag", "Object"))
                    })
                    .collect::<Result<_>>()?,
                None => Vec::new(),
            };
            let child = match r.child("RuleGroup") {
                Some(c) => Some(Box::new(load_rule_group(c)?)),
                None => None,
            };
            rules.push(TagRule {
                combine: int(r, "TagCombineOp").unwrap_or(0) as u8,
                speaker: int(r, "speaker").unwrap_or(SPEAKER_NONE),
                tags,
                child,
            });
        }
    }
    Ok(RuleGroup {
        combine: int(g, "TagCombineOp").unwrap_or(0) as u8,
        rules,
    })
}

fn load_tagged_text(t: &LsfNode) -> Result<TaggedText> {
    let rule = match t.child("RuleGroup") {
        Some(g) => load_rule_group(g)?,
        None => RuleGroup::default(),
    };
    let mut lines = Vec::new();
    if let Some(tts) = t.child("TagTexts") {
        for l in tts.children_named("TagText") {
            let (handle, version) = match l.attr("TagText") {
                Some(Value::TranslatedString(s)) => (s.handle.clone(), s.version),
                Some(Value::TranslatedFSString(s)) => (s.string.handle.clone(), s.string.version),
                _ => return Err(missing("TagText", "TagText")),
            };
            lines.push(TextLine {
                handle,
                version,
                line_id: l.attr("LineId").and_then(Value::as_guid),
                stub: flag(l, "stub"),
                custom_sequence_id: l.attr("CustomSequenceId").and_then(Value::as_guid),
                dialog_variables: l.attr_str("DialogVariables").map(str::to_owned),
            });
        }
    }
    Ok(TaggedText {
        has_tag_rule: flag(t, "HasTagRule"),
        rule,
        lines,
    })
}

fn flag(n: &LsfNode, name: &str) -> bool {
    n.attr(name).and_then(Value::as_bool).unwrap_or(false)
}

fn int(n: &LsfNode, name: &str) -> Option<i32> {
    n.attr(name)
        .and_then(Value::as_i64)
        .and_then(|v| i32::try_from(v).ok())
}

/// A GUID stored either as a `guid` attribute or as a string (`FixedString`/`LSString`).
/// Empty strings count as absent.
fn guid_attr(n: &LsfNode, name: &str) -> Result<Option<Guid>> {
    match n.attr(name) {
        None => Ok(None),
        Some(Value::Uuid(g)) => Ok(Some(*g)),
        Some(v) => match v.as_str() {
            Some("") => Ok(None),
            Some(s) => parse_guid(s, name).map(Some),
            None => Err(Error::Malformed(format!(
                "{name}: not a GUID ({:?})",
                v.ty()
            ))),
        },
    }
}

fn parse_guid(s: &str, what: &str) -> Result<Guid> {
    Guid::parse(s.trim()).ok_or_else(|| Error::Malformed(format!("{what}: bad GUID {s:?}")))
}

fn missing(node: &str, attr: &str) -> Error {
    Error::Malformed(format!("`{node}` without `{attr}`"))
}
