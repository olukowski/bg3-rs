//! The rules of character creation: what a choice consists of, which choices the level-1
//! progressions ask for (their *selectors*), point buy, validation, and the character a valid
//! choice makes.
//!
//! What the engine does is spread over `eoc::character_creation::Validate`,
//! `_private::ApplyBasics` / `ApplyRaceProgression` / `ApplyProgression` /
//! `ApplyAbilitySelectors` / `ApplyAbilityBonusSelectors` / `ApplySpellSelectors` /
//! `ApplyEquimentSelectors` and the `Validate*Selectors` functions (4.1.1.7631656, names
//! from the symbol table; their bodies were not read). The behaviour here follows the data
//! and is marked *approximate* where it is our reading of it.

use std::collections::HashSet;
use std::fmt;

use bg3_rules::context::{Ability, Skill};
use bg3_rules::fields::{BoostEntry, parse_boosts};

use super::data::{CcData, DefaultKind, Progression};
use crate::guid::Guid;

/// Point buy: every ability starts at 8 and can be raised to 15 before bonuses, for 27
/// points; 9–13 cost one point per step, 14 and 15 two.
///
/// *Approximate*: the D&D 5e point-buy table. The scores and budget are not in the data
/// files (they are the UI's `UsedAbilityPoints` / `TotalAbilityPoints`, computed in code); the
/// table is consistent with every class's `AbilityDistributionPresets` entry, each of which
/// spends exactly 27 points (test `cc_install::presets_spend_the_point_buy_budget`).
pub const POINT_BUY_BASE: i32 = 8;
pub const POINT_BUY_MAX: i32 = 15;
pub const POINT_BUY_POINTS: i32 = 27;

/// The point-buy cost of a score (`None` outside 8–15).
pub fn point_buy_cost(score: i32) -> Option<i32> {
    (POINT_BUY_BASE..=POINT_BUY_MAX)
        .contains(&score)
        .then(|| score - POINT_BUY_BASE + (score - 13).max(0))
}

/// Points spent on a set of scores (`None` if one is out of range).
pub fn point_buy_spent(scores: &[i32; 6]) -> Option<i32> {
    scores.iter().map(|s| point_buy_cost(*s)).sum()
}

fn modifier(score: i32) -> i32 {
    (score - 10).div_euclid(2)
}

/// The kind of a progression selector.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SelectorKind {
    /// Grants every spell of a list (no choice).
    AddSpells,
    SelectSpells,
    SelectSkills,
    SelectSkillsExpertise,
    SelectPassives,
    SelectEquipment,
    /// `+2`/`+1` (the amounts) to different abilities of a list.
    SelectAbilityBonus,
    /// `count` abilities of a list, `+amount` each.
    SelectAbilities,
    Other(String),
}

/// One selector of a progression's `Selectors` field.
#[derive(Clone, Debug, PartialEq)]
pub struct Selector {
    pub kind: SelectorKind,
    /// The list (`SpellLists`, `SkillLists`, …) it picks from.
    pub list: Guid,
    /// How many to pick.
    pub count: usize,
    /// Ability bonuses, one per pick (`SelectAbilityBonus`, `SelectAbilities`).
    pub amounts: Vec<i32>,
    /// `SelectorId`: groups selectors for the UI and `DefaultValues`.
    pub selector_id: Option<String>,
    /// The spellcasting ability spells from this selector use, if not the class's.
    pub ability: Option<Ability>,
}

fn arg(args: &[&str], i: usize) -> Option<String> {
    args.get(i)
        .map(|a| a.trim())
        .filter(|a| !a.is_empty())
        .map(str::to_owned)
}

fn count(args: &[&str], i: usize) -> usize {
    args.get(i).and_then(|a| a.trim().parse().ok()).unwrap_or(0)
}

