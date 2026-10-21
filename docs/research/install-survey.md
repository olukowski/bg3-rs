# Install survey — reference build

Surveyed 2026-10-21 on macOS (Apple Silicon).

## Location & version

- Real install: `~/Library/Application Support/Steam/steamapps/common/Baldurs Gate 3/Baldur's Gate 3.app`
  (~146 GB). `~/Applications/Baldur's Gate 3.app` is a 100 KB shim that runs `open steam://run/1086940`.
- `CFBundleShortVersionString`: **4.1.1.7631656**. Steam app id 1086940.

## Binaries

| File | Notes |
|------|-------|
| `Contents/MacOS/Baldur's Gate 3` | 479 MB universal (x86_64 + arm64). **Not stripped**: ~765k defined symbols, Itanium-mangled C++, demangle with `c++filt`. |
| `Contents/Frameworks/libOsiris.dylib` | 1.8 MB, ~2k symbols (`COsiris::*`, `osi::*`). The story/scripting VM. |
| `libBink2Mac.dylib` | Bink 2 video (cutscene video in `Gustav_Video.pak`). |
| PlayFab*, HttpClient, discord, steam_api | Platform services — out of scope. |

Code signature: hardened runtime **not** enabled (`flags=0x0`) → attaching lldb/Frida for tier-3
observation should be possible.

### Symbol namespaces (arm64 slice, rough counts by substring)

| Prefix | ~Count | Meaning (working assumption) |
|--------|--------|------------------------------|
| `ls::` | 255k | Larian engine core ("LSTools"): ECS, resources, render, IO |
| `eoc::` | 100k | Shared gameplay/rules components ("Engine of Creation"?) |
| `esv::` | 68k | Server-side simulation (authoritative game logic, AI, Osiris bindings) |
| `ecl::` | 42k | Client-side presentation |
| `Noesis` | 82k | NoesisGUI (XAML UI middleware) |
| `stats::` | 3.5k | Stats system |
| `Granny` | 2.5k | Granny 3D (GR2 models/animation) |
| `navcloud` | 1.7k | Pathfinding |
| `AK::`/Wwise | 1.7k | Audio middleware |
| `Osiris*`/`osi::` | ~3k | Story scripting glue (`OsirisCharacterFunctions.cpp`, …) |

Gameplay is an ECS: e.g. `ecs::WorldView<eoc::HealthComponent const, esv::Character, …>` — maps
naturally onto Bevy. Single-player still runs the client/server split (`esv`/`ecl`).

## Data paks

All `LSPK` magic, header version **18**. In `Contents/Data/`:

Engine, EngineShaders, Shared, SharedSounds, SharedSoundBanks, Game, GamePlatform, Gustav, GustavX,
Gustav_NavCloud, Gustav_Textures, Gustav_Video, Assets, Effects, Icons, Materials, Models,
Textures(+_1.._3), VirtualTextures(+_1.._19), DiceSet01/02/03/06.

`Contents/Data/Localization/`: English, English_Animations, Voice, VoiceMeta, plus per-language paks.

Shaders: `Contents/Data/Shaders/Metal/{Core,Materials}.metallib`.

## Saves (useful as test oracles later)

`~/Library/Application Support/Steam/userdata/<id>/1086940/remote/_SAVE_Public/Savegames/Story/`
— several Act 1 saves (Wilderness, Emerald Grove).

## Prior art (format knowledge, credit where used)

- Norbyte's **LSLib** (pak/LSF/LSX/GR2/Osiris story formats).
- Norbyte's **BG3 Script Extender** (ECS component layouts, Osiris/stats internals).
- BG3 modding wiki (bg3.wiki, modding docs).

## To verify

- Pak compression variants used in v18 (LZ4 / Zstd per-file flags), multi-part paks.
- Where the compiled story (`story.div.osi` or equivalent) lives.
- Whether GR2 files use Granny's proprietary compression (affects mesh loading).
- Virtual texture format (Granite `.gts`/`.gtp`) details.
