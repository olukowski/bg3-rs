//! Investigation helper: tally vertex formats / index widths / extended data across the install
//! and check the QTangent convention against geometry.
//! `cargo run --release -p bg3-gr2 --example gr2_scan -- [Models,Gustav] [max_files]`
use std::collections::BTreeMap;

use bg3_gr2::model::{decode_qtangent, quat_y_axis};
use bg3_gr2::{Gr2File, MemberType, Scene, Types};

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[derive(Default)]
struct Agree {
    n: u64,
    z_normal: u64,
    x_tangent: u64,
    y_tangent: u64,
    hand_match: u64,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let paks: Vec<String> = args
        .get(1)
        .map(|s| s.split(',').map(str::to_owned).collect())
        .unwrap_or_else(|| vec!["Models".into()]);
    let max: usize = args
        .get(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(usize::MAX);
    let install = bg3_install::Install::locate().unwrap();
    let mut tally: BTreeMap<String, usize> = BTreeMap::new();
    let mut agree = Agree::default();
    let mut seen = 0;
    for p in install.paks().unwrap() {
        let stem = p.file_stem().unwrap().to_string_lossy().into_owned();
        if !paks.contains(&stem) {
            continue;
        }
        let pak = bg3_pak::Package::open(&p).unwrap();
        for e in pak.entries() {
            if !e.name.to_ascii_lowercase().ends_with(".gr2") || seen >= max {
                continue;
            }
            seen += 1;
            let data = pak.read(e).unwrap();
            let file = Gr2File::parse(&data).unwrap();
            // Raw vertex component declarations + extended data member names.
            let types = Types::new(&file);
            let root = types.root().unwrap();
            let root_members: Vec<String> =
                root.ty.members.iter().map(|m| m.name.clone()).collect();
            *tally
                .entry(format!("root members: {}", root_members.join(",")))
                .or_default() += 1;
            for m in root.references("Meshes").unwrap().into_iter().flatten() {
                if let Ok(Some(ext)) = m.variant("ExtendedData") {
                    let names: Vec<String> = ext
                        .ty
                        .members
                        .iter()
                        .map(|m| format!("{}:{:?}", m.name, m.kind))
                        .collect();
                    *tally
                        .entry(format!("mesh ext: {}", names.join(",")))
                        .or_default() += 1;
                    if let Ok(u) = ext.inline("UserMeshProperties") {
                        if let Ok(f) = u.f32s("Flags") {
                            *tally.entry(format!("mesh flags: {f:?}")).or_default() += 1;
                        }
                        let names: Vec<String> =
                            u.ty.members
                                .iter()
                                .map(|m| format!("{}:{:?}x{}", m.name, m.kind, m.count()))
                                .collect();
                        *tally
                            .entry(format!("user props: {}", names.join(",")))
                            .or_default() += 1;
                    }
                }
            }
            let scene = Scene::from_file(&file).unwrap();
            if scene.animation_count > 0 {
                *tally.entry("files with animations".into()).or_default() += 1;
            }
            for mesh in &scene.meshes {
                let fmt: Vec<String> = mesh
                    .format
                    .iter()
                    .map(|c| format!("{}:{:?}x{}", c.name, c.kind, c.count))
                    .collect();
                *tally.entry(format!("vfmt {}", fmt.join(" "))).or_default() += 1;
                *tally
                    .entry(format!("index width {:?}", mesh.index_width))
                    .or_default() += 1;
                if mesh.morph_target_count > 0 {
                    *tally.entry("meshes with morph targets".into()).or_default() += 1;
                }
                *tally
                    .entry(format!("groups per mesh {}", mesh.groups.len().min(5)))
                    .or_default() += 1;
                // QTangent convention check on raw quaternions.
                let qc = mesh
                    .format
                    .iter()
                    .find(|c| c.name == "QTangent" && c.kind == MemberType::BinormalInt16);
                if qc.is_none() || mesh.uvs.is_empty() {
                    continue;
                }
                // Re-read raw quaternions via the generic walker.
                let vd = root
                    .references("Meshes")
                    .unwrap()
                    .into_iter()
                    .flatten()
                    .find(|m| m.string_or_empty("Name") == mesh.name)
                    .and_then(|m| m.reference("PrimaryVertexData").ok().flatten())
                    .unwrap();
                let verts = vd.array("Vertices").unwrap();
                let quats: Vec<[f32; 4]> = verts
                    .iter()
                    .map(|v| {
                        let q = v.f32s("QTangent").unwrap();
                        [q[0], q[1], q[2], q[3]]
                    })
                    .collect();
                let uv = &mesh.uvs[0];
                let (mut mn, mut mok, mut mt) = (0u64, 0u64, 0u64);
                for t in mesh.indices.as_chunks::<3>().0.iter().take(2000) {
                    let [a, b, c] = [t[0] as usize, t[1] as usize, t[2] as usize];
                    let (p0, p1, p2) = (mesh.positions[a], mesh.positions[b], mesh.positions[c]);
                    let e1 = sub(p1, p0);
                    let e2 = sub(p2, p0);
                    let fnrm = cross(e1, e2);
                    if dot(fnrm, fnrm) < 1e-12 {
                        continue;
                    }
                    let (du1, dv1) = (uv[b][0] - uv[a][0], uv[b][1] - uv[a][1]);
                    let (du2, dv2) = (uv[c][0] - uv[a][0], uv[c][1] - uv[a][1]);
                    let det = du1 * dv2 - du2 * dv1;
                    if det.abs() < 1e-8 {
                        continue;
                    }
                    let tu = [
                        (e1[0] * dv2 - e2[0] * dv1) / det,
                        (e1[1] * dv2 - e2[1] * dv1) / det,
                        (e1[2] * dv2 - e2[2] * dv1) / det,
                    ];
                    let bv = [
                        (e2[0] * du1 - e1[0] * du2) / det,
                        (e2[1] * du1 - e1[1] * du2) / det,
                        (e2[2] * du1 - e1[2] * du2) / det,
                    ];
                    let q = quats[a];
                    let (n, tan, w) = decode_qtangent(q);
                    agree.n += 1;
                    agree.z_normal += u64::from(dot(n, fnrm) > 0.0);
                    agree.x_tangent += u64::from(dot(tan, tu) > 0.0);
                    agree.y_tangent += u64::from(dot(quat_y_axis(q), tu) > 0.0);
                    let bt = cross(n, tan);
                    let ok = dot([bt[0] * w, bt[1] * w, bt[2] * w], bv) > 0.0;
                    agree.hand_match += u64::from(ok);
                    mn += 1;
                    mok += u64::from(ok);
                    mt += u64::from(dot(tan, tu) > 0.0);
                }
                if mn > 50 {
                    let dir: Vec<&str> = e.name.split('/').take(5).collect();
                    let k = format!(
                        "bt {} t {} {}",
                        if mok * 2 > mn { "ok  " } else { "flip" },
                        if mt * 2 > mn { "ok  " } else { "flip" },
                        dir.join("/")
                    );
                    *tally.entry(k).or_default() += 1;
                }
            }
        }
    }
    for (k, v) in &tally {
        println!("{v:8}  {k}");
    }
    let pct = |x: u64| 100.0 * x as f64 / agree.n.max(1) as f64;
    println!(
        "qtangent check over {} tris (decode_qtangent): normal·face>0 {:.1}%  tangent·dP/du>0 {:.1}%  (Y axis·dP/du>0 {:.1}%)  (N×T)·w·dP/dv>0 {:.1}%",
        agree.n,
        pct(agree.z_normal),
        pct(agree.x_tangent),
        pct(agree.y_tangent),
        pct(agree.hand_match)
    );
}
