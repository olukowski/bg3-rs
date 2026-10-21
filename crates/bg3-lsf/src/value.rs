//! Typed attribute values shared by the LSF, LSX and LSJ readers.

use std::fmt;

use crate::{Error, Result};

macro_rules! attribute_types {
    ($($variant:ident = $id:literal, $name:literal;)*) => {
        /// Attribute type ids as stored in LSF (low 6 bits of the type/length word) and their
        /// LSX/LSJ type names.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[repr(u8)]
        pub enum AttributeType {
            $($variant = $id,)*
        }

        impl AttributeType {
            /// All known types, in id order.
            pub const ALL: &[AttributeType] = &[$(AttributeType::$variant,)*];

            pub fn from_id(id: u32) -> Option<Self> {
                match id {
                    $($id => Some(Self::$variant),)*
                    _ => None,
                }
            }

            /// The `type="…"` name used by LSX/LSJ.
            pub fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $name,)*
                }
            }

            /// Accepts the type name, or a numeric id (old LSX/LSJ files use those).
            pub fn from_name(name: &str) -> Option<Self> {
                match name {
                    $($name => Some(Self::$variant),)*
                    _ => name.parse().ok().and_then(Self::from_id),
                }
            }
        }
    };
}

attribute_types! {
    None = 0, "None";
    Byte = 1, "uint8";
    Short = 2, "int16";
    UShort = 3, "uint16";
    Int = 4, "int32";
    UInt = 5, "uint32";
    Float = 6, "float";
    Double = 7, "double";
    IVec2 = 8, "ivec2";
    IVec3 = 9, "ivec3";
    IVec4 = 10, "ivec4";
    Vec2 = 11, "fvec2";
    Vec3 = 12, "fvec3";
    Vec4 = 13, "fvec4";
    Mat2 = 14, "mat2x2";
    Mat3 = 15, "mat3x3";
    Mat3x4 = 16, "mat3x4";
    Mat4x3 = 17, "mat4x3";
    Mat4 = 18, "mat4x4";
    Bool = 19, "bool";
    String = 20, "string";
    Path = 21, "path";
    FixedString = 22, "FixedString";
    LSString = 23, "LSString";
    ULongLong = 24, "uint64";
    ScratchBuffer = 25, "ScratchBuffer";
    OldInt64 = 26, "old_int64";
    Int8 = 27, "int8";
    TranslatedString = 28, "TranslatedString";
    WString = 29, "WString";
    LSWString = 30, "LSWString";
    Uuid = 31, "guid";
    Int64 = 32, "int64";
    TranslatedFSString = 33, "TranslatedFSString";
}

impl AttributeType {
    pub fn id(self) -> u32 {
        self as u32
    }

    /// Number of scalar components and whether they are floats, for vector/matrix types.
    pub fn components(self) -> Option<(usize, bool)> {
        Some(match self {
            Self::IVec2 => (2, false),
            Self::IVec3 => (3, false),
            Self::IVec4 => (4, false),
            Self::Vec2 => (2, true),
            Self::Vec3 => (3, true),
            Self::Vec4 => (4, true),
            Self::Mat2 => (4, true),
            Self::Mat3 => (9, true),
            Self::Mat3x4 | Self::Mat4x3 => (12, true),
            Self::Mat4 => (16, true),
            _ => return None,
        })
    }

    /// Row count and row width used by the Larian LSX element form (`<mat4><float4 …/>…`).
    pub(crate) fn rows(self) -> (usize, usize) {
        match self {
            Self::Mat2 => (2, 2),
            Self::Mat3 => (3, 3),
            // Not observed in the reference install; assumed row-major like the square ones.
            Self::Mat3x4 => (4, 3),
            Self::Mat4x3 => (3, 4),
            Self::Mat4 => (4, 4),
            _ => match self.components() {
                Some((n, _)) => (1, n),
                None => (0, 0),
            },
        }
    }
}

impl fmt::Display for AttributeType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A localised string reference: a handle into the localisation tables plus a version.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TranslatedString {
    /// Inline text. Only present in pre-BG3 LSF and in some LSX/LSJ files.
    pub value: Option<String>,
    pub handle: String,
    pub version: u16,
}

/// A translated string with named arguments, each of which is itself a [`TranslatedFSString`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TranslatedFSString {
    pub string: TranslatedString,
    pub arguments: Vec<FsArgument>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FsArgument {
    pub key: String,
    pub value: String,
    pub string: TranslatedFSString,
}

/// A 128-bit id, stored in the byte order of its canonical text form
/// (`00112233-4455-6677-8899-aabbccddeeff` → `[0x00, 0x11, …]`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
pub struct Guid(pub [u8; 16]);

