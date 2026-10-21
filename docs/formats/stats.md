# Stats text files (`Stats/Generated/**.txt`)

Implemented in `crates/bg3-stats`. Verified against the reference build (4.1.1.7631656) by the
ignored test `bg3_stats::tests::load_all_install_stats`: all 157 files of all seven modules parse
with zero errors, every `using` resolves.

## Where they are

`Public/<Module>/Stats/Generated/` in `Shared.pak` (`Shared`, `SharedDev`), `Gustav.pak` (`Gustav`,
`GustavDev`, `Honour`) and `GustavX.pak` (`GustavX`, `HonourX`, `PhotoMode`). No other pak ships
stats text files.

| File | Content | Modules |
|------|---------|---------|
| `Data/*.txt` (Armor, Weapon, Object, Character, Passive, Interrupt, Spell_*, Status_*, CriticalHitTypes) | `new entry` objects | all |
| `Data/Data.txt`, `Data/XPData.txt` | `key` settings | Shared, SharedDev, PhotoMode |
| `Data/ItemColor.txt` | `new itemcolor` | Shared |
| `Data/BloodTypes.txt` | `new bloodtype` | Shared |
| `Data/ItemProgressionNames.txt` | `new namegroup` | Shared |
| `Data/ItemProgressionVisuals.txt` | `new itemgroup` | Shared |
| `TreasureTable.txt` | `treasure itemtypes`, `new treasuretable` | Shared, SharedDev, Gustav, GustavDev, Honour |
| `Equipment.txt` | `new equipment` | Shared, SharedDev, Gustav, GustavDev, GustavX |
| `SpellSet.txt` | `new spellset` | Shared, SharedDev, GustavDev |
| `ItemCombos.txt` | `new ItemCombination`, `new ItemCombinationResult` | Shared, SharedDev, Gustav, GustavDev |
| `ObjectCategoriesItemComboPreviewData.txt` | `new CraftingPreviewData` | SharedDev |
| `ItemTypes.txt` | `new itemtype` (rarities) | Shared |
| `Structure/Modifiers.txt` | `modifier type` + `modifier` (field schema per stats type) | Shared |
| `Structure/Base/ValueLists.txt` | `valuelist` + `value` (enums) | Shared |

The binary also names `TreasureGroups.txt` and `ItemComboProperties.txt`; neither exists in the
reference build.

## Lexical syntax

- One statement per line. Lines end in LF or CRLF (both occur; ~10 files are CRLF). No BOM seen.
- Tokens: `"double-quoted strings"` or bare words, separated by spaces/tabs, `,` or `:`. No escapes.
- **Strings never span lines.** Five lines in `GustavX` have a value broken by a stray line break
  (`data "TargetCeiling" "0.8⏎"`) or an extra quote (`…ReduceCriticalAttackThreshold(1)""`). Reading
  the string up to the end of the line gives clean values (`0.8`, a valid sound event name), whereas
  letting the string span the newline would put `\r\n` into them — so we treat a missing closing
  quote as "ends at end of line" and skip the orphaned tail line (which then has no keyword). Both
  produce a warning, not an error. (Inference from the data, not yet confirmed in the binary.)
- `//` line comments are accepted; none occur in the shipped files.
- Values are kept verbatim (no trimming).

## Statements

All distinct line prefixes in the reference build, all supported:

