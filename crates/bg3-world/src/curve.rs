//! Editor curves (`Curve` nodes with `CurveKeys` and `CurveLimits`), as the engine evaluates
//! them. Used by terrain base layers (slope and height curves).
//!
//! Sources: disassembly of `ls::FCurve::Visit`, `ls::FCurve::UpdateDirty` and
//! `ls::FCurve::GetValueAt` (4.1.1.7631656, objdump on the arm64 slice).

use bg3_lsf::Node;

/// `CurveKeyInterpolationType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interpolation {
    /// Hold the key's value up to the next key.
    Constant,
    Linear,
    /// Cubic Hermite with the keys' tangents as slopes.
    Hermite,
    /// Any other value: the engine skips such a segment and keeps searching; if no segment
    /// matches, the value is 0.
    Other(i32),
}

impl Interpolation {
    fn from_i32(v: i32) -> Self {
        match v {
            0 => Self::Constant,
            1 => Self::Linear,
            2 => Self::Hermite,
            v => Self::Other(v),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CurveKey {
    /// `CurveKeyPosition` (x, value).
    pub position: [f32; 2],
    /// Slopes: `tan` of the stored `CurveKeyLeftTangent` / `CurveKeyRightTangent` angles.
    pub left_slope: f32,
    pub right_slope: f32,
    pub interpolation: Interpolation,
}

/// A curve: keys sorted by x, plus the `CurveLimits` box that clamps input and output.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FCurve {
    pub keys: Vec<CurveKey>,
    /// `[MinX, MaxX]`, `[MinY, MaxY]`.
    pub limits_x: [f32; 2],
    pub limits_y: [f32; 2],
}

/// The engine ignores a limit range of this width or less.
// bg3: ls::FCurve::GetValueAt (4.1.1.7631656)
const LIMIT_EPSILON: f32 = 1e-7;

fn clamp_to(v: f32, [lo, hi]: [f32; 2]) -> f32 {
    if hi - lo > LIMIT_EPSILON {
        // `lo` first, then `hi`, as the engine does (only matters if lo > hi).
        let v = if v < lo { lo } else { v };
        if v < hi { v } else { hi }
    } else {
        v
    }
}

impl FCurve {
    /// Read a `Curve` node: `Object/CurveKeys*` and `Object/CurveLimits`.
    pub fn read(curve: &Node) -> Self {
        let f = |n: &Node, name: &str| n.attr(name).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
        let mut out = FCurve::default();
        for object in curve.children_named("Object") {
            for k in object.children_named("CurveKeys") {
                let position = k
                    .attr("CurveKeyPosition")
                    .and_then(|v| v.components())
                    .filter(|c| c.len() == 2)
                    .map_or([0.0; 2], |c| [c[0] as f32, c[1] as f32]);
                let interpolation = k
                    .attr("CurveKeyInterpolationType")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0) as i32;
                out.keys.push(CurveKey {
                    position,
                    // bg3: ls::FCurve::UpdateDirty (4.1.1.7631656): tanf of both tangents.
                    left_slope: f(k, "CurveKeyLeftTangent").tan(),
                    right_slope: f(k, "CurveKeyRightTangent").tan(),
                    interpolation: Interpolation::from_i32(interpolation),
                });
            }
            if let Some(l) = object.child("CurveLimits") {
                out.limits_x = [f(l, "MinX"), f(l, "MaxX")];
                out.limits_y = [f(l, "MinY"), f(l, "MaxY")];
            }
        }
        // bg3: ls::FCurve::UpdateDirty sorts the keys by x.
        out.keys
            .sort_by(|a, b| a.position[0].total_cmp(&b.position[0]));
        out
    }

    /// The curve's value at `x`.
    // bg3: ls::FCurve::GetValueAt (4.1.1.7631656)
    pub fn value_at(&self, x: f32) -> f32 {
        let v = match self.keys.as_slice() {
            [] => 0.0,
            [only] => only.position[1],
            keys => {
                let x = clamp_to(x, self.limits_x);
                let (first, last) = (keys[0], keys[keys.len() - 1]);
                if x <= first.position[0] {
                    first.position[1]
                } else if x >= last.position[0] {
                    last.position[1]
                } else {
                    let mut v = 0.0;
                    for pair in keys.windows(2) {
                        let (a, b) = (pair[0], pair[1]);
                        if x < a.position[0] || x > b.position[0] {
                            continue;
                        }
                        let dx = b.position[0] - a.position[0];
                        let t = (x - a.position[0]) / dx;
                        let (y0, y1) = (a.position[1], b.position[1]);
                        match a.interpolation {
                            Interpolation::Constant => v = y0,
                            Interpolation::Linear => v = y0 + t * (y1 - y0),
                            Interpolation::Hermite => {
                                let (t2, t3) = (t * t, t * t * t);
                                let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
                                let h10 = t3 - 2.0 * t2 + t;
                                let h01 = 3.0 * t2 - 2.0 * t3;
                                let h11 = t3 - t2;
                                v = h00 * y0
                                    + h10 * dx * a.right_slope
                                    + h01 * y1
                                    + h11 * dx * b.left_slope;
                            }
                            Interpolation::Other(_) => continue,
                        }
                        break;
                    }
                    v
                }
            }
        };
        clamp_to(v, self.limits_y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(x: f32, y: f32, interpolation: Interpolation) -> CurveKey {
        CurveKey {
            position: [x, y],
            left_slope: 0.0,
            right_slope: 0.0,
            interpolation,
        }
    }

    #[test]
    fn evaluates_like_the_engine() {
        let mut c = FCurve {
            keys: vec![
                key(0.0, 1.0, Interpolation::Linear),
                key(10.0, 0.0, Interpolation::Constant),
                key(20.0, 1.0, Interpolation::Linear),
            ],
            limits_x: [0.0, 20.0],
            limits_y: [0.0, 1.0],
        };
        assert_eq!(c.value_at(-5.0), 1.0);
        assert_eq!(c.value_at(5.0), 0.5);
        assert_eq!(c.value_at(15.0), 0.0);
        assert_eq!(c.value_at(25.0), 1.0);
        // Output clamped to the limits.
        c.keys[0].position[1] = 3.0;
        assert_eq!(c.value_at(0.0), 1.0);
        // A degenerate limit box doesn't clamp.
        c.limits_y = [0.0, 0.0];
        assert_eq!(c.value_at(0.0), 3.0);
        // One key: its value everywhere; none: 0.
        let one = FCurve {
            keys: vec![key(0.0, 0.7, Interpolation::Linear)],
            ..FCurve::default()
        };
        assert_eq!(one.value_at(50.0), 0.7);
        assert_eq!(FCurve::default().value_at(1.0), 0.0);
    }

    #[test]
    fn hermite_uses_tangent_slopes() {
        let mut a = key(0.0, 0.0, Interpolation::Hermite);
        a.right_slope = 1.0;
        let mut b = key(2.0, 2.0, Interpolation::Linear);
        b.left_slope = 1.0;
        let c = FCurve {
            keys: vec![a, b],
            ..FCurve::default()
        };
        // Slope 1 at both ends of a line with slope 1: the cubic is the line.
        assert!((c.value_at(0.5) - 0.5).abs() < 1e-6);
        assert!((c.value_at(1.5) - 1.5).abs() < 1e-6);
    }
}