/// Parse a `Selectors` field: `Name(arg, …)` calls separated by `;`.
pub fn parse_selectors(text: &str) -> Vec<Selector> {
    let mut out = Vec::new();
    for item in text.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        let Some((name, rest)) = item.split_once('(') else {
            continue;
        };
        let inner = rest.trim_end().trim_end_matches(')');
        let args: Vec<&str> = inner.split(',').collect();
        let Some(list) = args.first().and_then(|a| Guid::parse(a.trim())) else {
            continue;
        };
        let mut s = Selector {
            kind: SelectorKind::Other(name.trim().to_owned()),
            list,
            count: 0,
            amounts: Vec::new(),
            selector_id: None,
            ability: None,
        };
        match name.trim() {
            // AddSpells(list, selectorId, ability, actionResource, prepareType, cooldown)
            "AddSpells" => {
                s.kind = SelectorKind::AddSpells;
                s.selector_id = arg(&args, 1);
                s.ability = arg(&args, 2).and_then(|a| Ability::parse(&a));
            }
            // SelectSpells(list, count, replaceCount, selectorId, ability, …)
            "SelectSpells" => {
                s.kind = SelectorKind::SelectSpells;
                s.count = count(&args, 1);
                s.selector_id = arg(&args, 3);
                s.ability = arg(&args, 4).and_then(|a| Ability::parse(&a));
            }
            "SelectSkills" => {
                s.kind = SelectorKind::SelectSkills;
                s.count = count(&args, 1);
                s.selector_id = arg(&args, 2);
            }
            // SelectSkillsExpertise(list, count, livesInSkills?, selectorId)
            "SelectSkillsExpertise" => {
                s.kind = SelectorKind::SelectSkillsExpertise;
                s.count = count(&args, 1);
                s.selector_id =
                    arg(&args, 3).or_else(|| arg(&args, 2).filter(|a| a != "true" && a != "false"));
            }
            "SelectPassives" => {
                s.kind = SelectorKind::SelectPassives;
                s.count = count(&args, 1);
                s.selector_id = arg(&args, 2);
            }
            "SelectEquipment" => {
                s.kind = SelectorKind::SelectEquipment;
                s.count = count(&args, 1);
                s.selector_id = arg(&args, 2);
            }
            // SelectAbilityBonus(list, selectorId, amount, amount, …)
            "SelectAbilityBonus" => {
                s.kind = SelectorKind::SelectAbilityBonus;
                s.selector_id = arg(&args, 1);
                s.amounts = args
                    .iter()
                    .skip(2)
                    .filter_map(|a| a.trim().parse().ok())
                    .collect();
                s.count = s.amounts.len();
            }
            // SelectAbilities(list, count, amount, selectorId)
            "SelectAbilities" => {
                s.kind = SelectorKind::SelectAbilities;
                s.count = count(&args, 1);
                let amount = args.get(2).and_then(|a| a.trim().parse().ok()).unwrap_or(1);
                s.amounts = vec![amount; s.count];
                s.selector_id = arg(&args, 3);
            }
            _ => {}
        }
        out.push(s);
    }
    out
}

/// The player's picks for one selector.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pick {
    /// The progression row and the selector's index in its `Selectors`.
    pub progression: Guid,
    pub index: usize,
    /// Spell, skill, passive, item stats or ability names. For ability bonuses, in the order
    /// of the selector's amounts (`+2` first).
    pub items: Vec<String>,
}

/// What the player chose.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CharacterChoice {
    pub race: Guid,
    pub subrace: Option<Guid>,
    /// 0 masculine, 1 feminine.
    pub body_type: u8,
    /// 0 regular, 1 strong.
    pub body_shape: u8,
    pub class: Guid,
    pub subclass: Option<Guid>,
    pub background: Guid,
    pub deity: Option<Guid>,
    /// Point-buy scores before any bonus, Strength … Charisma. All zero: the class's
    /// `AbilityDistributionPresets` entry (see [`CcData::complete`]).
    pub abilities: [i32; 6],
    pub picks: Vec<Pick>,
    pub appearance: super::appearance::Appearance,
}

/// An open choice of a level-1 progression.
#[derive(Clone, Debug, PartialEq)]
pub struct SelectorSlot {
    pub progression: Guid,
    pub table: Guid,
    pub level: i32,
    /// Progression `Name` (`Cleric`, `Human`, …).
    pub source: String,
    pub index: usize,
    pub selector: Selector,
    /// What can be picked: the selector's list (spells the stats define only).
    pub options: Vec<String>,
}

impl SelectorSlot {
    /// A short label: `Cleric: 3 of spells`, `Human: 1 of skills (HumanVersatility)`.
    pub fn label(&self) -> String {
        let what = match &self.selector.kind {
            SelectorKind::SelectSpells => "spells",
            SelectorKind::SelectSkills => "skills",
            SelectorKind::SelectSkillsExpertise => "expertise",
            SelectorKind::SelectPassives => "passives",
            SelectorKind::SelectEquipment => "equipment",
            SelectorKind::SelectAbilityBonus => "ability bonus",
            SelectorKind::SelectAbilities => "abilities",
            SelectorKind::AddSpells => "spells (all)",
            SelectorKind::Other(n) => n,
        };
        let id = self
            .selector
            .selector_id
            .as_deref()
            .map(|i| format!(" ({i})"))
            .unwrap_or_default();
        format!("{}: {} of {what}{id}", self.source, self.selector.count)
    }

    fn default_kind(&self) -> DefaultKind {
        match self.selector.kind {
            SelectorKind::SelectSkills | SelectorKind::SelectSkillsExpertise => DefaultKind::Skills,
            SelectorKind::SelectPassives => DefaultKind::Passives,
            SelectorKind::SelectEquipment => DefaultKind::Equipments,
            SelectorKind::SelectAbilityBonus | SelectorKind::SelectAbilities => {
                DefaultKind::Abilities
            }
            _ => DefaultKind::Spells,
        }
    }
}

