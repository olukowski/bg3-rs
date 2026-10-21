//! Turn-based combat, headless: entering combat, initiative, the turn order with shared
//! turns, action resources, spells and weapon attacks through the rules engine, damage, death
//! and the story events that go with them. See `docs/research/combat.md` for what each rule
//! is based on and what is still approximate.
//!
//! * [`data`]: the static data ([`CombatData`], in [`crate::GameData::combat`]).
//! * [`Combatant`]: per-character combat state (resources, equipment, known spells,
//!   initiative), set up from the data when the character first needs it.
//! * [`Combat`]: one fight: participants, the round, the turn order as [`TurnGroup`]s.
//! * [`action`]: what a combatant can do on its turn: move, cast a spell (weapon attacks are
//!   spells too), end the turn.
//! * [`ai`]: an approximate enemy turn (attack the nearest hostile).
//!
//! The front end reads [`Sim::combat_of`] / [`Sim::combatant`], submits actions with
//! [`Sim::combat_move`], [`Sim::combat_cast`] and [`Sim::combat_end_turn`], lets enemies act
//! with [`Sim::combat_run_ai`], and takes [`CombatEvent`]s with [`Sim::take_combat_events`].

pub mod action;
pub mod ai;
pub mod data;
pub mod game;
pub mod notice;
pub mod path;

#[cfg(test)]
mod tests;

use std::cmp::Ordering;
use std::collections::HashMap;

use bg3_rules::context::{Ability, DamageType, Hand, Resistance, Weapon};
use bg3_rules::natives::RollEvent;

pub use action::{CastReport, CombatError};
pub use ai::AiAction;
pub use data::{CombatData, Factions};
pub use path::Pathfinder;

use crate::entity::EntityKind;
use crate::events::SimValue;
use crate::guid::Guid;
use crate::sim::Sim;

/// A combatant's action resources: current and maximum amount by name (`ActionPoint`,
/// `BonusActionPoint`, `ReactionActionPoint`, `Movement` in metres, …).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Resources {
    pub entries: Vec<ResourceAmount>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResourceAmount {
    pub name: String,
    pub amount: f32,
    pub max: f32,
}

impl Resources {
    pub fn get(&self, name: &str) -> f32 {
        self.entries
            .iter()
            .find(|r| r.name == name)
            .map_or(0.0, |r| r.amount)
    }

    pub fn max(&self, name: &str) -> f32 {
        self.entries
            .iter()
            .find(|r| r.name == name)
            .map_or(0.0, |r| r.max)
    }

    /// Whether every cost can be paid.
    pub fn can_pay(&self, costs: &[(String, f32)]) -> bool {
        costs.iter().all(|(n, a)| self.get(n) + 1e-4 >= *a)
    }

    /// Pay the costs; false (and nothing paid) if one can't be.
    pub fn pay(&mut self, costs: &[(String, f32)]) -> bool {
        if !self.can_pay(costs) {
            return false;
        }
        for (n, a) in costs {
            if let Some(r) = self.entries.iter_mut().find(|r| &r.name == n) {
                r.amount = (r.amount - a).max(0.0);
            }
        }
        true
    }

    /// Refill the resources that replenish every turn (`ReplenishType` `Turn`).
    pub fn replenish_turn(&mut self, data: &CombatData) {
        for r in &mut self.entries {
            let turn = data
                .resource(&r.name)
                .is_none_or(|d| d.replenish == data::Replenish::Turn);
            if turn {
                r.amount = r.max;
            }
        }
    }
}

/// What a character fights with.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Equipment {
    pub melee_main: Option<String>,
    pub melee_off: Option<String>,
    pub ranged_main: Option<String>,
    pub body: Option<String>,
    pub shield: Option<String>,
}

/// A character's combat state.
#[derive(Clone, Debug, PartialEq)]
pub struct Combatant {
    pub id: Guid,
    /// The fight the character is in.
    pub combat: Option<Guid>,
    /// Last initiative roll result (die + bonus).
    pub initiative: i32,
    pub resources: Resources,
    pub equipment: Equipment,
    /// Spells the character can cast (weapon attacks included).
    pub spells: Vec<String>,
    /// `Character` stats `Armor` (base AC without equipment).
    pub base_armor: i32,
    pub proficiency: Vec<String>,
    pub unarmed_ability: Ability,
    pub resistances: Vec<(DamageType, Resistance)>,
    /// `Character` stats `Initiative` and `DefaultBoosts` AC boosts.
    pub base_initiative: i32,
    pub ac_boosts: i32,
    pub combat_group: Option<String>,
    /// Downed (0 HP, a party member): makes death saving throws instead of acting.
    pub downed: bool,
    pub death_saves: (u8, u8),
    pub dead: bool,
}

/// How turns are shared: consecutive entries with the same non-`None` key act as one group.
// bg3: esv::GetShareInitiativeParameters (0x10588e1d0, 4.1.1.7631656): a player-controlled
// character that has not lost control shares with adjacent ones; an AI swarm member shares
// with adjacent members of the same swarm group (swarms not modelled); anything else acts alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Share {
    None,
    Player,
    Swarm(String),
}

