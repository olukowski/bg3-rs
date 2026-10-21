//! What the rules need of a character: abilities, level, hit points, armour class and
//! proficiencies, from its `Character` stats entry (`Public/<Module>/Stats/Generated/Data/
//! Character.txt`, `using` inheritance already applied by `bg3_stats`) and its template's or
//! instance's `LevelOverride`.
//!
//! | Sheet | From |
//! |---|---|
//! | abilities | `Strength` … `Charisma` |
//! | level | `LevelOverride` of the object or its nearest template if > 0, else `Level` |
//! | max HP | `Vitality` (*inferred*: no class levels, no `IncreaseMaxHP` boosts) |
//! | armour class | `Armor` (equipment is not modelled) |
//! | proficiency bonus | `ProficiencyBonus` if set, else the `ProficiencyBonusScaling` level map at the level |
//! | saving throws / skills | `DefaultBoosts`: `ProficiencyBonus(SavingThrow, X)`, `ProficiencyBonus(Skill, X)`, `ExpertiseBonus(X)`, `Skill(X, n)` |
//! | spellcasting ability | `SpellCastingAbility` |
//! | passives | `Passives` |
//!
//! Not modelled: class progressions (player characters get their stats from character
//! creation and their classes; the sim uses their template's stats entry), boosts from
//! passives, statuses and equipment, conditional (`IF(…)`) boosts.

use bg3_rules::context::{Ability, Skill};
use bg3_rules::fields::{BoostEntry, parse_boosts};
use bg3_rules::levelmap::LevelMaps;

/// A `Character` stats entry, parsed.
#[derive(Clone, Debug, PartialEq)]
pub struct CharacterDef {
    pub name: String,
    /// Strength, Dexterity, Constitution, Intelligence, Wisdom, Charisma.
    pub abilities: [i32; 6],
    pub level: i32,
    pub vitality: i32,
    pub armor: i32,
    /// `ProficiencyBonus`: a fixed bonus, if the entry sets one.
    pub proficiency_bonus: Option<i32>,
    /// `ProficiencyBonusScaling`: UUID of a level map series.
    pub proficiency_scaling: Option<String>,
    pub spellcasting_ability: Option<Ability>,
    pub saving_throws: Vec<Ability>,
    pub skills: Vec<Skill>,
    pub expertise: Vec<Skill>,
    pub skill_bonus: Vec<(Skill, i32)>,
    pub passives: Vec<String>,
}

fn int(o: &bg3_stats::StatsObject, key: &str) -> Option<i32> {
    o.get(key)?.trim().parse().ok()
}

impl CharacterDef {
    /// From a stats entry of type `Character`.
    pub fn from_stats(o: &bg3_stats::StatsObject) -> Self {
        let mut d = Self {
            name: o.name.clone(),
            abilities: [10; 6],
            level: int(o, "Level").unwrap_or(1),
            vitality: int(o, "Vitality").unwrap_or(1),
            armor: int(o, "Armor").unwrap_or(10),
            proficiency_bonus: int(o, "ProficiencyBonus"),
            proficiency_scaling: o
                .get("ProficiencyBonusScaling")
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty()),
            spellcasting_ability: o
                .get("SpellCastingAbility")
                .and_then(|s| Ability::parse(s.trim())),
            saving_throws: Vec::new(),
            skills: Vec::new(),
            expertise: Vec::new(),
            skill_bonus: Vec::new(),
            passives: o
                .get("Passives")
                .unwrap_or_default()
                .split(';')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect(),
        };
        for a in Ability::ALL {
            if let Some(v) = int(o, a.name()) {
                d.abilities[*a as usize] = v;
            }
        }
        if let Some(b) = o.get("DefaultBoosts") {
            d.read_boosts(b);
        }
        d
    }

    /// The proficiency and skill boosts of a boost list; conditional boosts are skipped.
    // bg3: eoc::active_roll::CheckProficiencyBoost / CheckExpertiseBoost / CheckSkillBoost read
    // these boosts when a roll is made (4.1.1.7631656)
    fn read_boosts(&mut self, text: &str) {
        let Ok(entries) = parse_boosts(text) else {
            return;
        };
        for e in entries {
            let BoostEntry::Boost(b) = e else { continue };
            if b.condition.is_some() {
                continue;
            }
            let arg = |i: usize| b.args.get(i).map_or("", |s| s.trim());
            match b.name.as_str() {
                "ProficiencyBonus" => match arg(0) {
                    "SavingThrow" => {
                        if let Some(a) = Ability::parse(arg(1)) {
                            push_unique(&mut self.saving_throws, a);
                        }
                    }
                    "Skill" => {
                        if let Some(s) = Skill::parse(arg(1)) {
                            push_unique(&mut self.skills, s);
                        }
                    }
                    _ => {}
                },
                "ExpertiseBonus" => {
                    if let Some(s) = Skill::parse(arg(0)) {
                        push_unique(&mut self.expertise, s);
                    }
                }
                "Skill" => {
                    if let (Some(s), Ok(n)) = (Skill::parse(arg(0)), arg(1).parse::<i32>()) {
                        self.skill_bonus.push((s, n));
                    }
                }
                _ => {}
            }
        }
    }

    /// The proficiency bonus at `level`: the fixed `ProficiencyBonus`, else the scaling level
    /// map, else the D&D 5e table (`2 + (level - 1) / 4`) when the map is unknown.
    pub fn proficiency_at(&self, level: i32, maps: &LevelMaps) -> i32 {
        if let Some(p) = self.proficiency_bonus {
            return p;
        }
        self.proficiency_scaling
            .as_deref()
            .and_then(|u| maps.by_uuid(u))
            .and_then(|s| s.value(level))
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(2 + (level.max(1) - 1) / 4)
    }

    /// A fresh sheet (full hit points) at the given level override.
    pub fn sheet(&self, level_override: Option<i32>, maps: &LevelMaps) -> CharacterSheet {
        let level = level_override.filter(|l| *l > 0).unwrap_or(self.level);
        CharacterSheet {
            abilities: self.abilities,
            level,
            max_hp: self.vitality,
            hp: self.vitality,
            armor_class: self.armor,
            proficiency_bonus: self.proficiency_at(level, maps),
            saving_throws: self.saving_throws.clone(),
            skills: self.skills.clone(),
            expertise: self.expertise.clone(),
            skill_bonus: self.skill_bonus.clone(),
            spellcasting_ability: self.spellcasting_ability,
            passives: self.passives.clone(),
        }
    }
}

