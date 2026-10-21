//! Merged stats database: files applied in load order, `using` inheritance resolved.

use std::collections::HashMap;

use crate::Error;
use crate::file::{
    Equipment, ItemGroup, ItemType, ModifierList, NameGroup, RawEntry, Record, SpellSet, StatsFile,
    TreasureTable, ValueList, Warning,
};

/// Where a definition came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub module: String,
    pub path: String,
    pub line: u32,
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.path, self.line)
    }
}

/// A stats object (`new entry`, `new ItemCombination`, …) with inheritance applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatsObject {
    pub kind: String,
    pub name: String,
    /// `type "…"`; inherited from the parent when the entry does not declare one.
    pub type_name: Option<String>,
    pub using: Option<String>,
    /// Effective fields: the parent's effective fields overridden by this entry's own `data`
    /// lines. Order: first appearance.
    pub fields: Vec<(String, String)>,
    /// This entry's own `data` lines only.
    pub own: Vec<(String, String)>,
    pub source: Source,
    /// Earlier definitions of the same name that this one replaced (oldest first). They only
    /// matter through `using "<own name>"`, which inherits from the previous definition.
    pub previous: Vec<Source>,
}

impl StatsObject {
    /// Effective value of a field.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// An entry whose `using` parent could not be found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unresolved {
    pub kind: String,
    pub name: String,
    pub parent: String,
    pub source: Source,
}

/// Something with a name, stored in a [`Table`].
pub trait Named {
    fn name(&self) -> &str;
}

macro_rules! named {
    ($($t:ty),*) => {$(impl Named for $t { fn name(&self) -> &str { &self.name } })*};
}
named!(
    StatsObject,
    TreasureTable,
    Equipment,
    SpellSet,
    ItemType,
    Record,
    NameGroup,
    ItemGroup,
    ModifierList,
    ValueList
);

/// Name-indexed collection that keeps first-definition order. Redefinitions replace in place.
#[derive(Debug, Clone)]
pub struct Table<T> {
    items: Vec<T>,
    index: HashMap<String, usize>,
}

impl<T> Default for Table<T> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            index: HashMap::new(),
        }
    }
}

impl<T: Named> Table<T> {
    /// Insert or replace; returns `true` if an existing item was replaced.
    pub fn insert(&mut self, item: T) -> bool {
        match self.index.get(item.name()) {
            Some(&i) => {
                self.items[i] = item;
                true
            }
            None => {
                self.index.insert(item.name().to_owned(), self.items.len());
                self.items.push(item);
                false
            }
        }
    }

    pub fn get(&self, name: &str) -> Option<&T> {
        self.index.get(name).map(|&i| &self.items[i])
    }

    fn get_mut(&mut self, name: &str) -> Option<&mut T> {
        self.index.get(name).map(|&i| &mut self.items[i])
    }

