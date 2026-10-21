//! Animations: track groups, transform/vector tracks and Granny's compressed curve formats.
//!
//! A Granny curve is a B-spline stored as paired arrays: knot `i` (a time) and control `i`
//! (a `dimension`-wide value). The many `CurveDataHeader_*` formats differ only in how knots and
//! controls are stored (float, 16-bit or 8-bit quantised, constant, identity, …); every one is
//! decoded here into a plain [`Curve`] of f32 knots and controls, which is then evaluated with
//! the engine's B-spline rules ([`Curve::sample`]).
//!
//! See `docs/formats/gr2.md` (section "Animations") for the layouts, quantisation formulas and
//! the evaluation convention, and how they were established.

use crate::file::Gr2File;
use crate::reflect::{MemberType, Object, Transform, Types, numeric_to_f32};
use crate::{Error, Result};

/// The curve encodings Granny defines, by the suffix of the variant type's
/// `CurveDataHeader_<name>` member. The discriminant is the header's `Format` byte.
///
/// Naming: `D<dim>` fixed dimension (`Da` = any), `n` = normalised quaternion, `I<k>` = `k`
/// stored values expanded to the full dimension, `K<bits><f|u>` knot storage, `C<bits><f|u>`
/// control storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CurveFormat {
    DaKeyframes32f = 0,
    DaK32fC32f = 1,
    DaIdentity = 2,
    DaConstant32f = 3,
    D3Constant32f = 4,
    D4Constant32f = 5,
    DaK16uC16u = 6,
    DaK8uC8u = 7,
    D4nK16uC15u = 8,
    D4nK8uC7u = 9,
    D3K16uC16u = 10,
    D3K8uC8u = 11,
    D9I1K16uC16u = 12,
    D9I3K16uC16u = 13,
    D9I1K8uC8u = 14,
    D9I3K8uC8u = 15,
    D3I1K32fC32f = 16,
    D3I1K16uC16u = 17,
    D3I1K8uC8u = 18,
}

impl CurveFormat {
    pub const ALL: [CurveFormat; 19] = [
        Self::DaKeyframes32f,
        Self::DaK32fC32f,
        Self::DaIdentity,
        Self::DaConstant32f,
        Self::D3Constant32f,
        Self::D4Constant32f,
        Self::DaK16uC16u,
        Self::DaK8uC8u,
        Self::D4nK16uC15u,
        Self::D4nK8uC7u,
        Self::D3K16uC16u,
        Self::D3K8uC8u,
        Self::D9I1K16uC16u,
        Self::D9I3K16uC16u,
        Self::D9I1K8uC8u,
        Self::D9I3K8uC8u,
        Self::D3I1K32fC32f,
        Self::D3I1K16uC16u,
        Self::D3I1K8uC8u,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::DaKeyframes32f => "DaKeyframes32f",
            Self::DaK32fC32f => "DaK32fC32f",
            Self::DaIdentity => "DaIdentity",
            Self::DaConstant32f => "DaConstant32f",
            Self::D3Constant32f => "D3Constant32f",
            Self::D4Constant32f => "D4Constant32f",
            Self::DaK16uC16u => "DaK16uC16u",
            Self::DaK8uC8u => "DaK8uC8u",
            Self::D4nK16uC15u => "D4nK16uC15u",
            Self::D4nK8uC7u => "D4nK8uC7u",
            Self::D3K16uC16u => "D3K16uC16u",
            Self::D3K8uC8u => "D3K8uC8u",
            Self::D9I1K16uC16u => "D9I1K16uC16u",
            Self::D9I3K16uC16u => "D9I3K16uC16u",
            Self::D9I1K8uC8u => "D9I1K8uC8u",
            Self::D9I3K8uC8u => "D9I3K8uC8u",
            Self::D3I1K32fC32f => "D3I1K32fC32f",
            Self::D3I1K16uC16u => "D3I1K16uC16u",
            Self::D3I1K8uC8u => "D3I1K8uC8u",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.name() == name)
    }
}