/// Why a choice is not valid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CcError {
    UnknownRace(Guid),
    /// The race has subraces and none (or one of another race) was chosen.
    BadSubrace,
    /// No body preset for the race, subrace and body type/shape.
    NoBodyPreset,
    UnknownClass(Guid),
    /// The class picks a subclass at level 1 and none (or a foreign one) was chosen, or it
    /// doesn't and one was.
    BadSubclass,
    UnknownBackground(Guid),
    /// The class follows a deity (`HasGod`) and none was chosen.
    DeityRequired,
    /// A deity the race excludes, an unknown one, or one for a class without a deity.
    BadDeity(Guid),
    /// A score outside 8–15 before bonuses.
    AbilityOutOfRange(Ability, i32),
    /// More than [`POINT_BUY_POINTS`] spent.
    TooManyPoints(i32),
    /// A selector without the right number of picks.
    PickCount {
        slot: String,
        expected: usize,
        got: usize,
    },
    NotAnOption {
        slot: String,
        item: String,
    },
    Duplicate {
        slot: String,
        item: String,
    },
    /// A skill the character is already proficient in (race, background, another pick).
    AlreadyProficient(String),
    /// Expertise in a skill without proficiency.
    NotProficient(String),
    /// A spell the character already has.
    AlreadyKnown(String),
    /// A pick for a selector the choice doesn't have.
    UnknownSelector(Guid, usize),
}

impl fmt::Display for CcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CcError::UnknownRace(g) => write!(f, "unknown race {g}"),
            CcError::BadSubrace => write!(f, "choose a subrace of the race"),
            CcError::NoBodyPreset => write!(f, "no body preset for this race and body"),
            CcError::UnknownClass(g) => write!(f, "unknown class {g}"),
            CcError::BadSubclass => write!(f, "choose a subclass the class offers at level 1"),
            CcError::UnknownBackground(g) => write!(f, "unknown background {g}"),
            CcError::DeityRequired => write!(f, "the class follows a deity: choose one"),
            CcError::BadDeity(g) => write!(f, "deity {g} is not available"),
            CcError::AbilityOutOfRange(a, v) => {
                write!(
                    f,
                    "{a} {v}: point buy allows {POINT_BUY_BASE}–{POINT_BUY_MAX}"
                )
            }
            CcError::TooManyPoints(n) => {
                write!(f, "{n} points spent, {POINT_BUY_POINTS} available")
            }
            CcError::PickCount {
                slot,
                expected,
                got,
            } => write!(f, "{slot}: {got} picked, {expected} needed"),
            CcError::NotAnOption { slot, item } => write!(f, "{slot}: {item} is not an option"),
            CcError::Duplicate { slot, item } => write!(f, "{slot}: {item} picked twice"),
            CcError::AlreadyProficient(s) => write!(f, "already proficient in {s}"),
            CcError::NotProficient(s) => write!(f, "expertise in {s} needs proficiency"),
            CcError::AlreadyKnown(s) => write!(f, "{s} is already known"),
            CcError::UnknownSelector(p, i) => write!(f, "no selector {i} in progression {p}"),
        }
    }
}

/// The character a valid choice makes, at level 1.
#[derive(Clone, Debug, PartialEq)]
pub struct CreatedCharacter {
    pub choice: CharacterChoice,
    /// The body preset's root template.
    pub template: Guid,
    pub level: i32,
    /// Final scores (point buy, bonuses, `Ability` boosts).
    pub abilities: [i32; 6],
    /// Class `BaseHp` + Constitution modifier (+ `IncreaseMaxHP` boosts).
    pub max_hp: i32,
    /// Armour class with the starting equipment and an unarmoured base of 10.
    pub armor_class: i32,
    pub saving_throws: Vec<Ability>,
    pub skills: Vec<Skill>,
    pub expertise: Vec<Skill>,
    /// Armour and weapon proficiency groups (`Proficiency(X)` boosts).
    pub proficiencies: Vec<String>,
    pub passives: Vec<String>,
    /// Spells granted (`AddSpells`, `UnlockSpell` boosts) and picked.
    pub spells: Vec<String>,
    /// Starting equipment: item stats names (race set, class set, picks).
    pub equipment: Vec<String>,
    pub spellcasting_ability: Option<Ability>,
    /// The tags the engine gives the character (background, class, subclass, deity, the
    /// custom origin's appearance and "really" tags), in the order the trace announces them.
    pub tags: Vec<Guid>,
}

/// What the boosts and passives of a character add up to (without picks).
#[derive(Clone, Debug, Default)]
struct Grants {
    abilities: [i32; 6],
    saves: Vec<Ability>,
    skills: Vec<Skill>,
    expertise: Vec<Skill>,
    proficiencies: Vec<String>,
    spells: Vec<String>,
    passives: Vec<String>,
    max_hp: i32,
    ac: i32,
    /// Boosts of passives with `BoostConditions`: (condition, boosts).
    conditional: Vec<(String, String)>,
}

fn push<T: PartialEq>(v: &mut Vec<T>, x: T) {
    if !v.contains(&x) {
        v.push(x);
    }
}

