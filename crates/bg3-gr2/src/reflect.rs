//! Granny's self-describing type system ("member definitions") and a generic object walker.
//!
//! Every Granny file embeds the type of its root object. A type is an array of 44-byte member
//! definitions terminated by a member of kind [`MemberType::None`]:
//!
//! ```text
//! 0   u32  member type
//! 4   ptr  name (C string)
//! 12  ptr  referenced type (for inline / reference / array members)
//! 20  i32  array size (0 = scalar)
//! 24  u32  extra[3]
//! 36  u64  reserved
//! ```
//!
//! Objects are laid out packed (no alignment padding) in member order.

use std::fmt;

use crate::file::{Gr2File, Ptr};
use crate::{Error, Result};

pub const MEMBER_DEF_SIZE: usize = 44;
const PTR_SIZE: usize = 8;
const TRANSFORM_SIZE: usize = 68;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MemberType {
    None,
    Inline,
    Reference,
    ReferenceToArray,
    ArrayOfReferences,
    VariantReference,
    ReferenceToVariantArray,
    String,
    Transform,
    Real32,
    Int8,
    UInt8,
    BinormalInt8,
    NormalUInt8,
    Int16,
    UInt16,
    BinormalInt16,
    NormalUInt16,
    Int32,
    UInt32,
    Real16,
    EmptyReference,
}

impl MemberType {
    pub fn from_u32(v: u32) -> Option<Self> {
        use MemberType::*;
        Some(match v {
            0 => None,
            1 => Inline,
            2 => Reference,
            3 => ReferenceToArray,
            4 => ArrayOfReferences,
            5 => VariantReference,
            7 => ReferenceToVariantArray,
            8 => String,
            9 => Transform,
            10 => Real32,
            11 => Int8,
            12 => UInt8,
            13 => BinormalInt8,
            14 => NormalUInt8,
            15 => Int16,
            16 => UInt16,
            17 => BinormalInt16,
            18 => NormalUInt16,
            19 => Int32,
            20 => UInt32,
            21 => Real16,
            22 => EmptyReference,
            _ => return Option::None,
        })
    }

    /// Size of a scalar of this type, excluding inline structs (which depend on their type).
    pub fn scalar_size(self) -> usize {
        use MemberType::*;
        match self {
            None | Inline => 0,
            Reference | String | EmptyReference => PTR_SIZE,
            ReferenceToArray | ArrayOfReferences => 4 + PTR_SIZE,
            VariantReference => 2 * PTR_SIZE,
            ReferenceToVariantArray => PTR_SIZE + 4 + PTR_SIZE,
            Transform => TRANSFORM_SIZE,
            Real32 | Int32 | UInt32 => 4,
            Int8 | UInt8 | BinormalInt8 | NormalUInt8 => 1,
            Int16 | UInt16 | BinormalInt16 | NormalUInt16 | Real16 => 2,
        }
    }

    /// Whether this is a numeric leaf type.
    pub fn is_numeric(self) -> bool {
        self.scalar_size() > 0
            && !matches!(
                self,
                MemberType::Reference
                    | MemberType::String
                    | MemberType::EmptyReference
                    | MemberType::ReferenceToArray
                    | MemberType::ArrayOfReferences
                    | MemberType::VariantReference
                    | MemberType::ReferenceToVariantArray
                    | MemberType::Transform
            )
    }
}

/// One member of a Granny struct type.
#[derive(Debug, Clone)]
pub struct Member {
    pub kind: MemberType,
    pub name: String,
    /// Referenced struct type (for `Inline`, references, arrays).
    pub type_ref: Ptr,
    /// Fixed array length; 0 means a single value.
    pub array_size: u32,
    pub extra: [u32; 3],
    /// Byte offset of this member within its struct.
    pub offset: usize,
    /// Total byte size of this member (including fixed array length).
    pub size: usize,
}