    pub fn iter(&self) -> std::slice::Iter<'_, T> {
        self.items.iter()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// All stats loaded from a sequence of files.
#[derive(Debug, Default)]
pub struct Stats {
    /// Raw definitions by kind, then name; each name keeps its chain of definitions (oldest
    /// first). Resolved into `objects` by [`Stats::finish`].
    defs: HashMap<String, Table<DefChain>>,
    /// Resolved objects by kind (`entry`, `ItemCombination`, `ItemCombinationResult`,
    /// `CraftingPreviewData`). Each kind is its own namespace.
    objects: HashMap<String, Table<StatsObject>>,
    pub treasure_item_types: Vec<String>,
    pub treasure_tables: Table<TreasureTable>,
    pub equipment: Table<Equipment>,
    pub spell_sets: Table<SpellSet>,
    pub item_types: Table<ItemType>,
    pub item_colors: Table<Record>,
    pub blood_types: Table<Record>,
    pub name_groups: Table<NameGroup>,
    pub item_groups: Table<ItemGroup>,
    /// `key` settings (Data.txt, XPData.txt); later definitions win.
    pub keys: HashMap<String, (String, Source)>,
    pub modifier_lists: Table<ModifierList>,
    pub value_lists: Table<ValueList>,
    pub warnings: Vec<(Source, Warning)>,
    /// Definitions that replaced an earlier one of the same name and type (count).
    pub redefinitions: usize,
    /// Definitions dropped because an earlier definition of the same name has another type.
    pub rejected: Vec<(RawEntry, Source)>,
    unresolved: Vec<Unresolved>,
}

/// All definitions of one name, oldest first.
#[derive(Debug, Clone)]
struct DefChain {
    name: String,
    defs: Vec<(RawEntry, Source)>,
}

impl Named for DefChain {
    fn name(&self) -> &str {
        &self.name
    }
}

impl Stats {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse and apply one file. `module` is the mod folder (e.g. `Shared`), `path` is for
    /// diagnostics.
    pub fn load_str(&mut self, module: &str, path: &str, src: &str) -> Result<(), Error> {
        let file = StatsFile::parse(src).map_err(|e| Error::InFile {
            path: path.to_owned(),
            source: Box::new(e),
        })?;
        self.apply(module, path, file);
        Ok(())
    }

    /// Apply a parsed file on top of everything loaded so far. Call [`Stats::finish`] after the
    /// last file to resolve inheritance.
    ///
    /// Model (see `docs/formats/stats.md`): a later definition of a name replaces the earlier
    /// one, which is kept as its "previous" definition; a later definition with a different
    /// `type` is dropped. `using` is resolved only once everything is loaded, against the
    /// *final* definition of the parent — except `using "<own name>"`, which inherits from the
    /// previous definition of the same name.
    pub fn apply(&mut self, module: &str, path: &str, file: StatsFile) {
        let src = |line| Source {
            module: module.to_owned(),
            path: path.to_owned(),
            line,
        };
        for w in file.warnings {
            self.warnings.push((src(w.line), w));
        }
        for raw in file.entries {
            let source = src(raw.line);
            self.add_entry(raw, source);
        }
        if let Some(types) = file.treasure_item_types {
            self.treasure_item_types = types;
        }
        for t in file.treasure_tables {
            self.treasure_tables.insert(t);
        }
        for t in file.equipment {
            self.equipment.insert(t);
        }
        for t in file.spell_sets {
            self.spell_sets.insert(t);
        }
        for t in file.item_types {
            self.item_types.insert(t);
        }
        for t in file.item_colors {
            self.item_colors.insert(t);
        }
        for t in file.blood_types {
            self.blood_types.insert(t);
        }
        for t in file.name_groups {
            self.name_groups.insert(t);
        }
        for t in file.item_groups {
            self.item_groups.insert(t);
        }
        for (k, v) in file.keys {
            self.keys.insert(k, (v, src(0)));
        }
        for t in file.modifier_lists {
            self.modifier_lists.insert(t);
        }
        for t in file.value_lists {
            self.value_lists.insert(t);
        }
    }

    fn add_entry(&mut self, raw: RawEntry, source: Source) {
        // bg3: RPGStats::InsertDataBuffer (4.1.1.7631656) — same name replaces and links the
        // previous buffer; a differing type (we assume that is the compared field) is dropped.
        let table = self.defs.entry(raw.kind.clone()).or_default();
        match table.get_mut(&raw.name) {
            Some(chain) => {
                let head = &chain.defs.last().unwrap().0;
                if head.type_name.is_some()
                    && raw.type_name.is_some()
                    && head.type_name != raw.type_name
                {
                    self.rejected.push((raw, source));
                } else {
                    chain.defs.push((raw, source));
                    self.redefinitions += 1;
                }
            }
            None => {
                table.insert(DefChain {
                    name: raw.name.clone(),
                    defs: vec![(raw, source)],
                });
            }
        }
    }