impl std::fmt::Display for CurveFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// A decoded curve: a B-spline of `degree` with `knots.len()` knots and as many controls of
/// `dimension` floats each (`controls.len() == knots.len() * dimension`).
///
/// Identity curves have no knots; constant curves have one knot at 0.
#[derive(Debug, Clone, PartialEq)]
pub struct Curve {
    /// The encoding it was stored in.
    pub format: CurveFormat,
    pub degree: u8,
    pub dimension: usize,
    pub knots: Vec<f32>,
    pub controls: Vec<f32>,
}

/// Write the identity value for a curve of `dimension` into `out`: `(0, 0, 0, 1)` for a
/// quaternion, the 3×3 identity for scale/shear, zeros otherwise.
fn write_identity(out: &mut [f32]) {
    out.fill(0.0);
    match out.len() {
        4 => out[3] = 1.0,
        9 => {
            out[0] = 1.0;
            out[4] = 1.0;
            out[8] = 1.0;
        }
        _ => {}
    }
}

impl Curve {
    pub fn identity(dimension: usize) -> Self {
        Self {
            format: CurveFormat::DaIdentity,
            degree: 0,
            dimension,
            knots: Vec::new(),
            controls: Vec::new(),
        }
    }

    pub fn constant(format: CurveFormat, values: Vec<f32>) -> Self {
        Self {
            format,
            degree: 0,
            dimension: values.len(),
            knots: vec![0.0],
            controls: values,
        }
    }

    /// The `DaIdentity` curve (no data; samples to the identity value).
    pub fn is_identity(&self) -> bool {
        self.knots.is_empty()
    }

    /// One knot: the value never changes.
    pub fn is_constant(&self) -> bool {
        self.knots.len() == 1
    }

    pub fn knot_count(&self) -> usize {
        self.knots.len()
    }

    fn control(&self, i: usize) -> &[f32] {
        &self.controls[i * self.dimension..(i + 1) * self.dimension]
    }

    /// Index of the first knot strictly greater than `t`, clamped to the last knot.
    // bg3: granny::FindKnot (4.1.1.7631656) — upper bound, clamped to `count - 1`.
    fn find_knot(&self, t: f32) -> usize {
        let k = self.knots.partition_point(|&x| x <= t);
        k.min(self.knots.len() - 1)
    }

    /// Evaluate at time `t`, clamping outside the knot range (a non-looping sample).
    ///
    /// Writes `dimension` floats to `out` (which must be at least that long).
    pub fn sample(&self, t: f32, out: &mut [f32]) {
        self.sample_with(t, None, out);
    }

    /// Evaluate a looping curve of period `duration`: `t` wraps, and the B-spline neighbourhood
    /// of the ends comes from the other end (the last knot, which duplicates the first one
    /// period later, is dropped). Falls back to [`sample`](Self::sample) for curves with fewer
    /// than three knots or a non-positive duration.
    pub fn sample_looped(&self, t: f32, duration: f32, out: &mut [f32]) {
        if self.knots.len() < 3 || duration <= 0.0 {
            return self.sample(t, out);
        }
        let t = t.rem_euclid(duration);
        self.sample_with(t, Some(duration), out);
    }