impl Member {
    pub fn count(&self) -> usize {
        self.array_size.max(1) as usize
    }
}

/// A struct type: its members (with offsets) and total size.
#[derive(Debug, Clone)]
pub struct TypeDef {
    pub ptr: Ptr,
    pub members: Vec<Member>,
    pub size: usize,
}

impl TypeDef {
    pub fn member(&self, name: &str) -> Option<&Member> {
        self.members.iter().find(|m| m.name == name)
    }
}

/// Reads and caches type definitions of a [`Gr2File`].
pub struct Types<'f> {
    file: &'f Gr2File,
    cache: std::cell::RefCell<std::collections::HashMap<Ptr, std::rc::Rc<TypeDef>>>,
}

impl<'f> Types<'f> {
    pub fn new(file: &'f Gr2File) -> Self {
        Self {
            file,
            cache: Default::default(),
        }
    }

    pub fn file(&self) -> &'f Gr2File {
        self.file
    }

    pub fn get(&self, ptr: Ptr) -> Result<std::rc::Rc<TypeDef>> {
        self.get_depth(ptr, 0)
    }

    fn get_depth(&self, ptr: Ptr, depth: u32) -> Result<std::rc::Rc<TypeDef>> {
        if let Some(t) = self.cache.borrow().get(&ptr) {
            return Ok(t.clone());
        }
        if depth > 64 {
            return Err(Error::Corrupt(
                "type nesting too deep (cyclic inline type?)",
            ));
        }
        let f = self.file;
        let mut members = Vec::new();
        let mut offset = 0usize;
        let mut p = ptr;
        loop {
            let raw = f.u32(p)?;
            let kind = MemberType::from_u32(raw).ok_or(Error::UnknownMemberType(raw))?;
            if kind == MemberType::None {
                break;
            }
            let name_ptr = f.ptr(p.offset_by(4))?;
            let name = if name_ptr.is_null() {
                String::new()
            } else {
                f.cstr(name_ptr)?
            };
            let type_ref = f.ptr(p.offset_by(12))?;
            let array_size = f.u32(p.offset_by(20))?;
            let extra = [
                f.u32(p.offset_by(24))?,
                f.u32(p.offset_by(28))?,
                f.u32(p.offset_by(32))?,
            ];
            let elem = if kind == MemberType::Inline {
                if type_ref.is_null() {
                    return Err(Error::Corrupt("inline member without type"));
                }
                self.get_depth(type_ref, depth + 1)?.size
            } else {
                kind.scalar_size()
            };
            let size = elem * array_size.max(1) as usize;
            members.push(Member {
                kind,
                name,
                type_ref,
                array_size,
                extra,
                offset,
                size,
            });
            offset += size;
            p = p.offset_by(MEMBER_DEF_SIZE);
            if members.len() > 4096 {
                return Err(Error::Corrupt("unterminated type definition"));
            }
        }
        let t = std::rc::Rc::new(TypeDef {
            ptr,
            members,
            size: offset,
        });
        self.cache.borrow_mut().insert(ptr, t.clone());
        Ok(t)
    }

    /// The root object of the file, typed by the header's root type.
    pub fn root(&self) -> Result<Object<'_, 'f>> {
        let ty = self.get(self.file.root_type_ptr())?;
        Ok(Object {
            types: self,
            ty,
            ptr: self.file.root_object_ptr(),
        })
    }

    pub fn object(&self, ty: Ptr, ptr: Ptr) -> Result<Object<'_, 'f>> {
        Ok(Object {
            types: self,
            ty: self.get(ty)?,
            ptr,
        })
    }
}

/// A struct instance in the file: a type plus the address of its data.
#[derive(Clone)]
pub struct Object<'t, 'f> {
    pub types: &'t Types<'f>,
    pub ty: std::rc::Rc<TypeDef>,
    pub ptr: Ptr,
}