impl Grants {
    /// The unconditional boosts of a boost list the character sheet shows.
    fn boosts(&mut self, text: &str) {
        let Ok(entries) = parse_boosts(text) else {
            return;
        };
        for e in entries {
            let BoostEntry::Boost(b) = e else { continue };
            if b.condition.is_some() {
                continue;
            }
            let a = |i: usize| b.args.get(i).map_or("", |s| s.trim());
            match b.name.as_str() {
                "Ability" => {
                    if let (Some(ab), Ok(n)) = (Ability::parse(a(0)), a(1).parse::<i32>()) {
                        self.abilities[ab as usize] += n;
                    }
                }
                "ProficiencyBonus" => match a(0) {
                    "SavingThrow" => {
                        if let Some(x) = Ability::parse(a(1)) {
                            push(&mut self.saves, x);
                        }
                    }
                    "Skill" => {
                        if let Some(x) = Skill::parse(a(1)) {
                            push(&mut self.skills, x);
                        }
                    }
                    _ => {}
                },
                "Proficiency" => push(&mut self.proficiencies, a(0).to_owned()),
                "ExpertiseBonus" => {
                    if let Some(x) = Skill::parse(a(0)) {
                        push(&mut self.expertise, x);
                    }
                }
                "UnlockSpell" if !a(0).is_empty() => push(&mut self.spells, a(0).to_owned()),
                "IncreaseMaxHP" => self.max_hp += a(0).parse::<i32>().unwrap_or(0),
                "AC" => self.ac += a(0).parse::<i32>().unwrap_or(0),
                _ => {}
            }
        }
    }

    fn passive(&mut self, data: &CcData, name: &str) {
        if self.passives.iter().any(|p| p == name) {
            return;
        }
        self.passives.push(name.to_owned());
        if let Some(b) = data.passive_boosts.get(name) {
            match data.passive_conditions.get(name) {
                Some(c) => self.conditional.push((c.clone(), b.clone())),
                None => self.boosts(b),
            }
        }
    }
}

fn ability_at(name: &str) -> Option<usize> {
    Ability::parse(name.trim()).map(|a| a as usize)
}

impl CcData {
    /// The level-1 progression rows a choice gets: race, subrace, class, subclass.
    pub fn level1_rows(&self, choice: &CharacterChoice) -> Vec<&Progression> {
        let races = [Some(choice.race), choice.subrace]
            .into_iter()
            .flatten()
            .filter_map(|r| self.race(r)?.progression_table);
        let classes = [Some(choice.class), choice.subclass]
            .into_iter()
            .flatten()
            .filter_map(|c| self.class(c)?.progression_table);
        races
            .chain(classes)
            .filter_map(|t| self.table_row(t, 1))
            .collect()
    }

    /// The choices a character makes at level 1, in order (race, subrace, class, subclass).
    pub fn slots(&self, choice: &CharacterChoice) -> Vec<SelectorSlot> {
        let mut out = Vec::new();
        for row in self.level1_rows(choice) {
            for (index, sel) in row.selectors.iter().enumerate() {
                let options: Vec<String> = match sel.kind {
                    SelectorKind::AddSpells | SelectorKind::Other(_) => continue,
                    SelectorKind::SelectSpells => self
                        .spell_lists
                        .get(&sel.list)
                        .into_iter()
                        .flatten()
                        .filter(|s| self.spells.is_empty() || self.spells.contains(*s))
                        .cloned()
                        .collect(),
                    SelectorKind::SelectSkills | SelectorKind::SelectSkillsExpertise => {
                        self.skill_lists.get(&sel.list).cloned().unwrap_or_default()
                    }
                    SelectorKind::SelectPassives => self
                        .passive_lists
                        .get(&sel.list)
                        .cloned()
                        .unwrap_or_default(),
                    SelectorKind::SelectEquipment => self
                        .equipment_lists
                        .get(&sel.list)
                        .cloned()
                        .unwrap_or_default(),
                    SelectorKind::SelectAbilityBonus | SelectorKind::SelectAbilities => self
                        .ability_lists
                        .get(&sel.list)
                        .cloned()
                        .unwrap_or_default(),
                };
                if sel.count == 0 {
                    continue;
                }
                out.push(SelectorSlot {
                    progression: row.id,
                    table: row.table,
                    level: row.level,
                    source: row.name.clone(),
                    index,
                    selector: sel.clone(),
                    options,
                });
            }
        }
        out
    }

    /// What the choice gets before any pick: boosts and passives of its progressions, the
    /// background's and the custom origin's passives, and the spell lists it is given.
    fn fixed_grants(&self, choice: &CharacterChoice) -> Grants {
        let mut g = Grants::default();
        let rows = self.level1_rows(choice);
        for row in &rows {
            g.boosts(&row.boosts);
        }
        let background = self.background(choice.background);
        let origin = self.generic_origin();
        let passives = rows
            .iter()
            .flat_map(|r| r.passives_added.iter())
            .chain(background.into_iter().flat_map(|b| b.passives.iter()))
            .chain(origin.into_iter().flat_map(|o| o.passives.iter()));
        for p in passives {
            g.passive(self, p);
        }
        for row in &rows {
            for p in &row.passives_removed {
                g.passives.retain(|x| x != p);
            }
        }
        for row in &rows {
            for sel in &row.selectors {
                if sel.kind == SelectorKind::AddSpells {
                    for s in self.spell_lists.get(&sel.list).into_iter().flatten() {
                        if self.spells.is_empty() || self.spells.contains(s) {
                            push(&mut g.spells, s.clone());
                        }
                    }
                }
            }
        }
        g
    }

