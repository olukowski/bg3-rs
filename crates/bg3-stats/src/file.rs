//! Typed model of a single stats text file (before cross-file merging and `using` resolution).

use crate::Error;
use crate::lexer::{Statement, tokenize};

/// A `new <kind> "Name"` block that carries `type` / `using` / `data` lines.
///
/// `kind` is `entry` for the ordinary stats objects in `Data/*.txt`, and `ItemCombination`,
/// `ItemCombinationResult` or `CraftingPreviewData` for the item-combination files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawEntry {
    pub kind: String,
    pub name: String,
    pub type_name: Option<String>,
    pub using: Option<String>,
    /// `data "Key" "Value"` lines in file order (values verbatim, may contain line breaks).
    pub data: Vec<(String, String)>,
    pub line: u32,
}

/// `new treasuretable "Name"` with its subtables.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TreasureTable {
    pub name: String,
    /// `CanMerge 1`: a later table of the same name is merged into this one instead of replacing.
    pub can_merge: bool,
    pub subtables: Vec<TreasureSubTable>,
    pub line: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TreasureSubTable {
    /// The `new subtable "…"` argument verbatim, e.g. `"1,1"`, `"-1"`, `"1,2;2,1"`.
    pub drop_count: String,
    pub start_level: Option<i32>,
    pub end_level: Option<i32>,
    pub objects: Vec<TreasureObject>,
}

/// `object category "Name",frequency,c0,…,c6` (the seven counts follow the `treasure itemtypes`
/// rarity order).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreasureObject {
    pub name: String,
    pub frequency: i32,
    pub counts: Vec<i32>,
}

/// `new equipment "Name"`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Equipment {
    pub name: String,
    pub initial_weapon_set: Option<String>,
    /// Each `add equipmentgroup` opens a group; `add equipment entry "X"` adds to the last group.
    pub groups: Vec<Vec<String>>,
    pub line: u32,
}

/// `new spellset "Name"` + `add spell "X"` lines.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpellSet {
    pub name: String,
    pub spells: Vec<String>,
    pub line: u32,
}

/// `new itemtype "Name",…` (ItemTypes.txt): rarity definition.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ItemType {
    pub name: String,
    /// Remaining positional arguments verbatim (colour, frame, …).
    pub args: Vec<String>,
    pub min_level: Option<i32>,
    /// `new boostgroup <args…>` lines, verbatim tokens.
    pub boost_groups: Vec<Vec<String>>,
    pub line: u32,
}

/// `new namegroup "Name"` + `add name "handle;…","…"`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NameGroup {
    pub name: String,
    pub names: Vec<Vec<String>>,
    pub line: u32,
}

/// `new itemgroup "Name"` (ItemProgressionVisuals.txt).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ItemGroup {
    pub name: String,
    pub level_groups: Vec<LevelGroup>,
    pub line: u32,
}

/// `add levelgroup min,max,"Name"`; following `add rootgroup` / `add namegroup` lines belong to it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LevelGroup {
    pub min_level: i32,
    pub max_level: i32,
    pub name: String,
    pub root_groups: Vec<Vec<String>>,
    pub name_groups: Vec<Vec<String>>,
}

/// A `new <kind> "Name",arg,…` one-liner with positional arguments only (`itemcolor`, `bloodtype`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Record {
    pub name: String,
    pub args: Vec<String>,
    pub line: u32,
}

/// `modifier type "X"` followed by `modifier "Attr","ValueListType"` lines (Structure/Modifiers.txt).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModifierList {
    pub name: String,
    /// (attribute name, value list / type name)
    pub attributes: Vec<(String, String)>,
}

/// `valuelist "X"` followed by `value "Label" : index` lines (Structure/Base/ValueLists.txt).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValueList {
    pub name: String,
    pub values: Vec<(String, i32)>,
}

/// Something the parser accepted but that looks off (not fatal).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub line: u32,
    pub message: String,
}

