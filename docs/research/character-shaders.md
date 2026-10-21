# Character shaders: skin, hair, eyes, clothing, body cuts

Status: established from the compiled shaders of the reference build (4.1.1.7631656) and the
data; implemented in `crates/bg3-material/src/character/` (pure parameter logic and reference
math, unit-tested), `crates/bg3-world/src/appearance.rs` (override layering) and
`viewer/src/char_materials.rs` + `viewer/src/shaders/character.wesl` (Bevy). Anything not
established is marked **assumption**.

## Sources and method

- **Graph parameter tables** (`bg3x lsx - Public/Shared/Assets/Materials/Characters/<Graph>.lsf`):
  parameter names, defaults, render state (see `materials.md`).
- **Compiled shaders.** On macOS every graph pass is a function in the shared Metal library
  `Data/Shaders/Metal/Materials.metallib` (the `.bshd` stubs name the entry points, e.g.
  `Shared_Characters_CHAR_Skin_Body_ST_DEF_fragmentMain`). The macOS paks carry no Vulkan
  (SPIR-V) or HLSL for these graphs — only four GustavX "Spectre" graphs ship `_HLSL.shd`/
  `_Vulkan.bshd`. Each library function is LLVM bitcode (AIR):
  `bg3x metallib -f <substr>` lists them, `bg3x metallib --extract <name> -o .cache/…bc`
  extracts one, and `llvm-dis` (rustup component `llvm-tools`) prints it. The AIR metadata
  names every constant-buffer member (`FloatParameter_<name>`, `Vector3Parameter_<name>`) and
  texture (`Texture2DParameter_<name or default-ID>`), so the math reads in terms of the
  graph's own parameter names. The `DEF` pass (deferred G-buffer writer) is the material
  definition; `DEP` holds the depth-prepass alpha test; `vertexMain` the vertex cut.
  Disassembly stays in `.cache/` (never committed); below is our own description.
- **Engine code** (arm64 slice, `llvm-objdump` on named symbols): how presets are applied.
- **Data**: material instances (`MaterialBank`), character visuals, `MaterialPresetBank`,
  `VisualSlots.lsx`, character-creation tables.

### G-buffer (from `Base_VT` and the character graphs' `DEF` passes)

| Target | Contents |
|---|---|
| 0 | view-space normal, `xy·0.28126/(1−z) + 0.5` |
| 1 | linear albedo (saturated); alpha = ambient occlusion |
| 2 | roughness, reflectance ("specular", default 0.5), metalness, flags/255: shading model in bits 0–2 (0 lit, 3 subsurface, 5 hair), +8 receives decals, light channel·16 |
| 3 | subsurface: (diffusion profile index/255, amount, mask, 1); hair: packed strand tangent, scatter |

Tangent space everywhere is x → tangent, y → normal, z → binormal; normal maps decode as
`(A, B, −G)·2 − 1` (as in `materials.md`). Bevy mapping (viewer): albedo → `base_color`,
roughness → `perceptual_roughness`, AO → `diffuse_occlusion`, metalness → `metallic`,
reflectance r → Bevy `reflectance = sqrt(r/2)` (**assumption**: BG3's F0 = 0.08·r as in
Unreal; identical at the default 0.5 = 4 %). The game's lighting (subsurface diffusion
profiles, the Marschner hair BSDF in `Core.metallib` `ClusteredLighting_9`) is not reproduced:
Bevy's standard lit model shades every character surface.

## Which graphs characters use (Nautiloid survey, `nautiloid_characters_resolve`)