impl Guid {
    /// Converts from the 16 bytes stored in LSF: a little-endian u32, two little-endian u16s and
    /// then four little-endian u16s (the last part is "byte-swapped" relative to a .NET Guid).
    pub fn from_lsf_bytes(raw: [u8; 16]) -> Self {
        let mut t = [0u8; 16];
        t[0..4].copy_from_slice(&[raw[3], raw[2], raw[1], raw[0]]);
        t[4..6].copy_from_slice(&[raw[5], raw[4]]);
        for i in (6..16).step_by(2) {
            t[i] = raw[i + 1];
            t[i + 1] = raw[i];
        }
        Guid(t)
    }

    /// Inverse of [`Guid::from_lsf_bytes`].
    pub fn to_lsf_bytes(self) -> [u8; 16] {
        let t = self.0;
        let mut raw = [0u8; 16];
        raw[0..4].copy_from_slice(&[t[3], t[2], t[1], t[0]]);
        raw[4..6].copy_from_slice(&[t[5], t[4]]);
        for i in (6..16).step_by(2) {
            raw[i] = t[i + 1];
            raw[i + 1] = t[i];
        }
        raw
    }

    /// Parses the hyphenated form (hyphens are optional, case-insensitive).
    pub fn parse(s: &str) -> Option<Self> {
        let mut out = [0u8; 16];
        let mut nibbles = s.bytes().filter(|&b| b != b'-');
        for byte in &mut out {
            let hi = (nibbles.next()? as char).to_digit(16)?;
            let lo = (nibbles.next()? as char).to_digit(16)?;
            *byte = (hi << 4 | lo) as u8;
        }
        nibbles.next().is_none().then_some(Guid(out))
    }
}

impl fmt::Display for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, b) in self.0.iter().enumerate() {
            if matches!(i, 4 | 6 | 8 | 10) {
                f.write_str("-")?;
            }
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Guid({self})")
    }
}

/// A typed attribute value. Variant names follow the LSF type ids ([`AttributeType`]).
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    None,
    Byte(u8),
    Short(i16),
    UShort(u16),
    Int(i32),
    UInt(u32),
    Float(f32),
    Double(f64),
    IVec2([i32; 2]),
    IVec3([i32; 3]),
    IVec4([i32; 4]),
    Vec2([f32; 2]),
    Vec3([f32; 3]),
    Vec4([f32; 4]),
    /// Matrices are kept as a flat list of floats in stored order (row-major as written in LSX).
    Mat2([f32; 4]),
    Mat3([f32; 9]),
    Mat3x4([f32; 12]),
    Mat4x3([f32; 12]),
    Mat4([f32; 16]),
    Bool(bool),
    String(String),
    Path(String),
    FixedString(String),
    LSString(String),
    ULongLong(u64),
    ScratchBuffer(Vec<u8>),
    OldInt64(i64),
    Int8(i8),
    TranslatedString(TranslatedString),
    WString(String),
    LSWString(String),
    Uuid(Guid),
    Int64(i64),
    TranslatedFSString(TranslatedFSString),
}

impl Value {
    pub fn ty(&self) -> AttributeType {
        use AttributeType as T;
        match self {
            Value::None => T::None,
            Value::Byte(_) => T::Byte,
            Value::Short(_) => T::Short,
            Value::UShort(_) => T::UShort,
            Value::Int(_) => T::Int,
            Value::UInt(_) => T::UInt,
            Value::Float(_) => T::Float,
            Value::Double(_) => T::Double,
            Value::IVec2(_) => T::IVec2,
            Value::IVec3(_) => T::IVec3,
            Value::IVec4(_) => T::IVec4,
            Value::Vec2(_) => T::Vec2,
            Value::Vec3(_) => T::Vec3,
            Value::Vec4(_) => T::Vec4,
            Value::Mat2(_) => T::Mat2,
            Value::Mat3(_) => T::Mat3,
            Value::Mat3x4(_) => T::Mat3x4,
            Value::Mat4x3(_) => T::Mat4x3,
            Value::Mat4(_) => T::Mat4,
            Value::Bool(_) => T::Bool,
            Value::String(_) => T::String,
            Value::Path(_) => T::Path,
            Value::FixedString(_) => T::FixedString,
            Value::LSString(_) => T::LSString,
            Value::ULongLong(_) => T::ULongLong,
            Value::ScratchBuffer(_) => T::ScratchBuffer,
            Value::OldInt64(_) => T::OldInt64,
            Value::Int8(_) => T::Int8,
            Value::TranslatedString(_) => T::TranslatedString,
            Value::WString(_) => T::WString,
            Value::LSWString(_) => T::LSWString,
            Value::Uuid(_) => T::Uuid,
            Value::Int64(_) => T::Int64,
            Value::TranslatedFSString(_) => T::TranslatedFSString,
        }
    }

