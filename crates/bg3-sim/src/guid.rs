//! 128-bit identifiers as the game and the story use them.
//!
//! Resources (flags, tags, templates, level objects) are keyed by a UUID. The story refers to
//! them with a GUIDSTRING, usually `Name_<uuid>` (`S_Player_ShadowHeart_3ed7…`), sometimes the
//! bare UUID; the engine only looks at the UUID in the last 36 characters, like Osiris itself
//! (see `bg3_osiris::parse_uuid_suffix`).

use std::fmt;

/// A UUID. The all-zero value is the null id (`NULL_00000000-0000-0000-0000-000000000000`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Guid(pub u128);

impl Guid {
    pub const NULL: Self = Self(0);

    /// Parse a bare UUID (`8-4-4-4-12` hex digits, either case).
    pub fn parse(s: &str) -> Option<Self> {
        (s.len() == 36).then(|| Self::from_osiris(s)).flatten()
    }

    /// The UUID in the last 36 characters of a GUIDSTRING (`Name_<uuid>` or a bare UUID).
    /// `None` if the suffix is not a UUID.
    pub fn from_osiris(s: &str) -> Option<Self> {
        bg3_osiris::parse_uuid_suffix(s).map(Self)
    }

    pub fn is_null(self) -> bool {
        self.0 == 0
    }
}

impl fmt::Display for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let v = self.0;
        write!(
            f,
            "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
            (v >> 96) as u32,
            (v >> 80) as u16,
            (v >> 64) as u16,
            (v >> 48) as u16,
            v & 0xffff_ffff_ffff
        )
    }
}

impl fmt::Debug for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// `Name_<uuid>`, the way the engine renders a named object for the story; the bare UUID if
/// the name is empty.
pub fn osiris_name(name: &str, id: Guid) -> String {
    if name.is_empty() {
        id.to_string()
    } else {
        format!("{name}_{id}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_display_round_trip() {
        let s = "3ed74f06-3c60-42dc-83f6-f034cb47c679";
        let g = Guid::parse(s).unwrap();
        assert_eq!(g.to_string(), s);
        assert_eq!(Guid::from_osiris(&format!("S_Foo_{s}")), Some(g));
        assert_eq!(Guid::parse("3ED74F06-3C60-42DC-83F6-F034CB47C679"), Some(g));
        assert_eq!(Guid::parse("S_Foo"), None);
        assert!(
            Guid::from_osiris("NULL_00000000-0000-0000-0000-000000000000")
                .unwrap()
                .is_null()
        );
        assert_eq!(osiris_name("S_Foo", g), format!("S_Foo_{s}"));
        assert_eq!(osiris_name("", g), s);
    }
}
