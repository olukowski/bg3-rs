//! LSJ (JSON) reader.
//!
//! Shape: `{"save": {"header": {"version": "4.6.0.900", "time"?: n}, "regions": {Name: node}}}`.
//! A node is an object whose members are either attributes (`{"type": …, "value": …}` objects)
//! or child lists (`Name: [node, …]`). Children with different names lose their relative order
//! in this format; we keep member order otherwise.

use serde_json::{Map, Value as Json};

use crate::value::{FsArgument, TranslatedFSString, TranslatedString};
use crate::{
    Attribute, AttributeType, EngineVersion, Error, Metadata, Node, Region, Resource, Result,
    SourceFormat, Value,
};

fn structure(msg: impl Into<String>) -> Error {
    Error::Structure(format!("LSJ: {}", msg.into()))
}

/// Parses an LSJ document.
pub fn read(data: &[u8]) -> Result<Resource> {
    let data = data.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(data);
    let root: Json = serde_json::from_slice(data)?;
    let save = root
        .get("save")
        .and_then(Json::as_object)
        .ok_or_else(|| structure("missing \"save\" object"))?;
    let mut metadata = Metadata {
        format: SourceFormat::Lsj,
        ..Default::default()
    };
    if let Some(header) = save.get("header").and_then(Json::as_object) {
        if let Some(v) = header.get("version").and_then(Json::as_str) {
            metadata.version =
                EngineVersion::parse(v).ok_or_else(|| structure(format!("bad version {v:?}")))?;
        }
        metadata.timestamp = header.get("time").and_then(Json::as_i64);
    }
    let mut regions = Vec::new();
    if let Some(r) = save.get("regions") {
        let r = r
            .as_object()
            .ok_or_else(|| structure("\"regions\" is not an object"))?;
        for (name, node) in r {
            let node = node
                .as_object()
                .ok_or_else(|| structure(format!("region {name} is not an object")))?;
            regions.push(Region {
                name: name.clone(),
                node: read_node(name, node)?,
            });
        }
    }
    Ok(Resource { metadata, regions })
}

fn read_node(name: &str, obj: &Map<String, Json>) -> Result<Node> {
    let mut node = Node::new(name);
    for (key, member) in obj {
        match member {
            Json::Array(list) => {
                for child in list {
                    let child = child.as_object().ok_or_else(|| {
                        structure(format!("child {key} of {name} is not an object"))
                    })?;
                    node.children.push(read_node(key, child)?);
                }
            }
            Json::Object(attr) => node.attributes.push(Attribute {
                name: key.clone(),
                value: read_attribute(attr).map_err(|e| match e {
                    Error::Structure(m) => Error::Structure(format!("{m} (attribute {key})")),
                    e => e,
                })?,
            }),
            _ => return Err(structure(format!("unexpected member {key} in node {name}"))),
        }
    }
    Ok(node)
}

fn read_translated(obj: &Map<String, Json>) -> Result<TranslatedString> {
    Ok(TranslatedString {
        value: obj.get("value").and_then(Json::as_str).map(str::to_owned),
        handle: obj
            .get("handle")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_owned(),
        version: match obj.get("version") {
            Some(v) => v
                .as_u64()
                .and_then(|v| u16::try_from(v).ok())
                .ok_or_else(|| structure(format!("bad TranslatedString version {v}")))?,
            None => 0,
        },
    })
}

fn read_fs(obj: &Map<String, Json>) -> Result<TranslatedFSString> {
    let mut out = TranslatedFSString {
        string: read_translated(obj)?,
        arguments: Vec::new(),
    };
    if let Some(args) = obj.get("arguments").and_then(Json::as_array) {
        for arg in args {
            let arg = arg
                .as_object()
                .ok_or_else(|| structure("TranslatedFSString argument is not an object"))?;
            let text = |k: &str| {
                arg.get(k)
                    .and_then(Json::as_str)
                    .unwrap_or_default()
                    .to_owned()
            };
            out.arguments.push(FsArgument {
                key: text("key"),
                value: text("value"),
                string: match arg.get("string").and_then(Json::as_object) {
                    Some(s) => read_fs(s)?,
                    None => TranslatedFSString::default(),
                },
            });
        }
    }
    Ok(out)
}

fn read_attribute(obj: &Map<String, Json>) -> Result<Value> {
    let ty = match obj.get("type") {
        Some(Json::String(s)) => AttributeType::from_name(s),
        Some(Json::Number(n)) => n
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .and_then(AttributeType::from_id),
        _ => return Err(structure("attribute without \"type\"")),
    }
    .ok_or_else(|| Error::UnknownType(obj["type"].to_string()))?;
    match ty {
        AttributeType::TranslatedString => {
            return Ok(Value::TranslatedString(read_translated(obj)?));
        }
        AttributeType::TranslatedFSString => return Ok(Value::TranslatedFSString(read_fs(obj)?)),
        _ => {}
    }
    let bad = |v: &Json| Error::BadValue {
        ty: ty.name(),
        text: v.to_string().chars().take(64).collect(),
    };
    let value = obj.get("value").unwrap_or(&Json::Null);
    match value {
        Json::String(s) => Value::from_text(ty, s),
        Json::Bool(b) => match ty {
            AttributeType::Bool => Ok(Value::Bool(*b)),
            _ => Err(bad(value)),
        },
        Json::Number(n) => {
            use AttributeType as T;
            let int = || n.as_i64().ok_or_else(|| bad(value));
            let conv = |r: std::result::Result<Value, std::num::TryFromIntError>| {
                r.map_err(|_| bad(value))
            };
            match ty {
                T::Byte => conv(int()?.try_into().map(Value::Byte)),
                T::Short => conv(int()?.try_into().map(Value::Short)),
                T::UShort => conv(int()?.try_into().map(Value::UShort)),
                T::Int => conv(int()?.try_into().map(Value::Int)),
                T::UInt => conv(int()?.try_into().map(Value::UInt)),
                T::Int8 => conv(int()?.try_into().map(Value::Int8)),
                T::Int64 => Ok(Value::Int64(int()?)),
                T::OldInt64 => Ok(Value::OldInt64(int()?)),
                T::ULongLong => n.as_u64().map(Value::ULongLong).ok_or_else(|| bad(value)),
                T::Float => n
                    .as_f64()
                    .map(|f| Value::Float(f as f32))
                    .ok_or_else(|| bad(value)),
                T::Double => n.as_f64().map(Value::Double).ok_or_else(|| bad(value)),
                T::Bool => Ok(Value::Bool(int()? != 0)),
                _ => Err(bad(value)),
            }
        }
        Json::Array(items) if ty.components().is_some() => {
            let parts: Vec<String> = items
                .iter()
                .map(|i| match i {
                    Json::Number(n) => Ok(n.to_string()),
                    _ => Err(bad(value)),
                })
                .collect::<Result<_>>()?;
            let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
            Value::from_components(ty, &parts)
        }
        Json::Null => Value::from_text(ty, ""),
        _ => Err(bad(value)),
    }
}
