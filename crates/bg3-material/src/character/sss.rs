//! Skin subsurface scattering: diffusion profiles and the screen-space scattering kernels.
//!
//! The game blurs the lit diffuse irradiance of subsurface pixels (shading model 3) in two
//! separable screen-space passes with a 25-tap kernel per diffusion profile, and lights the
//! back of thin parts with a transmission term built from the same profile.
//! See `docs/research/character-shaders.md`.
//!
//! bg3: ls::SubStage_SSSS, ls::CascadedShadowMaskStage::UpdateDiffusionProfileBuffer,
//! PPSSSS_H / PPSSSS_V (Core.metallib) (4.1.1.7631656)

/// Taps per kernel (tap 0 is the centre).
pub const KERNEL_TAPS: usize = 25;
/// Kernel slots of the scattering pass (the game's `SSSSProperties.Kernel[16][25]`); slot 0 is
/// the engine's default profile.
pub const MAX_PROFILES: usize = 16;
/// `Engine_Default_DiffusionProfile`, slot 0 and the fallback of materials without a slot.
pub const ENGINE_DEFAULT_PROFILE: &str = "8ac56f5d-958c-e4ae-05cc-52cf6ad3aefd";

/// A `DiffusionProfileBank` resource's `DiffusionProfile`.
#[derive(Debug, Clone, PartialEq)]
pub struct DiffusionProfile {
    pub id: String,
    pub name: String,
    /// Per-channel falloff of the scattering kernel.
    pub scatter_color: [f32; 3],
    /// Scatter width in millimetres (the kernel's outermost tap).
    pub scatter_distance: f32,
    pub transmission_tint: [f32; 3],
    /// Thickness (mm) for thickness-map values 0 and 1.
    pub thickness_remap: [f32; 2],
    /// Transmission measures the thickness in the light's shadow map.
    pub thick_object: bool,
    /// Dual-lobe specular: roughness scales of lobes A and B, weight of B.
    pub dual_specular: [f32; 3],
}

impl Default for DiffusionProfile {
    /// The values the resource reader assumes for missing attributes.
    // bg3: ls::DiffusionProfileResource::Visit (4.1.1.7631656)
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            scatter_color: [1.0; 3],
            scatter_distance: 20.0,
            transmission_tint: [1.0; 3],
            thickness_remap: [0.0, 20.0],
            thick_object: false,
            dual_specular: [0.75, 1.3, 0.85],
        }
    }
}

impl DiffusionProfile {
    /// `Engine_Default_DiffusionProfile` as the engine pak defines it (used when the install's
    /// copy isn't loaded): no scattering width.
    pub fn engine_default() -> Self {
        Self {
            id: ENGINE_DEFAULT_PROFILE.into(),
            name: "Engine_Default_DiffusionProfile".into(),
            scatter_distance: 0.0,
            ..Self::default()
        }
    }

    /// Read a `DiffusionProfileBank` `Resource` node.
    pub fn read(resource: &bg3_lsf::Node) -> Option<Self> {
        let s = |n: &bg3_lsf::Node, a: &str| n.attr(a).and_then(|v| v.as_str()).map(str::to_owned);
        let p = resource
            .children
            .iter()
            .find(|c| c.name == "DiffusionProfile")?;
        let d = Self::default();
        let f = |a: &str, d: f32| p.attr(a).and_then(|v| v.as_f64()).map_or(d, |v| v as f32);
        let v3 = |a: &str, d: [f32; 3]| match p.attr(a) {
            Some(bg3_lsf::Value::Vec3(v)) => *v,
            _ => d,
        };
        Some(Self {
            id: s(resource, "ID")?,
            name: s(resource, "Name").unwrap_or_default(),
            scatter_color: v3("ScatterColor", d.scatter_color),
            scatter_distance: f("ScatterDistance", d.scatter_distance),
            transmission_tint: v3("TransmissionTint", d.transmission_tint),
            thickness_remap: [
                f("ThicknessRemapMin", d.thickness_remap[0]),
                f("ThicknessRemapMax", d.thickness_remap[1]),
            ],
            thick_object: p
                .attr("ThickObjectTransmission")
                .and_then(|v| v.as_bool())
                .unwrap_or(d.thick_object),
            dual_specular: [
                f("DualSpecularRoughnessA", d.dual_specular[0]),
                f("DualSpecularRoughnessB", d.dual_specular[1]),
                f("DualSpecularMix", d.dual_specular[2]),
            ],
        })
    }

