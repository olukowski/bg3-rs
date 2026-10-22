//! Survey the lights and light probes of a level around a point: which probes cover it (with a
//! capture for the lighting in effect there), and every light whose range reaches it, with the
//! illuminance it would give at the point. Used to audit the viewer's light and ambient set-up
//! (`docs/research/lighting.md`).
//!
//! Run: `cargo run --release -p bg3x --example light_survey [LEVEL] [x,y,z] [radius]`
//! (BG3 world space; game data: keep the output local).

use anyhow::Result;
use bg3_install::Install;
use bg3_vfs::Vfs;
use bg3_world::lights::{LightingSources, load_level_lights};
use bg3_world::probes::load_level_probes;
use bg3_world::{LightKind, RootTemplates};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let level = args.next().unwrap_or("TUT_Avernus_C".into());
    let at: [f32; 3] = args
        .next()
        .map(|s| {
            let v: Vec<f32> = s.split(',').filter_map(|x| x.parse().ok()).collect();
            [v[0], v[1], v[2]]
        })
        .unwrap_or([-46.6, 3.5, -402.8]);
    let search: f32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(30.0);
    let vfs = Vfs::from_install(&Install::locate()?)?;
    let templates = RootTemplates::load(&vfs)?;
    let sources = LightingSources::load(&vfs, &level)?;
    let lighting = sources.resolve(sources.zones_at(Some(at)));
    println!(
        "at {at:?}: lighting {} ({:?}), atmosphere {}",
        lighting.lighting_name, lighting.lighting_id, lighting.atmosphere_name
    );
    println!("sun {:?}\nsky {:?}", lighting.sun, lighting.sky);
    let dist = |p: [f32; 3]| {
        ((p[0] - at[0]).powi(2) + (p[1] - at[1]).powi(2) + (p[2] - at[2]).powi(2)).sqrt()
    };

    println!("\nprobes (distance, name, shape, radius/size, TD, intensity, capture):");
    let mut probes = load_level_probes(&vfs, &level, &templates)?;
    probes.sort_by(|a, b| dist(a.transform.position).total_cmp(&dist(b.transform.position)));
    for p in &probes {
        let d = dist(p.transform.position);
        let cap = lighting
            .lighting_id
            .as_deref()
            .and_then(|l| p.capture(l))
            .is_some();
        println!(
            "{d:7.1} {:40} pos {:?} shape {} r {} size {:?} rot {:?} td {} i {} enabled {} capture {cap} ({} captures)",
            p.name,
            p.transform.position,
            p.shape,
            p.radius,
            p.size,
            p.transform.rotation,
            p.transition_distance,
            p.intensity,
            p.enabled,
            p.captures.len(),
        );
    }

    // Mean of each face of the diffuse capture (irradiance/π, capture units × 100 = cd/m²)
    // for the probes nearest the point.
    if let Some(lighting) = lighting.lighting_id.as_deref() {
        for p in probes.iter().take(4) {
            let Some(path) = p.capture(lighting).and_then(|c| c.diffuse.clone()) else {
                continue;
            };
            match face_means(&vfs, &path) {
                Ok(m) => println!(
                    "  {}: diffuse ×100 per face (+X −X +Y −Y +Z −Z): {:?}",
                    p.name,
                    m.map(|f| f.map(|v| (v * 1000.0).round() / 10.0))
                ),
                Err(e) => println!("  {}: {e}", p.name),
            }
        }
    }

    // Band 0 of the nearest probes' spherical harmonics (radiance, capture units × 100), and
    // the fog volumes at the point.
    if let Some(lighting) = lighting.lighting_id.as_deref() {
        for p in probes.iter().take(4) {
            if let Some(path) = p.capture(lighting).and_then(|c| c.sh.clone()) {
                let d = vfs.read(&path)?;
                let f: Vec<f32> = d
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| f32::from_le_bytes(*c))
                    .collect();
                println!(
                    "  {}: SH ({} bytes) c00 × 0.282 × 100 = {:?}, coverage {:.2}",
                    p.name,
                    d.len(),
                    [0, 1, 2].map(|c| (f[c] * 28.2095 * 10.0).round() / 10.0),
                    f.get(3).copied().unwrap_or(0.0) * 0.282095
                );
            }
        }
    }
    for v in bg3_world::fog::load_fog_volumes(&vfs, &level, &templates)? {
        let w = v.weight(at);
        if std::env::var_os("FOG_LIST").is_some()
            && v.enabled
            && dist(v.transform.position) < search
        {
            println!(
                "  fog volume {} at {:?} ({:.0} m): shape {} r {} size {:?} density {} albedo {:?} td {} replaces {}",
                v.name,
                v.transform.position,
                dist(v.transform.position),
                v.shape,
                v.radius,
                v.size,
                v.density,
                v.albedo,
                v.transition_distance,
                v.replaces
            );
        }
        if w > 0.0 && v.enabled {
            println!(
                "  fog {}: density {} × weight {w:.2}, albedo {:?}, replaces {}",
                v.name, v.density, v.albedo, v.replaces
            );
        }
    }

    // `METER_PROBE=name`: the metered EV100 (5–95 % trimmed mean of the texels' EV100, as the
    // game's exposure does it) of each face of the probe's specular capture, over the central
    // window a 45° camera with a 1.67 aspect sees.
    if let (Ok(name), Some(lighting)) = (
        std::env::var("METER_PROBE"),
        lighting.lighting_id.as_deref(),
    ) {
        for p in probes.iter().filter(|p| p.name == name) {
            if let Some(path) = p.capture(lighting).and_then(|c| c.specular.clone()) {
                let faces: Vec<String> = (0..6)
                    .map(|f| meter_face(&vfs, &path, f, 0.69, 0.414).map(|v| format!("{v:.2}")))
                    .collect::<Result<_>>()?;
                if let Ok(dir) = std::env::var("METER_DUMP") {
                    for f in 0..6 {
                        dump_face(&vfs, &path, f, &format!("{dir}/{}-{f}.png", p.name))?;
                    }
                }
                println!(
                    "  {} at {:?}: metered EV100 per face (+X −X +Y −Y +Z −Z): {}",
                    p.name,
                    p.transform.position,
                    faces.join(" ")
                );
            }
        }
    }

    println!("\nlights within {search} m whose range reaches the point:");
    let lights = load_level_lights(&vfs, &level, &templates)?;
    let mut near: Vec<_> = lights
        .iter()
        .filter(|l| dist(l.transform.position) < search)
        .collect();
    near.sort_by(|a, b| dist(a.transform.position).total_cmp(&dist(b.transform.position)));
    let mut kinds = [0usize; 3];
    for l in &near {
        let d = dist(l.transform.position);
        let reaches = match l.kind {
            LightKind::Directional => {
                let local = box_local(l, at);
                let [w, h, depth] = l.direction_dimensions;
                local[0].abs() <= 0.5 * w
                    && local[1].abs() <= 0.5 * h
                    && (0.0..=depth).contains(&local[2])
            }
            _ => d < l.radius,
        };
        let i = match l.kind {
            LightKind::Point => 0,
            LightKind::Spot => 1,
            _ => 2,
        };
        if reaches && l.enabled {
            kinds[i] += 1;
        }
        println!(
            "{d:6.1} {} {:?} {:45} {:28} i {:8} r {:5} fill {} gain {} ch {} pre {} en {} shadow {} col {:?} K {} {} dims {:?} att {:?}/{} local {:?} pos {:?} rot {:?} angle {:?} {}",
            if reaches { "*" } else { " " },
            l.kind,
            l.name,
            l.object_type,
            l.intensity,
            l.radius,
            l.flat_falloff,
            l.gain,
            l.light_channel,
            l.pre_expose,
            l.enabled,
            l.shadow,
            l.color,
            l.kelvin,
            l.use_temperature,
            l.direction_dimensions,
            l.direction_attenuation,
            l.direction_attenuation_function,
            box_local(l, at).map(|v| (v * 10.0).round() / 10.0),
            l.transform.position,
            l.transform.rotation,
            l.angle,
            l.module,
        );
    }
    println!(
        "\n{} lights in total; reaching the point: {} point, {} spot, {} box",
        lights.len(),
        kinds[0],
        kinds[1],
        kinds[2]
    );
    Ok(())
}

