# Documentation

Start with the [project README](../README.md) for the overview and quick start.

## Project

- [Roadmap](roadmap.md): milestones and what's next.
- [Architecture](architecture.md): crate layout and layering.
- [Platforms](platforms.md): supported install layouts, search paths and environment variables.
- [ADR 0001: provenance and extraction](adr/0001-provenance-and-extraction.md): the rules for what
  may and may not be committed, and how behaviour is derived. All decision records are in
  [adr/](adr/README.md).
- [AGENTS.md](../AGENTS.md): working conventions, for AI agents and humans alike.

## File formats

How the game's files are laid out, written in our own words: [formats/](formats/README.md).

| Doc | Covers |
|---|---|
| [pak.md](formats/pak.md) | `.pak` archives (LSPK v18) |
| [lsf.md](formats/lsf.md) | LSF / LSX / LSJ resource files |
| [gr2.md](formats/gr2.md) | Granny `.GR2` models, BitKnit2 compression, vertex formats |
| [virtual-textures.md](formats/virtual-textures.md) | Granite virtual textures (`.gts` / `.gtp`) |
| [loca.md](formats/loca.md) | Localization strings |
| [stats.md](formats/stats.md) | Stats text files and their inheritance |
| [osiris-story.md](formats/osiris-story.md) | Compiled Osiris quest scripts (`story.div.osi`) |

## Research notes

Investigations into how the original engine works: what was found, how it was verified, and what is
still open.

| Note | Topic |
|---|---|
| [install-survey.md](research/install-survey.md) | What's in a BG3 install |
| [vfs-load-order.md](research/vfs-load-order.md) | How paks overlap and override each other |
| [coordinate-space.md](research/coordinate-space.md) | Why BG3's world is left-handed, and how we convert it |
| [level-structure.md](research/level-structure.md) | Level object → template → visual → mesh + material |
| [materials.md](research/materials.md) | Shader graphs, material instances, texture channel layouts |
| [terrain.md](research/terrain.md) | Terrain patches, holes and layer blending |
| [lighting.md](research/lighting.md) | Placed lights, level lighting and atmosphere |
| [osiris-vm.md](research/osiris-vm.md) | How the quest-script engine evaluates rules, and the replay plan |
| [osiris-runtime-log.md](research/osiris-runtime-log.md) | The engine's own runtime log format, used for verification |

## Glossary

**Osiris**
: Larian's rule-based scripting language and engine for quests and world logic. Scripts are compiled
  into a *story* (`story.div.osi`): a network of databases, rules and goals. The engine reacts to
  game events by updating databases and firing rules. Our VM is `bg3-osiris`.

**Goal**
: A unit of Osiris script with INIT and EXIT sections and its rules. Goals activate each other as
  the story progresses.

**Pak / LSPK**
: The game's archive format. Everything the game loads lives in `.pak` files.

**LSF / LSX / LSJ**
: Larian's resource tree format in binary, XML and JSON flavours. It's used for nearly all game data:
  levels, templates, dialogs, materials, and more.

**Root template**
: A reusable object definition (an item, a character, a piece of scenery). Level objects reference
  root templates and override some of their fields.

**Visual / resource bank**
: Tables mapping resource IDs to assets. A *visual* links a template to a GR2 model and its
  materials; other banks cover materials, textures, effects, and so on.

**GR2 (Granny 3D)**
: The model and animation format the game uses. Files are compressed with *BitKnit2*.

**Virtual textures (Granite)**
: Large textures split into tiles and streamed on demand. Most environment surfaces use them.

**Stats**
: Text files defining spells, items, statuses, characters and passives, with inheritance
  (`using "Parent"`) across modules. They include a small expression language for conditions and
  effects.

**Module ("mod")**
: A layer of game content. The base game is several modules loaded in order (`Shared`, `Gustav`,
  `GustavX`, …), with later ones overriding earlier ones.

**Loca**
: Localization tables mapping text handles (`h…;version`) to strings.

**Trace**
: A recording of the real game's behaviour, e.g. Osiris's runtime log, used to verify our
  reimplementation. Traces are captured locally and never committed.