    // bg3: granny::ConstructBSplineBuffers + granny::SampleBSpline (4.1.1.7631656).
    // Behaviour: the knot window around `k` is `knots[k-d .. k+d-1]` with controls
    // `controls[k-d ..= k]`; out-of-range entries either repeat the end knot/control (clamped)
    // or come from the other end shifted by the duration (looping).
    fn sample_with(&self, t: f32, period: Option<f32>, out: &mut [f32]) {
        let dim = self.dimension;
        let out = &mut out[..dim];
        let n = self.knots.len();
        if n == 0 {
            return write_identity(out);
        }
        if n == 1 {
            return out.copy_from_slice(self.control(0));
        }
        let d = usize::from(self.degree);
        let (k, last) = match period {
            None => {
                let t = t.clamp(self.knots[0], self.knots[n - 1]);
                (self.find_knot(t), n - 1)
            }
            Some(_) => (self.find_knot(t), n - 2),
        };
        let t = match period {
            None => t.clamp(self.knots[0], self.knots[n - 1]),
            Some(_) => t,
        };
        // Global index -> (knot, control index), extending past the ends.
        let at = |i: isize| -> (f32, usize) {
            match period {
                None => {
                    let c = i.clamp(0, last as isize) as usize;
                    (self.knots[c], c)
                }
                Some(p) => {
                    let m = (last + 1) as isize;
                    let wraps = i.div_euclid(m);
                    let c = i.rem_euclid(m) as usize;
                    (self.knots[c] + wraps as f32 * p, c)
                }
            }
        };
        let k = k as isize;
        // Local knots U[0..2d] = knot(k-d .. k+d-1); controls D[0..=d] = control(k-d ..= k).
        let mut u = [0f32; 8];
        for (j, slot) in u.iter_mut().enumerate().take(2 * d) {
            *slot = at(k - d as isize + j as isize).0;
        }
        let mut pts = [[0f32; 16]; 4];
        for (j, p) in pts.iter_mut().enumerate().take(d + 1) {
            let (_, c) = at(k - d as isize + j as isize);
            p[..dim.min(16)].copy_from_slice(&self.control(c)[..dim.min(16)]);
        }
        if dim > 16 || d > 3 {
            // Not seen in BG3 data; fall back to the nearest control.
            return out.copy_from_slice(self.control(at(k).1));
        }
        // de Boor: for r = 1..=d, for j = d..r: a = (t - U[j-1]) / (U[j+d-r] - U[j-1]).
        for r in 1..=d {
            for j in (r..=d).rev() {
                let lo = u[j - 1];
                let hi = u[j + d - r];
                let a = if hi > lo { (t - lo) / (hi - lo) } else { 0.0 };
                let (prev, cur) = pts.split_at_mut(j);
                for (c, p) in cur[0][..dim].iter_mut().zip(&prev[j - 1][..dim]) {
                    *c = (1.0 - a) * p + a * *c;
                }
            }
        }
        out.copy_from_slice(&pts[d][..dim]);
    }

    /// Decode a `curve2` struct (an object with a `CurveData` variant member).
    pub fn from_object(curve: &Object<'_, '_>) -> Result<Self> {
        let Some(data) = curve.variant("CurveData")? else {
            return Err(Error::Corrupt("curve has no CurveData"));
        };
        Self::from_curve_data(&data)
    }

    /// Decode the `CurveData` variant object itself.
    pub fn from_curve_data(data: &Object<'_, '_>) -> Result<Self> {
        let header_member = data
            .ty
            .members
            .first()
            .ok_or(Error::Corrupt("empty curve data type"))?;
        let name = header_member
            .name
            .strip_prefix("CurveDataHeader_")
            .ok_or_else(|| Error::UnsupportedCurveFormat(header_member.name.clone()))?;
        let format = CurveFormat::from_name(name)
            .ok_or_else(|| Error::UnsupportedCurveFormat(name.to_owned()))?;
        let header = data.inline(&header_member.name)?;
        let degree = header.int("Degree")? as u8;
        let raw = RawCurve::read(format, degree, data)?;
        raw.decode()
    }
}

/// Curve data as stored, before dequantisation. Exposed so the decoding can be tested without
/// building whole GR2 files.
#[derive(Debug, Clone, PartialEq)]
pub struct RawCurve {
    pub format: CurveFormat,
    pub degree: u8,
    /// `Dimension` member (keyframe and identity curves).
    pub dimension: usize,
    /// `OneOverKnotScaleTrunc` widened to f32 (`trunc << 16`), or `OneOverKnotScale`.
    pub one_over_knot_scale: f32,
    /// `ScaleOffsetTableEntries` (quaternion formats).
    pub scale_offset_entries: u16,
    /// `ControlScales` / `ControlScale`, or the first half of `ControlScaleOffsets`.
    pub scales: Vec<f32>,
    /// `ControlOffsets` / `ControlOffset`, or the second half of `ControlScaleOffsets`.
    pub offsets: Vec<f32>,
    /// `Knots` (float formats).
    pub knots: Vec<f32>,
    /// `Controls` (float formats), or `KnotsControls` (all knots, then all controls) for the
    /// quantised ones, with integers as exact f32 values.
    pub values: Vec<f32>,
}