    /// The material shader's lighting block (`character.wesl`, v[20..24]), packed as the game's
    /// `DiffusionProfileBuffer` entry plus the kernel slot: (scatter colour, distance),
    /// (transmission tint / 4, slot), (thickness min, max − min, thick object, transmission),
    /// (dual specular A, B, mix).
    // bg3: ls::CascadedShadowMaskStage::UpdateDiffusionProfileBuffer (4.1.1.7631656)
    pub fn uniform(&self, kernel_index: usize, transmission: bool) -> [[f32; 4]; 4] {
        let [r, g, b] = self.scatter_color;
        let [tr, tg, tb] = self.transmission_tint;
        let [lo, hi] = self.thickness_remap;
        let [a, bb, mix] = self.dual_specular;
        [
            [r, g, b, self.scatter_distance],
            [0.25 * tr, 0.25 * tg, 0.25 * tb, kernel_index as f32],
            [
                lo,
                hi - lo,
                if self.thick_object { 1.0 } else { 0.0 },
                if transmission { 1.0 } else { 0.0 },
            ],
            [a, bb, mix, 0.0],
        ]
    }

    /// The separable scattering kernel: per tap (rgb weight, offset in [-3, 3]), centre first.
    pub fn kernel(&self) -> [[f32; 4]; KERNEL_TAPS] {
        separable_kernel(self.scatter_color)
    }
}

/// The skin diffusion profile as a sum of five Gaussians (variance, weight), each channel
/// stretched by its falloff, at offset `r`. The engine writes π as 3.14 (it cancels in the
/// normalisation).
fn profile(r: f32, falloff: [f32; 3]) -> [f32; 3] {
    /// The engine's π (a literal 3.14 in the reference kernel code).
    #[allow(clippy::approx_constant)]
    const ENGINE_PI: f32 = 3.14;
    const GAUSSIANS: [(f32, f32); 5] = [
        (0.0484, 0.100),
        (0.187, 0.118),
        (0.567, 0.113),
        (1.99, 0.358),
        (7.41, 0.078),
    ];
    std::array::from_fn(|c| {
        let rr = r / (falloff[c] + 0.001);
        GAUSSIANS
            .iter()
            .map(|&(v, w)| w * (-(rr * rr) / (2.0 * v)).exp() / (2.0 * ENGINE_PI * v))
            .sum()
    })
}

/// The 25-tap separable kernel: offsets `3·t·|t|` for `t` evenly in [-1, 1], weights the
/// profile times the tap's span, normalised to 1 per channel, centre tap moved first.
// bg3: ls::DiffusionProfile::DiffusionProfile (4.1.1.7631656)
pub fn separable_kernel(falloff: [f32; 3]) -> [[f32; 4]; KERNEL_TAPS] {
    const RANGE: f32 = 3.0;
    let n = KERNEL_TAPS;
    let offsets: Vec<f32> = (0..n)
        .map(|i| {
            let t = -1.0 + 2.0 * i as f32 / (n - 1) as f32;
            RANGE * t * t.abs()
        })
        .collect();
    let mut k = [[0.0f32; 4]; KERNEL_TAPS];
    for i in 0..n {
        let mut area = 0.0;
        if i > 0 {
            area += (offsets[i] - offsets[i - 1]).abs();
        }
        if i + 1 < n {
            area += (offsets[i] - offsets[i + 1]).abs();
        }
        area *= 0.5;
        let p = profile(offsets[i], falloff);
        k[i] = [area * p[0], area * p[1], area * p[2], offsets[i]];
    }
    // Centre first.
    let centre = n / 2;
    let c = k[centre];
    for i in (1..=centre).rev() {
        k[i] = k[i - 1];
    }
    k[0] = c;
    for ch in 0..3 {
        let sum: f32 = k.iter().map(|t| t[ch]).sum();
        if sum > 0.0 {
            for t in &mut k {
                t[ch] /= sum;
            }
        }
    }
    k
}