/// Characters that act together (a shared turn).
#[derive(Clone, Debug, PartialEq)]
pub struct TurnGroup {
    pub members: Vec<Guid>,
    pub share: Share,
}

/// One fight.
#[derive(Clone, Debug, PartialEq)]
pub struct Combat {
    /// The combat's GUID (the story's `CombatGetGuidFor`); generated by the sim.
    pub id: Guid,
    pub round: i32,
    /// Participants in joining order.
    pub participants: Vec<Guid>,
    /// Turn order of the round.
    pub order: Vec<TurnGroup>,
    /// Index of the group whose turn it is.
    pub current: usize,
    /// Members of the current group who ended their turn.
    pub ended: Vec<Guid>,
    pub finished: bool,
    /// Members of the finished group whose story `TurnEnded` is still due (the engine raises
    /// it after the next group's `TurnStarted`).
    pub turn_ended_pending: Vec<Guid>,
}

impl Combat {
    pub fn current_group(&self) -> Option<&TurnGroup> {
        self.order.get(self.current)
    }

    /// Whether `who` may act now (in the current group and not done).
    pub fn is_active(&self, who: Guid) -> bool {
        !self.finished
            && self
                .current_group()
                .is_some_and(|g| g.members.contains(&who))
            && !self.ended.contains(&who)
    }

    /// The flat turn order.
    pub fn turn_order(&self) -> Vec<Guid> {
        self.order
            .iter()
            .flat_map(|g| g.members.iter().copied())
            .collect()
    }
}

/// What happened, for the presentation layer.
#[derive(Clone, Debug, PartialEq)]
pub enum CombatEvent {
    CombatStarted {
        combat: Guid,
        participants: Vec<Guid>,
    },
    Joined {
        combat: Guid,
        entity: Guid,
    },
    /// An initiative roll: `die` + `bonus` = `total`.
    Initiative {
        entity: Guid,
        die: u32,
        faces: u32,
        bonus: i32,
        total: i32,
    },
    RoundStarted {
        combat: Guid,
        round: i32,
    },
    TurnStarted {
        entity: Guid,
    },
    TurnEnded {
        entity: Guid,
    },
    Moved {
        entity: Guid,
        distance: f32,
        movement_left: f32,
    },
    SpellCast {
        caster: Guid,
        spell: String,
        target: Guid,
    },
    /// An attack roll, saving throw or raw die roll, with its breakdown.
    Roll {
        roll: RollEvent,
        text: String,
    },
    Damage {
        source: Guid,
        target: Guid,
        amount: i32,
        raw: i32,
        damage_type: DamageType,
        critical: bool,
        hp: i32,
        max_hp: i32,
    },
    StatusApplied {
        target: Guid,
        status: String,
    },
    Healed {
        target: Guid,
        amount: i32,
        hp: i32,
    },
    Downed {
        entity: Guid,
    },
    DeathSave {
        entity: Guid,
        roll: u32,
        successes: u8,
        failures: u8,
    },
    Died {
        entity: Guid,
    },
    CombatEnded {
        combat: Guid,
    },
    /// A line of the rules engine's text log.
    Log(String),
}

/// All combats and combatants.
#[derive(Clone, Debug, Default)]
pub struct CombatState {
    pub(crate) pathfinder: Option<std::sync::Arc<dyn Pathfinder>>,
    pub combats: Vec<Combat>,
    pub combatants: HashMap<Guid, Combatant>,
    next_id: u32,
    /// Story action ids handed out (UsingSpell … CastedSpell carry one).
    pub story_actions: i32,
    pub(crate) events: Vec<CombatEvent>,
    /// Objects the story took off stage (`SetOnStage(x, 0)`); they don't join fights.
    pub off_stage: std::collections::HashSet<Guid>,
    /// Race and class of characters made at runtime (the created avatar), which `Origins.lsx`
    /// does not have; see [`Sim::origin_def`].
    pub created_origins: HashMap<Guid, data::OriginDef>,
    /// Characters made by character creation (`crate::cc`): their starting equipment,
    /// proficiencies and spells ([`crate::cc::apply::combat_setup`]).
    pub created_characters: HashMap<Guid, crate::cc::CreatedCharacter>,
}

/// `ActionResource(Name, amount, level)` boosts of a progression (unconditional, fixed
/// amounts only).
fn progression_resources(boosts: &str) -> Vec<(String, f32)> {
    let Ok(entries) = bg3_rules::fields::parse_boosts(boosts) else {
        return Vec::new();
    };
    entries
        .into_iter()
        .filter_map(|e| match e {
            bg3_rules::fields::BoostEntry::Boost(b)
                if b.condition.is_none() && b.name == "ActionResource" =>
            {
                let name = b.args.first()?.trim().to_owned();
                let amount: f32 = b.args.get(1)?.trim().parse().ok()?;
                Some((name, amount))
            }
            _ => None,
        })
        .collect()
}

