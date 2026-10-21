//! Typed Osiris values, the string table and the comparison rules.
//!
//! Osiris values (`COsiTypedValueBase`) carry a type id (possibly an alias such as `CHARACTER`)
//! and a payload interpreted by the *root* built-in type. Strings are interned: the 64-bit
//! payload of a STRING/GUIDSTRING packs three 21-bit ids (`COsiStringTable::AddStr`):
//!
//! * the exact (case-sensitive) string,
//! * the ASCII-lowercased string — STRING `==` compares this one, so string equality is
//!   case-insensitive,
//! * for GUIDSTRING only: the 128-bit UUID parsed from the last 36 characters — GUIDSTRING
//!   `==` compares this one, so `"S_Foo_<uuid>"` equals `"<uuid>"`, and every string without a
//!   valid non-zero UUID (`""`, `NULL_00000000-…`) gets id 0 and compares equal to every other
//!   such string.
//!
//! [`OsiStr`] keeps those three ids, so equality never needs the table.

use std::cmp::Ordering;
use std::collections::HashMap;

use bg3_story::{BuiltinType, RelOp, Story};

pub use bg3_story::TypeId;

/// Root of a type's alias chain.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Root {
    /// Type 0 (unset), or a reference to an unknown type.
    None,
    Integer,
    Integer64,
    Real,
    String,
    GuidString,
    /// A "DIV object" type (no built-in root). Not used by BG3.
    Object,
}

impl Root {
    pub fn of_builtin(b: BuiltinType) -> Self {
        match b {
            BuiltinType::None => Self::None,
            BuiltinType::Integer => Self::Integer,
            BuiltinType::Integer64 => Self::Integer64,
            BuiltinType::Real => Self::Real,
            BuiltinType::String => Self::String,
            BuiltinType::GuidString => Self::GuidString,
        }
    }

    fn is_numeric(self) -> bool {
        matches!(self, Self::Integer | Self::Integer64 | Self::Real)
    }
}

/// Built-in type ids.
pub mod types {
    use super::TypeId;
    pub const NONE: TypeId = 0;
    pub const INTEGER: TypeId = 1;
    pub const INTEGER64: TypeId = 2;
    pub const REAL: TypeId = 3;
    pub const STRING: TypeId = 4;
    pub const GUIDSTRING: TypeId = 5;
}

/// Resolves type ids to their root, following aliases.
// bg3: COsiTypedValueBase::operator== — the alias walk at its start (4.1.1.7631656)
#[derive(Debug, Clone)]
pub struct TypeTable {
    roots: Vec<Root>,
}

impl TypeTable {
    pub fn from_story(story: &Story) -> Self {
        let roots = (0..256u16)
            .map(|id| match story.root_type(id) {
                Some(b) => Root::of_builtin(b),
                None if id == 0 => Root::None,
                None if story.type_info(id).is_some() => Root::Object,
                None => Root::None,
            })
            .collect();
        Self { roots }
    }

    /// Only the built-in types.
    pub fn builtin() -> Self {
        let mut roots = vec![Root::None; 256];
        roots[1] = Root::Integer;
        roots[2] = Root::Integer64;
        roots[3] = Root::Real;
        roots[4] = Root::String;
        roots[5] = Root::GuidString;
        Self { roots }
    }

    pub fn root(&self, ty: TypeId) -> Root {
        self.roots.get(ty as usize).copied().unwrap_or(Root::None)
    }
}

/// An interned string (see the module docs).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct OsiStr {
    exact: u32,
    folded: u32,
    guid: u32,
}

impl OsiStr {
    /// The empty / null string (all ids 0).
    pub const EMPTY: Self = Self {
        exact: 0,
        folded: 0,
        guid: 0,
    };
}

/// Interned strings. Id 0 is always the empty string.
// bg3: COsiStringTable::AddStr (4.1.1.7631656)
#[derive(Debug, Clone)]
pub struct StringTable {
    texts: Vec<Box<str>>,
    exact: HashMap<Box<str>, u32>,
    folded: HashMap<Box<str>, u32>,
    guids: HashMap<u128, u32>,
}

impl Default for StringTable {
    fn default() -> Self {
        Self {
            texts: vec!["".into()],
            exact: HashMap::new(),
            folded: HashMap::new(),
            guids: HashMap::new(),
        }
    }
}

impl StringTable {
    /// Intern `text`. `guid` requests the UUID id (done for GUIDSTRING-rooted values only).
    pub fn intern(&mut self, text: &str, guid: bool) -> OsiStr {
        if text.is_empty() {
            return OsiStr::EMPTY;
        }
        let exact = match self.exact.get(text) {
            Some(&id) => id,
            None => {
                let id = self.texts.len() as u32;
                self.texts.push(text.into());
                self.exact.insert(text.into(), id);
                id
            }
        };
        let lower = text.to_ascii_lowercase();
        let next = self.folded.len() as u32 + 1;
        let folded = *self.folded.entry(lower.into()).or_insert(next);
        let guid = if guid {
            match parse_uuid_suffix(text) {
                Some(u) if u != 0 => {
                    let next = self.guids.len() as u32 + 1;
                    *self.guids.entry(u).or_insert(next)
                }
                _ => 0,
            }
        } else {
            0
        };
        OsiStr {
            exact,
            folded,
            guid,
        }
    }

