//! Read the game version from a Windows executable's version resource.
//!
//! The full BG3 version (`4.1.1.7631656`) does not fit `VS_FIXEDFILEINFO`'s four 16-bit fields,
//! so we read the UTF-16 `StringFileInfo` value instead (`ProductVersion`, then `FileVersion`).
//! Rather than walking the resource directory tree we locate the `.rsrc` section via the PE
//! section table and scan it for the UTF-16 key followed by its value. That is robust against
//! resource-tree details and needs no Windows APIs, so it works on every host OS.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Bytes read from the start of the file to find the PE section table.
const HEADER_LEN: usize = 64 * 1024;

/// Read the version string from a PE file on disk. `Ok(None)` if the file has no recognisable
/// version string.
pub fn read_exe_version(path: &Path) -> std::io::Result<Option<String>> {
    let mut file = std::fs::File::open(path)?;
    let mut header = Vec::with_capacity(HEADER_LEN);
    file.by_ref()
        .take(HEADER_LEN as u64)
        .read_to_end(&mut header)?;
    let bytes = match rsrc_section(&header) {
        Some((offset, len)) => {
            let mut buf = Vec::new();
            file.seek(SeekFrom::Start(offset))?;
            file.take(len).read_to_end(&mut buf)?;
            buf
        }
        // Not a PE file we understand: fall back to scanning everything.
        None => std::fs::read(path)?,
    };
    Ok(find_version_string(&bytes))
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// File offset and size of the `.rsrc` section, parsed from the first bytes of a PE image.
pub fn rsrc_section(header: &[u8]) -> Option<(u64, u64)> {
    if header.get(..2)? != b"MZ" {
        return None;
    }
    let pe = u32_at(header, 0x3c)? as usize;
    if header.get(pe..pe + 4)? != b"PE\0\0" {
        return None;
    }
    let coff = pe + 4;
    let sections = u16_at(header, coff + 2)? as usize;
    let optional_len = u16_at(header, coff + 16)? as usize;
    let table = coff + 20 + optional_len;
    (0..sections).find_map(|i| {
        let s = table + i * 40;
        let name = header.get(s..s + 8)?;
        if name != b".rsrc\0\0\0" {
            return None;
        }
        let size = u32_at(header, s + 16)?;
        let offset = u32_at(header, s + 20)?;
        Some((u64::from(offset), u64::from(size)))
    })
}

/// Find the version string in raw bytes containing a `VS_VERSIONINFO` resource.
///
/// Prefers `ProductVersion` over `FileVersion`. Values written with commas (`4, 1, 1, 7631656`)
/// are normalised to dots.
pub fn find_version_string(bytes: &[u8]) -> Option<String> {
    ["ProductVersion", "FileVersion"]
        .iter()
        .find_map(|key| find_utf16_value(bytes, key))
}

fn utf16le(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

/// Scan for every occurrence of `key\0` (UTF-16LE) and return the first plausible version value
/// that follows it.
fn find_utf16_value(bytes: &[u8], key: &str) -> Option<String> {
    let mut pattern = utf16le(key);
    pattern.extend_from_slice(&[0, 0]);
    let mut from = 0;
    while let Some(i) = bytes
        .get(from..)?
        .windows(pattern.len())
        .position(|w| w == pattern.as_slice())
    {
        let key_at = from + i;
        from = key_at + 1;
        // A `String` struct is `wLength, wValueLength, wType, szKey, padding, Value`; when the
        // header is present, an empty value means there's nothing to read here.
        if key_at >= 6 && u16_at(bytes, key_at - 4) == Some(0) {
            continue;
        }
        let mut at = key_at + pattern.len();
        // Padding to a 32-bit boundary: at most one zero code unit.
        if u16_at(bytes, at) == Some(0) {
            at += 2;
        }
        let mut units = Vec::new();
        while let Some(u) = u16_at(bytes, at) {
            if u == 0 || units.len() >= 64 {
                break;
            }
            units.push(u);
            at += 2;
        }
        if let Some(v) = normalize_version(&String::from_utf16_lossy(&units)) {
            return Some(v);
        }
    }
    None
}

/// Accept `4.1.1.7631656`, `4, 1, 1, 7631656` or `4.1.1.7631656 (anything)`; reject non-versions.
fn normalize_version(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let candidate = if raw.contains(',') {
        raw.split(',').map(str::trim).collect::<Vec<_>>().join(".")
    } else {
        raw.split_whitespace().next()?.to_owned()
    };
    let ok = candidate.starts_with(|c: char| c.is_ascii_digit())
        && candidate.ends_with(|c: char| c.is_ascii_digit())
        && candidate.chars().all(|c| c.is_ascii_digit() || c == '.')
        && candidate.contains('.');
    ok.then_some(candidate)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// One `String` struct of a `StringTable`, laid out like the resource compiler does.
    fn string_struct(key: &str, value: &str) -> Vec<u8> {
        let key_bytes = utf16le(key);
        let value_units = value.encode_utf16().count() + 1;
        let mut out = vec![0u8; 6];
        out.extend_from_slice(&key_bytes);
        out.extend_from_slice(&[0, 0]);
        while !out.len().is_multiple_of(4) {
            out.push(0);
        }
        out.extend_from_slice(&utf16le(value));
        out.extend_from_slice(&[0, 0]);
        let len = out.len() as u16;
        out[0..2].copy_from_slice(&len.to_le_bytes());
        out[2..4].copy_from_slice(&(value_units as u16).to_le_bytes());
        out[4..6].copy_from_slice(&1u16.to_le_bytes());
        while !out.len().is_multiple_of(4) {
            out.push(0);
        }
        out
    }

    /// A synthetic version resource with the given string pairs, surrounded by noise.
    pub(crate) fn version_resource(pairs: &[(&str, &str)]) -> Vec<u8> {
        let mut out = b"noise\x01\x02\x03".to_vec();
        out.extend_from_slice(&utf16le("VS_VERSION_INFO"));
        out.extend_from_slice(&[0, 0, 0, 0]);
        for (k, v) in pairs {
            out.extend(string_struct(k, v));
        }
        out.extend_from_slice(b"trailing");
        out
    }

    /// A minimal PE image whose only section is `.rsrc`, containing `rsrc`.
    pub(crate) fn pe_image(rsrc: &[u8]) -> Vec<u8> {
        let mut img = vec![0u8; 0x200];
        img[0..2].copy_from_slice(b"MZ");
        let pe = 0x80usize;
        img[0x3c..0x40].copy_from_slice(&(pe as u32).to_le_bytes());
        img[pe..pe + 4].copy_from_slice(b"PE\0\0");
        let coff = pe + 4;
        img[coff + 2..coff + 4].copy_from_slice(&2u16.to_le_bytes()); // sections
        let opt_len = 0xF0u16;
        img[coff + 16..coff + 18].copy_from_slice(&opt_len.to_le_bytes());
        let table = coff + 20 + opt_len as usize;
        // Section 0: .text (decoy containing a bogus version string).
        img[table..table + 8].copy_from_slice(b".text\0\0\0");
        // Section 1: .rsrc
        let s = table + 40;
        img[s..s + 8].copy_from_slice(b".rsrc\0\0\0");
        let rsrc_at = img.len() as u32 + 0x40;
        img[s + 16..s + 20].copy_from_slice(&(rsrc.len() as u32).to_le_bytes());
        img[s + 20..s + 24].copy_from_slice(&rsrc_at.to_le_bytes());
        // Decoy outside .rsrc.
        img.extend(std::iter::repeat_n(0u8, 0x40));
        assert_eq!(img.len() as u32, rsrc_at);
        img.extend_from_slice(rsrc);
        img
    }

    #[test]
    fn reads_product_version() {
        let res = version_resource(&[
            ("CompanyName", "Larian Studios"),
            ("FileVersion", "4.1.1.1"),
            ("ProductVersion", "4.1.1.7631656"),
        ]);
        assert_eq!(find_version_string(&res).as_deref(), Some("4.1.1.7631656"));
    }

    #[test]
    fn falls_back_to_file_version() {
        let res = version_resource(&[("FileVersion", "4.1.1.7631656")]);
        assert_eq!(find_version_string(&res).as_deref(), Some("4.1.1.7631656"));
    }

    #[test]
    fn normalises_comma_separated_versions() {
        let res = version_resource(&[("ProductVersion", "4, 1, 1, 7631656")]);
        assert_eq!(find_version_string(&res).as_deref(), Some("4.1.1.7631656"));
    }

    #[test]
    fn skips_empty_and_non_version_values() {
        let res = version_resource(&[
            ("ProductVersion", ""),
            ("ProductVersion", "Baldur's Gate 3"),
            ("FileVersion", "4.1.1.42"),
        ]);
        assert_eq!(find_version_string(&res).as_deref(), Some("4.1.1.42"));
    }

    #[test]
    fn handles_odd_offsets() {
        let mut res = vec![0xAB];
        res.extend(version_resource(&[("ProductVersion", "1.2.3.4")]));
        assert_eq!(find_version_string(&res).as_deref(), Some("1.2.3.4"));
    }

    #[test]
    fn no_version_resource() {
        assert_eq!(find_version_string(b"nothing here"), None);
        assert_eq!(find_version_string(&utf16le("ProductVersion")), None);
    }

    #[test]
    fn finds_rsrc_section() {
        let rsrc = version_resource(&[("ProductVersion", "4.1.1.7631656")]);
        let img = pe_image(&rsrc);
        let (offset, len) = rsrc_section(&img).unwrap();
        assert_eq!(
            &img[offset as usize..(offset + len) as usize],
            rsrc.as_slice()
        );
        assert_eq!(rsrc_section(b"MZ"), None);
        assert_eq!(rsrc_section(b"not a pe file at all"), None);
    }

    #[test]
    fn reads_version_from_exe_on_disk() {
        let dir = crate::test_util::TempDir::new("pe");
        let rsrc = version_resource(&[("ProductVersion", "4.1.1.7631656")]);
        let exe = dir.path().join("bg3.exe");
        std::fs::write(&exe, pe_image(&rsrc)).unwrap();
        assert_eq!(
            read_exe_version(&exe).unwrap().as_deref(),
            Some("4.1.1.7631656")
        );
        // Not a PE: whole-file scan.
        let raw = dir.path().join("raw.bin");
        std::fs::write(&raw, &rsrc).unwrap();
        assert_eq!(
            read_exe_version(&raw).unwrap().as_deref(),
            Some("4.1.1.7631656")
        );
    }
}
