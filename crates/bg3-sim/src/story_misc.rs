//! Small Osiris APIs: persistent entity state, install-backed queries and front-end hints.
//! Trace-backed semantics and remaining limits: docs/research/osiris-engine-api.md.
use std::collections::{HashMap, HashSet};

use bg3_osiris::{Data, HostCtx, Value};

use crate::{EntityKind, Guid, ObservedArg, Sim, SimValue, TagSource};

#[derive(Clone, Debug, Default)]
pub struct EntityStory {
    /// Explicit runtime name handle supplied by a character-creation/platform host.
    pub display_name: Option<String>,
    /// Runtime inventory slot assignments, observed from Equipped/Unequipped events.
    pub equipped: HashMap<String, Guid>,
    pub removed_spells: HashSet<String>,
    pub has_dialog: Option<bool>,
    pub can_interact: Option<bool>,
    pub can_fight: Option<bool>,
    pub detached: bool,
    /// Chair/bed currently occupied after a supported use.
    pub using_item: Option<Guid>,
    pub is_player: bool,
    pub player_data_initialized: bool,
    pub active: bool,
    pub party_follower: bool,
    pub owner: Option<Guid>,
    pub all_crimes_disabled: bool,
    /// Explicit per-crime overrides, including enables after DisableAllCrimes.
    pub crimes: HashMap<String, bool>,
    pub preferred_ai_targets: Vec<Guid>,
    pub passives: Vec<String>,
    /// Entitlements supplied by a platform host; absent means unknown, not unowned.
    pub dlc: Option<HashSet<Guid>>,
}

/// A presentation request retained verbatim as typed arguments; it does not claim that a
/// save, recipe, waypoint or tutorial was actually shown/executed.
#[derive(Clone, Debug)]
pub struct StoryEffect {
    pub name: String,
    pub args: Vec<SimValue>,
}

#[derive(Clone, Debug, Default)]
pub struct StoryState {
    pub host: Option<Guid>,
    pub effects: Vec<StoryEffect>,
    /// Magic-pockets flags are separate from ordinary story flags.
    pub magic_pockets: HashSet<(Guid, Guid)>,
}

pub const CALLS: &[(&str, usize)] = &[
    ("RegisterWaypoint", 2),
    ("EnableTutorialEvent", 2),
    ("CharacterDisableCrime", 2),
    ("AddPreferredAiTargetTag", 2),
    ("CharacterDisableAllCrimes", 1),
    ("MakeNPC", 1),
    ("SetHasDialog", 2),
    ("IterateOriginTags", 3),
    ("InitializePlayerData", 1),
    ("SetCanInteract", 2),
    ("SetForceUpdate", 2),
    ("CharacterEnableCrime", 2),
    ("ClearMagicPocketsFlag", 3),
    ("SetMagicPocketsFlag", 3),
    ("UnlockRecipe", 3),
    ("AddEntryToCustomBook", 2),
    ("MapMarkerChangeLevel", 2),
    ("SetCanFight", 2),
    ("SetDetached", 2),
    ("IterateTagsCategory", 3),
    ("LookAtEntity", 3),
    ("SetAiHint", 2),
    ("AddPassive", 2),
    ("AutoSave", 0),
    ("Use", 5),
    ("RemoveSpell", 3),
    ("HideTutorial", 2),
    ("ShowTutorial", 2),
];
pub const QUERIES: &[(&str, usize)] = &[
    ("GetHostCharacter", 1),
    ("DataGetBackgroundTagCount", 2),
    ("DataGetGodTagCount", 2),
    ("DataGetReallyTagCount", 2),
    ("DataGetBackgroundTagAt", 3),
    ("DataGetGodTagAt", 3),
    ("DataGetReallyTagAt", 3),
    ("GetInventoryOwner", 2),
    ("CheckRulesetModifierBool", 3),
    ("GetIdentity", 3),
    ("GetRulesetModifierString", 2),
    ("GetMaxPartySize", 1),
    ("CheckRulesetModifierString", 3),
    ("TemplateIsInInventory", 3),
    ("GetDisplayName", 2),
    ("CharacterGetOwner", 2),
    ("CharacterHasDLC", 3),
    ("IsActive", 2),
    ("IsPartyFollower", 2),
    ("GetActionResourceValuePersonal", 4),
    ("IsDead", 2),
    ("IsEnemy", 3),
    ("GetEquippedItem", 3),
    ("CanFight", 2),
];