/// The spell lists of `AddSpells(list, …)` selectors.
fn progression_spell_lists(selectors: &str) -> Vec<Guid> {
    selectors
        .split(';')
        .filter_map(|s| {
            let rest = s.trim().strip_prefix("AddSpells(")?;
            Guid::parse(rest.split([',', ')']).next()?.trim())
        })
        .collect()
}

/// Distance between two positions (metres, straight line).
pub fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

impl Sim {
    // ---- setup ---------------------------------------------------------------------------

    /// The `ObjectCombat` fields of an entity: its own, falling back field by field along its
    /// template chain.
    fn object_combat(&self, id: Guid) -> data::ObjectCombat {
        let d = &self.data.combat;
        let e = self.entity(id);
        let mut out = d.objects.get(&id).cloned().unwrap_or_default();
        let chain = self.data.template_chain(e.and_then(|e| e.template));
        for t in chain {
            if let Some(c) = d.templates.get(&t.id) {
                if out.equipment.is_none() {
                    out.equipment = c.equipment.clone();
                }
                if out.spell_set.is_none() {
                    out.spell_set = c.spell_set.clone();
                }
                if out.skills.is_empty() {
                    out.skills = c.skills.clone();
                }
                if out.combat_group.is_none() {
                    out.combat_group = c.combat_group.clone();
                }
            }
        }
        out
    }

    /// Race and class of an origin character (`Origins.lsx`) or of a character made at
    /// runtime ([`CombatState::created_origins`]).
    pub fn origin_def(&self, id: Guid) -> Option<&data::OriginDef> {
        self.combat
            .created_origins
            .get(&id)
            .or_else(|| self.data.combat.origins.get(&id))
    }

    /// Build a character's combat state from the data (`None` for non-characters and
    /// characters without a `Character` stats entry).
    pub fn build_combatant(&self, id: Guid) -> Option<Combatant> {
        let e = self.entity(id)?;
        if e.kind != EntityKind::Character {
            return None;
        }
        let sheet = e.sheet.as_ref()?;
        let d = &self.data.combat;
        let def = e
            .stats
            .as_deref()
            .and_then(|s| d.characters.get(s))
            .cloned()
            .unwrap_or_default();
        let base_armor = e
            .stats
            .as_deref()
            .and_then(|s| self.data.characters.get(s))
            .map_or(sheet.armor_class, |c| c.armor);
        let oc = self.object_combat(id);
        let mut equipment = Equipment::default();
        // Each `add equipmentgroup` holds alternatives; the first entry with stats is taken.
        // *Approximate*: the engine's choice between alternatives is not known.
        for group in oc
            .equipment
            .as_deref()
            .and_then(|n| d.equipment_sets.get(n))
            .into_iter()
            .flatten()
        {
            let Some(item) = group
                .iter()
                .find(|i| d.weapons.contains_key(*i) || d.armors.contains_key(*i))
            else {
                continue;
            };
            if let Some(w) = d.weapons.get(item) {
                let slot = match w.slot {
                    data::Slot::RangedMain | data::Slot::RangedOff => &mut equipment.ranged_main,
                    data::Slot::MeleeOff => &mut equipment.melee_off,
                    _ => &mut equipment.melee_main,
                };
                if slot.is_none() {
                    *slot = Some(item.clone());
                }
            } else if let Some(a) = d.armors.get(item) {
                let slot = if a.shield {
                    &mut equipment.shield
                } else if a.slot == data::Slot::Breast {
                    &mut equipment.body
                } else {
                    continue;
                };
                if slot.is_none() {
                    *slot = Some(item.clone());
                }
            }
        }
        // Known spells: the skill list, the spell set, and the weapon attacks the equipment
        // allows. *Approximate*: the engine grants spells through progressions, boosts
        // (`UnlockSpell`) and weapon `BoostsOnEquip…`, which are not modelled.
        let mut spells: Vec<String> = Vec::new();
        let mut add = |s: &str| {
            if d.spells.contains_key(s) && !spells.iter().any(|x| x == s) {
                spells.push(s.to_owned());
            }
        };
        if equipment.melee_main.is_some() {
            add("Target_MainHandAttack");
        }
        if equipment.ranged_main.is_some() {
            add("Projectile_MainHandAttack");
        }
        for s in &oc.skills {
            add(s);
        }
        for s in oc
            .spell_set
            .as_deref()
            .and_then(|n| d.spell_sets.get(n))
            .into_iter()
            .flatten()
        {
            add(s);
        }
        if equipment.melee_main.is_none() {
            add("Target_UnarmedAttack");
        }
        let resources = Resources {
            entries: def
                .action_resources
                .iter()
                .map(|(n, a)| ResourceAmount {
                    name: n.clone(),
                    amount: *a,
                    max: *a,
                })
                .collect(),
        };
        // Origin characters: what their race and class progressions grant up to their level.
        // `ActionResource(Name, amount, level)` boosts (e.g. the race's Movement) and fixed
        // spell grants (`AddSpells(list)` selectors). *Approximate*: choices (`SelectSpells`,
        // …), passives and other boosts are not modelled.
        let mut resources = resources;
        if let Some(origin) = self.origin_def(id) {
            for t in d.origin_tables(origin) {
                for p in d.progressions.get(&t).into_iter().flatten() {
                    if p.level > sheet.level {
                        continue;
                    }
                    for (name, amount) in progression_resources(&p.boosts) {
                        match resources.entries.iter_mut().find(|r| r.name == name) {
                            Some(r) => {
                                r.max += amount;
                                r.amount = r.max;
                            }
                            None => resources.entries.push(ResourceAmount {
                                name,
                                amount,
                                max: amount,
                            }),
                        }
                    }
                    for list in progression_spell_lists(&p.selectors) {
                        for s in d.spell_lists.get(&list).into_iter().flatten() {
                            if d.spells.contains_key(s) && !spells.contains(s) {
                                spells.push(s.clone());
                            }
                        }
                    }
                }
            }
        }
        let mut c = Combatant {
            id,
            combat: None,
            initiative: 0,
            resources,
            equipment,
            spells,
            base_armor,
            proficiency: def.proficiency.clone(),
            unarmed_ability: def.unarmed_ability.unwrap_or(Ability::Strength),
            resistances: def.resistances.clone(),
            base_initiative: def.initiative,
            ac_boosts: def.ac_boosts,
            combat_group: oc.combat_group,
            downed: false,
            death_saves: (0, 0),
            dead: sheet.hp <= 0,
        };
        if let Some(cc) = self.combat.created_characters.get(&id) {
            crate::cc::apply::combat_setup(d, cc, &mut c);
        }
        c.resources.replenish_turn(d);
        Some(c)
    }

