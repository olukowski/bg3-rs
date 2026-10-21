//! Fact storage (`CReteDBase`).
//!
//! In libOsiris a database is a `std::list<CTuple>` (new tuples are appended at the end) plus,
//! per column, a `std::multimap<COsiTypedValue, list iterator>` index. Every query result
//! (`Select`) is produced by walking the tuple list, so **rows always come back in insertion
//! order**, regardless of which index narrowed the search. Deleting a row unlinks it from the
//! list; re-inserting it later appends it at the end again.
//!
//! Here: rows live in a slot vector in insertion order (tombstones for deleted rows,
//! compacted now and then), with a hash index for whole-row lookups and one per column.
//! Equality is the Osiris equivalence ([`Value::key`]).

use std::collections::HashMap;

use crate::value::{Key, Value};

pub type Row = Box<[Value]>;

#[derive(Debug, Clone)]
pub(crate) struct Db {
    arity: usize,
    rows: Vec<Option<Row>>,
    live: usize,
    exact: HashMap<Box<[Key]>, u32>,
    columns: Vec<HashMap<Key, Vec<u32>>>,
}

impl Db {
    pub fn new(arity: usize) -> Self {
        Self {
            arity,
            rows: Vec::new(),
            live: 0,
            exact: HashMap::new(),
            columns: vec![HashMap::new(); arity],
        }
    }

    pub fn arity(&self) -> usize {
        self.arity
    }

    /// Number of rows (`CReteDBase` tuple count, what `SysCount` reports).
    pub fn len(&self) -> usize {
        self.live
    }

    /// Rows in insertion order.
    pub fn rows(&self) -> impl Iterator<Item = &[Value]> {
        self.rows.iter().filter_map(|r| r.as_deref())
    }

    /// The `n`-th row (0-based) in insertion order.
    pub fn nth(&self, n: usize) -> Option<&[Value]> {
        self.rows().nth(n)
    }

    fn row_key(row: &[Value]) -> Option<Box<[Key]>> {
        row.iter()
            .map(|v| match v.key() {
                Key::Undef => None,
                k => Some(k),
            })
            .collect()
    }

    /// Exact lookup (`CReteDBase::find`). A zero-arity database, an empty tuple or a tuple with
    /// an unset value never matches (Osiris asserts "Value is not valid" on the latter).
    // bg3: CReteDBase::find (4.1.1.7631656)
    pub fn find(&self, tuple: &[Value]) -> Option<u32> {
        if self.arity == 0 || tuple.is_empty() {
            return None;
        }
        let key = Self::row_key(tuple)?;
        self.exact.get(&key).copied()
    }

    /// `CReteDBase::insert`: append unless an equivalent row exists. Returns whether the
    /// tuple is new. Zero-arity databases store nothing but report every empty tuple as new.
    // bg3: CReteDBase::insert, CReteDBase::_insert (4.1.1.7631656)
    pub fn insert(&mut self, tuple: &[Value]) -> bool {
        if self.arity == 0 {
            return tuple.is_empty();
        }
        if self.find(tuple).is_some() {
            return false;
        }
        self.push(tuple);
        true
    }

    /// Unconditional append (`_insert`); callers have done the `find`.
    pub(crate) fn push(&mut self, tuple: &[Value]) {
        if self.arity == 0 {
            return;
        }
        let slot = self.rows.len() as u32;
        let row: Row = tuple.into();
        if let Some(key) = Self::row_key(&row) {
            self.exact.entry(key).or_insert(slot);
        }
        for (col, v) in row.iter().enumerate().take(self.arity) {
            self.columns[col].entry(v.key()).or_default().push(slot);
        }
        self.rows.push(Some(row));
        self.live += 1;
    }

    fn remove_slot(&mut self, slot: u32) {
        let Some(row) = self.rows[slot as usize].take() else {
            return;
        };
        self.live -= 1;
        if let Some(key) = Self::row_key(&row)
            && self.exact.get(&key) == Some(&slot)
        {
            self.exact.remove(&key);
        }
        for (col, v) in row.iter().enumerate().take(self.arity) {
            let k = v.key();
            if let Some(list) = self.columns[col].get_mut(&k) {
                if let Ok(pos) = list.binary_search(&slot) {
                    list.remove(pos);
                }
                if list.is_empty() {
                    self.columns[col].remove(&k);
                }
            }
        }
    }