/// Numeric values of a member: a fixed inline array/scalar, or a `ReferenceToArray` of
/// single-scalar structs.
fn numbers(o: &Object<'_, '_>, name: &str) -> Result<Vec<f32>> {
    let m = o
        .member(name)
        .ok_or_else(|| Error::MissingMember(name.to_owned()))?;
    if m.kind == MemberType::ReferenceToArray {
        let arr = o.array(name)?;
        if arr.len == 0 {
            return Ok(Vec::new());
        }
        let [elem] = &arr.ty.members[..] else {
            return Err(Error::Corrupt("curve array element is not a single scalar"));
        };
        if !elem.kind.is_numeric() {
            return Err(Error::WrongMemberType {
                name: name.to_owned(),
                found: elem.kind,
            });
        }
        let mut out = Vec::with_capacity(arr.len * elem.count());
        numeric_to_f32(elem.kind, arr.bytes()?, arr.len * elem.count(), &mut out);
        Ok(out)
    } else {
        o.f32s(name)
    }
}

/// `OneOverKnotScaleTrunc`: the top 16 bits of an f32.
pub fn widen_trunc(trunc: u16) -> f32 {
    f32::from_bits(u32::from(trunc) << 16)
}

impl RawCurve {
    fn empty(format: CurveFormat, degree: u8) -> Self {
        Self {
            format,
            degree,
            dimension: 0,
            one_over_knot_scale: 1.0,
            scale_offset_entries: 0,
            scales: Vec::new(),
            offsets: Vec::new(),
            knots: Vec::new(),
            values: Vec::new(),
        }
    }

    pub fn read(format: CurveFormat, degree: u8, o: &Object<'_, '_>) -> Result<Self> {
        use CurveFormat::*;
        let mut r = Self::empty(format, degree);
        let trunc = |o: &Object<'_, '_>| -> Result<f32> {
            Ok(widen_trunc(o.int("OneOverKnotScaleTrunc")? as u16))
        };
        match format {
            DaIdentity => r.dimension = o.int("Dimension")?.max(0) as usize,
            DaKeyframes32f => {
                r.dimension = o.int("Dimension")?.max(0) as usize;
                r.values = numbers(o, "Controls")?;
            }
            DaConstant32f | D3Constant32f | D4Constant32f => r.values = numbers(o, "Controls")?,
            DaK32fC32f => {
                r.knots = numbers(o, "Knots")?;
                r.values = numbers(o, "Controls")?;
            }
            DaK16uC16u | DaK8uC8u => {
                r.one_over_knot_scale = trunc(o)?;
                let so = numbers(o, "ControlScaleOffsets")?;
                let half = so.len() / 2;
                r.scales = so[..half].to_vec();
                r.offsets = so[half..].to_vec();
                r.values = numbers(o, "KnotsControls")?;
            }
            D4nK16uC15u | D4nK8uC7u => {
                r.scale_offset_entries = o.int("ScaleOffsetTableEntries")? as u16;
                r.one_over_knot_scale = o.f32("OneOverKnotScale")?;
                r.values = numbers(o, "KnotsControls")?;
            }
            D3K16uC16u | D3K8uC8u | D9I3K16uC16u | D9I3K8uC8u | D3I1K16uC16u | D3I1K8uC8u => {
                r.one_over_knot_scale = trunc(o)?;
                r.scales = numbers(o, "ControlScales")?;
                r.offsets = numbers(o, "ControlOffsets")?;
                r.values = numbers(o, "KnotsControls")?;
            }
            D9I1K16uC16u | D9I1K8uC8u => {
                r.one_over_knot_scale = trunc(o)?;
                r.scales = numbers(o, "ControlScale")?;
                r.offsets = numbers(o, "ControlOffset")?;
                r.values = numbers(o, "KnotsControls")?;
            }
            D3I1K32fC32f => {
                r.scales = numbers(o, "ControlScales")?;
                r.offsets = numbers(o, "ControlOffsets")?;
                r.values = numbers(o, "KnotsControls")?;
            }
        }
        Ok(r)
    }

