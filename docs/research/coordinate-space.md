# Coordinate space

Status: **BG3 world and model space is left-handed, Y-up.** The viewer mirrors Z when converting to
Bevy's right-handed Y-up space (`viewer/src/space.rs`).

## Evidence

The human male body mesh
(`Generated/Public/Shared/Assets/Characters/_Models/Humans/_Male/Resources/HUM_M_NKD_Body_A.GR2`),
measured with `bg3x gr2 - <path> --bones`, which prints the mean position of the vertices each bone
dominates:

- Toes at z ≈ −0.147, ankles at z ≈ −0.012, so the character **faces −Z**.
- Every `*_L` bone sits at **+X** (e.g. `Wrist_L` x = +0.71, `Toes_L` x = +0.124) and `*_R` at −X.

In a right-handed Y-up system, a figure facing −Z has its left side at −X. Left at +X while facing
−Z only works in a left-handed system. Level placements and meshes share the same space, so
the whole world is affected. Without mirroring, the viewer shows every level mirrored.

Not yet confirmed by RE: the engine's projection/view matrix construction (`ls::CameraSystem::*`,
`GetViewProjectionMatrix`). A side-by-side with an in-game screenshot would also settle it.

## Conversion (BG3 → Bevy)

| Quantity | Conversion |
|----------|------------|
| Position, direction, normal | `(x, y, z) → (x, y, −z)` |
| Rotation quaternion `(x, y, z, w)` | `→ (−x, −y, z, w)` |
| Tangent `(x, y, z, w)` | `→ (x, y, −z, −w)` (the mirror flips the bitangent) |
| Triangle winding | reversed (swap two indices per triangle) |
| Scale | unchanged |

The viewer's `--camera` / `--look-at` take BG3 coordinates.