fn text(ctx: &HostCtx<'_>, v: &Value) -> String {
    ctx.text(v).unwrap_or_default().to_owned()
}
fn guid(ctx: &HostCtx<'_>, v: &Value) -> Guid {
    ctx.text(v)
        .and_then(Guid::from_osiris)
        .unwrap_or(Guid::NULL)
}
fn int(v: &Value) -> i32 {
    match v.data {
        Data::Int(i) => i,
        Data::Int64(i) => i as i32,
        _ => 0,
    }
}
fn values(ctx: &HostCtx<'_>, args: &[Value]) -> Vec<SimValue> {
    args.iter()
        .map(|v| match v.data {
            Data::Int(i) => SimValue::Int(i),
            Data::Int64(i) => SimValue::Int(i as i32),
            Data::Real(r) => SimValue::Real(r),
            _ if Guid::from_osiris(&text(ctx, v)).is_some() => SimValue::Guid(text(ctx, v)),
            _ => SimValue::Str(text(ctx, v)),
        })
        .collect()
}

/// Current tags, including a template's not-yet-merged tags during story INIT.
fn entity_tags(sim: &Sim, id: Guid, category: &str) -> Option<Vec<Guid>> {
    let e = sim.entity(id)?;
    let mut tags = e.tags.merged().to_vec();
    for source in [TagSource::Template, TagSource::Osiris, TagSource::Engine] {
        for t in e.tags.source(source) {
            if !tags.contains(t) {
                tags.push(*t);
            }
        }
    }
    tags.retain(|t| {
        sim.data.tags.get(*t).is_some_and(|d| {
            d.categories
                .iter()
                .any(|c| c.eq_ignore_ascii_case(category))
        })
    });
    Some(tags)
}