    /// Dequantise into a [`Curve`].
    pub fn decode(&self) -> Result<Curve> {
        use CurveFormat::*;
        let format = self.format;
        let degree = self.degree;
        let curve = |dimension, knots, controls| Curve {
            format,
            degree,
            dimension,
            knots,
            controls,
        };
        Ok(match format {
            DaIdentity => {
                let mut c = Curve::identity(self.dimension);
                c.format = format;
                c
            }
            DaConstant32f | D3Constant32f | D4Constant32f => {
                let want = match format {
                    D3Constant32f => 3,
                    D4Constant32f => 4,
                    _ => self.values.len(),
                };
                if self.values.len() != want || want == 0 {
                    return Err(Error::Corrupt("constant curve has the wrong size"));
                }
                Curve::constant(format, self.values.clone())
            }
            DaKeyframes32f => {
                // One control per frame; the knots are the frame indices.
                let dim = self.dimension;
                if dim == 0 || !self.values.len().is_multiple_of(dim) {
                    return Err(Error::Corrupt("keyframe curve size"));
                }
                let n = self.values.len() / dim;
                curve(dim, (0..n).map(|i| i as f32).collect(), self.values.clone())
            }
            DaK32fC32f => {
                let n = self.knots.len();
                if n == 0 {
                    return Err(Error::Corrupt("float curve without knots"));
                }
                if !self.values.len().is_multiple_of(n) {
                    return Err(Error::Corrupt(
                        "float curve controls not a multiple of knots",
                    ));
                }
                curve(
                    self.values.len() / n,
                    self.knots.clone(),
                    self.values.clone(),
                )
            }
            DaK16uC16u | DaK8uC8u => {
                let dim = self.scales.len();
                if dim == 0 || self.offsets.len() != dim {
                    return Err(Error::Corrupt("quantised curve scale/offset size"));
                }
                let (knots, ctrl) = self.split_knots(dim)?;
                let controls = ctrl
                    .chunks_exact(dim)
                    .flat_map(|c| {
                        c.iter()
                            .zip(self.scales.iter().zip(&self.offsets))
                            .map(|(v, (s, o))| v * s + o)
                    })
                    .collect();
                curve(dim, knots, controls)
            }
            D3K16uC16u | D3K8uC8u | D9I3K16uC16u | D9I3K8uC8u => {
                if self.scales.len() != 3 || self.offsets.len() != 3 {
                    return Err(Error::Corrupt("quantised curve scale/offset size"));
                }
                let (knots, ctrl) = self.split_knots(3)?;
                let d9 = matches!(format, D9I3K16uC16u | D9I3K8uC8u);
                let mut controls = Vec::with_capacity(ctrl.len() * if d9 { 3 } else { 1 });
                for c in ctrl.as_chunks::<3>().0 {
                    let v: [f32; 3] =
                        std::array::from_fn(|i| c[i] * self.scales[i] + self.offsets[i]);
                    if d9 {
                        controls.extend_from_slice(&diagonal(v));
                    } else {
                        controls.extend_from_slice(&v);
                    }
                }
                curve(if d9 { 9 } else { 3 }, knots, controls)
            }
            D9I1K16uC16u | D9I1K8uC8u => {
                let (Some(&s), Some(&o)) = (self.scales.first(), self.offsets.first()) else {
                    return Err(Error::Corrupt("quantised curve scale/offset size"));
                };
                let (knots, ctrl) = self.split_knots(1)?;
                let controls = ctrl.iter().flat_map(|c| diagonal([c * s + o; 3])).collect();
                curve(9, knots, controls)
            }
            D3I1K32fC32f | D3I1K16uC16u | D3I1K8uC8u => {
                if self.scales.len() != 3 || self.offsets.len() != 3 {
                    return Err(Error::Corrupt("quantised curve scale/offset size"));
                }
                let (knots, ctrl) = if format == D3I1K32fC32f {
                    let n = self.values.len() / 2;
                    if self.values.len() != 2 * n || n == 0 {
                        return Err(Error::Corrupt("knots/controls size"));
                    }
                    (self.values[..n].to_vec(), self.values[n..].to_vec())
                } else {
                    self.split_knots(1)?
                };
                let controls = ctrl
                    .iter()
                    .flat_map(|&c| {
                        let v: [f32; 3] =
                            std::array::from_fn(|i| c * self.scales[i] + self.offsets[i]);
                        v
                    })
                    .collect();
                curve(3, knots, controls)
            }
            D4nK16uC15u | D4nK8uC7u => {
                let (knots, ctrl) = self.split_knots(3)?;
                let bits = if format == D4nK16uC15u { 16 } else { 8 };
                let table = QuatTable::new(self.scale_offset_entries, bits);
                let mut controls = Vec::with_capacity(knots.len() * 4);
                for c in ctrl.as_chunks::<3>().0 {
                    let q = table.decode([c[0] as u16, c[1] as u16, c[2] as u16]);
                    controls.extend_from_slice(&q);
                }
                curve(4, knots, controls)
            }
        })
    }

