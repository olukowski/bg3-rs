//! A small in-memory [`RulesContext`] for tests, the CLI and demos.

use std::collections::{BTreeMap, BTreeSet};

use crate::context::{
    Ability, Advantage, AttackType, DamageType, EntityId, Hand, Resistance, RulesContext, Size,
    Weapon,
};
use crate::expr::Number;

#[derive(Debug, Clone)]
pub struct Combatant {
    pub name: String,
    /// Strength, Dexterity, Constitution, Intelligence, Wisdom, Charisma.
    pub abilities: [i32; 6],
    pub level: i32,
    pub proficiency_bonus: i32,
    pub saving_throws: BTreeSet<Ability>,
    pub spellcasting_ability: Option<Ability>,
    pub armor_class: i32,
    pub hp: i32,
    pub max_hp: i32,
    pub team: u32,
    pub player: bool,
    pub item: bool,
    pub size: Size,
    pub main_hand: Option<Weapon>,
    pub off_hand: Option<Weapon>,
    pub resistances: BTreeMap<DamageType, Resistance>,
    pub tags: BTreeSet<String>,
    pub passives: BTreeSet<String>,
    /// Status id → remaining turns (-1 = permanent).
    pub statuses: BTreeMap<String, i32>,
    pub level_map: BTreeMap<String, Number>,
    pub advantage_on_attacks: bool,
    pub disadvantage_on_attacks: bool,
}

impl Combatant {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            abilities: [10; 6],
            level: 1,
            proficiency_bonus: 2,
            saving_throws: BTreeSet::new(),
            spellcasting_ability: None,
            armor_class: 10,
            hp: 10,
            max_hp: 10,
            team: 0,
            player: false,
            item: false,
            size: Size::Medium,
            main_hand: None,
            off_hand: None,
            resistances: BTreeMap::new(),
            tags: BTreeSet::new(),
            passives: BTreeSet::new(),
            statuses: BTreeMap::new(),
            level_map: BTreeMap::new(),
            advantage_on_attacks: false,
            disadvantage_on_attacks: false,
        }
    }

    pub fn ability(mut self, a: Ability, score: i32) -> Self {
        self.abilities[a as usize] = score;
        self
    }

    pub fn hp(mut self, hp: i32) -> Self {
        self.hp = hp;
        self.max_hp = hp;
        self
    }

    pub fn ac(mut self, ac: i32) -> Self {
        self.armor_class = ac;
        self
    }

    pub fn team(mut self, team: u32) -> Self {
        self.team = team;
        self
    }

    pub fn weapon(mut self, w: Weapon) -> Self {
        self.main_hand = Some(w);
        self
    }
}

/// Entities by id; teams decide ally/enemy.
#[derive(Debug, Clone, Default)]
pub struct TestWorld {
    pub entities: BTreeMap<EntityId, Combatant>,
    next: u32,
}

impl TestWorld {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn spawn(&mut self, c: Combatant) -> EntityId {
        self.next += 1;
        let id = EntityId(self.next);
        self.entities.insert(id, c);
        id
    }

    pub fn get(&self, e: EntityId) -> &Combatant {
        &self.entities[&e]
    }

    pub fn get_mut(&mut self, e: EntityId) -> &mut Combatant {
        self.entities.get_mut(&e).expect("entity exists")
    }

    fn with<T: Default>(&self, e: EntityId, f: impl FnOnce(&Combatant) -> T) -> T {
        self.entities.get(&e).map(f).unwrap_or_default()
    }
}

impl RulesContext for TestWorld {
    fn exists(&self, e: EntityId) -> bool {
        self.entities.contains_key(&e)
    }
    fn name(&self, e: EntityId) -> String {
        self.with(e, |c| c.name.clone())
    }
    fn is_character(&self, e: EntityId) -> bool {
        self.with(e, |c| !c.item)
    }
    fn is_item(&self, e: EntityId) -> bool {
        self.with(e, |c| c.item)
    }
    fn is_player(&self, e: EntityId) -> bool {
        self.with(e, |c| c.player)
    }
    fn size(&self, e: EntityId) -> Size {
        self.entities.get(&e).map_or(Size::Medium, |c| c.size)
    }
    fn ability_score(&self, e: EntityId, a: Ability) -> i32 {
        self.with(e, |c| c.abilities[a as usize])
    }
    fn level(&self, e: EntityId) -> i32 {
        self.with(e, |c| c.level)
    }
    fn proficiency_bonus(&self, e: EntityId) -> i32 {
        self.with(e, |c| c.proficiency_bonus)
    }
    fn saving_throw_proficient(&self, e: EntityId, a: Ability) -> bool {
        self.with(e, |c| c.saving_throws.contains(&a))
    }
    fn spellcasting_ability(&self, e: EntityId) -> Option<Ability> {
        self.entities.get(&e).and_then(|c| c.spellcasting_ability)
    }
    fn armor_class(&self, e: EntityId) -> i32 {
        self.with(e, |c| c.armor_class)
    }
    fn hp(&self, e: EntityId) -> i32 {
        self.with(e, |c| c.hp)
    }
    fn max_hp(&self, e: EntityId) -> i32 {
        self.with(e, |c| c.max_hp)
    }
    fn weapon(&self, e: EntityId, hand: Hand) -> Option<Weapon> {
        let c = self.entities.get(&e)?;
        match hand {
            Hand::Main => c.main_hand.clone(),
            Hand::Off => c.off_hand.clone(),
        }
    }
    fn resistance(&self, e: EntityId, t: DamageType) -> Resistance {
        self.entities
            .get(&e)
            .and_then(|c| c.resistances.get(&t).copied())
            .unwrap_or_default()
    }
    fn attack_advantage(
        &self,
        attacker: EntityId,
        _target: EntityId,
        _attack: AttackType,
    ) -> Advantage {
        self.with(attacker, |c| Advantage {
            advantage: c.advantage_on_attacks,
            disadvantage: c.disadvantage_on_attacks,
        })
    }
    fn level_map_value(&self, e: EntityId, name: &str) -> Option<Number> {
        self.entities.get(&e)?.level_map.get(name).copied()
    }
    fn is_ally(&self, a: EntityId, b: EntityId) -> bool {
        self.exists(a) && self.exists(b) && self.get(a).team == self.get(b).team
    }
    fn is_enemy(&self, a: EntityId, b: EntityId) -> bool {
        self.exists(a) && self.exists(b) && self.get(a).team != self.get(b).team
    }
    fn has_tag(&self, e: EntityId, tag: &str) -> bool {
        self.with(e, |c| c.tags.contains(tag))
    }
    fn has_status(&self, e: EntityId, status: &str) -> bool {
        self.with(e, |c| c.statuses.contains_key(status))
    }
    fn has_passive(&self, e: EntityId, passive: &str) -> bool {
        self.with(e, |c| c.passives.contains(passive))
    }
    fn set_hp(&mut self, e: EntityId, hp: i32) {
        if let Some(c) = self.entities.get_mut(&e) {
            c.hp = hp.clamp(0, c.max_hp);
        }
    }
    fn apply_status(
        &mut self,
        target: EntityId,
        status: &str,
        duration: i32,
        _source: Option<EntityId>,
    ) {
        if let Some(c) = self.entities.get_mut(&target) {
            c.statuses.insert(status.to_owned(), duration);
        }
    }
    fn remove_status(&mut self, target: EntityId, status: &str) {
        if let Some(c) = self.entities.get_mut(&target) {
            c.statuses.remove(status);
        }
    }
}