    /// Make sure a character has combat state (and its armour class from its equipment).
    /// False for entities that can't fight.
    pub fn ensure_combatant(&mut self, id: Guid) -> bool {
        if self.combat.combatants.contains_key(&id) {
            return true;
        }
        let Some(c) = self.build_combatant(id) else {
            return false;
        };
        self.combat.combatants.insert(id, c);
        self.update_armor_class(id);
        self.update_class_hp(id);
        true
    }

    pub fn combatant(&self, id: Guid) -> Option<&Combatant> {
        self.combat.combatants.get(&id)
    }

    pub fn combatant_mut(&mut self, id: Guid) -> Option<&mut Combatant> {
        self.combat.combatants.get_mut(&id)
    }

    /// Origin characters' `Character` stats entries carry placeholder hit points (`Vitality`
    /// 1); their real maximum comes from their class (see [`CombatData::class_hp`]). Sets
    /// the maximum and fills the character up.
    // *Approximate*: no class progressions, boosts or the created avatar's own class.
    pub fn update_class_hp(&mut self, id: Guid) {
        let Some(class) = self.origin_def(id).and_then(|o| o.class) else {
            return;
        };
        let d = &self.data.combat;
        let Some(sheet) = self.entities.get(id).and_then(|e| e.sheet.as_ref()) else {
            return;
        };
        let con = (sheet.ability(Ability::Constitution) - 10).div_euclid(2);
        let Some(max) = d.class_hp(class, sheet.level, con) else {
            return;
        };
        if let Some(s) = self.entities.get_mut(id).and_then(|e| e.sheet.as_mut()) {
            s.max_hp = max;
            s.hp = max;
        }
    }

    /// Recompute a character's armour class.
    ///
    /// With body armour: the armour's `ArmorClass` plus the modifier of its
    /// `Armor Class Ability`, capped at `Ability Modifier Cap` when that is positive. Without:
    /// the `Character` entry's `Armor` plus the Dexterity modifier. Then a shield's
    /// `ArmorClass` and `AC(n)` boosts (default boosts and active statuses).
    // bg3: esv::StatsSystem::CalculateArmorClass (0x1057bdbc8, 4.1.1.7631656). Not modelled:
    // the ruleset's GlobalBaseACModifier, armour-type cap overrides from boosts,
    // ACOverrideFormula boosts (Unarmoured Defence), the AC blocker of ability bonuses.
    pub fn update_armor_class(&mut self, id: Guid) {
        let Some(c) = self.combat.combatants.get(&id) else {
            return;
        };
        let d = &self.data.combat;
        let Some(e) = self.entities.get(id) else {
            return;
        };
        let Some(sheet) = e.sheet.as_ref() else {
            return;
        };
        let modifier = |a: Ability| (sheet.ability(a) - 10).div_euclid(2);
        let mut ac = match c.equipment.body.as_deref().and_then(|b| d.armors.get(b)) {
            Some(a) => {
                let m = a.ability.map_or(0, modifier);
                let m = if a.ability_cap > 0 {
                    m.min(a.ability_cap)
                } else {
                    m
                };
                a.armor_class + m
            }
            None => c.base_armor + modifier(Ability::Dexterity),
        };
        if let Some(s) = c.equipment.shield.as_deref().and_then(|s| d.armors.get(s)) {
            ac += s.armor_class;
        }
        ac += c.ac_boosts;
        ac += e.statuses.iter().map(|s| d.status_ac(&s.id)).sum::<i32>();
        if let Some(sheet) = self.entities.get_mut(id).and_then(|e| e.sheet.as_mut()) {
            sheet.armor_class = ac;
        }
    }