    /// Split `KnotsControls` (n knots then n × `per_knot` controls) and dequantise the knots.
    fn split_knots(&self, per_knot: usize) -> Result<(Vec<f32>, Vec<f32>)> {
        let len = self.values.len();
        if !len.is_multiple_of(per_knot + 1) || len == 0 {
            return Err(Error::Corrupt("knots/controls size"));
        }
        let n = len / (per_knot + 1);
        if self.one_over_knot_scale == 0.0 || !self.one_over_knot_scale.is_finite() {
            return Err(Error::Corrupt("knot scale"));
        }
        let inv = 1.0 / self.one_over_knot_scale;
        let knots = self.values[..n].iter().map(|k| k * inv).collect();
        Ok((knots, self.values[n..].to_vec()))
    }
}

fn diagonal([a, b, c]: [f32; 3]) -> [f32; 9] {
    [a, 0.0, 0.0, 0.0, b, 0.0, 0.0, 0.0, c]
}

/// Granny's quaternion quantisation ranges: entry `i` maps a normalised stored value `v ∈ [0, 1]`
/// to `v · scale + offset`. Entries 8–15 are entries 0–7 negated.
///
/// The ranges are nested intervals of `[−1/√2, 1/√2]` — the range of every component but the
/// largest of a unit quaternion. Values as in `granny::QuaternionCurveScaleOffsetTable`
/// (4.1.1.7631656), written here as multiples of `1/√2`.
const QUAT_RANGES: [(f32, f32); 8] = {
    const R: f32 = std::f32::consts::FRAC_1_SQRT_2;
    [
        (2.0 * R, -R),
        (R, -0.5 * R),
        (0.5 * R, -0.75 * R),
        (0.5 * R, -0.25 * R),
        (0.5 * R, 0.25 * R),
        (0.25 * R, -0.25 * R),
        (0.25 * R, -0.125 * R),
        (0.25 * R, 0.0),
    ]
};

/// Per-component scale/offset selected by the four nibbles of `ScaleOffsetTableEntries`.
#[derive(Debug, Clone, Copy)]
pub struct QuatTable {
    scale: [f32; 4],
    offset: [f32; 4],
    bits: u32,
}

impl QuatTable {
    /// `bits` is the storage width: 16 (`D4nK16uC15u`) or 8 (`D4nK8uC7u`).
    pub fn new(entries: u16, bits: u32) -> Self {
        let max = ((1u32 << (bits - 1)) - 1) as f32;
        let mut scale = [0.0; 4];
        let mut offset = [0.0; 4];
        for c in 0..4 {
            let e = usize::from((entries >> (4 * c)) & 0xf);
            let (s, o) = QUAT_RANGES[e & 7];
            let sign = if e >= 8 { -1.0 } else { 1.0 };
            scale[c] = sign * s / max;
            offset[c] = sign * o;
        }
        Self {
            scale,
            offset,
            bits,
        }
    }

