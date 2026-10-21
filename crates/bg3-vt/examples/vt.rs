//! Inspect BG3 virtual textures.
//!
//! ```text
//! cargo run --release -p bg3-vt --example vt -- info <gts path>
//! cargo run --release -p bg3-vt --example vt -- tree <gts path> [max depth]
//! cargo run --release -p bg3-vt --example vt -- extract <gtex> <layer> <level> <out.png|out.dds>
//! ```
//!
//! Output files are for local inspection only; write them under `.cache/`.

use std::io::Write;

use anyhow::{Context, Result, bail};
use bg3_vfs::Vfs;
use bg3_vt::{
    FourCcNode, FourCcValue, Image, ImageFormat, LayerConfig, OutputFormat, VirtualTextures,
};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let install = bg3_install::Install::locate()?;
    let vfs = Vfs::from_install(&install)?;
    let vt = VirtualTextures::new(&vfs);
    match args.first().map(String::as_str) {
        Some("info") => {
            let path = args.get(1).context("path")?;
            let ts = vt.tile_set(path)?;
            println!(
                "tile {}x{} border {} page size {:#x}",
                ts.tile_width, ts.tile_height, ts.tile_border, ts.page_size
            );
            println!("layers: {:?}", ts.layers);
            for (i, l) in ts.levels.iter().enumerate() {
                println!("level {i}: {}x{} tiles", l.width, l.height);
            }
            println!(
                "{} parameter blocks, {} page files, {} textures, {} packed tiles, {} flat tiles",
                ts.parameter_blocks.len(),
                ts.page_files.len(),
                ts.textures.len(),
                ts.packed_tiles.len(),
                ts.flat_tiles.len()
            );
            for t in ts.textures.iter().take(5) {
                println!("  {t:?}");
            }
        }
        Some("entries") => {
            // Statistics of level-index entries: (stored/fallback, level delta, referenced codec).
            let path = args.get(1).context("path")?;
            let ts = vt.tile_set(path)?;
            let pages = vt.pages(path);
            let mut stats: std::collections::BTreeMap<String, usize> = Default::default();
            let mut codec_of: std::collections::HashMap<u32, String> = Default::default();
            for (li, lv) in ts.levels.iter().enumerate() {
                for y in 0..lv.height {
                    for x in 0..lv.width {
                        for layer in 0..ts.layers.len() {
                            let (kind, f) = match ts.tile(li, layer, x, y) {
                                Some(bg3_vt::TileRef::Stored(f)) => ("stored", f),
                                Some(bg3_vt::TileRef::Fallback(f)) => ("fallback", f),
                                None => continue,
                            };
                            let p = ts.packed_tiles[ts.flat_tiles[f as usize].packed_tile as usize];
                            let codec = codec_of
                                .entry(f)
                                .or_insert_with(|| match ts.read_tile(&pages, f) {
                                    Ok(bg3_vt::TileData::Uniform(_)) => "uniform".into(),
                                    Ok(bg3_vt::TileData::Bc { .. }) => "bc".into(),
                                    Err(e) => format!("err {e}"),
                                })
                                .clone();
                            let same_pos = (p.x(), p.y())
                                == (
                                    x >> (p.level() as usize).saturating_sub(li),
                                    y >> (p.level() as usize).saturating_sub(li),
                                );
                            let key = format!(
                                "{kind} layer {layer} dlevel {} same_layer {} covers {} -> {codec}",
                                p.level() as i64 - li as i64,
                                p.layer() as usize == layer,
                                same_pos && p.level() as usize >= li
                            );
                            *stats.entry(key).or_default() += 1;
                        }
                    }
                }
            }
            for (k, v) in stats {
                println!("{v:>9} {k}");
            }
        }
        Some("tree") => {
            let path = args.get(1).context("path")?;
            let depth = args.get(2).map(|d| d.parse()).transpose()?.unwrap_or(3);
            let ts = vt.tile_set(path)?;
            for n in &ts.fourcc {
                print_tree(n, 0, depth);
            }
        }
        Some("extract") => {
            let gtex = args.get(1).context("gtex")?;
            let layer: usize = args.get(2).context("layer")?.parse()?;
            let level: usize = args.get(3).context("level")?.parse()?;
            let out = args.get(4).context("output")?;
            let loc = vt.locate_gtex(gtex, LayerConfig::AlbedoNormalPhysical)?;
            println!("{} in {}", gtex, loc.tile_set_path);
            println!("{:?}", loc.texture);
            let format = if out.ends_with(".dds") {
                OutputFormat::Bc
            } else {
                OutputFormat::Rgba8
            };
            let img = loc.tile_set.extract(
                &vt.pages(&loc.tile_set_path),
                &loc.texture,
                layer,
                level,
                format,
            )?;
            println!("{}x{} {:?}", img.width, img.height, img.format);
            if out.ends_with(".dds") {
                write_dds(out, &img)?;
            } else {
                write_png(out, img.width, img.height, &img.to_rgba8())?;
            }
        }
        _ => bail!("usage: vt info|tree|extract …"),
    }
    Ok(())
}

