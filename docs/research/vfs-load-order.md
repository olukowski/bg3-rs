# VFS / pak load order

Status: **partially known**. Implemented in `crates/bg3-vfs`.

## What we measured (`bg3x dups`, reference build)

- 1,069,712 entries across the 27 primary paks.
- 774 archive paths appear in more than one pak. **772 are byte-identical**, so their order doesn't matter.
- Only 2 differ, both between `Game.pak` and `Gustav.pak`:
  `Mods/ModBrowser/meta.lsx` and `Mods/ModBrowser/pakgenerator.txt`.
- Most duplicate pairs: Assets+GustavX (483), Game+Gustav_Textures (186), Assets+Materials (49),
  Engine+EngineShaders (23), Game+Gustav (22), Game+GustavX (11).

For the unmodded base game, pak precedence therefore only affects the ModBrowser module.

## What the binary tells us

- No pak names are hardcoded (`strings` finds only the `.pak` extension and `VirtualTextures`), so the
  game discovers paks by scanning the data directories.
- Relevant symbols: `ls::ModManager::BuildLoadOrderedModules`, `ls::mod::BuildLoadOrderedModulesList`,
  `ls::ModManager::GetDataPaths`, `ls::FileManager::*`, `ls::PublishingSystemHelper::RefreshUGCPakFiles`.
  *Module* (mod) load order is separate from *pak* mount order and matters for stats, story and
  templates (see the stats docs).

## Current implementation

The VFS mounts in `Install::paks()` order (Data/ alphabetical, then Data/Localization/ alphabetical).
The last mount wins, and lookups are case-insensitive.

## TODO

- RE the directory-scan and mount code (callers of the `.pak` string) to confirm the precedence rule,
  especially for user mods in `~/Documents/Larian Studios/Baldur's Gate 3/Mods`.
- Confirm whether lookups in the original are case-insensitive.