    /// Rebuild a quaternion `(x, y, z, w)` from three stored words.
    ///
    /// The top bit of word 1 and of word 2 form the index of the omitted (largest) component;
    /// the three stored ones follow it cyclically. The omitted component is
    /// `±sqrt(1 − Σ others²)`, negative when the top bit of word 0 is set.
    // bg3: granny::CurveExtractKnotValuesD4nK16uC15u / …D4nK8uC7u (4.1.1.7631656).
    pub fn decode(&self, words: [u16; 3]) -> [f32; 4] {
        let top = 1u16 << (self.bits - 1);
        let mask = top - 1;
        let missing = (usize::from(words[1] & top != 0) << 1) | usize::from(words[2] & top != 0);
        let negative = words[0] & top != 0;
        let mut q = [0.0f32; 4];
        let mut sum = 0.0;
        for (i, w) in words.iter().enumerate() {
            let c = (missing + 1 + i) & 3;
            let v = f32::from(w & mask) * self.scale[c] + self.offset[c];
            q[c] = v;
            sum += v * v;
        }
        let m = (1.0 - sum).max(0.0).sqrt();
        q[missing] = if negative { -m } else { m };
        q
    }
}

/// One bone's animation.
#[derive(Debug, Clone)]
pub struct TransformTrack {
    /// Bone name.
    pub name: String,
    pub flags: u32,
    /// Quaternion `(x, y, z, w)`, parent-local.
    pub orientation: Curve,
    /// Translation, parent-local.
    pub position: Curve,
    /// 3×3 scale/shear, row-major (`Transform::scale_shear` layout).
    pub scale_shear: Curve,
}

impl TransformTrack {
    fn read(o: &Object<'_, '_>) -> Result<Self> {
        Ok(Self {
            name: o.string_or_empty("Name"),
            flags: o.int("Flags").unwrap_or(0) as u32,
            orientation: Curve::from_object(&o.inline("OrientationCurve")?)?,
            position: Curve::from_object(&o.inline("PositionCurve")?)?,
            scale_shear: Curve::from_object(&o.inline("ScaleShearCurve")?)?,
        })
    }

    /// Local transform at `t` (clamped). The orientation is normalised.
    pub fn sample(&self, t: f32) -> Transform {
        self.sample_impl(t, None)
    }

    /// Local transform at `t` for a looping playback of period `duration`.
    pub fn sample_looped(&self, t: f32, duration: f32) -> Transform {
        self.sample_impl(t, Some(duration))
    }

    fn sample_impl(&self, t: f32, period: Option<f32>) -> Transform {
        let eval = |c: &Curve, out: &mut [f32]| match period {
            None => c.sample(t, out),
            Some(p) => c.sample_looped(t, p, out),
        };
        let mut position = [0.0; 3];
        let mut q = [0.0, 0.0, 0.0, 1.0];
        let mut ss = [0.0; 9];
        if self.position.dimension == 3 {
            eval(&self.position, &mut position);
        }
        if self.orientation.dimension == 4 {
            eval(&self.orientation, &mut q);
            let len = q.iter().map(|v| v * v).sum::<f32>().sqrt();
            if len > 0.0 {
                q.iter_mut().for_each(|v| *v /= len);
            } else {
                q = [0.0, 0.0, 0.0, 1.0];
            }
        }
        if self.scale_shear.dimension == 9 {
            eval(&self.scale_shear, &mut ss);
        } else {
            write_identity(&mut ss);
        }
        let mut flags = 0;
        if !self.position.is_identity() {
            flags |= 1;
        }
        if !self.orientation.is_identity() {
            flags |= 2;
        }
        if !self.scale_shear.is_identity() {
            flags |= 4;
        }
        Transform {
            flags,
            position,
            orientation: q,
            scale_shear: [
                [ss[0], ss[1], ss[2]],
                [ss[3], ss[4], ss[5]],
                [ss[6], ss[7], ss[8]],
            ],
        }
    }
}

