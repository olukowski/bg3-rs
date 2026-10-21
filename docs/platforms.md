# Platforms

bg3-rs reads everything from your own BG3 install. The game data (paks) is identical across
platforms, so the parsers, the VFS and the viewer are meant to run on macOS, Windows and Linux.

| Platform | Status |
|---|---|
| macOS (Steam) | Supported and tested. This is the reference build (4.1.1.7631656). |
| Windows (Steam, GOG) | Expected to work. **Untested on a real install so far.** |
| Linux (Steam, native build) | **Tested on a real install:** install found automatically, version read from the binary (same build, 4.1.1.7631656), all paks read, viewer renders the Nautiloid (Vulkan, NVIDIA). |
| Linux (Steam + Proton) | Same files as Windows; expected to work, untested. |

CI builds the workspace, runs clippy and runs the install-free tests on macOS, Ubuntu and Windows.

## Install layouts

`bg3-install` recognises two layouts (`bg3_install::Layout`):

**`MacApp`**: the macOS app bundle. `root` is `Baldur's Gate 3.app`.

```
Baldur's Gate 3.app/Contents/
  Data/*.pak, Data/Localization/*.pak
  MacOS/Baldur's Gate 3            main binary
  Frameworks/libOsiris.dylib       Osiris story VM
  Info.plist                       version: CFBundleShortVersionString
```

**`WindowsStyle`**: a game folder with `Data/` and `bin/`. That covers the Windows build, Linux
via Proton (the same files), and the **native Linux build** that Steam installs on Linux. `root` is
the game folder (Steam: `steamapps/common/Baldurs Gate 3`).

```
<root>/
  Data/*.pak, Data/Localization/*.pak
  bin/bg3.exe                      Windows: Vulkan executable
  bin/bg3_dx11.exe                 Windows: DirectX 11 executable
  bin/osiris.dll                   Windows: Osiris story VM (see below)
  bin/bg3                          native Linux: executable (ELF)
  bin/libOsiris.so                 native Linux: Osiris story VM
```

The version comes from the Windows executable's version resource. Its fixed-size numeric fields
can't hold `4.1.1.7631656` (each part is 16 bits), so we read the UTF-16 `ProductVersion` string,
falling back to `FileVersion`. ELF has no version resource, so for the native Linux build (and as
a fallback on Windows) we scan the binary for its single embedded `a.b.c.build` string. On macOS
that scan returns exactly the `Info.plist` version, which the install test checks. If nothing is
found, the install still opens, with version `unknown` and a warning.

The native Linux build also ships extra paks with non-zero header priority (`Patch8_HotFix9`,
`Patch8_HotFix10`, `PhysXOverlay`), which replace e.g. the compiled stories. See
[vfs-load-order.md](research/vfs-load-order.md).

What's uncertain about the Windows layout:

- `Data` and `bin/bg3.exe` / `bin/bg3_dx11.exe` are well documented by modding tools such as LSLib
  and the BG3 Script Extender, which loads into `bin/`.
- `bin/osiris.dll`: the Script Extender loads Osiris by that name, so we expect it in `bin/`. We
  haven't checked this on a real install. `Install::osiris_library()` returns `None` if the file
  isn't there. Nothing in the workspace needs it yet.
- That `ProductVersion` holds the full `4.1.1.xxxxxxx` string is an assumption. Run
  `bg3x info` to check.

Directory names (`Data`, `Localization`, `bin`, `Contents`, …) are matched case-insensitively,
because Linux filesystems are case-sensitive and copied installs sometimes change case.

## How the install is found

`Install::locate()` tries these, in order:

1. `BG3_INSTALL`, if set. It can point at the `.app` bundle, the Steam folder containing it, a
   Windows-style root, its `bin` folder, or the executable. Only this path is tried.
2. `BG3_APP`, an older alias for `BG3_INSTALL`.
3. For each default Steam root: `<root>/steamapps/common/Baldurs Gate 3`, then the same folder in
   every library listed in `<root>/steamapps/libraryfolders.vdf`. Libraries on other drives are
   common, and the VDF lists them.
4. GOG defaults.

Default Steam roots:

| OS | Steam roots |
|---|---|
| macOS | `~/Library/Application Support/Steam` |
| Windows | registry `HKCU\Software\Valve\Steam\SteamPath` and `HKLM\SOFTWARE\(WOW6432Node\)Valve\Steam\InstallPath` (read with `reg.exe`), `%ProgramFiles(x86)%\Steam`, `%ProgramFiles%\Steam` |
| Linux | `~/.steam/steam`, `~/.steam/root`, `$XDG_DATA_HOME/Steam`, `~/.local/share/Steam`, Flatpak `~/.var/app/com.valvesoftware.Steam/.local/share/Steam`, Snap `~/snap/steam/common/.local/share/Steam` |

GOG locations:

| OS | Path |
|---|---|
| macOS | `/Applications/Baldur's Gate 3.app` |
| Windows | registry `HKLM\SOFTWARE\(WOW6432Node\)GOG.com\Games\1456460669\path` (we believe 1456460669 is BG3's GOG product id; unverified), `C:\GOG Games\Baldur's Gate 3`, `%ProgramFiles(x86)%\GOG Galaxy\Games\Baldur's Gate 3` |
| Linux | none. Use `BG3_INSTALL` for Heroic, Lutris and similar launchers. |

Paths reached through several roots are listed once. For example, `~/.steam/steam` is usually a
symlink to `~/.local/share/Steam`.

To see what was probed and why each candidate was rejected:

```
cargo run --release -p bg3x -- info --candidates
```

If no install is found, the error lists every path tried. When autodetection fails, set
`BG3_INSTALL`:

```sh
# macOS
export BG3_INSTALL="$HOME/Library/Application Support/Steam/steamapps/common/Baldurs Gate 3/Baldur's Gate 3.app"
# Linux (native or Proton)
export BG3_INSTALL="$HOME/.local/share/Steam/steamapps/common/Baldurs Gate 3"
```

```powershell
# Windows
$env:BG3_INSTALL = "D:\SteamLibrary\steamapps\common\Baldurs Gate 3"
```

## Environment variables

| Variable | Meaning |
|---|---|
| `BG3_INSTALL` | Location of the install (see above). |
| `BG3_APP` | Older alias for `BG3_INSTALL`. |
| `BG3_OSIRIS_TRACE` | libOsiris runtime log used by the Osiris replay test. |
| `BG3_VT_LEVEL` | Viewer: first virtual-texture mip level to extract (default 1, 0 = full resolution). |

## macOS-only tooling

These depend on the macOS build, which is our reverse-engineering reference. They are not
expected to work elsewhere:

- `scripts/symbol-index.sh`: builds the symbol index from the unstripped macOS binaries (`nm -arch`,
  `PlistBuddy`).
- `tools/osiris-trace/`: an lldb script that turns on libOsiris' runtime log in the running macOS
  game. Its traces can be replayed on any OS.
- REA / Ghidra work: the reference binary is the macOS `Baldur's Gate 3` and `libOsiris.dylib`
  (4.1.1.7631656). The Windows `bg3.exe` is only read for its version. Data extracted from the
  binary (ADR 0001, tier 2) is pinned to the macOS build.

The native Linux `bin/bg3` is the same build number and is also **not stripped** (about 152k
symbol lines from `nm -C`, against ~765k on macOS). It's a possible second RE reference (x86_64
ELF) and a place to capture traces with gdb or Frida, but nothing uses it yet.

Same build, different packaging: the Linux install ships `Patch8_HotFix9`, `Patch8_HotFix10` and
`PhysXOverlay` as separate higher-priority paks, while the macOS install has none of them, so the
macOS paks presumably include those changes already. Unverified; see
[vfs-load-order.md](research/vfs-load-order.md).

Everything else (`bg3x`, the viewer and all `crates/*`) should work on all three platforms.

## Reporting problems on Windows or Linux

Please open an issue and include:

1. OS and distribution, store (Steam/GOG), and for Linux whether Steam is native, Flatpak or Snap.
2. The output of `cargo run --release -p bg3x -- info --candidates`.
3. If the install was found, the output of
   `cargo test --release -p bg3-install -- --ignored --nocapture`.
4. The real install path and its top-level listing (`dir` / `ls` of the game folder and of `bin`).

Don't attach game files or extracted content (see [ADR 0001](adr/0001-provenance-and-extraction.md)).