impl fmt::Debug for Object<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Object@{:?}{{", self.ptr)?;
        for m in &self.ty.members {
            write!(f, "{}:{:?} ", m.name, m.kind)?;
        }
        write!(f, "}}")
    }
}

/// A homogeneous array of structs (`ReferenceToArray`, `ReferenceToVariantArray`).
#[derive(Clone)]
pub struct StructArray<'t, 'f> {
    pub types: &'t Types<'f>,
    pub ty: std::rc::Rc<TypeDef>,
    pub ptr: Ptr,
    pub len: usize,
}

impl<'t, 'f> StructArray<'t, 'f> {
    pub fn get(&self, i: usize) -> Option<Object<'t, 'f>> {
        (i < self.len).then(|| Object {
            types: self.types,
            ty: self.ty.clone(),
            ptr: self.ptr.offset_by(i * self.ty.size),
        })
    }

    pub fn iter(&self) -> impl Iterator<Item = Object<'t, 'f>> + '_ {
        (0..self.len).filter_map(|i| self.get(i))
    }

    /// The raw bytes of the whole array (`len * ty.size`).
    pub fn bytes(&self) -> Result<&'f [u8]> {
        if self.len == 0 {
            return Ok(&[]);
        }
        self.types.file().bytes(self.ptr, self.len * self.ty.size)
    }
}

/// Granny's `transform`: flags, position, orientation (x, y, z, w) and a 3×3 scale/shear matrix.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    /// Bit 0: has position, bit 1: has orientation, bit 2: has scale/shear.
    pub flags: u32,
    pub position: [f32; 3],
    pub orientation: [f32; 4],
    /// Row-major.
    pub scale_shear: [[f32; 3]; 3],
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            flags: 0,
            position: [0.0; 3],
            orientation: [0.0, 0.0, 0.0, 1.0],
            scale_shear: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        }
    }
}

pub(crate) fn f32_le(b: &[u8], i: usize) -> f32 {
    f32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap())
}

impl Transform {
    pub fn from_bytes(b: &[u8]) -> Self {
        let f = |i| f32_le(b, i);
        Self {
            flags: u32::from_le_bytes(b[0..4].try_into().unwrap()),
            position: [f(1), f(2), f(3)],
            orientation: [f(4), f(5), f(6), f(7)],
            scale_shear: [
                [f(8), f(9), f(10)],
                [f(11), f(12), f(13)],
                [f(14), f(15), f(16)],
            ],
        }
    }
}

/// Convert an IEEE half to f32.
pub fn f16_to_f32(h: u16) -> f32 {
    let sign = u32::from(h >> 15) << 31;
    let exp = u32::from((h >> 10) & 0x1f);
    let mant = u32::from(h & 0x3ff);
    let bits = match (exp, mant) {
        (0, 0) => sign,
        (0, _) => {
            // Subnormal: normalise.
            let mut e = 127 - 15 + 1;
            let mut m = mant;
            while m & 0x400 == 0 {
                m <<= 1;
                e -= 1;
            }
            sign | (e << 23) | ((m & 0x3ff) << 13)
        }
        (0x1f, 0) => sign | 0x7f80_0000,
        (0x1f, _) => sign | 0x7fc0_0000 | (mant << 13),
        _ => sign | ((exp + 127 - 15) << 23) | (mant << 13),
    };
    f32::from_bits(bits)
}

