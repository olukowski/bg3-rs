//! Unit tests for curve decoding and evaluation on synthetic, self-authored data.

use super::*;

fn close(a: &[f32], b: &[f32], eps: f32) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() <= eps)
}

fn raw(format: CurveFormat, degree: u8) -> RawCurve {
    RawCurve::empty(format, degree)
}

fn curve(degree: u8, dimension: usize, knots: &[f32], controls: &[f32]) -> Curve {
    Curve {
        format: CurveFormat::DaK32fC32f,
        degree,
        dimension,
        knots: knots.to_vec(),
        controls: controls.to_vec(),
    }
}

fn at(c: &Curve, t: f32) -> Vec<f32> {
    let mut out = vec![0.0; c.dimension];
    c.sample(t, &mut out);
    out
}

/// An independent encoder for the 4n formats: drop the largest component, store the other
/// three cyclically after it, quantised with the given table.
fn encode_quat(q: [f32; 4], table: &QuatTable) -> [u16; 3] {
    let top = 1u16 << (table.bits - 1);
    let missing = (0..4)
        .max_by(|&a, &b| q[a].abs().total_cmp(&q[b].abs()))
        .unwrap();
    let mut words = [0u16; 3];
    for (i, w) in words.iter_mut().enumerate() {
        let c = (missing + 1 + i) & 3;
        let v = ((q[c] - table.offset[c]) / table.scale[c]).round();
        *w = v.clamp(0.0, f32::from(top - 1)) as u16;
    }
    if missing & 2 != 0 {
        words[1] |= top;
    }
    if missing & 1 != 0 {
        words[2] |= top;
    }
    if q[missing] < 0.0 {
        words[0] |= top;
    }
    words
}

#[test]
fn format_names_round_trip() {
    for (i, f) in CurveFormat::ALL.into_iter().enumerate() {
        assert_eq!(f as usize, i);
        assert_eq!(CurveFormat::from_name(f.name()), Some(f));
    }
    assert_eq!(CurveFormat::from_name("D5Bogus"), None);
}

#[test]
fn truncated_knot_scale_is_the_top_half_of_an_f32() {
    assert_eq!(widen_trunc(0x4170), 15.0);
    assert_eq!(widen_trunc(0x3f80), 1.0);
}

#[test]
fn quaternion_tables_cover_the_non_largest_range() {
    let r = std::f32::consts::FRAC_1_SQRT_2;
    let full = QuatTable::new(0x0000, 16);
    // Entry 0 maps [0, max] onto [-1/sqrt2, 1/sqrt2].
    assert!((full.offset[0] + r).abs() < 1e-6);
    assert!((32767.0 * full.scale[0] + full.offset[0] - r).abs() < 1e-6);
    // Entries >= 8 are negated.
    let neg = QuatTable::new(0x8888, 16);
    assert!((neg.offset[3] - r).abs() < 1e-6);
    assert!(neg.scale[3] < 0.0);
    // Nibble c selects the entry of component c.
    let mixed = QuatTable::new(0x7310, 8);
    assert!((mixed.offset[0] + r).abs() < 1e-6); // entry 0
    assert!((mixed.offset[1] + 0.5 * r).abs() < 1e-6); // entry 1
    assert!((mixed.offset[2] + 0.25 * r).abs() < 1e-6); // entry 3
    assert!(mixed.offset[3].abs() < 1e-6); // entry 7
}

#[test]
fn quantised_quaternions_round_trip() {
    let quats = [
        [0.0, 0.0, 0.0, 1.0],
        [0.0, 0.0, 0.0, -1.0],
        [0.5, -0.5, 0.5, 0.5],
        [0.9, 0.1, -0.3, 0.2],
        [-0.1, 0.95, 0.2, -0.2],
        [0.2, -0.1, -0.96, 0.1],
    ];
    for bits in [16, 8] {
        let table = QuatTable::new(0, bits);
        let tol = if bits == 16 { 1e-4 } else { 2e-2 };
        for q in quats {
            let len = q.iter().map(|v| v * v).sum::<f32>().sqrt();
            let q = q.map(|v| v / len);
            let d = table.decode(encode_quat(q, &table));
            assert!(close(&d, &q, tol), "{bits} bits: {q:?} -> {d:?}");
        }
    }
}