pub(crate) fn call(
    sim: &mut Sim,
    ctx: &mut HostCtx<'_>,
    name: &str,
    args: &[Value],
) -> Option<bool> {
    if !CALLS.contains(&(name, args.len())) {
        return None;
    }
    let id = args.first().map_or(Guid::NULL, |v| guid(ctx, v));
    let enabled = args.get(1).is_some_and(|v| int(v) != 0);
    match name {
        // bg3: esv::OsirisCharacterFunctions::RemoveSpell (4.1.1.7631656)
        "RemoveSpell" => {
            let spell = text(ctx, &args[1]);
            let Some(e) = sim.entities.get_mut(id) else {
                return Some(false);
            };
            e.story.removed_spells.insert(spell.clone());
            if let Some(c) = sim.combatant_mut(id) {
                c.spells.retain(|s| s != &spell);
            }
        }
        // bg3: esv::OsirisGameFunctions::HideTutorial (4.1.1.7631656)
        // bg3: esv::OsirisGameFunctions::ShowTutorial (4.1.1.7631656)
        "HideTutorial" | "ShowTutorial" => {
            sim.story.effects.push(StoryEffect {
                name: name.into(),
                args: values(ctx, args),
            });
        }
        // bg3: esv::OsirisCharacterFunctions::Use (4.1.1.7631656)
        "Use" => {
            return Some(sim.use_item_scripted(
                id,
                guid(ctx, &args[1]),
                int(&args[2]),
                int(&args[3]),
                &text(ctx, &args[4]),
            ));
        }
        // bg3: esv::OsirisDataFunctions::IterateOriginTags (4.1.1.7631656)
        "IterateOriginTags" => {
            let Some(tags) = entity_tags(sim, id, "Origin") else {
                return Some(false);
            };
            for t in tags {
                sim.push_event(
                    "CharacterTagEvent",
                    vec![
                        SimValue::Guid(sim.render(id)),
                        SimValue::Guid(sim.tag_name(t)),
                        SimValue::Str(text(ctx, &args[1])),
                    ],
                );
            }
            sim.push_event(
                "EntityEvent",
                vec![
                    SimValue::Guid(sim.render(id)),
                    SimValue::Str(text(ctx, &args[2])),
                ],
            );
        }
        // bg3: esv::OsirisDataFunctions::IterateTagsCategory (4.1.1.7631656)
        "IterateTagsCategory" => {
            let category = match ctx.log_value(&args[0]).v {
                bg3_osiris::trace::LogData::Enum(_, label) => label.replace('_', ""),
                _ => return Some(false),
            };
            let tags: Vec<Guid> = sim
                .data
                .tags
                .iter()
                .filter(|d| {
                    d.categories
                        .iter()
                        .any(|c| c.eq_ignore_ascii_case(&category))
                })
                .map(|d| d.id)
                .collect();
            for t in tags {
                sim.push_event(
                    "TagEvent",
                    vec![
                        SimValue::Guid(sim.tag_name(t)),
                        SimValue::Str(text(ctx, &args[1])),
                    ],
                );
            }
            sim.push_event(
                "EntityEvent",
                vec![
                    SimValue::Guid(sim.render(Guid::NULL)),
                    SimValue::Str(text(ctx, &args[2])),
                ],
            );
        }
        // bg3: esv::OsirisDialogFunctions::SetMagicPocketsFlag (4.1.1.7631656)
        // bg3: esv::OsirisDialogFunctions::ClearMagicPocketsFlag (4.1.1.7631656)
        "SetMagicPocketsFlag" | "ClearMagicPocketsFlag" => {
            let key = (id, guid(ctx, &args[1]));
            if name == "SetMagicPocketsFlag" {
                sim.story.magic_pockets.insert(key);
            } else {
                sim.story.magic_pockets.remove(&key);
            }
            sim.story.effects.push(StoryEffect {
                name: name.into(),
                args: values(ctx, args),
            });
        }
        // bg3: esv::OsirisGameFunctions::SetHasDialog (4.1.1.7631656)
        // bg3: esv::OsirisItemFunctions::SetCanInteract (4.1.1.7631656)
        // bg3: esv::OsirisGameFunctions::SetCanFight (4.1.1.7631656)
        // bg3: esv::OsirisGameFunctions::SetDetached (4.1.1.7631656)
        // bg3: esv::OsirisCharacterFunctions::MakeNPC (4.1.1.7631656)
        // bg3: esv::OsirisCharacterFunctions::InitializePlayerData (4.1.1.7631656)
        // bg3: esv::OsirisCharacterFunctions::CharacterDisableCrime (4.1.1.7631656)
        // bg3: esv::OsirisCharacterFunctions::CharacterEnableCrime (4.1.1.7631656)
        // bg3: esv::OsirisCharacterFunctions::CharacterDisableAllCrimes (4.1.1.7631656)
        // bg3: esv::OsirisCharacterFunctions::AddPreferredAiTargetTag (4.1.1.7631656)
        // bg3: esv::PassiveSystem::AddPassive (4.1.1.7631656)
        "SetHasDialog"
        | "SetCanInteract"
        | "SetCanFight"
        | "SetDetached"
        | "MakeNPC"
        | "InitializePlayerData"
        | "CharacterDisableCrime"
        | "CharacterEnableCrime"
        | "CharacterDisableAllCrimes"
        | "AddPreferredAiTargetTag"
        | "AddPassive" => {
            let Some(e) = sim.entities.get_mut(id) else {
                return Some(false);
            };
            match name {
                "SetHasDialog" => e.story.has_dialog = Some(enabled),
                "SetCanInteract" => e.story.can_interact = Some(enabled),
                "SetCanFight" => e.story.can_fight = Some(enabled),
                "SetDetached" => e.story.detached = enabled,
                "MakeNPC" => e.story.is_player = false,
                "InitializePlayerData" => e.story.player_data_initialized = true,
                "CharacterDisableCrime" | "CharacterEnableCrime" => {
                    e.story
                        .crimes
                        .insert(text(ctx, &args[1]), name == "CharacterEnableCrime");
                }
                "CharacterDisableAllCrimes" => {
                    e.story.all_crimes_disabled = true;
                    e.story.crimes.clear();
                }
                "AddPreferredAiTargetTag" => {
                    let tag = guid(ctx, &args[1]);
                    if !e.story.preferred_ai_targets.contains(&tag) {
                        e.story.preferred_ai_targets.push(tag);
                    }
                }
                "AddPassive" => {
                    let passive = text(ctx, &args[1]);
                    if !e.story.passives.contains(&passive) {
                        e.story.passives.push(passive.clone());
                    }
                    if let Some(sheet) = &mut e.sheet
                        && !sheet.passives.contains(&passive)
                    {
                        sheet.passives.push(passive);
                    }
                }
                _ => unreachable!(),
            }
        }
        // Presentation requests only: see the per-function symbols in the API notes.
        // bg3: esv::OsirisGameFunctions::RegisterWaypoint (4.1.1.7631656)
        // bg3: esv::OsirisGameFunctions::EnableTutorialEvent (4.1.1.7631656)
        // bg3: esv::OsirisGameFunctions::SetForceUpdate (4.1.1.7631656)
        // bg3: esv::OsirisCharacterFunctions::UnlockRecipe (4.1.1.7631656)
        // bg3: esv::OsirisGameFunctions::AddEntryToCustomBook (4.1.1.7631656)
        // bg3: esv::OsirisGameFunctions::MapMarkerChangeLevel (4.1.1.7631656)
        // bg3: esv::OsirisCharacterFunctions::LookAtEntity (4.1.1.7631656)
        // bg3: esv::OsirisGameFunctions::SetAiHint (4.1.1.7631656)
        // bg3: esv::OsirisGameFunctions::AutoSave (4.1.1.7631656)
        _ => sim.story.effects.push(StoryEffect {
            name: name.into(),
            args: values(ctx, args),
        }),
    }
    Some(true)
}