/// Everything found in one stats text file, in file order per category.
#[derive(Debug, Clone, Default)]
pub struct StatsFile {
    pub entries: Vec<RawEntry>,
    /// `treasure itemtypes "Common",…`
    pub treasure_item_types: Option<Vec<String>>,
    pub treasure_tables: Vec<TreasureTable>,
    pub equipment: Vec<Equipment>,
    pub spell_sets: Vec<SpellSet>,
    pub item_types: Vec<ItemType>,
    pub item_colors: Vec<Record>,
    pub blood_types: Vec<Record>,
    pub name_groups: Vec<NameGroup>,
    pub item_groups: Vec<ItemGroup>,
    /// `key "Name","Value"` (Data.txt, XPData.txt).
    pub keys: Vec<(String, String)>,
    pub modifier_lists: Vec<ModifierList>,
    pub value_lists: Vec<ValueList>,
    pub warnings: Vec<Warning>,
}

/// Which block the following child lines attach to.
#[derive(Clone, Copy)]
enum Ctx {
    None,
    Entry,
    Treasure,
    Equipment,
    SpellSet,
    ItemType,
    NameGroup,
    ItemGroup,
    Modifier,
    ValueList,
}

impl StatsFile {
    pub fn parse(src: &str) -> Result<Self, Error> {
        let mut f = StatsFile::default();
        let mut ctx = Ctx::None;
        let mut prev_unterminated = false;
        for st in tokenize(src) {
            let continuation = prev_unterminated && st.unterminated;
            prev_unterminated = st.unterminated && !continuation;
            if st.tokens[0].quoted || continuation {
                // The tail of a value that was broken across two lines (the closing quote is on
                // this line). It has no keyword of its own; skipped (see docs/formats/stats.md).
                f.warn(
                    st.line,
                    format!(
                        "skipped continuation line {:?}",
                        st.texts().collect::<Vec<_>>()
                    ),
                );
                continue;
            }
            if st.unterminated {
                f.warn(st.line, "unterminated string (runs to end of line)");
            }
            ctx = f.statement(&st, ctx)?;
        }
        Ok(f)
    }

    fn warn(&mut self, line: u32, message: impl Into<String>) {
        self.warnings.push(Warning {
            line,
            message: message.into(),
        });
    }