    fn maybe_compact(&mut self) {
        let dead = self.rows.len() - self.live;
        if dead > 64 && dead > self.live * 2 {
            self.compact();
        }
    }

    fn compact(&mut self) {
        let rows: Vec<Row> = self.rows.drain(..).flatten().collect();
        self.live = 0;
        self.exact.clear();
        for c in &mut self.columns {
            c.clear();
        }
        for r in rows {
            self.push(&r);
        }
    }

    /// `CReteDBase::Select`: slots of all rows matching every `(column, value)` constraint, in
    /// insertion order. Any unset constraint value, or a column out of range, gives no rows.
    /// No constraints = all rows.
    // bg3: CReteDBase::Select, CReteDBase::_TupleRefs (4.1.1.7631656)
    pub fn select(&self, desc: &[(u8, Value)]) -> Vec<u32> {
        if desc.is_empty() {
            return (0..self.rows.len() as u32)
                .filter(|&s| self.rows[s as usize].is_some())
                .collect();
        }
        let mut keys = Vec::with_capacity(desc.len());
        for (col, v) in desc {
            let k = v.key();
            if k == Key::Undef || *col as usize >= self.arity {
                return Vec::new();
            }
            keys.push((*col as usize, k));
        }
        let mut best: Option<&Vec<u32>> = None;
        for &(col, k) in &keys {
            match self.columns[col].get(&k) {
                None => return Vec::new(),
                Some(list) if best.is_none_or(|b| list.len() < b.len()) => best = Some(list),
                Some(_) => {}
            }
        }
        let best = best.expect("non-empty constraints");
        best.iter()
            .copied()
            .filter(|&s| {
                let row = self.rows[s as usize].as_ref().expect("indexed row is live");
                keys.iter().all(|&(col, k)| row[col].key() == k)
            })
            .collect()
    }

    pub fn row(&self, slot: u32) -> &[Value] {
        self.rows[slot as usize].as_deref().expect("live row")
    }

    /// `CReteDBase::Valid`: exact lookup if every value of `tuple` is set, otherwise whether
    /// any row matches the set columns.
    // bg3: CReteDBase::Valid (4.1.1.7631656)
    pub fn valid(&self, tuple: &[Value]) -> bool {
        let desc = set_columns(tuple);
        if desc.len() == tuple.len() {
            self.find(tuple).is_some()
        } else {
            !self.select(&desc).is_empty()
        }
    }

    /// `CReteDBase::erase(CTuple)`: with unset values, erase every row matching the set
    /// columns; otherwise the exact row. Returns whether anything was erased.
    // bg3: CReteDBase::erase(CTuple const&) (4.1.1.7631656)
    pub fn erase(&mut self, tuple: &[Value]) -> bool {
        let desc = set_columns(tuple);
        if desc.len() < tuple.len() {
            let slots = self.select(&desc);
            let any = !slots.is_empty();
            for s in slots {
                self.remove_slot(s);
            }
            self.maybe_compact();
            any
        } else if let Some(s) = self.find(tuple) {
            self.remove_slot(s);
            self.maybe_compact();
            true
        } else {
            false
        }
    }

    /// Remove all rows (`CReteDBase::erase()`, used by `SysClear`). No propagation.
    pub fn clear(&mut self) {
        self.rows.clear();
        self.live = 0;
        self.exact.clear();
        for c in &mut self.columns {
            c.clear();
        }
    }
}

/// `COsiColumnIdxValuePairList::From(CTuple)`: the set (VALID) columns of a tuple.
fn set_columns(tuple: &[Value]) -> Vec<(u8, Value)> {
    tuple
        .iter()
        .enumerate()
        .filter(|(_, v)| v.is_valid())
        .map(|(i, v)| (i as u8, *v))
        .collect()
}