impl Sim {
    /// Drain presentation requests for the front end.
    pub fn take_story_effects(&mut self) -> Vec<StoryEffect> {
        std::mem::take(&mut self.story.effects)
    }

    fn tag_name(&self, id: Guid) -> String {
        self.data
            .tags
            .get(id)
            .map_or_else(|| id.to_string(), |d| crate::guid::osiris_name(&d.name, id))
    }

    /// Immediate inventory holder, with cycle-safe traversal available to callers below.
    // bg3: esv::OsirisGameFunctions::GetInventoryOwner (4.1.1.7631656)
    pub fn inventory_owner(&self, id: Guid) -> Option<Guid> {
        self.entity(id)?.holder
    }

    /// Whether an item is inside a holder, including nested containers.
    pub fn in_inventory(&self, item: Guid, holder: Guid) -> bool {
        let mut cur = item;
        let mut seen = HashSet::new();
        while seen.insert(cur) {
            let Some(parent) = self.inventory_owner(cur) else {
                return false;
            };
            if parent == holder {
                return true;
            }
            cur = parent;
        }
        false
    }
}

pub(crate) fn query(
    sim: &mut Sim,
    ctx: &mut HostCtx<'_>,
    name: &str,
    args: &mut [Value],
) -> Option<bool> {
    if !QUERIES.contains(&(name, args.len())) {
        return None;
    }
    let id = guid(ctx, &args[0]);

    let result = match name {
        // bg3: esv::OsirisCharacterFunctions::GetActionResourceValuePersonal (4.1.1.7631656)
        "GetActionResourceValuePersonal" => {
            if !sim.ensure_combatant(id) {
                return None;
            }
            let resource = text(ctx, &args[1]);
            if sim.data.combat.resource(&resource).is_none() {
                return Some(false);
            }
            let level = u32::try_from(int(&args[2])).ok()?;
            Some(Value::real(
                sim.combatant(id)?.resources.get_at_level(&resource, level),
            ))
        }
        // bg3: esv::OsirisCharacterFunctions::IsDead (4.1.1.7631656)
        "IsDead" => {
            let e = sim.entity(id)?;
            // During replay engine damage precedes its events; initial sheets cannot
            // establish live death state (same boundary as GetHitpointsPercentage).
            if !sim.stage.live && sim.combatant(id).is_none() {
                return None;
            }
            let dead = sim
                .combatant(id)
                .map_or_else(|| e.sheet.as_ref().is_some_and(|s| s.hp <= 0), |c| c.dead);
            Some(Value::int(i32::from(dead)))
        }
        // bg3: esv::OsirisGameFunctions::IsEnemy (4.1.1.7631656)
        "IsEnemy" => {
            let target = guid(ctx, &args[1]);
            sim.entity(id)?;
            sim.entity(target)?;
            Some(Value::int(i32::from(sim.is_hostile(id, target))))
        }
        // bg3: esv::OsirisGameFunctions::CanFight (4.1.1.7631656)
        "CanFight" => Some(Value::int(i32::from(sim.can_fight(id)?))),
        // bg3: esv::OsirisCharacterFunctions::GetEquippedItem (4.1.1.7631656)
        "GetEquippedItem" => sim
            .entity(id)?
            .story
            .equipped
            .get(&text(ctx, &args[1]))
            .map(|&item| ctx.guid(&sim.render(item))),
        // bg3: esv::OsirisCharacterFunctions::GetHostCharacter (4.1.1.7631656)
        "GetHostCharacter" => sim.story.host.map(|id| ctx.guid(&id.to_string())),
        // bg3: esv::OsirisGameFunctions::GetMaxPartySize (4.1.1.7631656)
        "GetMaxPartySize" => Some(Value::int(4)),
        // bg3: esv::OsirisGameFunctions::GetInventoryOwner (4.1.1.7631656)
        "GetInventoryOwner" => sim.inventory_owner(id).map(|id| ctx.guid(&id.to_string())),
        // bg3: esv::OsirisItemFunctions::TemplateIsInInventory (4.1.1.7631656)
        "TemplateIsInInventory" => {
            let holder = guid(ctx, &args[1]);
            Some(Value::int(
                sim.entities
                    .iter()
                    .filter(|e| {
                        e.kind == EntityKind::Item
                            && e.template == Some(id)
                            && sim.in_inventory(e.id, holder)
                    })
                    .count() as i32,
            ))
        }
        // bg3: esv::OsirisDataFunctions::DataGetBackgroundTagCount (4.1.1.7631656)
        // bg3: esv::OsirisDataFunctions::DataGetReallyTagCount (4.1.1.7631656)
        // bg3: esv::OsirisDataFunctions::DataGetGodTagCount (4.1.1.7631656)
        // bg3: esv::OsirisDataFunctions::DataGetBackgroundTagAt (4.1.1.7631656)
        // bg3: esv::OsirisDataFunctions::DataGetReallyTagAt (4.1.1.7631656)
        // bg3: esv::OsirisDataFunctions::DataGetGodTagAt (4.1.1.7631656)
        n if n.starts_with("DataGet") => {
            let origin = sim.data.story.origins.get(&id);
            let list = origin.map(|o| {
                if n.contains("Background") {
                    &o.background
                } else if n.contains("Really") {
                    &o.really
                } else {
                    &o.god
                }
            });
            if n.ends_with("Count") {
                list.filter(|v| !v.is_empty())
                    .map(|v| Value::int(v.len() as i32))
            } else {
                list.and_then(|v| {
                    usize::try_from(int(&args[1]) - 1)
                        .ok()
                        .and_then(|i| v.get(i))
                })
                .map(|id| ctx.guid(&sim.tag_name(*id)))
            }
        }
        // bg3: esv::OsirisGameFunctions::GetRulesetModifierString (4.1.1.7631656)
        "GetRulesetModifierString" => Some(ctx.string(sim.data.story.ruleset.get(&id)?)),
        // bg3: esv::OsirisGameFunctions::CheckRulesetModifierString (4.1.1.7631656)
        "CheckRulesetModifierString" => Some(Value::int(i32::from(
            sim.data.story.ruleset.get(&id)? == &text(ctx, &args[1]),
        ))),
        // bg3: esv::OsirisGameFunctions::CheckRulesetModifierBool (4.1.1.7631656)
        "CheckRulesetModifierBool" => {
            let value = sim.data.story.ruleset.get(&id)?.parse::<bool>().ok()?;
            Some(Value::int(i32::from(value == (int(&args[1]) != 0))))
        }
        // bg3: esv::OsirisCharacterFunctions::GetIdentity (4.1.1.7631656)
        "GetIdentity" => {
            if int(&args[1]) != 0 {
                return None;
            }
            let tags = entity_tags(sim, id, "Identity")?;
            let gender = tags
                .iter()
                .filter_map(|id| sim.data.tags.get(*id))
                .find_map(|d| match d.name.as_str() {
                    "FEMALE" => Some("Female"),
                    "MALE" => Some("Male"),
                    _ => None,
                })?;
            Some(ctx.string(gender))
        }
        // bg3: esv::OsirisGameFunctions::GetDisplayName (4.1.1.7631656)
        "GetDisplayName" => {
            let e = sim.entity(id)?;
            if let Some(name) = &e.story.display_name {
                args[1] = ctx.string(name);
                return Some(true);
            }
            // Characters (including companions loaded from a save) can have runtime
            // ResStr_… names. The trace has no name-change event to establish that state.
            if e.kind == EntityKind::Character && !sim.stage.live {
                return None;
            }
            let name = sim.display_name_handle(id)?;
            Some(ctx.string(name))
        }
        // bg3: esv::OsirisCharacterFunctions::CharacterGetOwner (4.1.1.7631656)
        "CharacterGetOwner" => sim
            .entity(id)?
            .story
            .owner
            .map(|id| ctx.guid(&id.to_string())),
        // bg3: esv::OsirisGameFunctions::CharacterHasDLC (4.1.1.7631656)
        "CharacterHasDLC" => Some(Value::int(i32::from(
            sim.entity(id)?
                .story
                .dlc
                .as_ref()?
                .contains(&guid(ctx, &args[1])),
        ))),
        // bg3: esv::OsirisGameFunctions::IsActive (4.1.1.7631656)
        "IsActive" => Some(Value::int(i32::from(sim.entity(id)?.story.active))),
        // bg3: esv::OsirisCharacterFunctions::IsPartyFollower (4.1.1.7631656)
        "IsPartyFollower" => Some(Value::int(i32::from(sim.entity(id)?.story.party_follower))),
        _ => return None,
    };
    Some(if let Some(value) = result {
        let n = args.len();
        args[n - 1] = value;
        true
    } else {
        false
    })
}

