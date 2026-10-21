//! Find the game version string embedded in an executable of any format.
//!
//! The game binaries contain the full version as a standalone NUL-terminated ASCII string
//! (e.g. `4.1.1.7631656`); in the macOS reference binary it is the only string of that shape.
//! Used for the native Linux build (ELF has no version resource) and as a fallback for Windows.

/// The single distinct `major.minor.patch.build` string in `bytes`, if exactly one exists.
pub fn find_embedded_version(bytes: &[u8]) -> Option<String> {
    let mut found: Option<&[u8]> = None;
    for run in bytes.split(|&b| b == 0) {
        if !looks_like_version(run) {
            continue;
        }
        match found {
            None => found = Some(run),
            Some(prev) if prev == run => {}
            Some(_) => return None, // ambiguous
        }
    }
    found.map(|v| String::from_utf8_lossy(v).into_owned())
}

/// `a.b.c.d` where a, b, c are 1–3 digits and d (the build number) is 5–9 digits.
fn looks_like_version(s: &[u8]) -> bool {
    let parts: Vec<&[u8]> = s.split(|&b| b == b'.').collect();
    let [a, b, c, d] = parts[..] else {
        return false;
    };
    let digits =
        |p: &[u8], min, max| (min..=max).contains(&p.len()) && p.iter().all(u8::is_ascii_digit);
    digits(a, 1, 3) && digits(b, 1, 3) && digits(c, 1, 3) && digits(d, 5, 9)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_single_version() {
        let bin = b"\x7fELF\0junk\x001.2.3\x004.1.1.7631656\0other 4.1.1.7631656 text\0";
        assert_eq!(find_embedded_version(bin).as_deref(), Some("4.1.1.7631656"));
    }

    #[test]
    fn repeated_identical_version_is_fine() {
        let bin = b"\x004.2.0.1234567\0x\x004.2.0.1234567\0";
        assert_eq!(find_embedded_version(bin).as_deref(), Some("4.2.0.1234567"));
    }

    #[test]
    fn ambiguous_or_missing_gives_none() {
        assert_eq!(
            find_embedded_version(b"\x004.1.1.11111\x005.0.0.22222\0"),
            None
        );
        assert_eq!(
            find_embedded_version(b"\x001.2.3.4\x00v4.1.1.7631656\0"),
            None
        );
    }
}