    /// Resolve `using` inheritance for everything applied so far. Call once after the last file.
    /// Entries whose parent doesn't exist (or that form a cycle) are kept with only their own
    /// fields and listed in [`Stats::unresolved`].
    pub fn finish(&mut self) {
        self.objects.clear();
        self.unresolved.clear();
        let kinds: Vec<String> = self.defs.keys().cloned().collect();
        for kind in kinds {
            let mut memo: HashMap<(String, usize), Resolved> = HashMap::new();
            let table = &self.defs[&kind];
            let mut out = Table::default();
            for chain in table.iter() {
                let top = chain.defs.len() - 1;
                let r = resolve(
                    table,
                    &chain.name,
                    top,
                    &mut memo,
                    &mut Vec::new(),
                    &mut self.unresolved,
                );
                let (raw, source) = &chain.defs[top];
                out.insert(StatsObject {
                    kind: kind.clone(),
                    name: chain.name.clone(),
                    type_name: r.type_name,
                    using: raw.using.clone(),
                    fields: r.fields,
                    own: raw.data.clone(),
                    source: source.clone(),
                    previous: chain.defs[..top].iter().map(|(_, s)| s.clone()).collect(),
                });
            }
            self.objects.insert(kind, out);
        }
    }

    /// Entries whose `using` parent was never found (after [`Stats::finish`]).
    pub fn unresolved(&self) -> &[Unresolved] {
        &self.unresolved
    }

    /// A stats object of kind `entry` (spells, statuses, passives, items, characters, …).
    pub fn get(&self, name: &str) -> Option<&StatsObject> {
        self.get_kind("entry", name)
    }

    pub fn get_kind(&self, kind: &str, name: &str) -> Option<&StatsObject> {
        self.objects.get(kind)?.get(name)
    }

    /// Object tables by kind.
    pub fn kinds(&self) -> impl Iterator<Item = (&str, &Table<StatsObject>)> {
        self.objects.iter().map(|(k, t)| (k.as_str(), t))
    }

    /// All objects of kind `entry`.
    pub fn entries(&self) -> impl Iterator<Item = &StatsObject> {
        self.objects.get("entry").into_iter().flat_map(Table::iter)
    }

    pub fn key(&self, name: &str) -> Option<&str> {
        self.keys.get(name).map(|(v, _)| v.as_str())
    }
}

#[derive(Debug, Clone, Default)]
struct Resolved {
    type_name: Option<String>,
    fields: Vec<(String, String)>,
}

/// Effective fields of definition `level` of `name` (memoised, cycle-safe).
fn resolve(
    table: &Table<DefChain>,
    name: &str,
    level: usize,
    memo: &mut HashMap<(String, usize), Resolved>,
    stack: &mut Vec<(String, usize)>,
    unresolved: &mut Vec<Unresolved>,
) -> Resolved {
    let key = (name.to_owned(), level);
    if let Some(r) = memo.get(&key) {
        return r.clone();
    }
    let (raw, source) = &table.get(name).unwrap().defs[level];
    let mut r = Resolved::default();
    if let Some(parent) = &raw.using {
        let target = if parent == name {
            level.checked_sub(1).map(|l| (parent.clone(), l))
        } else {
            table
                .get(parent)
                .map(|c| (parent.clone(), c.defs.len() - 1))
        };
        match target {
            Some(t) if !stack.contains(&t) => {
                stack.push(key.clone());
                r = resolve(table, &t.0, t.1, memo, stack, unresolved);
                stack.pop();
            }
            _ => unresolved.push(Unresolved {
                kind: raw.kind.clone(),
                name: name.to_owned(),
                parent: parent.clone(),
                source: source.clone(),
            }),
        }
    }
    if raw.type_name.is_some() {
        r.type_name = raw.type_name.clone();
    }
    for (key, value) in &raw.data {
        match r.fields.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = value.clone(),
            None => r.fields.push((key.clone(), value.clone())),
        }
    }
    memo.insert(key, r.clone());
    r
}