Skin: `CHAR_Skin_Body` (bodies, VT), `CHAR_Skin_Head_v3` (heads), `CHAR_Skin_Head_Creature`
(cambion wings). Hair/beards: `CHAR_Hair`. Eyes: `CHAR_Eye_Parallax`, `_FX` (glowing, e.g.
cambions), `_Creature_FX` (mind flayers). Head helpers: `CHAR_LashesShadow`, `CHAR_Tearline`,
`CHAR_Base_SSS_ExtraOcclusion_VT` (mouth), `Helper_Invisible` (scalp proxy). Clothing and
armour: the `CHAR_BASE_*` family (`_VT`, `_VertCut_VT`, `_AlphaTest_(VertCut_)VT`,
`_GM_SSS_VT`, `_SSS_GM_VertCut_DetailMap_VT`, …) and the `*_MSK_*` graphs. Imps use
`CHAR_BASE_SSS_AlphaTested_2S_MSK_VT` for body and head and `Base_GM_VT` for their eyes.

## Skin (`CHAR_Skin_Body`, `CHAR_Skin_Head_v3`)

Skin colour is not painted. It is built per texel from pigment concentrations and the
character's pigment colours:

- **Maps.** `HMVY`: R hemoglobin, G melanin (inverted: 1 = none), B veins, A yellowing. On
  bodies this is the **physical layer (2) of the virtual texture** — not a metal/rough/AO map.
  `CLEA`: R curvature, G eyebrows (head) / body hair (body), B lips (head) / make-up (body),
  A ambient occlusion. On bodies this is the **albedo layer (0) of the VT**, which the shader
  re-encodes with `pow(x, 1/2.2)` after the hardware's sRGB decode. Shading mask
  (`SKIN_Shared_Shading_MSK(A)`; head: unnamed input, body: parameter `Thickness`): head
  R roughness, G freckles, B vitiligo, A non-subsurface; body R freckles, G non-subsurface,
  B vitiligo. `CancelMSK`: R non-skin areas, G melanin removal. `BaseColor_NonSkin`.
  Atlases: `TattooAtlas` (4×4 cells on heads, 4×5 on bodies, cell = `Index`), head
  `MakeUpAtlas` 4×4 (R eye make-up, G "custom" paint).
- **Pigments.**
  - `H = sat(HemoglobinAmount · hmvy.r)`.
  - `m0 = 1 − hmvy.g`; melanin response: linear up to `MelaninDarkThreshold` T, then rising to
    `MelaninDarkMultiplier` D: `mix(T·sat(m0/T), D, sat((m0−T)/(1−T)))`, plus `MelaninAmount`.
  - Vitiligo multiplies melanin by `max(smoothstep(v−0.05, (v+0.05)·ceil(v), shading.vitiligo),
    1 − VitiligoWeight)` with `v = clamp(Vitiligo, 0, 0.95)`; freckles add
    `max((freckleMask + Freckles − 1)·FrecklesWeight / max(Freckles, 0.01), 0)`;
    `MelaninRemovalAmount·cancel.g` removes; clamp ≥ 0.
  - Absorption weights `h = 1 − e^(−H²)`, `m = 1 − e^(−M²)`.
  - `c = HemoglobinColor^h` (per channel), `mix(c, VeinColor, sat(VeinAmount·hmvy.b))`,
    `c ·= MelaninColor^m`, `mix(c, c·YellowingColor, YellowingAmount·hmvy.a)`,
    `c ·= (0.95222, 0.74410, 0.59524)` (a constant tint), then desaturate toward Rec. 709
    luminance by `h·m`: `mix(lum, c, 1 − h·m)`.
- **Layers (head).** Scalp/beard stubble from the head dirt map's alpha `s = 2a − 1`:
  desaturate by `max(s·ScalpDesaturation, −s·BeardDesaturation)` and darken likewise; lips
  `mix(c, Lips_Makeup_Color, clea.b·LipsMakeupIntensity)`; custom paint
  `mix(c, CustomColor, CustomIntensity·curvature·makeupAtlas[CustomIndex].g)`; tattoos (below);
  eye make-up `mix(c, MakeupColor, MakeupIntensity·makeupAtlas[MakeUpIndex].r)`; eyebrows
  `mix(c, clea.g·Eyebrow_Color, clea.g)`; non-skin `mix(c, nonSkin.r·NonSkinColor,
  NonSkin_Weight·cancel.r)`.