    /// The rules engine's view of a combatant's weapon for an attack type.
    pub fn combat_weapon(&self, id: Guid, ranged: bool, hand: Hand) -> Option<Weapon> {
        let c = self.combatant(id)?;
        let d = &self.data.combat;
        let name = match (ranged, hand) {
            (true, Hand::Main) => c.equipment.ranged_main.as_deref(),
            (false, Hand::Main) => c.equipment.melee_main.as_deref(),
            (false, Hand::Off) => c.equipment.melee_off.as_deref(),
            (true, Hand::Off) => None,
        }?;
        let w = d.weapons.get(name)?;
        // Characters without any proficiency data (player characters, whose proficiencies
        // come from their class progression, which is not modelled) count as proficient.
        let proficient =
            c.proficiency.is_empty() || w.proficiency.iter().any(|p| c.proficiency.contains(p));
        Some(Weapon {
            name: w.name.clone(),
            damage: w.damage,
            damage_type: w.damage_type,
            ranged: w.ranged(),
            finesse: w.has("Finesse"),
            proficient,
        })
    }

    // ---- relations -----------------------------------------------------------------------

    /// The relation of `a` towards `b` (0 hostile, 50 neutral, 100 allied) by faction.
    // bg3: eoc::relation::GetRelation (0x101c4fcec) → (anonymous)::GetRelation (0x101c4f9a4)
    // (4.1.1.7631656). Individual relations, attitudes and the party rule are not modelled.
    pub fn relation(&self, a: Guid, b: Guid) -> i32 {
        if a == b {
            return data::RELATION_ALLIED;
        }
        let (Some(ea), Some(eb)) = (self.entity(a), self.entity(b)) else {
            return data::RELATION_NEUTRAL;
        };
        let pa = self.party.contains(&a) || ea.user.is_some();
        let pb = self.party.contains(&b) || eb.user.is_some();
        if pa && pb {
            return data::RELATION_ALLIED;
        }
        match (ea.faction, eb.faction) {
            (Some(fa), Some(fb)) if fa == fb => data::RELATION_ALLIED,
            (Some(fa), Some(fb)) => self.data.combat.factions.relation(fa, fb),
            _ => data::RELATION_NEUTRAL,
        }
    }

    /// `IsEnemy`: the relation of either towards the other is hostile.
    // bg3: esv::OsirisGameFunctions::IsEnemy (0x1055646f0): relation == 0.
    pub fn is_hostile(&self, a: Guid, b: Guid) -> bool {
        self.relation(a, b) == data::RELATION_HOSTILE
            || self.relation(b, a) == data::RELATION_HOSTILE
    }

    /// Player-controlled (in the party or given to a user).
    pub fn is_player_controlled(&self, id: Guid) -> bool {
        self.party.contains(&id) || self.entity(id).is_some_and(|e| e.user.is_some())
    }

    // ---- combats -------------------------------------------------------------------------

    pub fn combat_of(&self, id: Guid) -> Option<&Combat> {
        let cid = self.combatant(id)?.combat?;
        self.combat_by_id(cid)
    }

    pub fn combat_by_id(&self, cid: Guid) -> Option<&Combat> {
        self.combat.combats.iter().find(|c| c.id == cid)
    }

    fn combat_index(&self, cid: Guid) -> Option<usize> {
        self.combat.combats.iter().position(|c| c.id == cid)
    }

    pub fn in_combat(&self, id: Guid) -> bool {
        self.combat_of(id).is_some_and(|c| !c.finished)
    }

    pub fn take_combat_events(&mut self) -> Vec<CombatEvent> {
        std::mem::take(&mut self.combat.events)
    }

    pub(crate) fn combat_event(&mut self, e: CombatEvent) {
        self.combat.events.push(e);
    }

    fn alive(&self, id: Guid) -> bool {
        self.combatant(id).is_some_and(|c| !c.dead)
            && self
                .entity(id)
                .and_then(|e| e.sheet.as_ref())
                .is_some_and(|s| s.hp > 0)
    }