/// One profile prepared for the scattering pass.
#[derive(Debug, Clone)]
pub struct ProfileKernel {
    pub kernel: [[f32; 4]; KERNEL_TAPS],
    pub scatter_distance: f32,
}

/// Kernel slots: slot 0 the engine default, then the subsurface profiles in use.
#[derive(Debug, Clone)]
pub struct SssProfiles {
    pub profiles: Vec<DiffusionProfile>,
}

impl Default for SssProfiles {
    fn default() -> Self {
        Self {
            profiles: vec![DiffusionProfile::engine_default()],
        }
    }
}

impl SssProfiles {
    /// Start with `default` (the install's engine default profile, if loaded) in slot 0.
    pub fn new(default: Option<DiffusionProfile>) -> Self {
        Self {
            profiles: vec![default.unwrap_or_else(DiffusionProfile::engine_default)],
        }
    }

    /// The slot of `profile`, taking the next free one; 0 (the default) when all are taken.
    pub fn slot(&mut self, profile: &DiffusionProfile) -> usize {
        if let Some(i) = self.profiles.iter().position(|p| p.id == profile.id) {
            return i;
        }
        if self.profiles.len() >= MAX_PROFILES {
            return 0;
        }
        self.profiles.push(profile.clone());
        self.profiles.len() - 1
    }

    pub fn kernels(&self) -> Vec<ProfileKernel> {
        self.profiles
            .iter()
            .map(|p| ProfileKernel {
                kernel: p.kernel(),
                scatter_distance: p.scatter_distance,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_is_normalised_with_the_centre_first() {
        let k = separable_kernel([1.0, 0.37, 0.3]);
        // Offsets 3·t·|t|: the second tap (first after the centre) sits at −3.
        assert_eq!(k[1][3], -3.0);
        assert!((k[12][3] + 0.0208333).abs() < 1e-6);
        for ch in 0..3 {
            let sum: f32 = k.iter().map(|t| t[ch]).sum();
            assert!((sum - 1.0).abs() < 1e-4, "{ch}: {sum}");
        }
        assert_eq!(k[0][3], 0.0);
        assert!(k.iter().all(|t| t[3].abs() <= 3.0 + 1e-6));
        // Red scatters furthest: less of it stays in the centre tap.
        assert!(k[0][0] < k[0][2], "{:?}", k[0]);
    }

    #[test]
    fn profile_uniform_packs_tint_quarter_and_remap() {
        let p = DiffusionProfile {
            transmission_tint: [0.8, 0.4, 0.2],
            thickness_remap: [2.0, 10.0],
            ..DiffusionProfile::default()
        };
        let u = p.uniform(1, true);
        assert_eq!(u[1], [0.2, 0.1, 0.05, 1.0]);
        assert_eq!(u[2], [2.0, 8.0, 0.0, 1.0]);
        assert_eq!(u[3][..3], [0.75, 1.3, 0.85]);
    }

    #[test]
    fn skin_kernel_matches_the_reference_values() {
        // CHAR_SHARED_Skin_SS falloff; values from an independent double-precision evaluation.
        let k = separable_kernel([1.0, 0.12752977, 0.0]);
        let near = |a: f32, b: f32| (a - b).abs() < 2e-5;
        assert!(near(k[0][0], 0.022094) && near(k[0][1], 0.152973) && near(k[0][2], 1.0));
        assert!(near(k[10][0], 0.102831) && near(k[10][1], 0.043755));
        assert!(near(k[24][0], 0.002029) && k[24][2] == 0.0);
    }

    #[test]
    fn slots_start_after_the_default_and_overflow_to_it() {
        let mut s = SssProfiles::default();
        let p = |id: &str| DiffusionProfile {
            id: id.into(),
            ..DiffusionProfile::default()
        };
        assert_eq!(s.slot(&p("skin")), 1);
        assert_eq!(s.slot(&p("creature")), 2);
        assert_eq!(s.slot(&p("skin")), 1);
        for i in 0..20 {
            s.slot(&p(&format!("x{i}")));
        }
        assert_eq!(s.profiles.len(), MAX_PROFILES);
        assert_eq!(s.slot(&p("late")), 0);
    }
}
