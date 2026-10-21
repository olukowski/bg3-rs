//! `bg3x metallib`: list or extract functions of the macOS build's Metal shader libraries
//! (`Data/Shaders/Metal/*.metallib`). The extracted bitcode (AIR) is LLVM IR that `llvm-dis`
//! (rustup component `llvm-tools`) prints; see `docs/research/character-shaders.md`.
//! Output is compiled game content: keep it in `.cache/`.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use bg3_install::Install;

/// One function entry of a Metal library.
struct Entry {
    name: String,
    /// `0` vertex, `1` fragment, … (the `TYPE` tag).
    kind: u8,
    bitcode: std::ops::Range<usize>,
}

fn u16_at(d: &[u8], o: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(
        d.get(o..o + 2).context("truncated")?.try_into()?,
    ))
}
fn u32_at(d: &[u8], o: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        d.get(o..o + 4).context("truncated")?.try_into()?,
    ))
}
fn u64_at(d: &[u8], o: usize) -> Result<usize> {
    Ok(u64::from_le_bytes(d.get(o..o + 8).context("truncated")?.try_into()?) as usize)
}

/// Parse the function list: header `MTLB`, function list offset at 0x18, bitcode section
/// offset at 0x48; each entry is a length-prefixed list of 4-byte tags with u16 sizes
/// (`NAME`, `TYPE`, `MDSZ` bitcode size, `OFFT` three offsets, …) ending in `ENDT`.
fn entries(d: &[u8]) -> Result<Vec<Entry>> {
    if d.get(..4) != Some(b"MTLB") {
        bail!("not a Metal library");
    }
    let list = u64_at(d, 0x18)?;
    let bitcode = u64_at(d, 0x48)?;
    let count = u32_at(d, list)? as usize;
    let mut p = list + 4;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let len = u32_at(d, p)? as usize;
        let mut q = p + 4;
        let (mut name, mut kind, mut size, mut offset) = (String::new(), 0, 0, 0);
        loop {
            let tag = d.get(q..q + 4).context("truncated")?;
            q += 4;
            if tag == b"ENDT" {
                break;
            }
            let n = u16_at(d, q)? as usize;
            q += 2;
            let v = d.get(q..q + n).context("truncated")?;
            match tag {
                b"NAME" => name = String::from_utf8_lossy(v).trim_end_matches('\0').to_owned(),
                b"TYPE" => kind = v.first().copied().unwrap_or(0),
                b"MDSZ" => size = u64_at(v, 0)?,
                b"OFFT" => offset = u64_at(v, 16)?,
                _ => {}
            }
            q += n;
        }
        out.push(Entry {
            name,
            kind,
            bitcode: bitcode + offset..bitcode + offset + size,
        });
        p += len;
    }
    Ok(out)
}

fn library(name: &str) -> Result<Vec<u8>> {
    let install = Install::locate()?;
    let path = install.data_dir().join("Shaders/Metal").join(name);
    std::fs::read(&path).with_context(|| format!("{}", path.display()))
}

/// `bg3x metallib list [-f substr]` / `bg3x metallib extract NAME OUT`.
pub fn run(lib: &str, filter: Option<&str>, extract: Option<(&str, PathBuf)>) -> Result<()> {
    let data = library(lib)?;
    let entries = entries(&data)?;
    match extract {
        Some((name, out)) => {
            let e = entries
                .iter()
                .find(|e| e.name == name)
                .with_context(|| format!("no function {name}"))?;
            let bytes = data
                .get(e.bitcode.clone())
                .context("bitcode out of range")?;
            std::fs::write(&out, bytes)?;
            eprintln!("{} bytes → {}", bytes.len(), out.display());
        }
        None => {
            for e in entries
                .iter()
                .filter(|e| filter.is_none_or(|f| e.name.contains(f)))
            {
                let kind = match e.kind {
                    0 => "vertex",
                    1 => "fragment",
                    2 => "kernel",
                    _ => "other",
                };
                println!("{:8} {:8} {}", kind, e.bitcode.len(), e.name);
            }
        }
    }
    Ok(())
}
