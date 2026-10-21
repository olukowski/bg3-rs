//! Investigation helper: decode every animation in some paks and tally curve formats, then check
//! the decoders against the data.
//!
//! `cargo run --release -p bg3-gr2 --example anim_survey -- [Models,English_Animations] [max_files]`
//!
//! Checks:
//! * per pak / curve role (orientation, position, scale-shear, vector): format, degree, dimension;
//! * decode failures by error;
//! * quantised quaternions: how often the three stored components exceed unit length (the
//!   reconstructed fourth is then clamped to 0), and the angle between consecutive controls
//!   (a wrong component order or table shows up as jumps);
//! * rest pose: the first frame of every `HUM_M_Rig` animation against the `HUM_M_Base` skeleton.
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use bg3_gr2::{Animation, Curve, CurveFormat, Scene};

const SKELETON: &str = "Public/Shared/Assets/Characters/_Anims/Humans/_Male/HUM_M_Base.GR2";
const RIG_PREFIX: &str = "Public/Shared/Assets/Characters/_Anims/Humans/_Male/HUM_M_Rig/";

#[derive(Default)]
struct Stats {
    tally: BTreeMap<String, usize>,
    failures: BTreeMap<String, (usize, String)>,
    animations: usize,
    files: usize,
    /// D4n controls whose stored part had length > 1 (clamped).
    quat_clamped: usize,
    quat_controls: usize,
    /// Consecutive D4n controls in opposite hemispheres (negative dot product).
    quat_hemisphere_flips: usize,
    quat_steps: usize,
    /// Histogram of angles (degrees) between consecutive quaternion controls, per format.
    step_angles: BTreeMap<String, Vec<f32>>,
    /// Rest-pose angle (degrees) and distance per format of the orientation/position curve.
    rest_angle: BTreeMap<String, Vec<f32>>,
    rest_dist: BTreeMap<String, Vec<f32>>,
    /// Per animated curve: first knot, and last knot minus the animation's duration.
    first_knot: Vec<f32>,
    end_minus_duration: Vec<f32>,
}

fn angle(a: &[f32], b: &[f32]) -> f32 {
    let d: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>().abs();
    2.0 * d.min(1.0).acos().to_degrees()
}

fn role_tally(stats: &mut Stats, pak: &str, role: &str, c: &Curve) {
    *stats
        .tally
        .entry(format!(
            "{pak:<20} {role} {:<14} deg={} dim={}",
            c.format, c.degree, c.dimension
        ))
        .or_default() += 1;
    if matches!(c.format, CurveFormat::D4nK16uC15u | CurveFormat::D4nK8uC7u) {
        let steps = stats.step_angles.entry(c.format.to_string()).or_default();
        for w in c.controls.as_chunks::<4>().0.windows(2) {
            steps.push(angle(&w[0], &w[1]));
            let d: f32 = w[0].iter().zip(&w[1]).map(|(x, y)| x * y).sum();
            stats.quat_steps += 1;
            if d < 0.0 {
                stats.quat_hemisphere_flips += 1;
            }
        }
        for q in c.controls.as_chunks::<4>().0 {
            stats.quat_controls += 1;
            // A clamped reconstruction leaves the omitted component at exactly 0 while the
            // stored three have length > 1.
            let n: f32 = q.iter().map(|v| v * v).sum();
            if n > 1.0 + 1e-4 {
                stats.quat_clamped += 1;
            }
        }
    }
}