    fn statement(&mut self, st: &Statement, ctx: Ctx) -> Result<Ctx, Error> {
        let line = st.line;
        let bad = |what: &str| Error::Syntax {
            line,
            message: format!("{what}: {:?}", st.texts().collect::<Vec<_>>()),
        };
        let head = st
            .bare(0)
            .ok_or_else(|| bad("statement must start with a keyword"))?;
        let n = st.tokens.len();
        let arg = |i: usize| st.word(i).ok_or_else(|| bad("missing argument"));
        let int = |i: usize| -> Result<i32, Error> {
            arg(i)?.trim().parse().map_err(|_| bad("expected integer"))
        };
        let rest = |from: usize| st.texts().skip(from).map(str::to_owned).collect::<Vec<_>>();
        let expect_len = |f: &mut Self, len: usize| {
            if n > len {
                f.warn(
                    line,
                    format!("ignored {} extra token(s) after `{head}`", n - len),
                );
            }
        };

        Ok(match (head, ctx) {
            ("new", _) => {
                let kind = st.bare(1).ok_or_else(|| bad("`new` without kind"))?;
                match kind {
                    "entry"
                    | "ItemCombination"
                    | "ItemCombinationResult"
                    | "CraftingPreviewData" => {
                        expect_len(self, 3);
                        self.entries.push(RawEntry {
                            kind: kind.to_owned(),
                            name: arg(2)?.to_owned(),
                            type_name: None,
                            using: None,
                            data: Vec::new(),
                            line,
                        });
                        Ctx::Entry
                    }
                    "treasuretable" => {
                        expect_len(self, 3);
                        self.treasure_tables.push(TreasureTable {
                            name: arg(2)?.to_owned(),
                            line,
                            ..Default::default()
                        });
                        Ctx::Treasure
                    }
                    "subtable" => {
                        expect_len(self, 3);
                        let table = match ctx {
                            Ctx::Treasure => self.treasure_tables.last_mut().unwrap(),
                            _ => return Err(bad("subtable outside treasuretable")),
                        };
                        table.subtables.push(TreasureSubTable {
                            drop_count: arg(2)?.to_owned(),
                            ..Default::default()
                        });
                        Ctx::Treasure
                    }
                    "equipment" => {
                        expect_len(self, 3);
                        self.equipment.push(Equipment {
                            name: arg(2)?.to_owned(),
                            line,
                            ..Default::default()
                        });
                        Ctx::Equipment
                    }
                    "spellset" => {
                        expect_len(self, 3);
                        self.spell_sets.push(SpellSet {
                            name: arg(2)?.to_owned(),
                            line,
                            ..Default::default()
                        });
                        Ctx::SpellSet
                    }
                    "itemtype" => {
                        self.item_types.push(ItemType {
                            name: arg(2)?.to_owned(),
                            args: rest(3),
                            line,
                            ..Default::default()
                        });
                        Ctx::ItemType
                    }
                    "boostgroup" => {
                        let Ctx::ItemType = ctx else {
                            return Err(bad("boostgroup outside itemtype"));
                        };
                        self.item_types
                            .last_mut()
                            .unwrap()
                            .boost_groups
                            .push(rest(2));
                        Ctx::ItemType
                    }
                    "itemcolor" | "bloodtype" => {
                        let rec = Record {
                            name: arg(2)?.to_owned(),
                            args: rest(3),
                            line,
                        };
                        if kind == "itemcolor" {
                            self.item_colors.push(rec);
                        } else {
                            self.blood_types.push(rec);
                        }
                        Ctx::None
                    }
                    "namegroup" => {
                        expect_len(self, 3);
                        self.name_groups.push(NameGroup {
                            name: arg(2)?.to_owned(),
                            line,
                            ..Default::default()
                        });
                        Ctx::NameGroup
                    }
                    "itemgroup" => {
                        expect_len(self, 3);
                        self.item_groups.push(ItemGroup {
                            name: arg(2)?.to_owned(),
                            line,
                            ..Default::default()
                        });
                        Ctx::ItemGroup
                    }
                    _ => return Err(bad("unknown `new` kind")),
                }
            }
            ("type", Ctx::Entry) => {
                expect_len(self, 2);
                let e = self.entries.last_mut().unwrap();
                if e.type_name.is_some() {
                    return Err(bad("duplicate `type`"));
                }
                e.type_name = Some(arg(1)?.to_owned());
                ctx
            }
            ("using", Ctx::Entry) => {
                expect_len(self, 2);
                let e = self.entries.last_mut().unwrap();
                if e.using.is_some() {
                    return Err(bad("duplicate `using`"));
                }
                e.using = Some(arg(1)?.to_owned());
                ctx
            }
            ("data", Ctx::Entry) => {
                if n < 3 {
                    return Err(bad("`data` needs key and value"));
                }
                // One shipped line ends in `…""`: the extra `"` is an unterminated empty string.
                expect_len(self, 3);
                let kv = (arg(1)?.to_owned(), arg(2)?.to_owned());
                self.entries.last_mut().unwrap().data.push(kv);
                ctx
            }
            ("treasure", _) if st.bare(1) == Some("itemtypes") => {
                self.treasure_item_types = Some(rest(2));
                Ctx::None
            }
            ("CanMerge", Ctx::Treasure) => {
                expect_len(self, 2);
                self.treasure_tables.last_mut().unwrap().can_merge = int(1)? != 0;
                ctx
            }
            ("StartLevel" | "EndLevel", Ctx::Treasure) => {
                expect_len(self, 2);
                let v = int(1)?;
                let sub = self
                    .treasure_tables
                    .last_mut()
                    .unwrap()
                    .subtables
                    .last_mut()
                    .ok_or_else(|| bad("level outside subtable"))?;
                if head == "StartLevel" {
                    sub.start_level = Some(v);
                } else {
                    sub.end_level = Some(v);
                }
                ctx
            }
            ("object", Ctx::Treasure) if st.bare(1) == Some("category") => {
                let counts = (4..n).map(int).collect::<Result<Vec<_>, _>>()?;
                let obj = TreasureObject {
                    name: arg(2)?.to_owned(),
                    frequency: int(3)?,
                    counts,
                };
                self.treasure_tables
                    .last_mut()
                    .unwrap()
                    .subtables
                    .last_mut()
                    .ok_or_else(|| bad("object outside subtable"))?
                    .objects
                    .push(obj);
                ctx
            }
            ("add", Ctx::Equipment) => {
                let eq = self.equipment.last_mut().unwrap();
                match st.bare(1) {
                    Some("initialweaponset") => eq.initial_weapon_set = Some(arg(2)?.to_owned()),
                    Some("equipmentgroup") => eq.groups.push(Vec::new()),
                    Some("equipment") if st.bare(2) == Some("entry") => {
                        let item = arg(3)?.to_owned();
                        eq.groups
                            .last_mut()
                            .ok_or_else(|| bad("equipment entry outside equipmentgroup"))?
                            .push(item);
                    }
                    _ => return Err(bad("unknown `add` in equipment")),
                }
                ctx
            }
            ("add", Ctx::SpellSet) if st.bare(1) == Some("spell") => {
                expect_len(self, 3);
                let spell = arg(2)?.to_owned();
                self.spell_sets.last_mut().unwrap().spells.push(spell);
                ctx
            }
            ("minlevel", Ctx::ItemType) => {
                expect_len(self, 2);
                self.item_types.last_mut().unwrap().min_level = Some(int(1)?);
                ctx
            }
            ("add", Ctx::NameGroup) if st.bare(1) == Some("name") => {
                self.name_groups.last_mut().unwrap().names.push(rest(2));
                ctx
            }
            ("add", Ctx::ItemGroup) => {
                let group = self.item_groups.last_mut().unwrap();
                match st.bare(1) {
                    Some("levelgroup") => group.level_groups.push(LevelGroup {
                        min_level: int(2)?,
                        max_level: int(3)?,
                        name: arg(4)?.to_owned(),
                        ..Default::default()
                    }),
                    Some(k @ ("rootgroup" | "namegroup")) => {
                        let lg = group
                            .level_groups
                            .last_mut()
                            .ok_or_else(|| bad("group outside levelgroup"))?;
                        if k == "rootgroup" {
                            lg.root_groups.push(rest(2));
                        } else {
                            lg.name_groups.push(rest(2));
                        }
                    }
                    _ => return Err(bad("unknown `add` in itemgroup")),
                }
                ctx
            }
            ("key", _) => {
                expect_len(self, 3);
                self.keys.push((arg(1)?.to_owned(), arg(2)?.to_owned()));
                Ctx::None
            }
            ("modifier", _) if st.bare(1) == Some("type") => {
                expect_len(self, 3);
                self.modifier_lists.push(ModifierList {
                    name: arg(2)?.to_owned(),
                    attributes: Vec::new(),
                });
                Ctx::Modifier
            }
            ("modifier", Ctx::Modifier) => {
                expect_len(self, 3);
                let attr = (arg(1)?.to_owned(), arg(2)?.to_owned());
                self.modifier_lists
                    .last_mut()
                    .unwrap()
                    .attributes
                    .push(attr);
                ctx
            }
            ("valuelist", _) => {
                expect_len(self, 2);
                self.value_lists.push(ValueList {
                    name: arg(1)?.to_owned(),
                    values: Vec::new(),
                });
                Ctx::ValueList
            }
            ("value", Ctx::ValueList) => {
                expect_len(self, 3);
                let v = (arg(1)?.to_owned(), int(2)?);
                self.value_lists.last_mut().unwrap().values.push(v);
                ctx
            }
            _ => return Err(bad("unexpected statement here")),
        })
    }
}