| Statement | Count | Context / meaning |
|-----------|------:|-------------------|
| `new entry "Name"` | 16,593 | starts a stats object |
| `type "SpellData"` | 16,602 | object type = modifier list name (`Armor`, `Character`, `CriticalHitTypeData`, `InterruptData`, `Object`, `PassiveData`, `SpellData`, `StatusData`, `Weapon`, and `ObjectCategories` for `CraftingPreviewData`) |
| `using "Parent"` | 12,093 | inherit from another object of the same kind |
| `data "Key" "Value"` | 113,188 | field; also used by the ItemCombination kinds |
| `new ItemCombination "N"` / `new ItemCombinationResult "N"` | 251 / 251 | data-only objects (`data "Type 1" …`) |
| `new CraftingPreviewData "N"` | 9 | has `type` and `data` |
| `treasure itemtypes "Common",…` | 5 | rarity column order for `object category` counts |
| `new treasuretable "N"` | 1,565 | |
| `CanMerge 1` | 13 | treasure table flag (meaning in game not verified; we keep it as a flag, redefinition still replaces) |
| `new subtable "1,1"` | 3,831 | drop-count spec kept verbatim (`"-1"`, `"1,2;2,1"`, …) |
| `StartLevel "n"` / `EndLevel "n"` | 171 / 147 | level range of the current subtable |
| `object category "Name",freq,c1..c7` | 6,299 | `Name` is an item (`I_…`) or table (`T_…`) |
| `new equipment "N"` | 738 | |
| `add initialweaponset "Melee"` | 697 | |
| `add equipmentgroup` | 1,855 | opens a group |
| `add equipment entry "Item"` | 1,855 | adds to the last group |
| `new spellset "N"` / `add spell "S"` | 71 / 580 | |
| `new itemtype "Common","ffffff","","1.000",Common,Common,0` | 7 | rarity; args kept verbatim |
| `minlevel 1` / `new boostgroup dropChance 1` | 7 / 7 | belong to the last itemtype |
| `new itemcolor "N","rgb","rgb","rgb"` | 156 | |
| `new bloodtype "N",…` | 11 | 9 positional args (effects / GUIDs) |
| `new namegroup "N"` / `add name "handle;v:…",""` | 53 / 52 | |
| `new itemgroup "N"` / `add levelgroup min,max,"N"` / `add rootgroup "guid",""` / `add namegroup "N","",""` | 51 each | rootgroup/namegroup attach to the last levelgroup |
| `key "Name","Value"` | 345 | global settings |
| `modifier type "T"` / `modifier "Field","ValueType"` | 9 / 628 | schema |
| `valuelist "N"` / `value "Label" : n` | 112 / 1,563 | enums |

Unknown statements and child statements outside their block are errors.

## Module load order

From `Mods/<Module>/meta.lsx` dependencies:

- `Shared`, `SharedDev`: no dependencies, not listed as anyone's dependency — engine-level, always
  first (the strings `Shared` / `SharedDev` appear in the binary next to the other module names).
- `Gustav` → DiceSet_01..07, `MainUI`, `ModBrowser`, `PhotoMode`, `CrossplayUI`.
- `GustavDev` → `Gustav`; `GustavX` → `GustavDev`.
- `Honour` → `GustavDev`; `HonourX` → `GustavX`, `Honour`.

So `bg3_stats::install::BASE_LOAD_ORDER` = `Shared, SharedDev, PhotoMode, Gustav, GustavDev,
GustavX`, and `HONOUR_MODULES` = `Honour, HonourX` appended for Honour mode. Within a module,
`Structure/` first, then files sorted case-insensitively.