    pub fn text(&self, s: OsiStr) -> &str {
        &self.texts[s.exact as usize]
    }
}

/// The UUID in the last 36 characters (`8-4-4-4-12` hex digits), as Osiris parses it for
/// GUIDSTRING ids. `None` if the suffix is not a well-formed UUID.
// bg3: COsiStringTable::AddStr — GUID suffix parse (4.1.1.7631656)
pub fn parse_uuid_suffix(text: &str) -> Option<u128> {
    let b = text.as_bytes();
    if b.len() < 36 {
        return None;
    }
    let tail = &b[b.len() - 36..];
    let mut v: u128 = 0;
    for (i, &c) in tail.iter().enumerate() {
        if matches!(i, 8 | 13 | 18 | 23) {
            if c != b'-' {
                return None;
            }
            continue;
        }
        let d = (c as char).to_digit(16)?;
        v = (v << 4) | d as u128;
    }
    Some(v)
}

/// Payload of a value. `Undef` = the VALID flag is clear (an unbound variable, an out
/// parameter before the call, ...).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Data {
    Undef,
    Int(i32),
    Int64(i64),
    Real(f32),
    Str(OsiStr),
}

/// A typed Osiris value.
#[derive(Clone, Copy, Debug)]
pub struct Value {
    /// Declared type id (may be an alias, e.g. `CHARACTER`).
    pub ty: TypeId,
    /// Root built-in type of `ty`.
    pub root: Root,
    pub data: Data,
}

impl Value {
    /// The fully unset value (type 0), as used for undefined token slots.
    pub const UNDEF: Self = Self {
        ty: types::NONE,
        root: Root::None,
        data: Data::Undef,
    };

    pub fn int(v: i32) -> Self {
        Self {
            ty: types::INTEGER,
            root: Root::Integer,
            data: Data::Int(v),
        }
    }

    pub fn int64(v: i64) -> Self {
        Self {
            ty: types::INTEGER64,
            root: Root::Integer64,
            data: Data::Int64(v),
        }
    }

    pub fn real(v: f32) -> Self {
        Self {
            ty: types::REAL,
            root: Root::Real,
            data: Data::Real(v),
        }
    }

    /// An unset value of type `ty`.
    pub fn undef(ty: TypeId, root: Root) -> Self {
        Self {
            ty,
            root,
            data: Data::Undef,
        }
    }

    /// The VALID flag.
    pub fn is_valid(&self) -> bool {
        !matches!(self.data, Data::Undef)
    }

