//! Action resource pools indexed by resource name and resource level.
use std::collections::HashMap;

use super::{CombatData, data::Replenish};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceSelector {
    pub name: String,
    pub level: u32,
    /// Intermediate numeric field of four-part UseCosts; retained without guessing
    /// semantics for values other than the observed 1.
    pub qualifier: Option<u32>,
}

impl ResourceSelector {
    pub fn as_str(&self) -> &str {
        &self.name
    }
}

impl From<&str> for ResourceSelector {
    fn from(name: &str) -> Self {
        Self {
            name: name.into(),
            level: 0,
            qualifier: None,
        }
    }
}

impl std::fmt::Display for ResourceSelector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.level == 0 {
            write!(f, "{}", self.name)
        } else {
            write!(f, "{} L{}", self.name, self.level)
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Resources {
    pub entries: Vec<ResourceAmount>,
    pub groups: HashMap<String, Vec<String>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResourceAmount {
    pub name: String,
    /// Zero for an unlevelled resource; spell slots use 1–9 as defined in the install.
    pub level: u32,
    pub amount: f32,
    pub max: f32,
}

impl Resources {
    pub fn get(&self, name: &str) -> f32 {
        self.get_at_level(name, 0)
    }
    pub fn max(&self, name: &str) -> f32 {
        self.max_at_level(name, 0)
    }
    // bg3: eoc::action_resource::GetResourceValue (4.1.1.7631656)
    pub fn get_at_level(&self, name: &str, level: u32) -> f32 {
        self.entries
            .iter()
            .find(|r| r.name == name && r.level == level)
            .map_or(0.0, |r| r.amount)
    }
    // bg3: eoc::action_resource::GetResourceMaxValue (4.1.1.7631656)
    pub fn max_at_level(&self, name: &str, level: u32) -> f32 {
        self.entries
            .iter()
            .find(|r| r.name == name && r.level == level)
            .map_or(0.0, |r| r.max)
    }

    /// Resolve the full payment against a scratch balance, so duplicate/overlapping costs
    /// cannot overspend and refusal never partly pays an action.
    // bg3: eoc::action_resource::CanUseResourceCosts (4.1.1.7631656)
    fn payment(
        &self,
        costs: &[(ResourceSelector, f32)],
        slot: Option<u32>,
    ) -> Option<Vec<(usize, f32)>> {
        let mut balances: Vec<_> = self.entries.iter().map(|r| r.amount).collect();
        let mut payment = Vec::new();
        for (cost, amount) in costs {
            if !amount.is_finite() || *amount < 0.0 || cost.qualifier.is_some_and(|q| q != 1) {
                return None;
            }
            if *amount == 0.0 {
                continue;
            }
            let group = self.groups.get(&cost.name);
            let selected_level = if group.is_some() { slot } else { None };
            if selected_level.is_some_and(|l| l < cost.level) {
                return None;
            }
            let (i, _) = self
                .entries
                .iter()
                .enumerate()
                .filter(|(i, r)| {
                    let matches = match group {
                        Some(members) => {
                            members.contains(&r.name)
                                && r.level >= cost.level
                                && selected_level.is_none_or(|l| r.level == l)
                        }
                        None => r.name == cost.name && r.level == cost.level,
                    };
                    matches && balances[*i] + 1e-4 >= *amount
                })
                .min_by_key(|(_, r)| {
                    (
                        r.level,
                        group
                            .and_then(|g| g.iter().position(|n| n == &r.name))
                            .unwrap_or(0),
                    )
                })?;
            balances[i] -= amount;
            payment.push((i, *amount));
        }
        Some(payment)
    }

    /// The largest maximum a cost could be paid from: for a group (`SpellSlotsGroup`), its
    /// member resources at the cost's level or higher.
    pub fn max_for(&self, cost: &ResourceSelector) -> f32 {
        match self.groups.get(&cost.name) {
            Some(members) => self
                .entries
                .iter()
                .filter(|r| members.contains(&r.name) && r.level >= cost.level)
                .map(|r| r.max)
                .fold(0.0, f32::max),
            None => self.max_at_level(&cost.name, cost.level),
        }
    }

    /// The resource a cost names, with a group resolved to its first member.
    pub fn resource_name<'a>(&'a self, cost: &'a ResourceSelector) -> &'a str {
        self.groups
            .get(&cost.name)
            .and_then(|m| m.first())
            .map_or(cost.name.as_str(), String::as_str)
    }

    pub fn can_pay(&self, costs: &[(ResourceSelector, f32)]) -> bool {
        self.payment(costs, None).is_some()
    }
    pub fn pay(&mut self, costs: &[(ResourceSelector, f32)]) -> bool {
        self.pay_with_slot(costs, None)
    }
    /// Default: lowest sufficient slot level. Explicit selection never silently falls back.
    pub fn pay_with_slot(&mut self, costs: &[(ResourceSelector, f32)], slot: Option<u32>) -> bool {
        let Some(payment) = self.payment(costs, slot) else {
            return false;
        };
        for (i, amount) in payment {
            self.entries[i].amount = (self.entries[i].amount - amount).max(0.0);
        }
        true
    }

    pub fn replenish_turn(&mut self, data: &CombatData) {
        for r in &mut self.entries {
            if data
                .resource(&r.name)
                .map_or(r.level == 0, |d| d.replenish == Replenish::Turn)
            {
                r.amount = r.max;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::data::parse_costs;

    fn pools() -> Resources {
        Resources {
            entries: [
                ("ActionPoint", 0, 1.0),
                ("SpellSlot", 1, 2.0),
                ("SpellSlot", 3, 1.0),
            ]
            .into_iter()
            .map(|(name, level, amount)| ResourceAmount {
                name: name.into(),
                level,
                amount,
                max: amount,
            })
            .collect(),
            groups: HashMap::from([("SpellSlotsGroup".into(), vec!["SpellSlot".into()])]),
        }
    }

    #[test]
    fn lowest_available_slot_and_explicit_upcast() {
        let costs = parse_costs("SpellSlotsGroup:1:1:1");
        let mut r = pools();
        assert!(r.pay(&costs));
        assert_eq!(r.get_at_level("SpellSlot", 1), 1.0);
        assert_eq!(r.get_at_level("SpellSlot", 3), 1.0);
        assert!(r.pay_with_slot(&costs, Some(3)));
        assert_eq!(r.get_at_level("SpellSlot", 1), 1.0);
        assert_eq!(r.get_at_level("SpellSlot", 3), 0.0);
        assert!(
            !r.pay_with_slot(&costs, Some(2)),
            "explicit selection cannot fall back"
        );
        assert!(r.pay(&costs));
        assert!(!r.pay(&costs));
        let mut r = pools();
        assert!(r.pay(&costs));
        assert!(r.pay(&costs));
        assert!(r.pay(&costs), "falls back to the next sufficient level");
        assert_eq!(r.get_at_level("SpellSlot", 3), 0.0);
    }

    #[test]
    fn insufficient_level_and_duplicate_costs_are_atomic() {
        let mut r = pools();
        let before = r.clone();
        assert!(!r.pay(&parse_costs("ActionPoint:1;SpellSlotsGroup:1:1:4")));
        assert!(!r.pay_with_slot(&parse_costs("ActionPoint:1;SpellSlotsGroup:1:1:3"), Some(1)));
        assert!(!r.pay(&parse_costs("ActionPoint:1;ActionPoint:1")));
        assert_eq!(r, before);
        assert!(r.pay(&parse_costs("SpellSlotsGroup:1:1:2")));
        assert_eq!(
            r.get_at_level("SpellSlot", 1),
            2.0,
            "a lower slot cannot pay L2"
        );
        assert!(!r.can_pay(&parse_costs("SpellSlotsGroup:1:1:2")));
    }

    #[test]
    fn resource_groups_use_installed_members_without_merging_pools() {
        let mut r = pools();
        r.entries.push(ResourceAmount {
            name: "Pact".into(),
            level: 2,
            amount: 1.0,
            max: 1.0,
        });
        r.groups
            .get_mut("SpellSlotsGroup")
            .unwrap()
            .push("Pact".into());
        assert!(r.pay(&parse_costs("SpellSlotsGroup:1:1:2")));
        assert_eq!(r.get_at_level("Pact", 2), 0.0);
        assert_eq!(r.get_at_level("SpellSlot", 3), 1.0);
    }
}