#[test]
fn d4n_curve_splits_knots_and_controls() {
    let table = QuatTable::new(0, 16);
    let q = [0.0, 0.6, 0.0, 0.8];
    let mut r = raw(CurveFormat::D4nK16uC15u, 2);
    r.one_over_knot_scale = 10.0;
    r.values = vec![0.0, 5.0, 10.0];
    for _ in 0..3 {
        r.values
            .extend(encode_quat(q, &table).iter().map(|&w| f32::from(w)));
    }
    let c = r.decode().unwrap();
    assert_eq!((c.dimension, c.degree), (4, 2));
    assert_eq!(c.knots, vec![0.0, 0.5, 1.0]);
    assert!(close(&at(&c, 0.7), &q, 1e-4));
    r.values.pop();
    assert!(r.decode().is_err());
}

#[test]
fn d3_and_d9_quantised_curves_dequantise() {
    let mut r = raw(CurveFormat::D3K16uC16u, 2);
    r.one_over_knot_scale = 2.0;
    r.scales = vec![1.0, 2.0, 0.5];
    r.offsets = vec![0.0, -1.0, 10.0];
    r.values = vec![0.0, 4.0, 1.0, 1.0, 2.0, 3.0, 3.0, 4.0];
    let c = r.decode().unwrap();
    assert_eq!(c.knots, vec![0.0, 2.0]);
    assert_eq!(c.controls, vec![1.0, 1.0, 11.0, 3.0, 5.0, 12.0]);

    r.format = CurveFormat::D9I3K8uC8u;
    let c = r.decode().unwrap();
    assert_eq!(c.dimension, 9);
    assert_eq!(
        &c.controls[..9],
        &[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 11.0]
    );

    let mut r = raw(CurveFormat::D9I1K16uC16u, 2);
    r.one_over_knot_scale = 1.0;
    r.scales = vec![0.5];
    r.offsets = vec![1.0];
    r.values = vec![0.0, 1.0, 2.0, 4.0];
    let c = r.decode().unwrap();
    assert_eq!(c.knots, vec![0.0, 1.0]);
    assert_eq!(
        &c.controls[9..],
        &[3.0, 0.0, 0.0, 0.0, 3.0, 0.0, 0.0, 0.0, 3.0]
    );

    let mut r = raw(CurveFormat::D3I1K8uC8u, 2);
    r.one_over_knot_scale = 1.0;
    r.scales = vec![1.0, 0.0, -1.0];
    r.offsets = vec![0.0, 7.0, 0.0];
    r.values = vec![0.0, 1.0, 2.0, 3.0];
    let c = r.decode().unwrap();
    assert_eq!(c.controls, vec![2.0, 7.0, -2.0, 3.0, 7.0, -3.0]);

    r.format = CurveFormat::D3I1K32fC32f;
    r.values = vec![0.0, 0.25, 2.0, 3.0];
    let c = r.decode().unwrap();
    assert_eq!(c.knots, vec![0.0, 0.25]);
    assert_eq!(c.controls, vec![2.0, 7.0, -2.0, 3.0, 7.0, -3.0]);
}

#[test]
fn da_quantised_curves_use_split_scale_offsets() {
    let mut r = raw(CurveFormat::DaK16uC16u, 2);
    r.one_over_knot_scale = 4.0;
    r.scales = vec![2.0, 3.0];
    r.offsets = vec![-1.0, 1.0];
    r.values = vec![0.0, 4.0, 1.0, 1.0, 2.0, 2.0];
    let c = r.decode().unwrap();
    assert_eq!(c.dimension, 2);
    assert_eq!(c.knots, vec![0.0, 1.0]);
    assert_eq!(c.controls, vec![1.0, 4.0, 3.0, 7.0]);
}

#[test]
fn constant_and_identity_curves() {
    let mut r = raw(CurveFormat::D3Constant32f, 0);
    r.values = vec![1.0, 2.0, 3.0];
    let c = r.decode().unwrap();
    assert!(c.is_constant());
    assert_eq!(at(&c, 123.0), vec![1.0, 2.0, 3.0]);
    r.values.push(4.0);
    assert!(r.decode().is_err());

    for (dim, want) in [
        (3, vec![0.0; 3]),
        (4, vec![0.0, 0.0, 0.0, 1.0]),
        (9, vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]),
    ] {
        let mut r = raw(CurveFormat::DaIdentity, 0);
        r.dimension = dim;
        let c = r.decode().unwrap();
        assert!(c.is_identity());
        assert_eq!(at(&c, 0.5), want);
    }
}

