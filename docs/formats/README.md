# File formats

Our own descriptions of BG3's file formats, written as we implement them. Each write-up covers
the layout, the versions seen in the reference build, quirks, how it was verified, and credits to
prior art (e.g. LSLib) where that informed us.

| Doc | Format | Crate |
|---|---|---|
| [pak.md](pak.md) | `.pak` archives (LSPK v18) | `bg3-pak` |
| [lsf.md](lsf.md) | LSF / LSX / LSJ resources | `bg3-lsf` |
| [gr2.md](gr2.md) | Granny `.GR2` models, BitKnit2 | `bg3-gr2` |
| [virtual-textures.md](virtual-textures.md) | Granite `.gts` / `.gtp` | `bg3-vt` |
| [loca.md](loca.md) | `.loca` localization | `bg3-loca` |
| [stats.md](stats.md) | Stats text files | `bg3-stats` |
| [stats-expressions.md](stats-expressions.md) | Stats expression languages (functors, conditions, formulas, boosts), Thoth `.khn` scripts, rolls | `bg3-rules` |
| [osiris-story.md](osiris-story.md) | Compiled Osiris story | `bg3-story` |
| [dialog.md](dialog.md) | Dialogs (`DialogsBinary`), dialog resources, flags, and how dialogs run | `bg3-dialog` |
| [navcloud.md](navcloud.md) | Navigation: AI grid (`aigrid.data`), nav cloud (`navcloud.meta/.data`), path finding | `bg3-nav` |

New write-ups go in this folder, one file per format, and get a row in this table.