/// Decode `count` numeric scalars of `kind` from `b` to f32, applying Granny's normalisation for
/// the `Binormal*` (signed, /max) and `Normal*` (unsigned, /max) types.
pub fn numeric_to_f32(kind: MemberType, b: &[u8], count: usize, out: &mut Vec<f32>) {
    use MemberType::*;
    for i in 0..count {
        let v = match kind {
            Real32 => f32_le(b, i),
            Real16 => f16_to_f32(u16::from_le_bytes([b[2 * i], b[2 * i + 1]])),
            Int8 => f32::from(b[i] as i8),
            UInt8 => f32::from(b[i]),
            BinormalInt8 => (f32::from(b[i] as i8) / 127.0).max(-1.0),
            NormalUInt8 => f32::from(b[i]) / 255.0,
            Int16 => f32::from(i16::from_le_bytes([b[2 * i], b[2 * i + 1]])),
            UInt16 => f32::from(u16::from_le_bytes([b[2 * i], b[2 * i + 1]])),
            BinormalInt16 => {
                (f32::from(i16::from_le_bytes([b[2 * i], b[2 * i + 1]])) / 32767.0).max(-1.0)
            }
            NormalUInt16 => f32::from(u16::from_le_bytes([b[2 * i], b[2 * i + 1]])) / 65535.0,
            Int32 => i32::from_le_bytes(b[4 * i..4 * i + 4].try_into().unwrap()) as f32,
            UInt32 => u32::from_le_bytes(b[4 * i..4 * i + 4].try_into().unwrap()) as f32,
            _ => f32::NAN,
        };
        out.push(v);
    }
}

/// Decode integer scalars of `kind` to i64 (non-integer kinds yield `None`).
pub fn numeric_to_i64(kind: MemberType, b: &[u8], i: usize) -> Option<i64> {
    use MemberType::*;
    Some(match kind {
        Int8 | BinormalInt8 => i64::from(b[i] as i8),
        UInt8 | NormalUInt8 => i64::from(b[i]),
        Int16 | BinormalInt16 => i64::from(i16::from_le_bytes([b[2 * i], b[2 * i + 1]])),
        UInt16 | NormalUInt16 => i64::from(u16::from_le_bytes([b[2 * i], b[2 * i + 1]])),
        Int32 => i64::from(i32::from_le_bytes(b[4 * i..4 * i + 4].try_into().unwrap())),
        UInt32 => i64::from(u32::from_le_bytes(b[4 * i..4 * i + 4].try_into().unwrap())),
        _ => return Option::None,
    })
}

impl<'t, 'f> Object<'t, 'f> {
    pub fn member(&self, name: &str) -> Option<&Member> {
        self.ty.member(name)
    }

    pub fn has(&self, name: &str) -> bool {
        self.member(name).is_some()
    }

