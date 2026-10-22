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
| 2 | roughness, reflectance ("specular", default 0.5), metalness (hair: `HairBacklit`), flags/255: shading model in bits 0–2 (0 lit, 3 subsurface, 5 hair), +8 receives decals, light channel in bits 4–6 |
| 3 | subsurface: (diffusion profile slot/255, scatter amount, thickness map, transmission on); hair: octahedral strand tangent (2 × 12 bits in RGB), scatter |

Tangent space everywhere is x → tangent, y → normal, z → binormal; normal maps decode as
`(A, B, −G)·2 − 1` (as in `materials.md`). Bevy mapping (viewer): albedo → `base_color`,
roughness → `perceptual_roughness`, AO → `diffuse_occlusion`, metalness → `metallic`,
reflectance r → Bevy `reflectance = r` (the lighting pass computes F0 = 0.16·r², as Bevy
does). Character surfaces are lit with the game's own light loop; see "Lighting" below.

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
- **Subsurface** (target 3): shading model 3; profile slot from the engine; scatter amount
  head `sat(max(MakeupThicknessExclusion, 1 − makeup)·(1 − cancel.R)·(1 − ⌊shading.A⌋)·(1 −
  clea.G)·(1 − metal))`, body `sat((1 − ⌊shading.G⌋)·(1 − metal))`; thickness map head
  `sat(max(metal, clea.G + cancel.R + ⌊shading.A⌋))` (body: not fully traced, **assumption**:
  the same with the body's flag); transmission on (constant 1). Lighting: see "Lighting".
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
- G-buffer for the lighting: target 2 blue = `sat(HairBacklit)` (0.4), target 3 = the strand
  tangent (octahedral, 12 + 12 bits) and scatter `clamp(clamp(Scatter + gray·(2·sat(ΣGraying
  colour) − 1), 0, 1)·(1 − 0.3·wet), 0.01, 1)` (Scatter 0.3); the tangent is `normalize(B +
  tilt·N)` with `tilt = (1 − 2·ID)·StrandDirectionVariation (0.35) + noise·DreadNoiseNormal`.
- Not reproduced: dirt/wetness/blood, `BaseColorVar` (BC7 mask,
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
  roughness `mix(0.9, 0, G·B)` (wet sclera); AO = region R². Shading model 0, reflectance 0.5,
  metalness 0 (no cornea or wet-specular parameter). Normal: the iris normal map, flattened
  toward the mesh normal by `blindness·mask.G`, plus `Sclera_Normal_Weight·(x, 0, z)` of the
  graph's fixed `EYES_Sclera_NM` (fine bumps; weight 1 by default and fixed 1 in the FX graphs).
  Emissive pass (catchlight): the tangent-space view vector reflected about the sclera detail
  normal only (so essentially the eye mesh's normal), taken to the mesh's **unskinned** local
  frame, looks up the lat-long `ATM_HDRi_EyesCatchlight` (BC6H): `rgb/3`, over the whole eye,
  with no mask, parameter or exposure. `_FX` and `_Creature_FX` blend iris and sclera by G only,
  with slightly different blind-iris tints, and `_Creature_FX` scales the parallax by
  `ParallaxWeight` (0.1 on mind flayers). Their emissive adds a glow:
  `n1 = noise(uv + time·FlameSpeed)`, `uv₂ = uv + FlameDirection·n1·region.G·region.B`,
  `mask = |1 − (colors.G + colors.B)(uv₂)|^FxMasking · smoothstep(sat(1 − region.G(uv₂)))`,
  `glow = mask·GlowColour·noise(uv + (n0, n0 + time·AnimationSpeed))·GlowBrightness·(1 − blind)`
  plus `GlowColour·(1 − region.B)·GlowBrightnessPupil` (noise = blindness mask R, `n0 =
  noise(uv)`; `_L` set by heterochromia; ersatz removes it). Defaults: FxMasking 6, GlowBrightness
  30, speeds 0 (static). Mind flayers: GlowColour (1, 0.19, 0).
  bg3: `Shared_Characters_CHAR_Eye_Parallax{,_FX,_Creature_FX}_ST_{DEF,EMI}_fragmentMain`.
- **`CHAR_LashesShadow`**: alpha-blended, two-sided. Colour `Eyelashes_Color·R·B·GradientIntensity`;
  alpha `ShadowAlphaIntensity·G·(1 − R) + (G·R)^(2·(1 − LashesAlphaIntensity))`.
- **`CHAR_Tearline`**: blend state 1 (factors not established); only adds reflectance
  (`Reflectance·Opacity·Alpha.R`) on a black albedo. The viewer doesn't draw it.
- **Mouth** (`CHAR_Base_SSS_ExtraOcclusion_VT`): an ordinary subsurface VT material plus a
  teeth discard on vertex colour R vs `Teeth_Broken`; equal to the generic path by default.
- Imp eyes are `Base_GM_VT`: emissive `Glowmap.R × GlowColour × GlowMultiplier` (imp: 20,
  (0.851, 0.101, 0.003)), unexposed; the RGB glow map only with `UseGMColor`. The viewer draws
  them through the generic level glow path, which differs (clamped strength × 150, full RGB;
  not changed here).

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

## Lighting

Source: the deferred lighting compute kernels `ClusteredLighting_N` in `Core.metallib` (one
variant per mix of shading models in a tile: `_0` lit only, `_2` subsurface only, `_9` hair,
`_4` everything; helpers `DoClusteredLight`, `DoClusteredLight_Subsurface`, `Specular_Hair`,
`CalculateHairParameters`), the screen-space scattering passes `PPSSSS_H`/`PPSSSS_V`, and the
engine's profile and kernel set-up (`ls::DiffusionProfile::DiffusionProfile`,
`ls::DiffusionProfileResource::Visit`, `ls::CascadedShadowMaskStage::UpdateDiffusionProfileBuffer`,
`ls::SubStage_SSSS::ExecuteWTKernel`, `ls::CullStage::ExecuteWTKernel`,
`ls::RenderableObject::SetupEngineParameters`), all 4.1.1.7631656. Read, not traced at run time.

### Common BRDF (model 0, and the base of model 3)

Perceptual roughness `r0 = max(0.09, GB2.r)`, then geometric specular anti-aliasing from the
normal's screen-space change: `r = sqrt(sat(r0² + min(|∂N/∂x|² + |∂N/∂y|², 0.18)))`, `a = r²`.
Diffuse: renormalised Disney/Burley (Frostbite form), `fd90 = 0.5·r + 2·LoH²·r`,
`Fd = (1 + (fd90 − 1)(1 − NoL)⁵)(1 + (fd90 − 1)(1 − NoV)⁵)·lerp(1, 1/1.51, r)/π`.
Specular: GGX `D`, Schlick `F` on LoH with `F0 = lerp(0.16·refl², albedo, metal)`, and a
height-correlated Smith visibility with the remapped `k² = ((r + 1)/2)²` in place of α².
The sun's specular uses the point of its disc (≈0.26°) closest to the reflection vector.
Lights only reach pixels whose light channel (GB2 bits 4–6) is in their channel mask.

### Skin and other subsurface surfaces (model 3)

- **Diffuse** as above, but without the albedo (`(1 − metal)` only) when screen-space
  scattering is on: the lighting writes irradiance, the scattering pass multiplies the albedo
  in afterwards ("post-scatter texturing").
- **Dual-lobe specular** from the profile's `DualSpecularRoughnessA/B` and `DualSpecularMix`
  (0.75 / 1.3 / 0.85 for every character profile): `rA = max(0.09, sat(r0·A))`, `rB` likewise,
  `D = lerp(D(lerp(rA², r², metal)), D(lerp(rB², r², metal)), Mix)`; F and the visibility stay
  single.
- **Transmission** (added to the diffuse, when target 3's alpha is set): wrap
  `Wt = sat((cos 75° − N·L)/(1 + cos 75°)²)` with no N·L factor, thickness
  `t = ThicknessRemapMin + (Max − Min)·thickness map` (mm), shape
  `S = 1/max(0.0966094·ScatterColor·ScatterDistance·amount, 1e-4)` per channel,
  `T = Tint/4·(e^{−tS} + 3e^{−tS/3})`, `diffuse += light·T·diffuseColour·Wt/π`. Thin objects
  multiply by the light's shadow (sampled also on back faces); thick objects
  (`ThickObjectTransmission`, every character profile) instead use
  `t = max(t, 1000·depth through the occluder in the light's shadow map)` and no shadow factor;
  lights without shadows give a depth of 0. The 0.0966 factor is unexplained.
- **Screen-space scattering** (`PPSSSS_H` then `PPSSSS_V`, on subsurface pixels only): 25 taps
  along x then y, step `ScatterDistance·amount·0.001·proj / (2·depth) / 3` (so the outermost
  tap is `ScatterDistance` mm away), a tap is used only if the four texels around it are all
  model 3, and it falls back to the centre colour across depth steps:
  `mix(tap, centre, sat(1000·|Δdepth| / (ScatterDistance·amount)))`. V multiplies by the albedo.
- **Kernel** (per profile, built once on the CPU): Jimenez's separable kernel with the
  `ScatterColor` as per-channel falloff and strength 1: offsets `3·t·|t|` for 25 evenly spaced
  `t` in [−1, 1]; weight = the offset's span (half the distance to each neighbour) × a sum of
  five Gaussians of `o/(falloff + 0.001)` (variances 0.0484, 0.187, 0.567, 1.99, 7.41;
  weights 0.100, 0.118, 0.113, 0.358, 0.078; π written as 3.14); centre tap moved first;
  each channel normalised to 1. A falloff of 0 (skin blue) makes that channel a pure centre tap.
- **Profiles** (`DiffusionProfileBank`): the graph's `DiffusionProfileUUID`, replaced by the
  material instance's when that is non-empty. Skin graphs: `CHAR_SHARED_Skin_SS`
  (ScatterColor (1, 0.128, 0), 8 mm, tint 1, thickness 0–30, thick); imps
  (`CHAR_BASE_SSS_AlphaTested_2S_MSK_VT`): `CHAR_SHARED_Creatures_SS` ((1, 0.114, 0.114),
  15 mm, tint (1, 0.797, 0.797)). Slot 0 is always `Engine_Default_DiffusionProfile`
  (distance 0: no blur); visible subsurface profiles take slots 1–15 per frame; a material whose
  profile has no slot uses 0. Missing attributes default to distance 20, remap 0–20, tint 1.
- Ambient is the same as for model 0 (with the albedo-less diffuse colour); no dual lobe and
  no transmission in it.

### Hair (model 5)

Unreal Engine 4's `HairShading` (Marschner R/TT/TRT with Karis' approximations, area 0) plus
its Kajiya-Kay scatter term, with the strand tangent T from target 3:
`sinθL = T·L`, `sinθV = T·V`, `cosθd = cos(½|asin sinθV − asin sinθL|)`, `cosφ` between L and V
projected off T, `cosφ½ = sqrt(sat(½ + ½cosφ))`; Fresnel with IOR 1.55; `g(β, x)` a normalised
Gaussian; shifts −0.07 / 0.035 / 0.14, widths `√2·r²·cosφ½`, `r²/2`, `2r²` (`r = max(0.09, r0)`).
R `= g·0.25cosφ½·F(sqrt(½ + ½V·L))·mix(1, Backlit, sat(−V·L))`;
TT `= g·exp(−3.65cosφ − 3.98)·(1 − F(cosθd·sqrt(1 − h²)))²·A^(0.5·sqrt(1 − h²a²)/cosθd)·Backlit`
with `a = 1/(1.19/cosθd + 0.36cosθd)`, `h = cosφ½(1 + a(0.6 − 0.8cosφ))`;
TRT `= g·exp(17cosφ − 16.78)·(1 − f)²f·A^(0.8/cosθd)`, `f = F(½cosθd)`.
Scatter (diffuse): `sqrt(A)·mix(sat((N·L + 1)/4), 1 − |T·L|, 0.33)·scatter/π·(A/luma)^(1 − σ)`
(luma weights 0.3/0.59/0.11; the sun uses `1 − σ²`), where BG3 uses the real normal N where
Unreal builds one from V. No N·L factor: σ is the light's shadow, or `sat(2·N·L + 0.5)` for
lights without one. Ambient: Lambert + split sum, evaluated with N and −N and blended 0.3/0.7,
`NoV = |N·V|`.

### Our port (viewer)

`character.wesl` replaces Bevy's `apply_pbr_lighting` for every character material with
`char_lighting`: a loop over Bevy's clustered point/spot lights and directional lights (the
game's falloff via `game_light.wesl`, Bevy's cone and shadow lookups; see `lighting.md`), the game's
BRDFs above per shading model, the level's light probes (and Bevy's uniform ambient, zero with
probes) through Bevy's split-sum environment-map code, and
the emissive. Exact: the Burley/GGX/remapped-Smith forms, specular anti-aliasing, the sun-disc
specular, dual-lobe specular, the transmission formula and its wrap, the hair lobes and
scatter term, the profile values and the kernel (unit-tested against an independent evaluation).
Approximated or missing:

- **What is blurred.** Bevy's forward pass yields one lit colour, so subsurface pixels mark
  themselves in the (now HDR) target's alpha, `−(1 + slot + amount/2)`, and `sss.rs` blurs the
  final colour (albedo and specular included: pre-scatter texturing) after the main passes,
  before bloom and tonemapping. Kernel, step, the four-texel test and the depth fallback are the
  game's. Transparent surfaces over skin change its alpha and drop out of the blur.
- **Thick-object transmission.** Without shadow maps for the level lights (the viewer renders
  none), a light that casts shadows in the game is marked (`lights.rs`, negative normal bias)
  and gives no transmission through thick objects (assumes the occluder is much thicker than
  the scatter length: ears and fingers don't glow); lights that don't cast shadows in the game
  give the full transmission, as in the game. With a Bevy shadow map we would gate by it rather
  than measure the occluder depth.
- Light channels, box lights (approximated, see `lighting.md`), the probe blending (the level's
  probes are Bevy reflection probes, evaluated through Bevy's environment-map code; hair blends
  N and −N 0.3/0.7), SSAO and the specular occlusion are not the game's. Subsurface clothing (imps) reads no thickness map (the VT physical layer's alpha):
  amount 1, thickness 0.
- **Eyes**: the catchlight is now unexposed (it was multiplied by the camera exposure, i.e.
  invisible) and reflects about the mesh normal plus the sclera detail normal; the sclera
  detail normal and the blindness flattening are applied; `_FX`/`_Creature_FX` glow and blend
  are ported. Still approximate: the catchlight uses the mesh's object frame rather than the
  unskinned bind pose. The HDRI is BC6H, which the texture loader now supports.

