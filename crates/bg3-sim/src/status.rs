//! Statuses, minimal: definitions from the stats (`StatusData` entries) and the set of active
//! statuses per entity with their remaining duration.
//!
//! The real status machine (`esv::StatusMachine`: attempts, immunities, stacking, boosts,
//! turn-based durations) is not modelled; `ApplyStatus` from the story makes the status active
//! and raises `StatusAttempt` + `StatusApplied`, `RemoveStatus` removes it and raises
//! `StatusRemoved`.

use std::collections::HashMap;

use crate::guid::Guid;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusDef {
    pub name: String,
    /// `StatusType` (`BOOST`, `EFFECT`, `INCAPACITATED`, …).
    pub status_type: String,
    /// `StatusGroups`, split on `;`.
    pub groups: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct StatusDefs {
    by_name: HashMap<String, StatusDef>,
}

impl StatusDefs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, def: StatusDef) {
        self.by_name.insert(def.name.clone(), def);
    }

    pub fn get(&self, name: &str) -> Option<&StatusDef> {
        self.by_name.get(name)
    }

    /// All `StatusData` entries of a merged stats database.
    pub fn from_stats(stats: &bg3_stats::Stats) -> Self {
        let mut out = Self::new();
        for o in stats.entries() {
            if o.type_name.as_deref() != Some("StatusData") {
                continue;
            }
            out.insert(StatusDef {
                name: o.name.clone(),
                status_type: o.get("StatusType").unwrap_or_default().to_owned(),
                groups: o
                    .get("StatusGroups")
                    .unwrap_or_default()
                    .split(';')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect(),
            });
        }
        out
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
}

/// An active status on an entity.
#[derive(Clone, Debug, PartialEq)]
pub struct ActiveStatus {
    pub id: String,
    /// Source object; null if none.
    pub source: Guid,
    /// Seconds left; negative = until removed.
    pub remaining: f32,
}
