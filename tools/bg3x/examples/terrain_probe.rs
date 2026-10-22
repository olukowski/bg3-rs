//! Print the terrain cells around a world point (BG3 coordinates): which terrain covers it,
//! the cell's height, whether it's a hole, and each layer's weight with its brush name.
//!
//! Run: `cargo run --release -p bg3x --example terrain_probe -- TUT_Avernus_C -46.6,2.8,-402.8 [RADIUS]`
//! (game data: keep the output local).

use anyhow::{Context, Result};
use bg3_install::Install;
use bg3_vfs::Vfs;
use bg3_world::terrain::{load_level_terrains, load_terrain_brushes};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let level = args.next().unwrap_or("TUT_Avernus_C".into());
    let p: Vec<f32> = args
        .next()
        .context("point x,y,z")?
        .split(',')
        .map(|v| v.parse())
        .collect::<Result<_, _>>()?;
    let radius: i64 = args.next().map_or(Ok(2), |r| r.parse())?;
    let vfs = Vfs::from_install(&Install::locate()?)?;
    let brushes = load_terrain_brushes(&vfs)?;
    for t in load_level_terrains(&vfs, &level)? {
        // Unrotated terrains only (all of the Nautiloid's): local = world - position.
        let [ox, oy, oz] = t.transform.position;
        let (lx, lz) = (p[0] - ox, p[2] - oz);
        let (cx, cz) = (lx.floor() as i64, lz.floor() as i64);
        let [cw, ch] = t.cells.map(|c| c as i64);
        if cx < -radius || cz < -radius || cx >= cw + radius || cz >= ch + radius {
            continue;
        }
        println!(
            "{} ({}×{} cells) at local ({lx:.2}, {lz:.2})",
            t.name, cw, ch
        );
        let names: Vec<&str> = t
            .layers
            .iter()
            .map(|l| brushes.get(&l.brush).map_or("?", |b| b.name.as_str()))
            .collect();
        println!("  layers: {names:?}");
        for z in (cz - radius).max(0)..=(cz + radius).min(ch - 1) {
            for x in (cx - radius).max(0)..=(cx + radius).min(cw - 1) {
                let (xu, zu) = (x as usize, z as usize);
                let w: Vec<u8> = t.weights.iter().map(|l| l[zu * t.cells[0] + xu]).collect();
                println!(
                    "  cell ({x}, {z}) y {:.2} hole {} weights {w:?}",
                    oy + t.height(xu, zu),
                    t.is_hole(xu, zu)
                );
            }
        }
    }
    Ok(())
}