Comparisons: `BG3_CHAR_BEVY_LIGHTING=1` puts character materials back on Bevy's lighting,
`BG3_NO_SSS=1` turns the scattering pass off.

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
character-creation slider amounts (graying, highlights, make-up) and the passive
material swaps (`CharacterCreationMaterialOverrides.lsx`; metadata is loaded by CC but graph
activation is not applied). Appearance visual and material-preset selections now use the
existing assembly/layering path; see `character-creation.md` for its assumptions. Viewer **assumption**: a worn item's
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
`BG3_CHAR_DEBUG=1` logs each character material and vertex cut. Lighting, the subsurface
pass and their switches: "Lighting" above (`viewer/src/sss.rs`, `shaders/sss.wesl`; the camera
is HDR and its depth texture is bindable for that pass).

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
  The flat grey patch at Lae'zel's skirt looked the same in both: it was her half-plate's cloth
  simulation proxy being drawn (fixed, see `characters.md`, "Cloth proxies";
  `light-skirt-before.png` / `light-skirt-after.png`).
- `cs-avatar-front.png` (`--combat-autoplay --camera=-49,4.3,-405.8 --look-at=-46,3.6,-402.5`):
  the gnome's skin is darker grey-blue than `avatar-deepgnome-cleric.png` (template preset
  `UND_Dusk_6` vs the player's lighter choice) and he is bald (see above); chain shirt with
  blue cloth; the imp beside him is darker and browner than `imp.png` — its colour is the
  unchanged VT albedo, so the difference is lighting (no subsurface, no lava light).