- **Layers (body).** Tattoos; body hair `mix(c, curvature·Body_Hair_Color, clea.g)`; make-up
  `mix(c, CLEA_Makeup_Color, clea.b·CLEA_Makeup_Weight)`; non-skin with the RGB of
  `BaseColor_NonSkin`. `isGenitals` scales tattoos/vitiligo/freckle weights by `1 − isGenitals`.
- **Tattoos.** `t = atlas[Index]` (4 channels), `t = mix(t, t·curvature, TattooCurvatureInfluence)`
  `· TattooIntensity` (body: `BodyTattooIntensity·doDrawTattoo`); `c = c·(1 − Σt) +
  Σ tᵢ·TattooColor{,G,B,A}`. Lae'zel's leopard spots are this layer (head atlas
  `Skin_Atlas_Head_UNI_Tattoo_A_MSK1`, body `BodyTattooIndex 1`).
- **Roughness.** Head: `base = sat(1 − curv + curv·shading.r)`, lips `mix((1−lips)·base, base,
  LipsMakeupRoughness)`; body: `sat(TMP_BODY_ROUGH·curv + 1 − curv)`. Then eyebrows/body hair
  → 0.8, tattoos add `TattooRoughnessOffset`, make-up → `MakeupRoughness`
  (`CLEA_Makeup_Roughness`). Metalness from non-skin/tattoo/make-up metalness parameters
  (0 by default). Reflectance 0.5. AO = `clea.a`.
- **Subsurface**: shading model 3; amount = 1 except non-skin, eyebrows, flagged texels (not
  reproduced in Bevy).