    /// Who joins a fight started by `seeds`: the seeds, everyone sharing a `CombatGroupID`
    /// with a participant, and characters allied to a participant within
    /// `JoinAllyInCombatRadius`, repeated until nothing changes.
    // bg3: esv::combat::System::ProcessStartCombat (0x1052835d0) reads JoinAllyInCombatRadius;
    // the rest of its selection (sight, CanJoinCombat, dialogs, surprise) is *approximate*.
    fn gather_participants(&mut self, seeds: &[Guid]) -> Vec<Guid> {
        let mut out: Vec<Guid> = Vec::new();
        for &s in seeds {
            if self.ensure_combatant(s) && self.alive(s) && !out.contains(&s) {
                out.push(s);
            }
        }
        let radius = self.data.combat.join_ally_radius;
        let level = self.level.clone();
        let candidates: Vec<Guid> = self
            .entities
            .iter()
            .filter(|e| e.kind == EntityKind::Character && e.sheet.is_some())
            .filter(|e| e.global || e.level == level || self.party.contains(&e.id))
            .map(|e| e.id)
            .collect();
        loop {
            let mut added = false;
            for &c in &candidates {
                if out.contains(&c)
                    || self.combat.off_stage.contains(&c)
                    || !self.on_stage(c)
                    || !self.ensure_combatant(c)
                    || !self.alive(c)
                {
                    continue;
                }
                if self.combat_of(c).is_some_and(|x| !x.finished) {
                    continue;
                }
                let group = self.combatant(c).and_then(|x| x.combat_group.clone());
                let pos = self.entity(c).map_or([0.0; 3], |e| e.position);
                let joins = out.iter().any(|&p| {
                    let same_group = group.is_some()
                        && self.combatant(p).and_then(|x| x.combat_group.as_ref())
                            == group.as_ref();
                    let near_ally = self.relation(c, p) == data::RELATION_ALLIED
                        && self
                            .entity(p)
                            .is_some_and(|e| distance(e.position, pos) <= radius);
                    same_group || near_ally
                });
                if joins {
                    out.push(c);
                    added = true;
                }
            }
            if !added {
                break;
            }
        }
        out
    }

    /// The initiative bonus: Dexterity modifier + the `Character` entry's `Initiative`
    /// (negative values count as 0) + `Initiative(n)` boosts of active statuses.
    // bg3: esv::StatsSystem::CalculateInitiative (0x1057bd3ac): BaseStats initiative + the
    // InitiativeBoost of every active boost + floor((Dexterity - 10) / 2); the base is the
    // stats `Initiative` clamped at 0 in esv::BaseStatsSystem::Update (4.1.1.7631656).
    pub fn initiative_bonus(&self, id: Guid) -> i32 {
        let Some(e) = self.entity(id) else {
            return 0;
        };
        let dex = e
            .sheet
            .as_ref()
            .map_or(0, |s| (s.ability(Ability::Dexterity) - 10).div_euclid(2));
        let base = self.combatant(id).map_or(0, |c| c.base_initiative.max(0));
        let boosts: i32 = e
            .statuses
            .iter()
            .map(|s| self.data.combat.status_initiative(&s.id))
            .sum();
        dex + base + boosts
    }

    /// Roll initiative: `InitiativeDie` (d4 in the reference build) + the bonus.
    // bg3: esv::combat::System::NewInitiativeRoll (0x10528e328) → SystemHelper::RollDie
    // (0x1052684b0), the die from Data.txt `InitiativeDie` mapped to EDiceType in
    // ProcessStartCombat (4.1.1.7631656). Not modelled: swarm members reuse their swarm
    // group's roll; the engine draws from a per-entity roll stream.
    pub fn roll_initiative(&mut self, id: Guid) -> i32 {
        use bg3_rules::dice::DiceRng;
        let faces = match self.data.combat.initiative_die {
            0 => 20,
            f => f,
        };
        let die = self.dice.roll_die(faces);
        let bonus = self.initiative_bonus(id);
        let total = die as i32 + bonus;
        if let Some(c) = self.combatant_mut(id) {
            c.initiative = total;
        }
        self.combat_event(CombatEvent::Initiative {
            entity: id,
            die,
            faces,
            bonus,
            total,
        });
        total
    }

    /// Turn order comparison: higher initiative first; then higher Dexterity score; then
    /// player characters; then the lower UUID; then the lower handle.
    // bg3: the comparator of eoc::TurnOrderSort (0x1020e1fd8, 4.1.1.7631656). It also orders
    // non-swarm before swarm members and swarm groups by name (swarms not modelled). The UUID
    // is compared as the engine's two 64-bit halves; which half the sim's u128 puts first is
    // *unverified*.
    pub fn turn_order_cmp(&self, a: Guid, b: Guid) -> Ordering {
        let init = |x: Guid| self.combatant(x).map_or(0, |c| c.initiative);
        let dex = |x: Guid| {
            self.entity(x)
                .and_then(|e| e.sheet.as_ref())
                .map_or(10, |s| s.ability(Ability::Dexterity))
        };
        init(b)
            .cmp(&init(a))
            .then(dex(b).cmp(&dex(a)))
            .then(
                self.is_player_controlled(b)
                    .cmp(&self.is_player_controlled(a)),
            )
            .then(a.0.cmp(&b.0))
            .then(self.entities.handle(a).cmp(&self.entities.handle(b)))
    }

    fn share_key(&self, id: Guid) -> Share {
        if self.is_player_controlled(id) {
            Share::Player
        } else {
            Share::None
        }
    }