    /// Fill in what the player left open, the way character creation starts out: the first
    /// level-1 subclass, the first deity the race allows (classes with `HasGod`), the class's
    /// `AbilityDistributionPresets` scores, and for every selector without picks its
    /// `DefaultValues` row (options already granted are skipped), then the selector's list in
    /// order. *Approximate*: the engine's own default logic was not read; with these rules
    /// the trace's character gets the defaults the UI shows.
    pub fn complete(&self, choice: &mut CharacterChoice) {
        if choice.subrace.is_none()
            && let Some(s) = self.subraces(choice.race).first()
        {
            choice.subrace = Some(s.id);
        }
        self.complete_appearance(choice);
        let subs = self.level1_subclasses(choice.class);
        if choice.subclass.is_none()
            && let Some(s) = subs.first()
        {
            choice.subclass = Some(*s);
        }
        if choice.deity.is_none()
            && self.class(choice.class).is_some_and(|c| c.has_god)
            && let Some(g) = self.allowed_gods(choice.race, choice.subrace).first()
        {
            choice.deity = Some(g.id);
        }
        if choice.abilities == [0; 6] {
            choice.abilities = self
                .ability_presets
                .get(&choice.class)
                .copied()
                .unwrap_or([15, 14, 13, 12, 10, 8]);
        }
        let races: Vec<Guid> = [Some(choice.race), choice.subrace]
            .into_iter()
            .flatten()
            .collect();
        let fixed = self.fixed_grants(choice);
        let mut spells: Vec<String> = fixed.spells.clone();
        let mut skills: Vec<String> = fixed.skills.iter().map(|s| s.name().to_owned()).collect();
        for slot in self.slots(choice) {
            if let Some(p) = choice
                .picks
                .iter()
                .find(|p| p.progression == slot.progression && p.index == slot.index)
            {
                if slot.selector.kind == SelectorKind::SelectSkills {
                    skills.extend(p.items.iter().cloned());
                }
                if slot.selector.kind == SelectorKind::SelectSpells {
                    spells.extend(p.items.iter().cloned());
                }
                continue;
            }
            let defaults = self.default_picks(
                slot.default_kind(),
                slot.table,
                slot.level,
                slot.selector.selector_id.as_deref(),
                &races,
            );
            let taken = |x: &String, items: &[String]| {
                items.contains(x)
                    || match slot.selector.kind {
                        SelectorKind::SelectSkills => skills.contains(x),
                        SelectorKind::SelectSkillsExpertise => !skills.contains(x),
                        SelectorKind::SelectSpells => spells.contains(x),
                        _ => false,
                    }
            };
            let mut items: Vec<String> = Vec::new();
            for x in defaults.iter().chain(slot.options.iter()) {
                if items.len() >= slot.selector.count {
                    break;
                }
                if slot.options.contains(x) && !taken(x, &items) {
                    items.push(x.clone());
                }
            }
            if slot.selector.kind == SelectorKind::SelectSkills {
                skills.extend(items.iter().cloned());
            }
            if slot.selector.kind == SelectorKind::SelectSpells {
                spells.extend(items.iter().cloned());
            }
            choice.picks.push(Pick {
                progression: slot.progression,
                index: slot.index,
                items,
            });
        }
    }