Uncertain:
- Whether Honour/HonourX stats are loaded only in Honour mode (strings `HonourMode`,
  `LoadHonourModeWhilePlaying`, `RulesetHonour` suggest a mode switch) and whether they go before or
  after `GustavX` (the dependency graph doesn't order `Honour` vs `GustavX`). Four entry names are
  defined in both `Honour` and `GustavX`, so the choice affects those; we put Honour after GustavX.
- The real ordering algorithm (`ls::ModManager::BuildLoadOrderedModules`) is not yet read.
- Within-module file order doesn't matter for the reference build: no `new entry` name occurs twice
  in one module.

## Redefinition and inheritance

Read from the arm64 disassembly of `RPGStats::InsertDataBuffer` (4.1.1.7631656): the loader first
collects one buffer per object name; when a name is defined again, the new buffer becomes current
and keeps a link to the previous one — but only if a field of both buffers compares equal (we
assume it is the type; otherwise the new definition is dropped). Parsing (`RPGStats::ParseDataBuffer`)
happens after collection. Our model:

1. Later definition of a name (same kind) **replaces** the earlier one completely; the earlier is
   kept only as "previous". A later definition with a different `type` is dropped
   (`Stats::rejected`; none in the reference build).
2. `using "P"` is resolved after everything is loaded, against the **final** definition of `P`
   (recursively). This matters: 120 base-game entries (194 with Honour) inherit from a parent that
   is redefined in a later module, e.g. Shared's `Mephit_Magma` → `_Elemental` redefined in
   `SharedDev`.
3. `using "<own name>"` inherits from the **previous** definition of the same name. This is how
   later modules patch an entry (31 such entries in the base game, 355 with Honour).
4. Fields = parent's effective fields overridden by own `data` lines; `type` is inherited if absent
   (never absent in practice). Unresolvable parents and cycles are reported in `Stats::unresolved`
   (zero in the reference build).

Open: the deferred-resolution reading is from one function's control flow, not yet verified at
runtime. A reference trace (e.g. a `Mephit_Magma` field that differs between the two `_Elemental`
definitions) would settle it. The other block kinds (treasure tables, equipment, …) are assumed to be
replaced wholesale by a redefinition; not verified.

Namespaces: `new entry` objects share one namespace across all types; `ItemCombination`,
`ItemCombinationResult` and `CraftingPreviewData` each have their own.

## Expression-valued fields (kept as raw strings)

`bg3-stats` keeps these as strings; their languages are parsed and evaluated by `bg3-rules`, see
[stats-expressions.md](stats-expressions.md).

The field schema is `Structure/Modifiers.txt` (`modifier "Field","ValueType"`). Expression types:

- `StatsFunctors` — e.g. SpellData `SpellProperties`, `SpellSuccess`, `SpellFail`,
  `OriginSpell*`, `ThrowableSpell*`; StatusData `OnApply*`, `OnRemove*`, `OnTick*`, `TickFunctors`,
  `AuraStatuses`, `OnSuccess`, `OnRollsFailed`; PassiveData `StatsFunctors`, `ToggleOnFunctors`,
  `ToggleOffFunctors`; InterruptData `Properties`, `Success`, `Failure`; Weapon `WeaponFunctors`.
- `Conditions` / `TargetConditions` / `RollConditions` — `TargetConditions`, `AoEConditions`,
  `SpellRoll`, `UseConditions`, `OnApplyConditions`, `RemoveConditions`, `BoostConditions`,
  `EnabledConditions`, InterruptData `Conditions`, `Roll`, …
- `StatsFunctorContext` (`…Context` fields), `Requirements`, `MemorizationRequirements`,
  `SpellRequirement`.
- Typed `FixedString` but expression-shaped: `Boosts`, `DefaultBoosts`, `BoostsOnEquipMainHand/
  OffHand`, `UseCosts`, `DualWieldingUseCosts`, `HitCosts`, `RitualCosts`, `Cost`, `Damage`,
  `VersatileDamage`, `HealValue`, `StableRoll`, `DescriptionParams` (+ `Extra`/`Short`/`TooltipUpcast`
  variants), `TooltipDamageList`, `TooltipConditionalDamage`, `TooltipUseCosts`, `TooltipDamage`.
- Lists (`;`-separated): `Passives*`, `Spells`, `ContainerSpells`, `ProjectileSpells`,
  `PersonalStatusImmunities`, `Flags`, …
- `TranslatedString` fields hold `handle;version` loca references (see `loca.md`).

Relevant parser symbols for later: `eoc::StatsFunctorBase::ParseStatsFunctor`,
`eoc::StatsExpressionManagerBase::ParseStatsExpression`, `RPGStats::AddCondition`,
`ParseRequirements`.

## Counts (reference build, base load order / with Honour)

15,754 / 16,132 entries; 129 / 461 redefinitions; 1,512 treasure tables; 738 equipment; 71 spell
sets; 7 item types; 156 item colours; 11 blood types; 53 name groups; 51 item groups; 343 keys;
9 modifier lists; 112 value lists; 251 + 251 item combinations/results; 9 crafting previews;
10 warnings (the five broken lines above).

## Prior art

LSLib's stats parser and Norbyte's Script Extender documentation describe the same statements. The
grammar coverage, the line-ending rule and the inheritance model above were derived from the
shipped files and the binary.
