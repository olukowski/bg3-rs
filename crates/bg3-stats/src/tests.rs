//! Fixtures here are self-authored and only mimic the syntax of the shipped files.

use super::*;
use crate::lexer::tokenize;

#[test]
fn tokenizer_basics() {
    let st =
        tokenize("new entry \"A\"\r\n\r\n  data \"K\" \"x, y\"\nobject category \"C\",1,0,2\n");
    assert_eq!(st.len(), 3);
    assert_eq!(st[0].line, 1);
    assert_eq!(st[1].line, 3);
    assert_eq!(st[1].texts().collect::<Vec<_>>(), ["data", "K", "x, y"]);
    assert!(st[1].tokens[1].quoted && !st[1].tokens[0].quoted);
    assert_eq!(
        st[2].texts().collect::<Vec<_>>(),
        ["object", "category", "C", "1", "0", "2"]
    );
}

#[test]
fn tokenizer_strings_end_at_line_end_and_comments() {
    let st = tokenize("// note\ndata \"K\" \"one\r\ntwo\"\ndata \"L\" \"\"\n");
    assert_eq!(st.len(), 3);
    assert_eq!(st[0].word(2), Some("one"));
    assert!(st[0].unterminated);
    assert_eq!(st[1].texts().collect::<Vec<_>>(), ["two", ""]);
    assert!(st[1].unterminated && st[1].tokens[1].quoted);
    assert_eq!(st[2].line, 4);
    assert_eq!(st[2].word(2), Some(""));
    assert!(!st[2].unterminated);
}

#[test]
fn broken_values_like_the_shipped_files() {
    // A value broken across two lines, and a value with a stray trailing quote.
    let f = StatsFile::parse(
        "new entry \"E\"\ntype \"SpellData\"\ndata \"A\" \"0.8\n\"\ndata \"B\" \"x(1)\"\"\ndata \"C\" \"y\"\n",
    )
    .unwrap();
    let e = &f.entries[0];
    assert_eq!(
        e.data,
        [
            ("A".into(), "0.8".into()),
            ("B".into(), "x(1)".into()),
            ("C".into(), "y".into())
        ]
    );
    assert!(!f.warnings.is_empty());
}

const ENTRIES: &str = r#"
new entry "Base"
type "SpellData"
data "Level" "1"
data "Cost" "Action:1"

new entry "Child"
type "SpellData"
using "Base"
data "Level" "2"
data "Extra" "IF(Foo()):Bar()"
"#;

#[test]
fn entries_and_inheritance() {
    let mut stats = Stats::new();
    stats.load_str("ModA", "a.txt", ENTRIES).unwrap();
    stats.finish();
    let child = stats.get("Child").unwrap();
    assert_eq!(child.type_name.as_deref(), Some("SpellData"));
    assert_eq!(child.get("Level"), Some("2"));
    assert_eq!(child.get("Cost"), Some("Action:1"));
    assert_eq!(child.get("Extra"), Some("IF(Foo()):Bar()"));
    assert_eq!(child.own.len(), 2);
    assert_eq!(child.source.line, 7);
    assert!(stats.unresolved().is_empty());
}

#[test]
fn later_module_redefines_and_self_using_patches() {
    let mut stats = Stats::new();
    stats.load_str("ModA", "a.txt", ENTRIES).unwrap();
    // Patch "Base" in place by inheriting from its previous definition.
    stats
        .load_str(
            "ModB",
            "b.txt",
            "new entry \"Base\"\ntype \"SpellData\"\nusing \"Base\"\ndata \"Level\" \"9\"\n",
        )
        .unwrap();
    // Full redefinition without using: previous fields are gone.
    stats
        .load_str(
            "ModB",
            "c.txt",
            "new entry \"Child\"\ntype \"SpellData\"\ndata \"Level\" \"3\"\n",
        )
        .unwrap();
    stats.finish();
    let base = stats.get("Base").unwrap();
    assert_eq!(base.get("Level"), Some("9"));
    assert_eq!(base.get("Cost"), Some("Action:1"));
    let child = stats.get("Child").unwrap();
    assert_eq!(child.get("Cost"), None);
    assert_eq!(stats.redefinitions, 2);
    assert!(stats.unresolved().is_empty());
}