    /// Sort participants and merge consecutive shared turns into groups.
    fn build_order(&self, participants: &[Guid]) -> Vec<TurnGroup> {
        let mut list: Vec<Guid> = participants
            .iter()
            .copied()
            .filter(|&p| self.combatant(p).is_some_and(|c| !c.dead))
            .collect();
        list.sort_by(|&a, &b| self.turn_order_cmp(a, b));
        let mut out: Vec<TurnGroup> = Vec::new();
        for p in list {
            let share = self.share_key(p);
            match out.last_mut() {
                Some(g) if share != Share::None && g.share == share => g.members.push(p),
                _ => out.push(TurnGroup {
                    members: vec![p],
                    share,
                }),
            }
        }
        out
    }

    /// `EnterCombat(source, target)`: start a fight between them (or pull the newcomer into
    /// the fight the other is in). Returns the combat GUID.
    ///
    /// Story events, in this order (*order unverified*): `EnteredCombat(x, combat)` per
    /// participant, `CombatStarted(combat)`, `CombatRoundStarted(combat, 1)`, then
    /// `TurnStarted` for the first group.
    pub fn enter_combat(&mut self, source: Guid, target: Guid) -> Option<Guid> {
        if !self.ensure_combatant(source) || !self.ensure_combatant(target) {
            return None;
        }
        let existing = [source, target]
            .iter()
            .find_map(|&x| self.combat_of(x).filter(|c| !c.finished).map(|c| c.id));
        if let Some(cid) = existing {
            for x in [source, target] {
                self.join_combat(cid, x);
            }
            return Some(cid);
        }
        let participants = self.gather_participants(&[source, target]);
        if participants.len() < 2 {
            return None;
        }
        self.combat.next_id += 1;
        let cid = Guid(0xC0BA_7000_0000_4000_8000_0000_0000_0000 | self.combat.next_id as u128);
        for &p in &participants {
            if let Some(c) = self.combatant_mut(p) {
                c.combat = Some(cid);
            }
            let d = self.data.clone();
            if let Some(c) = self.combatant_mut(p) {
                c.resources.replenish_turn(&d.combat);
            }
        }
        for &p in &participants {
            self.roll_initiative(p);
        }
        let order = self.build_order(&participants);
        self.combat.combats.push(Combat {
            id: cid,
            round: 1,
            participants: participants.clone(),
            order,
            current: 0,
            ended: Vec::new(),
            finished: false,
            turn_ended_pending: Vec::new(),
        });
        self.combat_event(CombatEvent::CombatStarted {
            combat: cid,
            participants: participants.clone(),
        });
        let cs = SimValue::Guid(cid.to_string());
        for &p in &participants {
            let who = self.render(p);
            self.push_event("EnteredCombat", vec![SimValue::Guid(who), cs.clone()]);
        }
        self.push_event("CombatStarted", vec![cs.clone()]);
        self.push_event("CombatRoundStarted", vec![cs, SimValue::Int(1)]);
        self.combat_event(CombatEvent::RoundStarted {
            combat: cid,
            round: 1,
        });
        self.start_group(cid);
        Some(cid)
    }

    /// Add a character to a running fight: initiative is rolled and it is placed in the
    /// order of the next round (*approximate*: the engine can also slot late joiners into the
    /// current round).
    pub fn join_combat(&mut self, cid: Guid, who: Guid) -> bool {
        if !self.ensure_combatant(who) || self.combat_of(who).is_some_and(|c| c.id == cid) {
            return false;
        }
        let Some(i) = self.combat_index(cid) else {
            return false;
        };
        if let Some(c) = self.combatant_mut(who) {
            c.combat = Some(cid);
        }
        self.roll_initiative(who);
        self.combat.combats[i].participants.push(who);
        let who_s = self.render(who);
        self.push_event(
            "EnteredCombat",
            vec![SimValue::Guid(who_s), SimValue::Guid(cid.to_string())],
        );
        self.combat_event(CombatEvent::Joined {
            combat: cid,
            entity: who,
        });
        true
    }

    /// Start the turn of the current group: refill per-turn resources, raise `TurnStarted`.
    /// Downed members roll their death saving throw and end their turn at once.
    fn start_group(&mut self, cid: Guid) {
        let Some(i) = self.combat_index(cid) else {
            return;
        };
        let members = self.combat.combats[i]
            .current_group()
            .map(|g| g.members.clone())
            .unwrap_or_default();
        let data = self.data.clone();
        let round = self.combat.combats[i].round;
        let shared = members.len() > 1;
        for (k, m) in members.iter().enumerate() {
            if let Some(c) = self.combatant_mut(*m) {
                c.resources.replenish_turn(&data.combat);
            }
            let who = self.render(*m);
            self.push_event("TurnStarted", vec![SimValue::Guid(who.clone())]);
            self.combat_event(CombatEvent::TurnStarted { entity: *m });
            // *Verified by trace* (combat-20261021-183321, imp fight): a shared group raises
            // `ShareInitiative(x)` after each member's `TurnStarted`, except after the last
            // member in round 1. Why the last one is skipped in the first round is unknown.
            if shared && (round > 1 || k + 1 < members.len()) {
                self.push_event("ShareInitiative", vec![SimValue::Guid(who)]);
            }
        }
        let pending = std::mem::take(&mut self.combat.combats[i].turn_ended_pending);
        for p in pending {
            let w = self.render(p);
            self.push_event("TurnEnded", vec![SimValue::Guid(w)]);
        }
        for m in members {
            if self.combatant(m).is_some_and(|c| c.downed || c.dead) {
                if self.combatant(m).is_some_and(|c| c.downed) {
                    self.death_saving_throw(m);
                }
                self.combat_end_turn(m).ok();
            }
        }
    }