fn print_tree(n: &FourCcNode, indent: usize, max: usize) {
    let pad = "  ".repeat(indent);
    match &n.value {
        FourCcValue::Node(c) => {
            println!("{pad}{} ({} children)", n.tag(), c.len());
            if indent < max {
                for c in c.iter().take(8) {
                    print_tree(c, indent + 1, max);
                }
            }
        }
        FourCcValue::String(s) => println!("{pad}{} = {s:?}", n.tag()),
        FourCcValue::U32(v) => println!("{pad}{} = {v}", n.tag()),
        FourCcValue::Binary(b) | FourCcValue::Guid(b) => {
            println!("{pad}{} = <{} bytes>", n.tag(), b.len())
        }
        FourCcValue::Other(k, b) => println!("{pad}{} = kind {k} <{} bytes>", n.tag(), b.len()),
    }
}

fn write_png(path: &str, w: u32, h: u32, rgba: &[u8]) -> Result<()> {
    let mut raw = Vec::with_capacity((w as usize * 4 + 1) * h as usize);
    for row in rgba.chunks_exact(w as usize * 4) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    z.write_all(&raw)?;
    let idat = z.finish()?;
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    f.write_all(b"\x89PNG\r\n\x1a\n")?;
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    for (ty, data) in [
        (b"IHDR", &ihdr[..]),
        (b"IDAT", &idat[..]),
        (b"IEND", &[][..]),
    ] {
        f.write_all(&(data.len() as u32).to_be_bytes())?;
        let mut crc = flate2::Crc::new();
        crc.update(ty);
        crc.update(data);
        f.write_all(ty)?;
        f.write_all(data)?;
        f.write_all(&crc.sum().to_be_bytes())?;
    }
    Ok(())
}

fn write_dds(path: &str, img: &Image) -> Result<()> {
    let ImageFormat::Bc(f) = img.format else {
        bail!("DDS output needs BC data");
    };
    let fourcc: &[u8; 4] = match f {
        bg3_vt::BcFormat::Bc1 => b"DXT1",
        bg3_vt::BcFormat::Bc3 => b"DXT5",
        bg3_vt::BcFormat::Bc4 => b"ATI1",
        bg3_vt::BcFormat::Bc5 => b"ATI2",
    };
    let mut h = vec![0u8; 128];
    let put = |h: &mut Vec<u8>, o: usize, v: u32| h[o..o + 4].copy_from_slice(&v.to_le_bytes());
    h[0..4].copy_from_slice(b"DDS ");
    put(&mut h, 4, 124);
    put(&mut h, 8, 0x1 | 0x2 | 0x4 | 0x1000 | 0x80000);
    put(&mut h, 12, img.height);
    put(&mut h, 16, img.width);
    put(&mut h, 20, img.data.len() as u32);
    put(&mut h, 28, 1);
    put(&mut h, 76, 32);
    put(&mut h, 80, 0x4);
    h[84..88].copy_from_slice(fourcc);
    put(&mut h, 108, 0x1000);
    let mut f = std::fs::File::create(path)?;
    f.write_all(&h)?;
    f.write_all(&img.data)?;
    Ok(())
}