#[test]
fn children_see_the_final_parent_and_type_changes_are_rejected() {
    let mut stats = Stats::new();
    stats.load_str("ModA", "a.txt", ENTRIES).unwrap();
    stats
        .load_str(
            "ModB",
            "b.txt",
            "new entry \"Base\"\ntype \"SpellData\"\ndata \"Cost\" \"Bonus:1\"\n\n\
             new entry \"Child\"\ntype \"StatusData\"\n",
        )
        .unwrap();
    stats.finish();
    let child = stats.get("Child").unwrap();
    // Child (from ModA) inherits from the final Base (from ModB), which dropped "Level".
    assert_eq!(child.get("Cost"), Some("Bonus:1"));
    assert_eq!(child.get("Level"), Some("2"));
    assert_eq!(child.type_name.as_deref(), Some("SpellData"));
    assert_eq!(stats.get("Base").unwrap().previous.len(), 1);
    assert_eq!(stats.rejected.len(), 1);
    assert_eq!(stats.rejected[0].0.name, "Child");
}

#[test]
fn forward_reference_and_unresolved() {
    let mut stats = Stats::new();
    stats
        .load_str(
            "M",
            "a.txt",
            r#"
new entry "C"
using "B"
data "c" "1"
new entry "B"
type "StatusData"
using "A"
data "b" "1"
new entry "A"
type "StatusData"
data "a" "1"
new entry "Orphan"
type "StatusData"
using "Missing"
new entry "Loop"
type "StatusData"
using "Loop"
"#,
        )
        .unwrap();
    stats.finish();
    let c = stats.get("C").unwrap();
    assert_eq!(c.type_name.as_deref(), Some("StatusData"));
    assert_eq!(
        (c.get("a"), c.get("b"), c.get("c")),
        (Some("1"), Some("1"), Some("1"))
    );
    let names: Vec<_> = stats.unresolved().iter().map(|u| u.name.as_str()).collect();
    assert_eq!(names, ["Orphan", "Loop"]);
}

#[test]
fn other_block_kinds() {
    let src = r#"
treasure itemtypes "Common","Rare"
new treasuretable "T_A"
CanMerge 1
new subtable "1,1"
StartLevel "3"
EndLevel "5"
object category "I_Thing",2,0,1
new subtable "-1"
object category "T_Other",1,0,0

new equipment "EQ"
add initialweaponset "Melee"
add equipmentgroup
add equipment entry "W1"
add equipment entry "W2"
add equipmentgroup
add equipment entry "A1"

new spellset "SS"
add spell "S1"
add spell "S2"

new itemtype "Common","ffffff","","1.000",Common,Common,0
minlevel 1
new boostgroup dropChance 1

new itemcolor "Red","ff0000","880000","ffaaaa"
new bloodtype "Ink","a","b",""

new namegroup "NG"
add name "hxyz;1","x"

new itemgroup "IG"
add levelgroup 0,0,"All"
add rootgroup "uuid",""
add namegroup "NG","",""

key "SomeKey","42"

modifier type "SpellData"
modifier "Level","ConstantInt"

valuelist "YesNo"
value "No" : 0
value "Yes" : 1

new ItemCombination "Combo"
data "Type 1" "Object"

new CraftingPreviewData "Prev"
type "ObjectCategories"
data "Icon" "x"
"#;
    let f = StatsFile::parse(src).unwrap();
    assert!(f.warnings.is_empty(), "{:?}", f.warnings);
    assert_eq!(
        f.treasure_item_types.as_deref().unwrap(),
        ["Common", "Rare"]
    );
    let t = &f.treasure_tables[0];
    assert!(t.can_merge);
    assert_eq!(t.subtables.len(), 2);
    assert_eq!(t.subtables[0].start_level, Some(3));
    assert_eq!(t.subtables[0].end_level, Some(5));
    assert_eq!(t.subtables[0].objects[0].counts, [0, 1]);
    assert_eq!(t.subtables[1].drop_count, "-1");
    assert_eq!(f.equipment[0].groups, [vec!["W1", "W2"], vec!["A1"]]);
    assert_eq!(f.equipment[0].initial_weapon_set.as_deref(), Some("Melee"));
    assert_eq!(f.spell_sets[0].spells, ["S1", "S2"]);
    assert_eq!(f.item_types[0].min_level, Some(1));
    assert_eq!(f.item_types[0].boost_groups, [vec!["dropChance", "1"]]);
    assert_eq!(f.item_colors[0].args.len(), 3);
    assert_eq!(f.blood_types[0].args, ["a", "b", ""]);
    assert_eq!(f.name_groups[0].names, [vec!["hxyz;1", "x"]]);
    assert_eq!(f.item_groups[0].level_groups[0].name_groups[0][0], "NG");
    assert_eq!(f.keys, [("SomeKey".into(), "42".into())]);
    assert_eq!(f.modifier_lists[0].attributes[0].1, "ConstantInt");
    assert_eq!(f.value_lists[0].values[1], ("Yes".into(), 1));
    assert_eq!(f.entries.len(), 2);
    assert_eq!(f.entries[1].kind, "CraftingPreviewData");

    let mut stats = Stats::new();
    stats.apply("M", "x.txt", f);
    stats.finish();
    assert!(stats.get_kind("ItemCombination", "Combo").is_some());
    assert!(stats.get("Combo").is_none());
    assert_eq!(stats.key("SomeKey"), Some("42"));
}