No runtime capture of the original's G-buffer was made; the shaders were read, not traced.

### Lighting (2026-10-22)

Unit tests: the skin kernel against an independent double-precision evaluation, offsets,
normalisation, slot assignment, profile packing (`sss.rs`); shading models (`kinds.rs`); the
graph/instance profile in `resolve.rs`. Screenshots (`.cache/shots/`), same cameras as above:

- `light-before-laezel-face.png` → `light-after-laezel-face.png` (and
  `light-ab-bevy-laezel-face.png`, the new build with `BG3_CHAR_BEVY_LIGHTING=1 BG3_NO_SSS=1`):
  the hair loses its red cast and gains strand highlights along the braids, closer to the
  brown of `laezel.png`; the skin is slightly softer (8 mm scatter is a few pixels at this
  distance); the gold irises now show the catchlight. The reference's skin is still lighter
  and more even (its lighting, not the material).
- `light-before-avatar.png` → `light-after-avatar.png`: skirt fixed (cloth proxy, see above);
  skin of the arms/legs warmer where back-lit by lights without game shadows (transmission).
- `light-before-imp.png` (`--camera=-41.2,4.2,-399.8 --look-at=-38.9,3.4,-397.6
  --shot-after 4`, old lighting) → `light-after-imp.png`: slightly redder and warmer, but the imp
  stays darker and browner than `imp.png`. The reference is lit by bright orange light from
  behind/above (wings glowing through transmission); our level lights don't reproduce that
  light, so the lighting model alone does not close the gap.
- `light-before-front.png` → `light-after-front.png` (autoplay; positions vary between runs).

## Open

- Lighting: blur irradiance instead of lit colour (needs a separate diffuse target), shadow-map
  occluder depth for thick-object transmission (needs level shadow maps), light channels,
  probes; catchlight in the unskinned frame; shading model 4 (transmission only; not
  identified); the generic glow path for `Base_GM_VT` (imp eyes: ×20, R only).
- Detail normals, scars/age/wrinkles, wounds/dirt state.
- `ForcePresetValues`, `RealMaterialOverrides`/per-material overrides, CC material settings,
  slider amounts, passive material swaps.
- `CHAR_Skin_Head_Creature` (cambion wings): skin model in a non-VT variant, not ported.