    pub fn as_int(&self) -> Option<i32> {
        match self.data {
            Data::Int(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_int64(&self) -> Option<i64> {
        match self.data {
            Data::Int64(v) => Some(v),
            Data::Int(v) => Some(v as i64),
            _ => None,
        }
    }

    pub fn as_real(&self) -> Option<f32> {
        match self.data {
            Data::Real(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<OsiStr> {
        match self.data {
            Data::Str(s) => Some(s),
            _ => None,
        }
    }

    fn int_payload(&self) -> i32 {
        match self.data {
            Data::Int(v) => v,
            _ => 0,
        }
    }

    fn int64_payload(&self) -> i64 {
        match self.data {
            Data::Int64(v) => v,
            Data::Int(v) => v as i64,
            _ => 0,
        }
    }

    fn real_payload(&self) -> f32 {
        match self.data {
            Data::Real(v) => v,
            _ => 0.0,
        }
    }

    fn str_payload(&self) -> OsiStr {
        match self.data {
            Data::Str(s) => s,
            _ => OsiStr::EMPTY,
        }
    }

    /// Equivalence class used by database indexes. Two values with the same key are
    /// equivalent under `COsiTypedValueBase::operator<` (and `==`).
    pub(crate) fn key(&self) -> Key {
        if !self.is_valid() {
            return Key::Undef;
        }
        match self.root {
            Root::Integer => Key::Int(self.int_payload() as i64),
            Root::Integer64 => Key::Int64(self.int64_payload()),
            Root::Real => {
                let f = self.real_payload();
                Key::Real(if f == 0.0 { 0 } else { f.to_bits() })
            }
            Root::String => Key::Str(self.str_payload().folded),
            Root::GuidString => Key::Guid(self.str_payload().guid),
            Root::None | Root::Object => Key::Undef,
        }
    }
}

/// Hashable equivalence key of a value (see [`Value::key`]).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Key {
    Undef,
    Int(i64),
    Int64(i64),
    Real(u32),
    Str(u32),
    Guid(u32),
}

/// `COsiTypedValueBase::operator==`: both roots must match (otherwise Osiris logs "invalid
/// comparison" and answers false — reported as `Err`). INTEGER/INTEGER64/REAL compare by value,
/// STRING by lowercased string, GUIDSTRING by UUID id. Unset payloads compare as 0.
// bg3: COsiTypedValueBase::operator== (4.1.1.7631656)
pub fn osi_eq(a: &Value, b: &Value) -> Result<bool, ()> {
    if a.root != b.root {
        return Err(());
    }
    Ok(match a.root {
        Root::Integer => a.int_payload() == b.int_payload(),
        Root::Integer64 => a.int64_payload() == b.int64_payload(),
        Root::Real => a.real_payload() == b.real_payload(),
        Root::String => a.str_payload().folded == b.str_payload().folded,
        Root::GuidString => a.str_payload().guid == b.str_payload().guid,
        Root::None | Root::Object => return Err(()),
    })
}

/// C `strcasecmp` (ASCII case folding).
fn strcasecmp(a: &str, b: &str) -> Ordering {
    let a = a.bytes().map(|c| c.to_ascii_lowercase());
    let b = b.bytes().map(|c| c.to_ascii_lowercase());
    a.cmp(b)
}

/// The last 36 characters of a GUID string. Osiris takes `strlen - 36` unconditionally; for
/// shorter strings (undefined behaviour there) we use the whole string.
fn uuid_tail(s: &str) -> &str {
    if s.len() >= 36 && s.is_char_boundary(s.len() - 36) {
        &s[s.len() - 36..]
    } else {
        s
    }
}

fn apply<T: PartialOrd>(a: T, b: T, op: RelOp) -> bool {
    match op {
        RelOp::Less => a < b,
        RelOp::LessOrEqual => a <= b,
        RelOp::Greater => a > b,
        RelOp::GreaterOrEqual => a >= b,
        RelOp::Equal => a == b,
        RelOp::NotEqual => a != b,
        RelOp::Unknown(_) => false,
    }
}

fn apply_ord(o: Ordering, op: RelOp) -> bool {
    apply(o, Ordering::Equal, op)
}

/// `Compare(a, b, op)` of a compare node. `Err` = combination Osiris rejects ("Invalid
/// compare", result false).
///
/// * Numbers mix freely: if either side is REAL the comparison is done in `f32`, else if
///   either side is INTEGER64 in `i64`, else in `i32`.
/// * STRING op STRING: `==`/`!=` by lowercased id, ordering by `strcasecmp` of the full text.
/// * GUIDSTRING op GUIDSTRING: `==`/`!=` by UUID id, ordering by `strcasecmp` of the last 36
///   characters.
/// * GUIDSTRING op STRING: everything by `strcasecmp` of the last 36 characters of both.
/// * STRING op GUIDSTRING, numbers vs strings, DIV objects: rejected.
// bg3: Compare(COsiTypedValueBase const&, COsiTypedValueBase const&, TOsiRelOp) (4.1.1.7631656)
pub fn osi_compare(strings: &StringTable, a: &Value, b: &Value, op: RelOp) -> Result<bool, ()> {
    if matches!(op, RelOp::Unknown(_)) {
        return Err(());
    }
    match (a.root, b.root) {
        (l, r) if l.is_numeric() && r.is_numeric() => Ok(if r == Root::Real || l == Root::Real {
            let fl = match l {
                Root::Integer => a.int_payload() as f32,
                Root::Integer64 => a.int64_payload() as f32,
                _ => a.real_payload(),
            };
            let fr = match r {
                Root::Integer => b.int_payload() as f32,
                Root::Integer64 => b.int64_payload() as f32,
                _ => b.real_payload(),
            };
            apply(fl, fr, op)
        } else if l == Root::Integer64 || r == Root::Integer64 {
            apply(a.int64_payload(), b.int64_payload(), op)
        } else {
            apply(a.int_payload(), b.int_payload(), op)
        }),
        (Root::String, Root::String) => Ok(match op {
            RelOp::Equal | RelOp::NotEqual => {
                apply(a.str_payload().folded == b.str_payload().folded, true, op)
            }
            _ => apply_ord(
                strcasecmp(strings.text(a.str_payload()), strings.text(b.str_payload())),
                op,
            ),
        }),
        (Root::GuidString, Root::GuidString) => Ok(match op {
            RelOp::Equal | RelOp::NotEqual => {
                apply(a.str_payload().guid == b.str_payload().guid, true, op)
            }
            _ => apply_ord(
                strcasecmp(
                    uuid_tail(strings.text(a.str_payload())),
                    uuid_tail(strings.text(b.str_payload())),
                ),
                op,
            ),
        }),
        (Root::GuidString, Root::String) => Ok(apply_ord(
            strcasecmp(
                uuid_tail(strings.text(a.str_payload())),
                uuid_tail(strings.text(b.str_payload())),
            ),
            op,
        )),
        _ => Err(()),
    }
}