#[test]
fn syntax_errors_and_warnings() {
    assert!(StatsFile::parse("data \"K\" \"V\"\n").is_err()); // data outside entry
    assert!(StatsFile::parse("new widget \"X\"\n").is_err());
    assert!(StatsFile::parse("bogus\n").is_err());
    let f = StatsFile::parse("new entry \"E\"\ndata \"K\" \"V\" \"W\"\n").unwrap();
    assert_eq!(f.entries[0].data[0].1, "V");
    assert_eq!(f.warnings.len(), 1);
}

#[test]
fn module_paths() {
    use install::stats_module;
    assert_eq!(
        stats_module("Public/Shared/Stats/Generated/Data/Armor.txt"),
        Some("Shared")
    );
    assert_eq!(
        stats_module("Public/GustavX/Stats/Generated/Equipment.txt"),
        Some("GustavX")
    );
    assert_eq!(
        stats_module("Public/Shared/Stats/Generated/Data/X.lsx"),
        None
    );
    assert_eq!(stats_module("Mods/Shared/meta.lsx"), None);
}

#[test]
#[ignore = "needs BG3 install"]
fn load_all_install_stats() {
    let Ok(install) = bg3_install::Install::locate() else {
        eprintln!("BG3 install not found; skipping");
        return;
    };
    let paks: Vec<_> = install
        .paks()
        .unwrap()
        .iter()
        .map(|p| bg3_pak::Package::open(p).unwrap())
        .collect();
    let mut order = install::BASE_LOAD_ORDER.to_vec();
    order.extend_from_slice(install::HONOUR_MODULES);
    let unlisted = install::unlisted_modules(&paks, &order);
    assert!(
        unlisted.is_empty(),
        "modules not in load order: {unlisted:?}"
    );

    for (label, order) in [
        ("base", install::BASE_LOAD_ORDER.to_vec()),
        ("honour", order),
    ] {
        let files = install::collect_files(&paks, &order).len();
        let stats = install::load(&paks, &order).unwrap();
        eprintln!(
            "[{label}] files={files} entries={} redefinitions={} rejected={} \
             treasure={} equipment={} spellsets={} itemtypes={} itemcolors={} bloodtypes={} \
             namegroups={} itemgroups={} keys={} modifierlists={} valuelists={} warnings={}",
            stats.entries().count(),
            stats.redefinitions,
            stats.rejected.len(),
            stats.treasure_tables.len(),
            stats.equipment.len(),
            stats.spell_sets.len(),
            stats.item_types.len(),
            stats.item_colors.len(),
            stats.blood_types.len(),
            stats.name_groups.len(),
            stats.item_groups.len(),
            stats.keys.len(),
            stats.modifier_lists.len(),
            stats.value_lists.len(),
            stats.warnings.len(),
        );
        for (kind, table) in stats.kinds() {
            eprintln!("  kind {kind}: {}", table.len());
        }
        for (src, w) in &stats.warnings {
            eprintln!("  warning {src}: {}", w.message);
        }
        for (raw, src) in &stats.rejected {
            eprintln!(
                "  rejected type change: {} {:?} ({src})",
                raw.name, raw.type_name
            );
        }
        for u in stats.unresolved() {
            eprintln!(
                "  unresolved: {} {} using {} ({})",
                u.kind, u.name, u.parent, u.source
            );
        }
        // How much the deferred-`using` model matters: children whose (final) parent was
        // redefined after the child's own definition was loaded.
        let files: Vec<String> = install::collect_files(&paks, &order)
            .into_iter()
            .map(|(_, f)| f.path)
            .collect();
        let pos = |s: &Source| (files.iter().position(|p| *p == s.path), s.line);
        let affected = stats
            .entries()
            .filter(|c| {
                c.using
                    .as_deref()
                    .and_then(|u| stats.get(u))
                    .is_some_and(|p| {
                        p.name != c.name
                            && !p.previous.is_empty()
                            && pos(&p.source) > pos(&c.source)
                    })
            })
            .count();
        let self_using = stats
            .entries()
            .filter(|c| c.using.as_deref() == Some(c.name.as_str()))
            .count();
        eprintln!(
            "  children of later-redefined parents: {affected}; self-using entries: {self_using}"
        );
        let untyped = stats.entries().filter(|o| o.type_name.is_none()).count();
        assert_eq!(untyped, 0, "entries without a type");
        assert!(stats.entries().count() > 10_000);
        assert!(stats.unresolved().is_empty());
    }
}