    /// End `who`'s turn; when the whole group is done the next group starts (and after the
    /// last group, the next round).
    pub fn combat_end_turn(&mut self, who: Guid) -> Result<(), CombatError> {
        let cid = self
            .combat_of(who)
            .filter(|c| c.is_active(who))
            .map(|c| c.id)
            .ok_or(CombatError::NotYourTurn)?;
        let i = self.combat_index(cid).ok_or(CombatError::NotInCombat)?;
        self.combat.combats[i].ended.push(who);
        // The story's `TurnEnded` follows the next group's `TurnStarted` (see start_group).
        self.combat.combats[i].turn_ended_pending.push(who);
        self.combat_event(CombatEvent::TurnEnded { entity: who });
        let done = {
            let c = &self.combat.combats[i];
            c.current_group()
                .is_none_or(|g| g.members.iter().all(|m| c.ended.contains(m)))
        };
        if done {
            self.advance_turn(cid);
        }
        Ok(())
    }

    /// Move to the next group with a living member; start a new round after the last.
    fn advance_turn(&mut self, cid: Guid) {
        let Some(i) = self.combat_index(cid) else {
            return;
        };
        if self.combat.combats[i].finished {
            return;
        }
        loop {
            let c = &mut self.combat.combats[i];
            c.ended.clear();
            c.current += 1;
            if c.current >= c.order.len() {
                c.round += 1;
                let round = c.round;
                let parts = c.participants.clone();
                let order = self.build_order(&parts);
                let c = &mut self.combat.combats[i];
                c.order = order;
                c.current = 0;
                self.push_event(
                    "CombatRoundStarted",
                    vec![SimValue::Guid(cid.to_string()), SimValue::Int(round)],
                );
                self.combat_event(CombatEvent::RoundStarted { combat: cid, round });
            }
            let c = &self.combat.combats[i];
            let Some(g) = c.current_group() else {
                return;
            };
            if g.members
                .iter()
                .any(|m| self.combatant(*m).is_some_and(|x| !x.dead))
            {
                break;
            }
        }
        self.start_group(cid);
    }

    /// End the fight if no two living participants are hostile to each other: `LeftCombat`
    /// for each participant, then `CombatEnded` (*order unverified*).
    pub fn check_combat_end(&mut self, cid: Guid) -> bool {
        let Some(i) = self.combat_index(cid) else {
            return false;
        };
        if self.combat.combats[i].finished {
            return true;
        }
        let alive: Vec<Guid> = self.combat.combats[i]
            .participants
            .iter()
            .copied()
            .filter(|p| self.combatant(*p).is_some_and(|c| !c.dead && !c.downed))
            .collect();
        let fighting = alive
            .iter()
            .any(|&a| alive.iter().any(|&b| self.is_hostile(a, b)));
        if fighting {
            return false;
        }
        // *Verified by trace* (combat-20261021-183321): when the last enemy falls, the acting
        // group's `TurnEnded` events come first, then `LeftCombat` for each remaining
        // participant, then `CombatEnded`. (The trace also shows a `CombatRoundStarted` just
        // before `CombatEnded`, which the sim does not raise.)
        let c = &self.combat.combats[i];
        let mut turn_ended = c.turn_ended_pending.clone();
        if let Some(g) = c.current_group() {
            turn_ended.extend(g.members.iter().filter(|m| !c.ended.contains(m)).copied());
        }
        let parts = c.participants.clone();
        self.combat.combats[i].finished = true;
        self.combat.combats[i].turn_ended_pending.clear();
        for m in turn_ended {
            let w = self.render(m);
            self.push_event("TurnEnded", vec![SimValue::Guid(w)]);
            self.combat_event(CombatEvent::TurnEnded { entity: m });
        }
        let cs = SimValue::Guid(cid.to_string());
        for p in parts {
            let still_in = self.combatant(p).is_some_and(|c| c.combat == Some(cid));
            if let Some(c) = self.combatant_mut(p) {
                c.combat = None;
            }
            if still_in {
                let who = self.render(p);
                self.push_event("LeftCombat", vec![SimValue::Guid(who), cs.clone()]);
            }
        }
        self.push_event("CombatEnded", vec![cs]);
        self.combat_event(CombatEvent::CombatEnded { combat: cid });
        true
    }
}