    /// Everything wrong with a choice (empty: valid).
    // Mirrors eoc::character_creation::Validate / ValidateGod / Validate*Selectors in what
    // they check (by name); the checks themselves are ours (4.1.1.7631656).
    pub fn validate(&self, choice: &CharacterChoice) -> Vec<CcError> {
        let mut errs = Vec::new();
        for (slot, id) in choice.appearance.visuals() {
            if !self
                .appearance_options(choice, &slot)
                .iter()
                .any(|v| v.id == id)
            {
                errs.push(CcError::NotAnOption {
                    slot: format!("appearance {slot}"),
                    item: id.to_string(),
                });
            }
        }
        for (kind, id) in choice.appearance.material_choices() {
            if !self
                .appearance
                .material_options(&kind)
                .iter()
                .any(|m| m.id == id)
            {
                errs.push(CcError::NotAnOption {
                    slot: format!("appearance {kind} material"),
                    item: id.to_string(),
                });
            }
        }
        if !self.playable_races().iter().any(|r| r.id == choice.race) {
            errs.push(CcError::UnknownRace(choice.race));
        }
        let subs = self.subraces(choice.race);
        let sub_ok = match choice.subrace {
            Some(s) => subs.iter().any(|r| r.id == s),
            None => subs.is_empty(),
        };
        if !sub_ok {
            errs.push(CcError::BadSubrace);
        } else if self
            .preset(
                choice.race,
                choice.subrace,
                choice.body_type,
                choice.body_shape,
            )
            .is_none()
        {
            errs.push(CcError::NoBodyPreset);
        }
        let class = self
            .class(choice.class)
            .filter(|c| c.parent.is_none() && self.playable_classes().iter().any(|p| p.id == c.id));
        if class.is_none() {
            errs.push(CcError::UnknownClass(choice.class));
        }
        let subclasses = self.level1_subclasses(choice.class);
        let subclass_ok = match choice.subclass {
            Some(s) => subclasses.contains(&s),
            None => subclasses.is_empty(),
        };
        if !subclass_ok {
            errs.push(CcError::BadSubclass);
        }
        if !self
            .playable_backgrounds()
            .iter()
            .any(|b| b.id == choice.background)
        {
            errs.push(CcError::UnknownBackground(choice.background));
        }
        let has_god = class.is_some_and(|c| c.has_god);
        match choice.deity {
            None if has_god => errs.push(CcError::DeityRequired),
            Some(g)
                if !has_god
                    || !self
                        .allowed_gods(choice.race, choice.subrace)
                        .iter()
                        .any(|x| x.id == g) =>
            {
                errs.push(CcError::BadDeity(g))
            }
            _ => {}
        }
        for (i, a) in Ability::ALL.iter().enumerate() {
            let v = choice.abilities[i];
            if point_buy_cost(v).is_none() {
                errs.push(CcError::AbilityOutOfRange(*a, v));
            }
        }
        if let Some(n) = point_buy_spent(&choice.abilities)
            && n > POINT_BUY_POINTS
        {
            errs.push(CcError::TooManyPoints(n));
        }
        // Picks.
        let slots = self.slots(choice);
        for p in &choice.picks {
            if !slots
                .iter()
                .any(|s| s.progression == p.progression && s.index == p.index)
            {
                errs.push(CcError::UnknownSelector(p.progression, p.index));
            }
        }
        let fixed = self.fixed_grants(choice);
        let mut skills: Vec<String> = fixed.skills.iter().map(|s| s.name().to_owned()).collect();
        let mut expertise_picks: Vec<String> = Vec::new();
        let mut spells: Vec<String> = fixed.spells.clone();
        for slot in &slots {
            let label = slot.label();
            let items = choice
                .picks
                .iter()
                .find(|p| p.progression == slot.progression && p.index == slot.index)
                .map(|p| p.items.as_slice())
                .unwrap_or_default();
            if items.len() != slot.selector.count {
                errs.push(CcError::PickCount {
                    slot: label.clone(),
                    expected: slot.selector.count,
                    got: items.len(),
                });
            }
            let mut seen: HashSet<&String> = HashSet::new();
            for item in items {
                if !seen.insert(item) {
                    errs.push(CcError::Duplicate {
                        slot: label.clone(),
                        item: item.clone(),
                    });
                    continue;
                }
                if !slot.options.contains(item) {
                    errs.push(CcError::NotAnOption {
                        slot: label.clone(),
                        item: item.clone(),
                    });
                    continue;
                }
                match slot.selector.kind {
                    SelectorKind::SelectSkills => {
                        if skills.contains(item) {
                            errs.push(CcError::AlreadyProficient(item.clone()));
                        } else {
                            skills.push(item.clone());
                        }
                    }
                    SelectorKind::SelectSkillsExpertise => expertise_picks.push(item.clone()),
                    SelectorKind::SelectSpells => {
                        if spells.contains(item) {
                            errs.push(CcError::AlreadyKnown(item.clone()));
                        } else {
                            spells.push(item.clone());
                        }
                    }
                    _ => {}
                }
            }
        }
        for e in expertise_picks {
            if !skills.contains(&e) {
                errs.push(CcError::NotProficient(e));
            }
        }
        errs
    }

    /// Unspent point-buy points (the game shows them as to-do; not an error here).
    pub fn unspent_points(choice: &CharacterChoice) -> i32 {
        point_buy_spent(&choice.abilities).map_or(0, |n| POINT_BUY_POINTS - n)
    }