fn percentiles(v: &mut [f32]) -> String {
    if v.is_empty() {
        return "-".into();
    }
    v.sort_by(f32::total_cmp);
    let p = |q: f64| v[((v.len() - 1) as f64 * q) as usize];
    format!(
        "n={} p50={:.3} p90={:.3} p99={:.3} p99.9={:.3} max={:.3}",
        v.len(),
        p(0.5),
        p(0.9),
        p(0.99),
        p(0.999),
        v[v.len() - 1]
    )
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let paks: Vec<String> = args
        .get(1)
        .map(|s| s.split(',').map(str::to_owned).collect())
        .unwrap_or_else(|| vec!["Models".into(), "English_Animations".into()]);
    let max: usize = args
        .get(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(usize::MAX);
    let install = bg3_install::Install::locate().expect("BG3 install");
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let mut rest: BTreeMap<String, ([f32; 4], [f32; 3])> = BTreeMap::new();
    let total = Mutex::new(Stats::default());

    for p in install.paks().unwrap() {
        let stem = p.file_stem().unwrap().to_string_lossy().into_owned();
        if !paks.contains(&stem) {
            continue;
        }
        let pak = bg3_pak::Package::open(&p).unwrap();
        if let Some(e) = pak.find(SKELETON) {
            let scene = Scene::from_bytes(&pak.read(e).unwrap()).unwrap();
            for b in &scene.skeletons[0].bones {
                rest.insert(
                    b.name.clone(),
                    (b.transform.orientation, b.transform.position),
                );
            }
        }
        let entries: Vec<_> = pak
            .entries()
            .iter()
            .filter(|e| e.name.to_ascii_lowercase().ends_with(".gr2"))
            .take(max)
            .collect();
        let next = AtomicUsize::new(0);
        let rest = &rest;
        std::thread::scope(|s| {
            for _ in 0..threads {
                s.spawn(|| {
                    let mut st = Stats::default();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(e) = entries.get(i) else { break };
                        st.files += 1;
                        let anims = match pak
                            .read(e)
                            .map_err(|e| e.to_string())
                            .and_then(|d| Animation::from_bytes(&d).map_err(|e| e.to_string()))
                        {
                            Ok(a) => a,
                            Err(err) => {
                                let f = st.failures.entry(err).or_insert((0, e.name.clone()));
                                f.0 += 1;
                                continue;
                            }
                        };
                        let check_rest = e.name.starts_with(RIG_PREFIX);
                        for a in &anims {
                            st.animations += 1;
                            for tg in &a.track_groups {
                                for t in &tg.transform_tracks {
                                    role_tally(&mut st, &stem, "O", &t.orientation);
                                    role_tally(&mut st, &stem, "P", &t.position);
                                    role_tally(&mut st, &stem, "S", &t.scale_shear);
                                    if check_rest && let Some((rq, rp)) = rest.get(&t.name) {
                                        let x = t.sample(0.0);
                                        st.rest_angle
                                            .entry(t.orientation.format.to_string())
                                            .or_default()
                                            .push(angle(&x.orientation, rq));
                                        let d = (0..3)
                                            .map(|i| (x.position[i] - rp[i]).powi(2))
                                            .sum::<f32>()
                                            .sqrt();
                                        st.rest_dist
                                            .entry(t.position.format.to_string())
                                            .or_default()
                                            .push(d);
                                    }
                                }
                                for t in &tg.transform_tracks {
                                    for c in [&t.orientation, &t.position, &t.scale_shear] {
                                        if c.knots.len() > 1 {
                                            st.first_knot.push(c.knots[0]);
                                            st.end_minus_duration
                                                .push(c.knots[c.knots.len() - 1] - a.duration);
                                        }
                                    }
                                }
                                for v in &tg.vector_tracks {
                                    role_tally(&mut st, &stem, "V", &v.curve);
                                }
                            }
                        }
                    }
                    let mut t = total.lock().unwrap();
                    t.files += st.files;
                    t.animations += st.animations;
                    t.quat_clamped += st.quat_clamped;
                    t.quat_controls += st.quat_controls;
                    t.quat_steps += st.quat_steps;
                    t.quat_hemisphere_flips += st.quat_hemisphere_flips;
                    for (k, v) in st.tally {
                        *t.tally.entry(k).or_default() += v;
                    }
                    for (k, (n, ex)) in st.failures {
                        t.failures.entry(k).or_insert((0, ex)).0 += n;
                    }
                    for (k, v) in st.step_angles {
                        t.step_angles.entry(k).or_default().extend(v);
                    }
                    for (k, v) in st.rest_angle {
                        t.rest_angle.entry(k).or_default().extend(v);
                    }
                    t.first_knot.extend(st.first_knot);
                    t.end_minus_duration.extend(st.end_minus_duration);
                    for (k, v) in st.rest_dist {
                        t.rest_dist.entry(k).or_default().extend(v);
                    }
                });
            }
        });
    }
    let mut t = total.into_inner().unwrap();
    println!("{} files, {} animations", t.files, t.animations);
    for (k, v) in &t.tally {
        println!("{v:>10} {k}");
    }
    println!(
        "failures: {}",
        t.failures.values().map(|(n, _)| n).sum::<usize>()
    );
    for (k, (n, ex)) in &t.failures {
        println!("  {n:>8} {k} (e.g. {ex})");
    }
    println!(
        "D4n controls: {} ({} with stored part > unit length)",
        t.quat_controls, t.quat_clamped
    );
    println!(
        "D4n consecutive controls: {} ({} with a negative dot product)",
        t.quat_steps, t.quat_hemisphere_flips
    );
    for (k, v) in &mut t.step_angles {
        println!("step angle (deg) {k}: {}", percentiles(v));
    }
    println!("first knot: {}", percentiles(&mut t.first_knot));
    let mut neg: Vec<f32> = t.end_minus_duration.iter().map(|v| -v).collect();
    println!(
        "last knot - duration: {}",
        percentiles(&mut t.end_minus_duration)
    );
    println!("duration - last knot: {}", percentiles(&mut neg));
    for (k, v) in &mut t.rest_angle {
        println!(
            "HUM_M rest angle (deg) at t=0, orientation {k}: {}",
            percentiles(v)
        );
    }
    for (k, v) in &mut t.rest_dist {
        println!(
            "HUM_M rest distance at t=0, position {k}: {}",
            percentiles(v)
        );
    }
}