    fn file(&self) -> &'f Gr2File {
        self.types.file()
    }

    fn need(&self, name: &str) -> Result<&Member> {
        self.member(name)
            .ok_or_else(|| Error::MissingMember(name.to_owned()))
    }

    fn need_kind(&self, name: &str, kinds: &[MemberType]) -> Result<&Member> {
        let m = self.need(name)?;
        if !kinds.contains(&m.kind) {
            return Err(Error::WrongMemberType {
                name: name.to_owned(),
                found: m.kind,
            });
        }
        Ok(m)
    }

    /// Raw bytes of member `name`.
    pub fn raw(&self, name: &str) -> Result<&'f [u8]> {
        let m = self.need(name)?;
        self.file().bytes(self.ptr.offset_by(m.offset), m.size)
    }

    pub fn string(&self, name: &str) -> Result<Option<String>> {
        let m = self.need_kind(name, &[MemberType::String])?;
        let p = self.file().ptr(self.ptr.offset_by(m.offset))?;
        if p.is_null() {
            return Ok(None);
        }
        self.file().cstr(p).map(Some)
    }

    /// A string member, or an empty string when absent or null.
    pub fn string_or_empty(&self, name: &str) -> String {
        self.string(name).ok().flatten().unwrap_or_default()
    }

    /// All numeric values of a member, converted to f32.
    pub fn f32s(&self, name: &str) -> Result<Vec<f32>> {
        let m = self.need(name)?;
        if !m.kind.is_numeric() {
            return Err(Error::WrongMemberType {
                name: name.to_owned(),
                found: m.kind,
            });
        }
        let b = self.file().bytes(self.ptr.offset_by(m.offset), m.size)?;
        let mut out = Vec::with_capacity(m.count());
        numeric_to_f32(m.kind, b, m.count(), &mut out);
        Ok(out)
    }

    pub fn f32(&self, name: &str) -> Result<f32> {
        Ok(self.f32s(name)?[0])
    }

    /// First value of an integer member.
    pub fn int(&self, name: &str) -> Result<i64> {
        let m = self.need(name)?;
        let b = self.file().bytes(self.ptr.offset_by(m.offset), m.size)?;
        numeric_to_i64(m.kind, b, 0).ok_or(Error::WrongMemberType {
            name: name.to_owned(),
            found: m.kind,
        })
    }

    pub fn transform(&self, name: &str) -> Result<Transform> {
        let m = self.need_kind(name, &[MemberType::Transform])?;
        Ok(Transform::from_bytes(
            self.file()
                .bytes(self.ptr.offset_by(m.offset), TRANSFORM_SIZE)?,
        ))
    }

    /// An inline struct member (first element if it is a fixed array).
    pub fn inline(&self, name: &str) -> Result<Object<'t, 'f>> {
        let m = self.need_kind(name, &[MemberType::Inline])?;
        Ok(Object {
            types: self.types,
            ty: self.types.get(m.type_ref)?,
            ptr: self.ptr.offset_by(m.offset),
        })
    }

    /// Follow a `Reference` member. `None` when null.
    pub fn reference(&self, name: &str) -> Result<Option<Object<'t, 'f>>> {
        let m = self.need_kind(name, &[MemberType::Reference])?;
        let p = self.file().ptr(self.ptr.offset_by(m.offset))?;
        if p.is_null() {
            return Ok(None);
        }
        Ok(Some(Object {
            types: self.types,
            ty: self.types.get(m.type_ref)?,
            ptr: p,
        }))
    }

    /// Follow a `VariantReference` member (type stored next to the pointer).
    pub fn variant(&self, name: &str) -> Result<Option<Object<'t, 'f>>> {
        let m = self.need_kind(name, &[MemberType::VariantReference])?;
        let ty = self.file().ptr(self.ptr.offset_by(m.offset))?;
        let p = self.file().ptr(self.ptr.offset_by(m.offset + PTR_SIZE))?;
        if p.is_null() || ty.is_null() {
            return Ok(None);
        }
        Ok(Some(self.types.object(ty, p)?))
    }

    /// A `ReferenceToArray` or `ReferenceToVariantArray` member as an array of structs.
    pub fn array(&self, name: &str) -> Result<StructArray<'t, 'f>> {
        let m = self.need_kind(
            name,
            &[
                MemberType::ReferenceToArray,
                MemberType::ReferenceToVariantArray,
            ],
        )?;
        let f = self.file();
        let base = self.ptr.offset_by(m.offset);
        let (ty, len, ptr) = if m.kind == MemberType::ReferenceToArray {
            (m.type_ref, f.u32(base)? as usize, f.ptr(base.offset_by(4))?)
        } else {
            (
                f.ptr(base)?,
                f.u32(base.offset_by(PTR_SIZE))? as usize,
                f.ptr(base.offset_by(PTR_SIZE + 4))?,
            )
        };
        if ptr.is_null() || len == 0 || ty.is_null() {
            return Ok(StructArray {
                types: self.types,
                ty: std::rc::Rc::new(TypeDef {
                    ptr: Ptr::NULL,
                    members: Vec::new(),
                    size: 0,
                }),
                ptr: Ptr::NULL,
                len: 0,
            });
        }
        let ty = self.types.get(ty)?;
        Ok(StructArray {
            types: self.types,
            ty,
            ptr,
            len,
        })
    }

    /// An `ArrayOfReferences` member: each element is a (possibly null) pointer to a struct.
    pub fn references(&self, name: &str) -> Result<Vec<Option<Object<'t, 'f>>>> {
        let m = self.need_kind(name, &[MemberType::ArrayOfReferences])?;
        let f = self.file();
        let base = self.ptr.offset_by(m.offset);
        let len = f.u32(base)? as usize;
        let arr = f.ptr(base.offset_by(4))?;
        if arr.is_null() || len == 0 {
            return Ok(Vec::new());
        }
        let ty = self.types.get(m.type_ref)?;
        (0..len)
            .map(|i| {
                let p = f.ptr(arr.offset_by(i * PTR_SIZE))?;
                Ok((!p.is_null()).then(|| Object {
                    types: self.types,
                    ty: ty.clone(),
                    ptr: p,
                }))
            })
            .collect()
    }

    /// Flatten a `ReferenceToArray` of single-member numeric structs (e.g. `Indices`) into i64s.
    pub fn int_array(&self, name: &str) -> Result<Vec<i64>> {
        let arr = self.array(name)?;
        if arr.len == 0 {
            return Ok(Vec::new());
        }
        let [m] = &arr.ty.members[..] else {
            return Err(Error::Corrupt("int array element is not a single scalar"));
        };
        let b = arr.bytes()?;
        let mut out = Vec::with_capacity(arr.len * m.count());
        for i in 0..arr.len * m.count() {
            out.push(numeric_to_i64(m.kind, b, i).ok_or(Error::WrongMemberType {
                name: name.to_owned(),
                found: m.kind,
            })?);
        }
        Ok(out)
    }

    /// Materialise this object into a [`Value`] tree (arrays truncated to `max_array` items,
    /// recursion limited to `max_depth`).
    pub fn to_value(&self, max_depth: u32, max_array: usize) -> Value {
        if max_depth == 0 {
            return Value::Elided;
        }
        let mut fields = Vec::with_capacity(self.ty.members.len());
        for m in &self.ty.members {
            let v = self
                .member_value(m, max_depth, max_array)
                .unwrap_or_else(|e| Value::Error(e.to_string()));
            fields.push((m.name.clone(), v));
        }
        Value::Struct(fields)
    }

    fn member_value(&self, m: &Member, depth: u32, max_array: usize) -> Result<Value> {
        use MemberType::*;
        let f = self.file();
        let at = self.ptr.offset_by(m.offset);
        let fixed = |one: &dyn Fn(Ptr) -> Result<Value>, elem: usize| -> Result<Value> {
            if m.array_size == 0 {
                one(at)
            } else {
                let n = m.count().min(max_array);
                Ok(Value::Array(
                    (0..n)
                        .map(|i| one(at.offset_by(i * elem)))
                        .collect::<Result<_>>()?,
                    m.count(),
                ))
            }
        };
        Ok(match m.kind {
            None => Value::Null,
            Inline => {
                let ty = self.types.get(m.type_ref)?;
                let size = ty.size;
                fixed(
                    &|p| {
                        Ok(Object {
                            types: self.types,
                            ty: ty.clone(),
                            ptr: p,
                        }
                        .to_value(depth - 1, max_array))
                    },
                    size,
                )?
            }
            Reference => match self.reference(&m.name)? {
                Some(o) => o.to_value(depth - 1, max_array),
                Option::None => Value::Null,
            },
            VariantReference => match self.variant(&m.name)? {
                Some(o) => o.to_value(depth - 1, max_array),
                Option::None => Value::Null,
            },
            ReferenceToArray | ReferenceToVariantArray => {
                let arr = self.array(&m.name)?;
                let n = arr.len.min(max_array);
                Value::Array(
                    (0..n)
                        .filter_map(|i| arr.get(i))
                        .map(|o| o.to_value(depth - 1, max_array))
                        .collect(),
                    arr.len,
                )
            }
            ArrayOfReferences => {
                let refs = self.references(&m.name)?;
                let total = refs.len();
                Value::Array(
                    refs.into_iter()
                        .take(max_array)
                        .map(|o| match o {
                            Some(o) => o.to_value(depth - 1, max_array),
                            Option::None => Value::Null,
                        })
                        .collect(),
                    total,
                )
            }
            String => fixed(
                &|p| {
                    let s = f.ptr(p)?;
                    Ok(if s.is_null() {
                        Value::Null
                    } else {
                        Value::String(f.cstr(s)?)
                    })
                },
                PTR_SIZE,
            )?,
            Transform => fixed(
                &|p| {
                    Ok(Value::Transform(crate::reflect::Transform::from_bytes(
                        f.bytes(p, TRANSFORM_SIZE)?,
                    )))
                },
                TRANSFORM_SIZE,
            )?,
            EmptyReference => Value::Null,
            Real32 | Real16 => {
                let mut v = Vec::new();
                numeric_to_f32(m.kind, f.bytes(at, m.size)?, m.count(), &mut v);
                scalar_or_array(v.into_iter().map(Value::Float).collect(), m)
            }
            _ => {
                let b = f.bytes(at, m.size)?;
                let v = (0..m.count())
                    .map(|i| Value::Int(numeric_to_i64(m.kind, b, i).unwrap_or(0)))
                    .collect();
                scalar_or_array(v, m)
            }
        })
    }
}