    /// The character a choice makes; the errors of [`CcData::validate`] if it isn't valid.
    pub fn build(&self, choice: &CharacterChoice) -> Result<CreatedCharacter, Vec<CcError>> {
        let errs = self.validate(choice);
        if !errs.is_empty() {
            return Err(errs);
        }
        let class = self.class(choice.class).expect("validated");
        let preset = self
            .preset(
                choice.race,
                choice.subrace,
                choice.body_type,
                choice.body_shape,
            )
            .expect("validated");
        let mut g = self.fixed_grants(choice);
        let mut abilities = choice.abilities;
        for (i, b) in g.abilities.iter().enumerate() {
            abilities[i] += b;
        }
        let mut equipment: Vec<String> = Vec::new();
        let race_sets = [choice.subrace, Some(choice.race)]
            .into_iter()
            .flatten()
            .filter_map(|r| self.race(r)?.equipment.clone())
            .take(1);
        // A subclass with its own `ClassEquipment` (paladin oaths, some sorcerers) replaces
        // the class's.
        let class_set = choice
            .subclass
            .and_then(|s| self.class(s)?.equipment.clone())
            .or_else(|| class.equipment.clone());
        for set in race_sets.chain(class_set) {
            equipment.extend(self.equipment_sets.get(&set).cloned().unwrap_or_default());
        }
        let mut picked_skills: Vec<Skill> = Vec::new();
        let mut picked_expertise: Vec<Skill> = Vec::new();
        let mut picked_spells: Vec<String> = Vec::new();
        for slot in self.slots(choice) {
            let Some(p) = choice
                .picks
                .iter()
                .find(|p| p.progression == slot.progression && p.index == slot.index)
            else {
                continue;
            };
            match slot.selector.kind {
                SelectorKind::SelectAbilityBonus | SelectorKind::SelectAbilities => {
                    for (item, amount) in p.items.iter().zip(&slot.selector.amounts) {
                        if let Some(i) = ability_at(item) {
                            abilities[i] += amount;
                        }
                    }
                }
                SelectorKind::SelectSkills => {
                    picked_skills.extend(p.items.iter().filter_map(|s| Skill::parse(s)))
                }
                SelectorKind::SelectSkillsExpertise => {
                    picked_expertise.extend(p.items.iter().filter_map(|s| Skill::parse(s)))
                }
                SelectorKind::SelectSpells => picked_spells.extend(p.items.iter().cloned()),
                SelectorKind::SelectPassives => {
                    for x in &p.items {
                        g.passive(self, x);
                    }
                }
                SelectorKind::SelectEquipment => equipment.extend(p.items.iter().cloned()),
                SelectorKind::AddSpells | SelectorKind::Other(_) => {}
            }
        }
        let mut skills = g.skills.clone();
        for s in picked_skills {
            push(&mut skills, s);
        }
        let mut expertise = g.expertise.clone();
        for s in picked_expertise {
            push(&mut expertise, s);
        }
        let mut spells = g.spells.clone();
        for s in picked_spells {
            push(&mut spells, s);
        }
        // Weapon actions of the main-hand weapon (`BoostsOnEquipMainHand`), when proficient.
        // *Approximate*: the engine applies them as equipment boosts.
        let proficient = |item: &super::CcItem| {
            item.proficiency.is_empty()
                || item.proficiency.iter().any(|p| g.proficiencies.contains(p))
        };
        if let Some(w) = equipment
            .iter()
            .filter_map(|e| self.items.get(e))
            .find(|i| i.kind == "Weapon" && i.slot.starts_with("Melee Main"))
            && proficient(w)
        {
            for s in &w.unlocks {
                push(&mut spells, s.clone());
            }
        }
        let con = modifier(abilities[Ability::Constitution as usize]);
        let max_hp = (class.base_hp + con + g.max_hp).max(1);
        let dex = modifier(abilities[Ability::Dexterity as usize]);
        let items: Vec<&super::CcItem> =
            equipment.iter().filter_map(|e| self.items.get(e)).collect();
        // Armour class as `crate::Sim::update_armor_class` computes it, with the unarmoured
        // base 10 (the sim uses the `Character` entry's `Armor`).
        let body = items
            .iter()
            .find(|i| i.kind == "Armor" && i.slot == "Breast" && !i.shield);
        let mut armor_class = match body {
            Some(a) => {
                let m = a.ability.map_or(0, |ab| modifier(abilities[ab as usize]));
                a.armor_class
                    + if a.ability_cap > 0 {
                        m.min(a.ability_cap)
                    } else {
                        m
                    }
            }
            None => 10 + dex,
        };
        let shield = items.iter().find(|i| i.shield);
        // Conditional passives that depend on the armour worn: `AC(n)` while
        // `WearingArmor()` (Defence), `ACOverrideFormula(base, shield, abilities…)` while
        // `not WearingArmor()` (Unarmoured Defence, Draconic Resilience). *Approximate*: the
        // override is taken when it is higher; other conditions are not evaluated.
        // `WearingArmor()`: body armour other than clothing (*assumption*: `ArmorType`
        // `None` and `Cloth` are clothing, e.g. robes and the barbarian's outfit).
        let wearing = body.is_some_and(|b| !matches!(b.armor_type.as_str(), "" | "None" | "Cloth"));
        for (cond, boosts) in &g.conditional {
            let holds = match cond.replace(' ', "").as_str() {
                "WearingArmor(context.Source)" => wearing,
                "notWearingArmor(context.Source)" => !wearing,
                "notWearingArmor(context.Source)andnotHasShieldEquipped(context.Source)" => {
                    !wearing && shield.is_none()
                }
                _ => false,
            };
            let Ok(entries) = parse_boosts(boosts).map_err(|_| ()) else {
                continue;
            };
            for e in entries {
                let BoostEntry::Boost(b) = e else { continue };
                if !holds || b.condition.is_some() {
                    continue;
                }
                let a = |i: usize| b.args.get(i).map_or("", |s| s.trim());
                match b.name.as_str() {
                    "AC" => g.ac += a(0).parse::<i32>().unwrap_or(0),
                    "ACOverrideFormula" => {
                        let base: i32 = a(0).parse().unwrap_or(10);
                        let mods: i32 = b
                            .args
                            .iter()
                            .skip(2)
                            .filter_map(|x| ability_at(x))
                            .map(|i| modifier(abilities[i]))
                            .sum();
                        let with_shield = a(1) == "true";
                        let v = base
                            + mods
                            + if with_shield {
                                shield.map_or(0, |s| s.armor_class)
                            } else {
                                0
                            };
                        let normal = armor_class + shield.map_or(0, |s| s.armor_class);
                        if v > normal {
                            armor_class = v - shield.map_or(0, |s| s.armor_class);
                        }
                    }
                    _ => {}
                }
            }
        }
        if let Some(s) = shield {
            armor_class += s.armor_class;
        }
        armor_class += g.ac;
        let mut tags: Vec<Guid> = Vec::new();
        let background = self.background(choice.background);
        let subclass = choice.subclass.and_then(|s| self.class(s));
        let god = choice.deity.and_then(|d| self.god(d));
        let origin = self.generic_origin();
        for t in background
            .into_iter()
            .flat_map(|b| b.tags.iter())
            .chain(class.tags.iter())
            .chain(subclass.into_iter().flat_map(|c| c.tags.iter()))
            .chain(god.into_iter().flat_map(|g| g.tags.iter()))
            .chain(origin.into_iter().flat_map(|o| o.appearance_tags.iter()))
            .chain(origin.into_iter().flat_map(|o| o.really_tags.iter()))
        {
            push(&mut tags, *t);
        }
        Ok(CreatedCharacter {
            choice: choice.clone(),
            template: preset.root_template,
            level: 1,
            abilities,
            max_hp,
            armor_class,
            saving_throws: g.saves.clone(),
            skills,
            expertise,
            proficiencies: g.proficiencies.clone(),
            passives: g.passives.clone(),
            spells,
            equipment,
            spellcasting_ability: class.spellcasting_ability,
            tags,
        })
    }
}