- **Not reproduced** (all 0 / off on the characters checked): wounds and grime state (`Blood`,
  `Dirt`, `Bruises`, `Sweat`, `Grime`: the masks are zero when the amounts are 0, which is
  every instance seen except some scenery thralls' visuals), scars, age normals and wrinkle
  maps, the bloodline (`BM_Adjust`) layer, illithid veins, the detail normal map
  (`DetailNormalMap` × 0.5 where `CancelMSK` allows), `CHAR_Skin_Head_Creature`.

## Hair (`CHAR_Hair`)

Render state: shading model 5 (hair), opaque, two-sided. The `DEF` pass never discards; the
depth prepass keeps a pixel when `alpha·(1 − root·SoftenTipsAlpha)·sat(5|N·V|)·
DontTouchMe_Beard_Alpha ≥ 1/3` (with TAA: a per-frame dither with probability alpha).
Inputs: `ID_Depth_Root_Alpha_MSKA` (R strand ID, G depth, B root gradient, A alpha), the shared
`HAIR_Splatter_MSK` noise, the hue LUT `HAIR_ColorVar_LUT_MSK`, vertex colour (R graying seed,
B highlight mask, A root occlusion). Colours come from the character (`03Hair` presets);
beards (`DontTouchMe_isBeard`) use the `Beard_*` set.

- Frizz: sample the mask at `uv` and at `uv + HairFrizz·noise` and keep, per channel, the
  value further from 0.5 (ID) / 0.
- Graying: `g = 0.8·Graying_Intensity`, `f = fract(v)·(1 − 4·max(g − 0.75, 0))`,
  `seed = 0.4·fract(vc.r + Graying_Seed)`, `gray = sat(ceil(g)·(g − 1 + |f − (2·ID − 0.5 + seed)|))`.
- Base `mix(Hair_Color, Hair_Graying_Color, gray)`, highlight
  `mix(·, Highlight_Color, Highlight_Intensity·sat(sat(vc.b − falloff)·1.5/(1 − falloff)))`.
- Roots: `r = smoothstep` of `(root + soft/2 − RootTransitionMidPoint)/soft`, depth term
  `min(sat(4v), vc.a, r)`; roots go to half brightness with 1.2× saturation.
- `× mix(1, 1.8·ID + 0.2, IDContrast) × mix(1, 1.5·depth^2.2 + 0.5, ColorDepthContrast)`,
  hue LUT `× max(0, (lut(ID) − 0.5)·HueShiftColorWeight·gray + 1)`.
- Roughness `Roughness·mix(1, 1.5·(1 − depth) + 0.5, RoughnessContrast)`; AO 1; normal = vertex
  normal (no normal map); strand tangent = binormal tilted by `(1 − 2·ID)·StrandDirectionVariation`.
- Not reproduced: the Marschner lighting, dirt/wetness/blood, `BaseColorVar` (BC7 mask,
  weight 0 by default), pixel-depth offset, vertex wind.

## Eyes and head helpers

- **`CHAR_Eye_Parallax`** (opaque, lit, plus an emissive pass). Masks: `RegionMask` R AO, G 0 on
  the iris, B 0 on the pupil, A parallax height; `ColorsMask` R redness, G iris brightness,
  B limbal ring, A secondary-colour pattern; blindness mask (R noise, G region).
  One-step parallax `uv' = uv − 0.045·(1 + h)·(V·T, −V·B)` (tangent-space view vector).
  Vertex colour R selects the `_L` parameter set (with `Heterochromia`) and disables `Ersatz`.
  Iris `mix(IrisColour, IrisSecondaryColour, sat(SecondaryColourIntensity·A))` (blind/ersatz
  tints are fixed colours) `× G × (1 − B·IrisEdgeStrength)`; sclera
  `mix(BloodColour, ScleraColour, sat(R^Redness))·G·B`; albedo `mix(iris, sclera, G·B)`;
  roughness `mix(0.9, 0, G·B)` (wet sclera); AO = region R². Emissive catchlight: the reflected
  view direction in object space looks up the lat-long `ATM_HDRi_EyesCatchlight`, `rgb/3`.
  `_FX` adds an animated glow (not reproduced); `_Creature_FX` blends the iris by G only.
- **`CHAR_LashesShadow`**: alpha-blended, two-sided. Colour `Eyelashes_Color·R·B·GradientIntensity`;
  alpha `ShadowAlphaIntensity·G·(1 − R) + (G·R)^(2·(1 − LashesAlphaIntensity))`.
- **`CHAR_Tearline`**: blend state 1 (factors not established); only adds reflectance
  (`Reflectance·Opacity·Alpha.R`) on a black albedo. The viewer doesn't draw it.
- **Mouth** (`CHAR_Base_SSS_ExtraOcclusion_VT`): an ordinary subsurface VT material plus a
  teeth discard on vertex colour R vs `Teeth_Broken`; equal to the generic path by default.
- Imp eyes are `Base_GM_VT` (glow map × `GlowColour` × 20): the generic glow path.

## Clothing and armour colours

- **12-colour palette** (`CHAR_BASE_VT` family, texture `MSKColor`): each of `Cloth_*`,
  `Leather_*`, `Metal_*` (Primary/Secondary/Tertiary), `Accent_Color`, `Custom_1/2` sits at a
  fixed point of the RGB cube; weight `4·sat(1 − |m − P|/√3 − 0.75)`; albedo = VT albedo ×
  Σ weightᵢ·colourᵢ (no normalisation). Over 98 % of mask texels lie exactly on the points.
  The default mask is `Engine_Default_Custom_2_MSKCloth` with `Custom_2 = 1`: untinted.
- **3-colour mask** (`*_MSK_*` graphs, texture `MSKcloth`): `tinted = albedo·(r·Color_01 +
  g·Color_02 + b·Color_03)`, `mix(albedo, tinted, sat(r + g + b))`. Default mask black.
- Alpha test only in the depth prepass (`albedo.a < 0.5` discarded); two-sided graphs flip the
  normal on back faces. Imps: their mask stays `Engine_Black`, so their red skin and yellow
  belly are entirely the VT albedo.

## Body cut masks (`VertCut`)

The vertex shaders of `CHAR_Skin_Body`, `CHAR_Skin_Head_Creature` and every `*_VertCut_*`
graph read the vertex colour's green channel `g`, point-sample a 16×16 texture
`VertexColor_MSK` at `(frac(16g), floor(16g)/16)` — texel `(byte & 15, byte >> 4)` — and move
the vertex to the object origin when the sample's green ≥ 0.01, collapsing its triangles.
The `_VertCut_` fragment shaders are identical to the plain ones. The engine replaces the
mask texture per character (`ls::VisualSetSlots::UpdateVertexColorMaskTexture`); the bytes are
the `MapValue`s of the regions the worn visuals list in `VertexColorMaskSlots`
(`VisualSlots.lsx`, `NakedBodySystem`: Torso 50, Thighs 185, …; see `characters.md`).

Viewer: for parts whose visual has `SupportsVertexColorMask`, triangles whose three vertices
are cut by the character's `body_masks` are removed on the CPU (exactly the collapsed ones).
On the Nautiloid no triangle is only partly cut. **Assumptions:** all of the character's
supporting visuals share one mask (the union), and the engine marks texels with green ≥ 0.01.

## Where the parameter values come from

Per material parameter, lowest first:

1. graph default; 2. material instance (`Enabled` values) — `materials.md`;
3. the `MaterialPresets` of the character visual (and of a worn item's template), each
   `MaterialPresetBank` resource's `Presets` list (only `Enabled` entries), in list order;
4. the visual's own `MaterialOverrides` parameters (`Enabled` only).

3 → 4 is the order of `ls::VisualSetSlots::InitVisualSlots`, which applies every preset with
`ApplyMaterialPresetsResources` and then the own list with `ApplyCustomPresets`
(4.1.1.7631656, disassembly). Each entry goes through
`ls::AppliedMaterial::TryOverride{Scalar,Vector2,Vector3,Vector4}Parameter`: by name, only if
the material declares a parameter of that type; a Vector3 entry also sets a Vector4 parameter
(w = 1), a Vector4 entry a Vector3 one. So overrides apply by name to every material of the
character — a skin preset sets the eyelash card's `LashesAlphaIntensity`, a hair preset the
skin's `Eyebrow_Color`. Not established: what `ForcePresetValues` changes, the per-material
`Materials` and mesh-level `RealMaterialOverrides` maps of a character visual (not parsed),
character-creation material settings (graying, highlights, make-up sliders) and the passive
material swaps (`CharacterCreationMaterialOverrides.lsx`). Viewer **assumption**: a worn item's
own overrides apply to its meshes before the character's (Lae'zel's visual carries the
`02 Colour` preset of her half-plate).

Character-creation colour tables (`CharacterCreation{Skin,Hair,Eye}Colors.lsx`) map a choice
to a `MaterialPresetUUID`; the race lists in `Races.lsx`. The combat test's avatar shows its
template's own presets (`SKIN_UND_Dusk_6`, `HAIR_Color_White_1`, `Underdark Black A` eyes):
bg3-sim's character creation does not model appearance, and the reference trace has none.
The reference screenshot's lighter skin and red-violet eyes are a player's choice.

## Concrete check: Lae'zel

Visual `ORIGIN_Laezel` (Gustav `[PAK]_CharacterVisuals`), forced presets from
`Origin Presets/[PAK]_Lae'zel`: skin MelaninAmount 0.5, MelaninColor (0.041, 0.058, 0.004),
HemoglobinAmount 0.8, TattooColor (0.019, 0.006, 0.002); hair Hair_Color (0.118, 0.036, 0.015);
eyes Iris (1, 0.523, 0.128). Own: NonSkinColor, TattooIntensity (1, 0, 0, 0).

## Viewer implementation

`CharKind::classify` routes `CHAR_Skin_Body`/`CHAR_Skin_Head_v3`, the eye graphs,
`CHAR_Hair`, `CHAR_LashesShadow`, `CHAR_Tearline` (hidden) and `CHAR_*` graphs with
`MSKColor`/`MSKcloth` masks to `CharacterMaterial` (`ExtendedMaterial<StandardMaterial,
CharacterExtension>`, one WESL with a mode per kind); everything else keeps the generic
`StandardMaterial`. Templates are built per material ID; each character part gets its own
material with its overrides applied. Body VT layer 2 is loaded unconverted
(`Usage::VirtualLayerRaw`). Unnamed graph inputs are found by their default texture's
resource name (`SKIN_Shared_Shading_MSK…`, `EYES_Blindness…`, `HAIR_Splatter…`, …).
`BG3_STANDARD_CHARACTERS=1` switches back to the generic path for comparisons;
`BG3_CHAR_DEBUG=1` logs each character material and vertex cut.

Related fix: bones a mesh brings from its own rig (hair sockets, face bones) are now placed
by their inverse bind relative to our parent (`rest_world · IBM = I`), instead of the foreign
rig's local rest. Lae'zel's hair rig has `Head_M` 9 cm above her body rig's, which buried her
top knot inside her head. **Assumption:** the engine keeps attached meshes in their authored
bind pose.

The deep gnome avatar has no hair because his template visual has no `Hair` slot; in the
game the hairstyle is a character-creation choice (`CharacterCreationAppearanceVisuals.lsx`)
that bg3-sim's character creation doesn't make.

## Verification (2026-10-21)

Unit tests (synthetic inputs): melanin response, absorption, pigment hue/vein/vitiligo,
head layers and roughness, tattoos, genital weights (`skin.rs`); palette weights and eye
uniforms (`kinds.rs`); hair strand colour, beard colours, graying (`hair.rs`); typed overrides
(`params.rs`); vertex-cut texel mapping and triangle removal (`vertex_cut.rs`); preset layering
and cycles (`appearance.rs`).

Screenshots (`.cache/shots/`, compared with the reference captures in `.cache/reference/`):

- `cs-before.png` vs `cs-1.png` (`--play --combat-test --shot … --shot-after 3`): bodies were
  bright red (the skin graph's default texture through `StandardMaterial`); now Lae'zel is
  yellow-ochre and the gnome grey-blue with a blue-shouldered chain shirt.
- `cs-laezel-face.png` (`--no-follow --camera=-46.3,4.5,-401.8 --look-at=-45.38,4.4,-402.23`):
  ochre skin with dark leopard spots on the scalp and face, dark eye make-up, gold irises,
  brown hair in a top knot with rings — matching `laezel.png`. Her hair reads somewhat
  redder than the reference (no hair BSDF/subsurface in Bevy).
- `cs-avatar.png` (`--camera=-47.9,4.0,-402.2 --look-at=-46.58,3.5,-402.83`) vs
  `cs-avatar-std.png` (same, `BG3_STANDARD_CHARACTERS=1`): spotted legs on Lae'zel, blue chain-
  shirt cloth on the gnome; with the generic path the arms/legs are red and the shirt white.
  The flat grey patch at Lae'zel's skirt looks the same in both, so it is not caused by the
  character shaders (not investigated further).
- `cs-avatar-front.png` (`--combat-autoplay --camera=-49,4.3,-405.8 --look-at=-46,3.6,-402.5`):
  the gnome's skin is darker grey-blue than `avatar-deepgnome-cleric.png` (template preset
  `UND_Dusk_6` vs the player's lighter choice) and he is bald (see above); chain shirt with
  blue cloth; the imp beside him is darker and browner than `imp.png` — its colour is the
  unchanged VT albedo, so the difference is lighting (no subsurface, no lava light).

No runtime capture of the original's G-buffer was made; the shaders were read, not traced.

## Open

- Subsurface (diffusion profiles), the hair BSDF, eye glow (`_FX`), catchlight intensity
  calibration (emissive units), detail normals, scars/age/wrinkles, wounds/dirt state.
- `ForcePresetValues`, `RealMaterialOverrides`/per-material overrides, CC material settings,
  CC hairstyle, passive material swaps.
- `CHAR_Skin_Head_Creature` (cambion wings): skin model in a non-VT variant, not ported.