    /// The text of any string-like value (including the handle of translated strings).
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s)
            | Value::Path(s)
            | Value::FixedString(s)
            | Value::LSString(s)
            | Value::WString(s)
            | Value::LSWString(s) => Some(s),
            Value::TranslatedString(t) => Some(&t.handle),
            Value::TranslatedFSString(t) => Some(&t.string.handle),
            _ => None,
        }
    }

    /// Any integer or bool value widened to `i64` (`u64` values that don't fit return `None`).
    pub fn as_i64(&self) -> Option<i64> {
        Some(match *self {
            Value::Byte(v) => v.into(),
            Value::Short(v) => v.into(),
            Value::UShort(v) => v.into(),
            Value::Int(v) => v.into(),
            Value::UInt(v) => v.into(),
            Value::ULongLong(v) => i64::try_from(v).ok()?,
            Value::OldInt64(v) | Value::Int64(v) => v,
            Value::Int8(v) => v.into(),
            Value::Bool(v) => v.into(),
            _ => return None,
        })
    }

    pub fn as_f64(&self) -> Option<f64> {
        match *self {
            Value::Float(v) => Some(v.into()),
            Value::Double(v) => Some(v),
            _ => self.as_i64().map(|v| v as f64),
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match *self {
            Value::Bool(b) => Some(b),
            _ => None,
        }
    }

    pub fn as_guid(&self) -> Option<Guid> {
        match self {
            Value::Uuid(g) => Some(*g),
            _ => None,
        }
    }

    /// Vector/matrix components as `f64`, in stored order.
    pub fn components(&self) -> Option<Vec<f64>> {
        fn f(v: &[f32]) -> Vec<f64> {
            v.iter().map(|&x| x.into()).collect()
        }
        fn i(v: &[i32]) -> Vec<f64> {
            v.iter().map(|&x| x.into()).collect()
        }
        Some(match self {
            Value::IVec2(v) => i(v),
            Value::IVec3(v) => i(v),
            Value::IVec4(v) => i(v),
            Value::Vec2(v) => f(v),
            Value::Vec3(v) => f(v),
            Value::Vec4(v) => f(v),
            Value::Mat2(v) => f(v),
            Value::Mat3(v) => f(v),
            Value::Mat3x4(v) | Value::Mat4x3(v) => f(v),
            Value::Mat4(v) => f(v),
            _ => return None,
        })
    }

    /// Builds a vector/matrix value from its components (`ty` must be a vector/matrix type and
    /// `parts` must have the right length).
    pub(crate) fn from_components(ty: AttributeType, parts: &[&str]) -> Result<Value> {
        let bad = || Error::BadValue {
            ty: ty.name(),
            text: parts.join(" "),
        };
        let Some((n, is_float)) = ty.components() else {
            return Err(bad());
        };
        if parts.len() != n {
            return Err(bad());
        }
        if is_float {
            let mut v = [0f32; 16];
            for (slot, p) in v.iter_mut().zip(parts) {
                *slot = parse_f32(p).ok_or_else(bad)?;
            }
            Ok(match ty {
                AttributeType::Vec2 => Value::Vec2([v[0], v[1]]),
                AttributeType::Vec3 => Value::Vec3([v[0], v[1], v[2]]),
                AttributeType::Vec4 => Value::Vec4([v[0], v[1], v[2], v[3]]),
                AttributeType::Mat2 => Value::Mat2(v[..4].try_into().unwrap()),
                AttributeType::Mat3 => Value::Mat3(v[..9].try_into().unwrap()),
                AttributeType::Mat3x4 => Value::Mat3x4(v[..12].try_into().unwrap()),
                AttributeType::Mat4x3 => Value::Mat4x3(v[..12].try_into().unwrap()),
                _ => Value::Mat4(v),
            })
        } else {
            let mut v = [0i32; 4];
            for (slot, p) in v.iter_mut().zip(parts) {
                *slot = p.trim().parse().map_err(|_| bad())?;
            }
            Ok(match ty {
                AttributeType::IVec2 => Value::IVec2([v[0], v[1]]),
                AttributeType::IVec3 => Value::IVec3([v[0], v[1], v[2]]),
                _ => Value::IVec4(v),
            })
        }
    }

    /// Parses the textual form used in LSX `value="…"` (and LSJ string values) for every type
    /// except the translated-string types. Vectors/matrices accept space-separated components.
    pub fn from_text(ty: AttributeType, text: &str) -> Result<Value> {
        use AttributeType as T;
        let bad = || Error::BadValue {
            ty: ty.name(),
            text: text.chars().take(64).collect(),
        };
        fn int<I: std::str::FromStr>(s: &str) -> Option<I> {
            s.trim().parse().ok()
        }
        Ok(match ty {
            T::None => Value::None,
            T::Byte => Value::Byte(int(text).ok_or_else(bad)?),
            T::Short => Value::Short(int(text).ok_or_else(bad)?),
            T::UShort => Value::UShort(int(text).ok_or_else(bad)?),
            T::Int => Value::Int(int(text).ok_or_else(bad)?),
            T::UInt => Value::UInt(int(text).ok_or_else(bad)?),
            T::Float => Value::Float(parse_f32(text).ok_or_else(bad)?),
            T::Double => Value::Double(parse_f64(text).ok_or_else(bad)?),
            T::Bool => Value::Bool(match text.trim() {
                "true" | "True" | "1" => true,
                "false" | "False" | "0" => false,
                _ => return Err(bad()),
            }),
            T::String => Value::String(text.to_owned()),
            T::Path => Value::Path(text.to_owned()),
            T::FixedString => Value::FixedString(text.to_owned()),
            T::LSString => Value::LSString(text.to_owned()),
            T::WString => Value::WString(text.to_owned()),
            T::LSWString => Value::LSWString(text.to_owned()),
            T::ULongLong => Value::ULongLong(int(text).ok_or_else(bad)?),
            T::OldInt64 => Value::OldInt64(int(text).ok_or_else(bad)?),
            T::Int64 => Value::Int64(int(text).ok_or_else(bad)?),
            T::Int8 => Value::Int8(int(text).ok_or_else(bad)?),
            T::ScratchBuffer => Value::ScratchBuffer(crate::base64::decode(text).ok_or_else(bad)?),
            T::Uuid => Value::Uuid(Guid::parse(text.trim()).ok_or_else(bad)?),
            T::TranslatedString => Value::TranslatedString(TranslatedString {
                value: Some(text.to_owned()),
                ..Default::default()
            }),
            T::TranslatedFSString => Value::TranslatedFSString(TranslatedFSString {
                string: TranslatedString {
                    value: Some(text.to_owned()),
                    ..Default::default()
                },
                arguments: Vec::new(),
            }),
            _ => {
                let parts: Vec<&str> = text.split_whitespace().collect();
                Value::from_components(ty, &parts).map_err(|_| bad())?
            }
        })
    }
}