pub(crate) fn observe(sim: &mut Sim, name: &str, args: &[ObservedArg]) {
    let id = args.first().and_then(|a| Guid::from_osiris(&a.text));
    // bg3: esv::OsirisCharacterFunctions::GetEquippedItem (4.1.1.7631656)
    if matches!(name, "Equipped" | "Unequipped") && args.len() == 2 {
        let owner = Guid::from_osiris(&args[1].text);
        if let (Some(item), Some(owner)) = (id, owner) {
            let slot = sim
                .entity(item)
                .and_then(|e| {
                    e.stats.as_ref().or_else(|| {
                        sim.data
                            .template_chain(e.template)
                            .into_iter()
                            .find_map(|t| t.stats.as_ref())
                    })
                })
                .and_then(|stats| sim.data.story.item_slots.get(stats))
                .cloned();
            if let Some(e) = sim.entities.get_mut(owner) {
                if name == "Unequipped" {
                    e.story.equipped.retain(|_, id| *id != item);
                } else if let Some(slot) = slot {
                    e.story.equipped.insert(slot, item);
                }
            }
        }
    }
    if let Some(id) = id {
        if name == "CharacterSelectedForUser" && sim.story.host.is_none() {
            sim.story.host = Some(id);
        }
        if let Some(e) = sim.entities.get_mut(id) {
            match name {
                "CharacterMadePlayer" => e.story.is_player = true,
                "CharacterMadeNPC" => e.story.is_player = false,
                "Activated" => e.story.active = true,
                "Deactivated" => e.story.active = false,
                "RemovedFrom" => e.holder = None,
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{Entity, GameData, Origin, TagDef, host::SimHost, story_data::OriginData};
    use crate::{EntityKind, Guid, Sim, TagSource};
    use bg3_osiris::{Vm, builder::*};

    #[test]
    fn origin_data_queries_and_iteration_drive_the_story_without_a_trace() {
        let character = "11111111-0000-0000-0000-000000000001";
        let tag = "22222222-0000-0000-0000-000000000001";
        let id = Guid::parse(character).unwrap();
        let t = Guid::parse(tag).unwrap();
        let mut data = GameData::default();
        data.tags.insert(TagDef {
            id: t,
            name: "TestOrigin".into(),
            categories: vec!["Origin".into()],
        });
        data.story.origins.insert(
            id,
            OriginData {
                really: vec![t],
                ..Default::default()
            },
        );
        let mut sim = Sim::new(data);
        let e = sim
            .entities
            .insert(Entity::new(id, "Hero", EntityKind::Character, Origin::Data));
        e.tags.add(TagSource::Template, t);
        let mut b = StoryBuilder::new();
        b.event("Start", &[]);
        b.event("CharacterTagEvent", &[GUIDSTRING, GUIDSTRING, STRING]);
        b.event("EntityEvent", &[GUIDSTRING, STRING]);
        b.query(
            "DataGetReallyTagAt",
            &[GUIDSTRING, INT, GUIDSTRING],
            &[false, false, true],
        );
        b.call("IterateOriginTags", &[GUIDSTRING, STRING, STRING]);
        b.db("DB_Result", &[GUIDSTRING]);
        b.db("DB_Iterated", &[GUIDSTRING]);
        b.db("DB_Done", &[GUIDSTRING]);
        let goal = b.goal("Main", &[]);
        b.rule(
            goal,
            Rule::when("Start", [])
                .and("DataGetReallyTagAt", [guid(character), int(1), v("Tag")])
                .then("DB_Result", [v("Tag")]),
        );
        b.rule(
            goal,
            Rule::when("Start", [])
                .then("IterateOriginTags", [guid(character), s("Each"), s("Done")]),
        );
        b.rule(
            goal,
            Rule::when("CharacterTagEvent", [v("C"), v("Tag"), s("Each")])
                .then("DB_Iterated", [v("Tag")]),
        );
        b.rule(
            goal,
            Rule::when("EntityEvent", [v("C"), s("Done")]).then("DB_Done", [v("C")]),
        );
        let story = b.build();
        let mut vm = Vm::new(&story);
        vm.init_game(&mut SimHost::new(&mut sim));
        vm.event(&mut SimHost::new(&mut sim), "Start", &[]);
        crate::driver::run_events(&mut vm, &mut sim, 8);
        let result = vm.db("DB_Result", 1).unwrap();
        let iterated = vm.db("DB_Iterated", 1).unwrap();
        assert_eq!(vm.text(&result[0][0]), vm.text(&iterated[0][0]));
        assert_eq!(vm.db("DB_Result", 1).unwrap().len(), 1);
        assert_eq!(vm.db("DB_Done", 1).unwrap().len(), 1);
    }

    #[test]
    fn inventory_walk_handles_nested_containers_and_cycles() {
        let mut sim = Sim::default();
        for (id, holder) in [(1, None), (2, Some(1)), (3, Some(2))] {
            let mut e = Entity::new(Guid(id), "Object", EntityKind::Item, Origin::Data);
            e.holder = holder.map(Guid);
            sim.entities.insert(e);
        }
        assert!(sim.in_inventory(Guid(3), Guid(1)));
        assert!(!sim.in_inventory(Guid(1), Guid(3)));
        sim.entities.get_mut(Guid(1)).unwrap().holder = Some(Guid(3));
        assert!(!sim.in_inventory(Guid(3), Guid(4)), "cycle must terminate");
    }
}

impl Sim {
    /// `CanFight`: the story's `SetCanFight`, else the placed object's or its template
    /// chain's `CanFight`, else true for characters; `None` for unknown objects.
    pub fn can_fight(&self, id: Guid) -> Option<bool> {
        let e = self.entity(id)?;
        let from_data = self
            .data
            .story
            .objects
            .get(&id)
            .and_then(|o| o.can_fight)
            .or_else(|| {
                self.data
                    .template_chain(e.template)
                    .into_iter()
                    .find_map(|t| self.data.story.objects.get(&t.id)?.can_fight)
            });
        Some(
            e.story
                .can_fight
                .or(from_data)
                .unwrap_or(e.kind == EntityKind::Character),
        )
    }
}