/// A named scalar/vector channel (`Textkeys`, `IKFK`, `inverseScaleX`, `visibility`, …).
#[derive(Debug, Clone)]
pub struct VectorTrack {
    pub name: String,
    pub track_key: u32,
    pub dimension: usize,
    pub curve: Curve,
}

#[derive(Debug, Clone)]
pub struct TrackGroup {
    pub name: String,
    pub transform_tracks: Vec<TransformTrack>,
    pub vector_tracks: Vec<VectorTrack>,
    pub initial_placement: Transform,
    pub accumulation_flags: u32,
    pub loop_translation: [f32; 3],
}

impl TrackGroup {
    pub fn track(&self, bone: &str) -> Option<&TransformTrack> {
        self.transform_tracks.iter().find(|t| t.name == bone)
    }
}

#[derive(Debug, Clone)]
pub struct Animation {
    pub name: String,
    /// Seconds.
    pub duration: f32,
    /// Seconds per frame of the source animation.
    pub time_step: f32,
    pub oversampling: f32,
    pub default_loop_count: i64,
    pub flags: u32,
    pub track_groups: Vec<TrackGroup>,
}

impl Animation {
    /// Parse a file and decode all of its animations.
    pub fn from_bytes(data: &[u8]) -> Result<Vec<Animation>> {
        read_animations(&Gr2File::parse(data)?)
    }

    fn read(a: &Object<'_, '_>) -> Result<Self> {
        let mut track_groups = Vec::new();
        for tg in a.references("TrackGroups")?.into_iter().flatten() {
            track_groups.push(read_track_group(&tg)?);
        }
        Ok(Self {
            name: a.string_or_empty("Name"),
            duration: a.f32("Duration")?,
            time_step: a.f32("TimeStep").unwrap_or(0.0),
            oversampling: a.f32("Oversampling").unwrap_or(1.0),
            default_loop_count: a.int("DefaultLoopCount").unwrap_or(0),
            flags: a.int("Flags").unwrap_or(0) as u32,
            track_groups,
        })
    }
}

fn read_track_group(tg: &Object<'_, '_>) -> Result<TrackGroup> {
    let mut transform_tracks = Vec::new();
    for t in tg.array("TransformTracks")?.iter() {
        transform_tracks.push(TransformTrack::read(&t)?);
    }
    let mut vector_tracks = Vec::new();
    if tg.has("VectorTracks") {
        for v in tg.array("VectorTracks")?.iter() {
            vector_tracks.push(VectorTrack {
                name: v.string_or_empty("Name"),
                track_key: v.int("TrackKey").unwrap_or(0) as u32,
                dimension: v.int("Dimension").unwrap_or(0).max(0) as usize,
                curve: Curve::from_object(&v.inline("ValueCurve")?)?,
            });
        }
    }
    let lt = tg.f32s("LoopTranslation").unwrap_or_default();
    Ok(TrackGroup {
        name: tg.string_or_empty("Name"),
        transform_tracks,
        vector_tracks,
        initial_placement: tg.transform("InitialPlacement").unwrap_or_default(),
        accumulation_flags: tg.int("AccumulationFlags").unwrap_or(0) as u32,
        loop_translation: [
            lt.first().copied().unwrap_or(0.0),
            lt.get(1).copied().unwrap_or(0.0),
            lt.get(2).copied().unwrap_or(0.0),
        ],
    })
}

/// Decode every entry of the root's `Animations` list.
pub fn read_animations(file: &Gr2File) -> Result<Vec<Animation>> {
    let types = Types::new(file);
    let root = types.root()?;
    if !root.has("Animations") {
        return Ok(Vec::new());
    }
    root.references("Animations")?
        .into_iter()
        .flatten()
        .map(|a| Animation::read(&a))
        .collect()
}

#[cfg(test)]
#[path = "anim_tests.rs"]
mod tests;