fn push_unique<T: PartialEq>(v: &mut Vec<T>, x: T) {
    if !v.contains(&x) {
        v.push(x);
    }
}

/// A character's current rule stats.
#[derive(Clone, Debug, PartialEq)]
pub struct CharacterSheet {
    pub abilities: [i32; 6],
    pub level: i32,
    pub max_hp: i32,
    pub hp: i32,
    pub armor_class: i32,
    pub proficiency_bonus: i32,
    pub saving_throws: Vec<Ability>,
    pub skills: Vec<Skill>,
    pub expertise: Vec<Skill>,
    pub skill_bonus: Vec<(Skill, i32)>,
    pub spellcasting_ability: Option<Ability>,
    pub passives: Vec<String>,
}

impl CharacterSheet {
    pub fn ability(&self, a: Ability) -> i32 {
        self.abilities[a as usize]
    }
}

/// Every `Character` entry of a merged stats database, by name.
pub fn character_defs(stats: &bg3_stats::Stats) -> std::collections::HashMap<String, CharacterDef> {
    stats
        .entries()
        .filter(|o| o.type_name.as_deref() == Some("Character"))
        .map(|o| (o.name.clone(), CharacterDef::from_stats(o)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn character_entry_to_sheet() {
        let mut stats = bg3_stats::Stats::new();
        stats
            .load_str(
                "Test",
                "Character.txt",
                r#"new entry "_Base"
type "Character"
data "Level" "1"
data "Strength" "10"
data "Vitality" "5"
data "Armor" "10"

new entry "Test_Brute"
type "Character"
using "_Base"
data "Strength" "16"
data "Wisdom" "8"
data "Vitality" "22"
data "Armor" "13"
data "ProficiencyBonus" "3"
data "SpellCastingAbility" "Wisdom"
data "Passives" "AttackOfOpportunity; Brute"
data "DefaultBoosts" "ProficiencyBonus(SavingThrow, Constitution);ProficiencyBonus(Skill,Perception);Skill(Stealth, 6);IF(not Combat()):ExpertiseBonus(Athletics);BlockRegainHP(Undead;Construct)"
"#,
            )
            .unwrap();
        stats.finish();
        let defs = character_defs(&stats);
        let d = &defs["Test_Brute"];
        assert_eq!(d.abilities, [16, 10, 10, 10, 8, 10]);
        assert_eq!(d.saving_throws, [Ability::Constitution]);
        assert_eq!(d.skills, [Skill::Perception]);
        assert_eq!(d.skill_bonus, [(Skill::Stealth, 6)]);
        assert!(d.expertise.is_empty(), "conditional boosts are skipped");
        assert_eq!(d.passives, ["AttackOfOpportunity", "Brute"]);
        let s = d.sheet(Some(4), &LevelMaps::default());
        assert_eq!((s.level, s.hp, s.max_hp, s.armor_class), (4, 22, 22, 13));
        assert_eq!(s.proficiency_bonus, 3);
        let base = defs["_Base"].sheet(None, &LevelMaps::default());
        assert_eq!((base.level, base.proficiency_bonus), (1, 2));
        assert_eq!(defs["_Base"].proficiency_at(5, &LevelMaps::default()), 3);
    }
}