#[test]
fn degree0_holds_the_control_of_the_next_knot() {
    let c = curve(0, 1, &[0.0, 1.0, 2.0], &[10.0, 20.0, 30.0]);
    assert_eq!(at(&c, 0.0), vec![20.0]);
    assert_eq!(at(&c, 0.99), vec![20.0]);
    assert_eq!(at(&c, 1.0), vec![30.0]);
    assert_eq!(at(&c, 5.0), vec![30.0]);
}

#[test]
fn degree1_interpolates_linearly() {
    let c = curve(1, 2, &[0.0, 1.0, 3.0], &[0.0, 0.0, 1.0, 10.0, 3.0, 30.0]);
    assert!(close(&at(&c, 0.5), &[0.5, 5.0], 1e-6));
    assert!(close(&at(&c, 2.0), &[2.0, 20.0], 1e-6));
    assert!(close(&at(&c, -1.0), &[0.0, 0.0], 1e-6));
    assert!(close(&at(&c, 9.0), &[3.0, 30.0], 1e-6));
}

#[test]
fn degree2_is_a_uniform_quadratic_b_spline_inside() {
    let c = curve(2, 1, &[0.0, 1.0, 2.0, 3.0], &[0.0, 0.0, 1.0, 1.0]);
    // Basis (1/8, 6/8, 1/8) at the middle of a uniform span: controls (0, 0, 1).
    assert!(close(&at(&c, 1.5), &[0.125], 1e-6));
    // Clamped ends interpolate the end controls.
    assert!(close(&at(&c, 0.0), &[0.0], 1e-6));
    assert!(close(&at(&c, 3.0), &[1.0], 1e-6));
    // Constant controls stay constant; the curve is continuous across knots.
    let flat = curve(2, 1, &[0.0, 0.3, 1.0, 1.1], &[2.0; 4]);
    for i in 0..=22 {
        assert!(close(&at(&flat, i as f32 * 0.05), &[2.0], 1e-6));
    }
    let left = at(&c, 2.0 - 1e-4)[0];
    let right = at(&c, 2.0)[0];
    assert!((left - right).abs() < 1e-3);
}

#[test]
fn looping_wraps_and_matches_inside() {
    // Periodic data over [0, 4): the last knot duplicates the first one period later.
    let knots = [0.0, 1.0, 2.0, 3.0, 4.0];
    let controls = [0.0, 1.0, 0.0, -1.0, 0.0];
    let c = curve(2, 1, &knots, &controls);
    let mut a = [0.0];
    let mut b = [0.0];
    c.sample_looped(4.0 - 1e-4, 4.0, &mut a);
    c.sample_looped(0.0, 4.0, &mut b);
    assert!((a[0] - b[0]).abs() < 1e-3, "{a:?} vs {b:?}");
    c.sample_looped(6.5, 4.0, &mut a);
    c.sample_looped(2.5, 4.0, &mut b);
    assert_eq!(a, b);
    // Away from the ends looping and clamping agree.
    c.sample_looped(2.0, 4.0, &mut a);
    c.sample(2.0, &mut b);
    assert!((a[0] - b[0]).abs() < 1e-6);
}

#[test]
fn track_sample_normalises_and_defaults() {
    let track = TransformTrack {
        name: "Bone".into(),
        flags: 0,
        orientation: Curve::constant(CurveFormat::D4Constant32f, vec![0.0, 0.0, 0.0, 2.0]),
        position: Curve::constant(CurveFormat::D3Constant32f, vec![1.0, 2.0, 3.0]),
        scale_shear: Curve::identity(9),
    };
    let t = track.sample(0.3);
    assert_eq!(t.orientation, [0.0, 0.0, 0.0, 1.0]);
    assert_eq!(t.position, [1.0, 2.0, 3.0]);
    assert_eq!(
        t.scale_shear,
        [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
    );
    assert_eq!(t.flags, 3);
}