/// Mean RGB of each face's top mip of a BC6H cubemap.
fn face_means(vfs: &Vfs, path: &str) -> Result<[[f32; 3]; 6]> {
    use bg3_material::dds::{DdsFormat, DdsHeader};
    let data = vfs.read(path)?;
    let h = DdsHeader::parse(&data).ok_or_else(|| anyhow::anyhow!("not a DDS file"))?;
    let signed = h.format == DdsFormat::Bc6hSigned;
    let levels = h.mip_levels().ok_or_else(|| anyhow::anyhow!("layout"))?;
    let face = levels.last().map_or(0, |l| l.2.end) - h.data_offset();
    let top = &levels[0];
    let blocks = top.0.div_ceil(4) as usize * top.1.div_ceil(4) as usize;
    let mut out = [[0.0f32; 3]; 6];
    for (f, o) in out.iter_mut().enumerate() {
        let mut sum = [0.0f64; 3];
        for b in 0..blocks {
            let at = top.2.start + f * face + b * 16;
            let mut texels = [0.0f32; 48];
            bcdec_rs::bc6h_float(&data[at..at + 16], &mut texels, 12, signed);
            for t in texels.chunks(3) {
                for c in 0..3 {
                    sum[c] += f64::from(t[c]);
                }
            }
        }
        let n = (blocks * 16) as f64;
        *o = sum.map(|s| (s / n) as f32);
    }
    Ok(out)
}