/// Text form of a value as written in LSX `value="…"`. Vectors/matrices are space-separated,
/// scratch buffers are base64, translated strings show their handle.
impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn join<T: fmt::Display>(f: &mut fmt::Formatter<'_>, v: &[T]) -> fmt::Result {
            for (i, x) in v.iter().enumerate() {
                if i > 0 {
                    f.write_str(" ")?;
                }
                write!(f, "{x}")?;
            }
            Ok(())
        }
        match self {
            Value::None => Ok(()),
            Value::Byte(v) => write!(f, "{v}"),
            Value::Short(v) => write!(f, "{v}"),
            Value::UShort(v) => write!(f, "{v}"),
            Value::Int(v) => write!(f, "{v}"),
            Value::UInt(v) => write!(f, "{v}"),
            Value::Float(v) => write!(f, "{v}"),
            Value::Double(v) => write!(f, "{v}"),
            Value::IVec2(v) => join(f, v),
            Value::IVec3(v) => join(f, v),
            Value::IVec4(v) => join(f, v),
            Value::Vec2(v) => join(f, v),
            Value::Vec3(v) => join(f, v),
            Value::Vec4(v) => join(f, v),
            Value::Mat2(v) => join(f, v),
            Value::Mat3(v) => join(f, v),
            Value::Mat3x4(v) | Value::Mat4x3(v) => join(f, v),
            Value::Mat4(v) => join(f, v),
            Value::Bool(v) => write!(f, "{v}"),
            Value::String(s)
            | Value::Path(s)
            | Value::FixedString(s)
            | Value::LSString(s)
            | Value::WString(s)
            | Value::LSWString(s) => f.write_str(s),
            Value::ULongLong(v) => write!(f, "{v}"),
            Value::ScratchBuffer(b) => f.write_str(&crate::base64::encode(b)),
            Value::OldInt64(v) | Value::Int64(v) => write!(f, "{v}"),
            Value::Int8(v) => write!(f, "{v}"),
            Value::TranslatedString(t) => f.write_str(&t.handle),
            Value::TranslatedFSString(t) => f.write_str(&t.string.handle),
            Value::Uuid(g) => write!(f, "{g}"),
        }
    }
}

/// Lenient float parsing: accepts C-style `inf`/`nan` spellings too.
fn parse_f64(s: &str) -> Option<f64> {
    let s = s.trim();
    s.parse().ok().or_else(|| {
        let lower = s.to_ascii_lowercase();
        let neg = lower.starts_with('-');
        if lower.contains("inf") {
            Some(if neg {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            })
        } else if lower.contains("nan") {
            Some(f64::NAN)
        } else {
            None
        }
    })
}

fn parse_f32(s: &str) -> Option<f32> {
    let s = s.trim();
    s.parse().ok().or_else(|| parse_f64(s).map(|v| v as f32))
}