impl CharacterChoice {
    /// A one-line form for command lines (`bg3-viewer --character`): `key=value` fields
    /// separated by `;`, picks as `pick=<progression>:<index>:<item>,<item>`.
    pub fn to_spec(&self) -> String {
        let opt = |g: Option<Guid>| g.map(|g| g.to_string()).unwrap_or_default();
        let mut out = vec![
            format!("race={}", self.race),
            format!("subrace={}", opt(self.subrace)),
            format!("body={},{}", self.body_type, self.body_shape),
            format!("class={}", self.class),
            format!("subclass={}", opt(self.subclass)),
            format!("background={}", self.background),
            format!("deity={}", opt(self.deity)),
            format!(
                "abilities={}",
                self.abilities
                    .iter()
                    .map(i32::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        ];
        for (slot, id) in self.appearance.visuals() {
            out.push(format!("visual={slot}:{id}"));
        }
        for (kind, id) in self.appearance.material_choices() {
            out.push(format!("material={kind}:{id}"));
        }
        for p in &self.picks {
            out.push(format!(
                "pick={}:{}:{}",
                p.progression,
                p.index,
                p.items.join(",")
            ));
        }
        out.join(";")
    }

    /// Parse [`CharacterChoice::to_spec`]'s form.
    pub fn from_spec(spec: &str) -> Result<Self, String> {
        let mut c = CharacterChoice::default();
        let guid = |v: &str| Guid::parse(v.trim()).ok_or_else(|| format!("bad uuid {v:?}"));
        let opt = |v: &str| {
            if v.trim().is_empty() {
                Ok(None)
            } else {
                guid(v).map(Some)
            }
        };
        for field in spec.split(';').map(str::trim).filter(|f| !f.is_empty()) {
            let (k, v) = field
                .split_once('=')
                .ok_or_else(|| format!("expected key=value: {field:?}"))?;
            match k.trim() {
                "race" => c.race = guid(v)?,
                "subrace" => c.subrace = opt(v)?,
                "body" => {
                    let (t, s) = v.split_once(',').unwrap_or((v, "0"));
                    c.body_type = t.trim().parse().map_err(|_| format!("bad body {v:?}"))?;
                    c.body_shape = s.trim().parse().map_err(|_| format!("bad body {v:?}"))?;
                }
                "class" => c.class = guid(v)?,
                "subclass" => c.subclass = opt(v)?,
                "background" => c.background = guid(v)?,
                "deity" => c.deity = opt(v)?,
                "abilities" => {
                    let a: Vec<i32> = v
                        .split(',')
                        .map(|x| x.trim().parse().map_err(|_| format!("bad score {x:?}")))
                        .collect::<Result<_, _>>()?;
                    c.abilities = a
                        .try_into()
                        .map_err(|_| "abilities: six scores".to_owned())?;
                }
                "visual" | "material" => {
                    let (name, value) = v
                        .split_once(':')
                        .ok_or_else(|| format!("bad appearance {v:?}"))?;
                    if name.trim().is_empty() {
                        return Err("appearance needs a slot/category".into());
                    }
                    let id = opt(value)?;
                    if k.trim() == "visual" {
                        c.appearance.set_visual(name.trim(), id);
                    } else {
                        c.appearance.set_material(name.trim(), id);
                    }
                }
                "pick" => {
                    let mut it = v.splitn(3, ':');
                    let p = guid(it.next().unwrap_or(""))?;
                    let index = it
                        .next()
                        .and_then(|i| i.trim().parse().ok())
                        .ok_or_else(|| format!("bad pick {v:?}"))?;
                    let items = it
                        .next()
                        .unwrap_or("")
                        .split(',')
                        .map(str::trim)
                        .filter(|x| !x.is_empty())
                        .map(str::to_owned)
                        .collect();
                    c.picks.push(Pick {
                        progression: p,
                        index,
                        items,
                    });
                }
                other => return Err(format!("unknown field {other:?}")),
            }
        }
        Ok(c)
    }
}