/// Trimmed (5–95 %) mean EV100 of a cubemap face's top mip inside |s| < a, |t| < b (face
/// coordinates in −1…1), with texels in units of 100 cd/m².
fn meter_face(vfs: &Vfs, path: &str, face: usize, a: f32, b: f32) -> Result<f32> {
    use bg3_material::dds::{DdsFormat, DdsHeader};
    let data = vfs.read(path)?;
    let h = DdsHeader::parse(&data).ok_or_else(|| anyhow::anyhow!("not a DDS file"))?;
    let signed = h.format == DdsFormat::Bc6hSigned;
    let levels = h.mip_levels().ok_or_else(|| anyhow::anyhow!("layout"))?;
    let face_bytes = levels.last().map_or(0, |l| l.2.end) - h.data_offset();
    let top = &levels[0];
    let (w, hh) = (top.0 as usize, top.1 as usize);
    let bw = w.div_ceil(4);
    let mut evs = Vec::new();
    for by in 0..hh.div_ceil(4) {
        for bx in 0..bw {
            let at = top.2.start + face * face_bytes + (by * bw + bx) * 16;
            let mut texels = [0.0f32; 48];
            bcdec_rs::bc6h_float(&data[at..at + 16], &mut texels, 12, signed);
            for (i, t) in texels.chunks(3).enumerate() {
                let (x, y) = (bx * 4 + i % 4, by * 4 + i / 4);
                let s = (x as f32 + 0.5) / w as f32 * 2.0 - 1.0;
                let tt = (y as f32 + 0.5) / hh as f32 * 2.0 - 1.0;
                if s.abs() > a || tt.abs() > b {
                    continue;
                }
                let l = 0.2126 * t[0] + 0.7152 * t[1] + 0.0722 * t[2];
                evs.push((l * 100.0 * 8.0).max(1e-5).log2().clamp(-8.0, 18.0));
            }
        }
    }
    evs.sort_by(f32::total_cmp);
    let n = evs.len();
    let kept = &evs[n / 20..n - n / 20];
    Ok(kept.iter().sum::<f32>() / kept.len() as f32)
}

/// Write a capture face as an sRGB PNG exposed at EV100 8.5 (`x / (1 + x)` tone curve): a
/// visual check only.
fn dump_face(vfs: &Vfs, path: &str, face: usize, out: &str) -> Result<()> {
    use bg3_material::dds::{DdsFormat, DdsHeader};
    let data = vfs.read(path)?;
    let h = DdsHeader::parse(&data).ok_or_else(|| anyhow::anyhow!("not a DDS file"))?;
    let signed = h.format == DdsFormat::Bc6hSigned;
    let levels = h.mip_levels().ok_or_else(|| anyhow::anyhow!("layout"))?;
    let face_bytes = levels.last().map_or(0, |l| l.2.end) - h.data_offset();
    let top = &levels[0];
    let (w, hh) = (top.0, top.1);
    let bw = w.div_ceil(4) as usize;
    let exposure = 100.0 / (1.2 * 2f32.powf(8.5));
    let mut img = image::RgbImage::new(w, hh);
    for by in 0..hh.div_ceil(4) as usize {
        for bx in 0..bw {
            let at = top.2.start + face * face_bytes + (by * bw + bx) * 16;
            let mut texels = [0.0f32; 48];
            bcdec_rs::bc6h_float(&data[at..at + 16], &mut texels, 12, signed);
            for (i, t) in texels.chunks(3).enumerate() {
                let (x, y) = ((bx * 4 + i % 4) as u32, (by * 4 + i / 4) as u32);
                let px = [0, 1, 2].map(|c| {
                    let v = t[c] * exposure;
                    let v = v / (1.0 + v);
                    let v = if v <= 0.0031308 {
                        12.92 * v
                    } else {
                        1.055 * v.powf(1.0 / 2.4) - 0.055
                    };
                    (v.clamp(0.0, 1.0) * 255.0).round() as u8
                });
                img.put_pixel(x, y, image::Rgb(px));
            }
        }
    }
    img.save(out)?;
    Ok(())
}

/// A point in a light's local frame (rotation only; box lights span ±w/2, ±h/2, z 0..depth).
fn box_local(l: &bg3_world::Light, p: [f32; 3]) -> [f32; 3] {
    let [x, y, z, w] = l.transform.rotation;
    let d = [
        p[0] - l.transform.position[0],
        p[1] - l.transform.position[1],
        p[2] - l.transform.position[2],
    ];
    // Rotate by the conjugate quaternion.
    let q = [-x, -y, -z];
    let t = [
        2.0 * (q[1] * d[2] - q[2] * d[1]),
        2.0 * (q[2] * d[0] - q[0] * d[2]),
        2.0 * (q[0] * d[1] - q[1] * d[0]),
    ];
    [
        d[0] + w * t[0] + (q[1] * t[2] - q[2] * t[1]),
        d[1] + w * t[1] + (q[2] * t[0] - q[0] * t[2]),
        d[2] + w * t[2] + (q[0] * t[1] - q[1] * t[0]),
    ]
}