fn scalar_or_array(mut v: Vec<Value>, m: &Member) -> Value {
    if m.array_size == 0 {
        v.pop().unwrap_or(Value::Null)
    } else {
        let n = v.len();
        Value::Array(v, n)
    }
}

/// A materialised generic value (for inspection / debugging).
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Int(i64),
    Float(f32),
    String(String),
    Transform(Transform),
    Struct(Vec<(String, Value)>),
    /// Items (possibly truncated) and the full length.
    Array(Vec<Value>, usize),
    /// Depth limit reached.
    Elided,
    Error(String),
}

impl Value {
    pub fn get(&self, name: &str) -> Option<&Value> {
        match self {
            Value::Struct(fields) => fields.iter().find(|(n, _)| n == name).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Indented, human-readable dump.
    pub fn write_tree(&self, out: &mut String, indent: usize) {
        use std::fmt::Write;
        let pad = "  ".repeat(indent);
        match self {
            Value::Struct(fields) => {
                for (n, v) in fields {
                    match v {
                        Value::Struct(_) => {
                            let _ = writeln!(out, "{pad}{n}:");
                            v.write_tree(out, indent + 1);
                        }
                        Value::Array(items, total)
                            if items.iter().any(|i| matches!(i, Value::Struct(_))) =>
                        {
                            let _ = writeln!(out, "{pad}{n}: [{total}]");
                            for (i, it) in items.iter().enumerate() {
                                let _ = writeln!(out, "{pad}  [{i}]");
                                it.write_tree(out, indent + 2);
                            }
                            if items.len() < *total {
                                let _ = writeln!(out, "{pad}  …");
                            }
                        }
                        _ => {
                            let _ = writeln!(out, "{pad}{n}: {}", v.short());
                        }
                    }
                }
            }
            other => {
                let _ = writeln!(out, "{pad}{}", other.short());
            }
        }
    }

    fn short(&self) -> String {
        match self {
            Value::Null => "null".into(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => format!("{f}"),
            Value::String(s) => format!("{s:?}"),
            Value::Transform(t) => format!(
                "T(flags={} pos={:?} rot={:?} ss={:?})",
                t.flags, t.position, t.orientation, t.scale_shear
            ),
            Value::Struct(f) => format!("{{{} fields}}", f.len()),
            Value::Array(items, total) => {
                let s: Vec<String> = items.iter().map(Value::short).collect();
                if items.len() < *total {
                    format!("[{}, … ({total})]", s.join(", "))
                } else {
                    format!("[{}]", s.join(", "))
                }
            }
            Value::Elided => "…".into(),
            Value::Error(e) => format!("<error: {e}>"),
        }
    }
}
