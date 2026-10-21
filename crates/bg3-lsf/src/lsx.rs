//! LSX (XML) reader and writer.
//!
//! Larian's own LSX writes vectors and matrices as child elements
//! (`<attribute type="fvec3"><float3 x=".." y=".." z=".."/></attribute>`,
//! `<mat4><float4 …/>×4</mat4>`); LSLib-converted files put them space-separated in `value`.
//! Both are accepted; the writer uses Larian's form.

use std::fmt::Write as _;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::value::{FsArgument, TranslatedFSString, TranslatedString};
use crate::{
    Attribute, AttributeType, EngineVersion, Error, Metadata, Node, Region, Resource, Result,
    SourceFormat, Value,
};

fn xml_err(e: impl std::fmt::Display) -> Error {
    Error::Xml(e.to_string())
}

/// Element attributes, unescaped but *not* whitespace-normalised: Larian stores multi-line text
/// with literal newlines inside attribute values, and the game preserves them.
fn attrs(e: &BytesStart<'_>) -> Result<Vec<(String, String)>> {
    e.attributes()
        .map(|a| {
            let a = a.map_err(xml_err)?;
            let key: &str = a.key.as_ref();
            let key = key.to_owned();
            let value = quick_xml::escape::unescape(&a.value)
                .map_err(xml_err)?
                .into_owned();
            Ok((key, value))
        })
        .collect()
}

fn get<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn require<'a>(attrs: &'a [(String, String)], key: &str, element: &str) -> Result<&'a str> {
    get(attrs, key).ok_or_else(|| Error::Structure(format!("LSX: <{element}> without {key}=")))
}

fn translated(attrs: &[(String, String)]) -> Result<TranslatedString> {
    Ok(TranslatedString {
        value: get(attrs, "value").map(str::to_owned),
        handle: get(attrs, "handle").unwrap_or_default().to_owned(),
        version: match get(attrs, "version") {
            Some(v) => v.trim().parse().map_err(|_| Error::BadValue {
                ty: "TranslatedString",
                text: v.to_owned(),
            })?,
            None => 0,
        },
    })
}

/// State while inside an `<attribute>` element.
struct PendingAttribute {
    name: String,
    ty: AttributeType,
    value: Option<Value>,
    /// Components gathered from `<floatN>`/`<intN>` children.
    components: Vec<String>,
    /// Open TranslatedFSString / argument frames (root string first).
    fs: Vec<FsFrame>,
}

enum FsFrame {
    String(TranslatedFSString),
    Argument {
        key: String,
        value: String,
        string: Option<TranslatedFSString>,
    },
}

#[derive(Default)]
struct State {
    metadata: Metadata,
    regions: Vec<Region>,
    region: Option<String>,
    stack: Vec<Node>,
    attribute: Option<PendingAttribute>,
}

impl State {
    fn start(&mut self, e: &BytesStart<'_>, empty: bool) -> Result<()> {
        let tag = e.name();
        let tag: &str = tag.as_ref();
        let a = attrs(e)?;
        if let Some(pending) = &mut self.attribute {
            match tag {
                "float2" | "float3" | "float4" | "int2" | "int3" | "int4" => {
                    for c in ["x", "y", "z", "w"] {
                        if let Some(v) = get(&a, c) {
                            pending.components.push(v.to_owned());
                        }
                    }
                }
                "mat2" | "mat3" | "mat4" | "mat3x4" | "mat4x3" | "arguments" => {}
                "argument" => pending.fs.push(FsFrame::Argument {
                    key: get(&a, "key").unwrap_or_default().to_owned(),
                    value: get(&a, "value").unwrap_or_default().to_owned(),
                    string: None,
                }),
                "string" => {
                    pending.fs.push(FsFrame::String(TranslatedFSString {
                        string: translated(&a)?,
                        arguments: Vec::new(),
                    }));
                }
                _ => {
                    return Err(Error::Structure(format!(
                        "LSX: unexpected <{tag}> inside attribute {}",
                        pending.name
                    )));
                }
            }
            if empty {
                self.end(tag)?;
            }
            return Ok(());
        }
        match tag {
            "save" | "children" => {}
            "header" => {
                self.metadata.timestamp = get(&a, "time").and_then(|t| t.trim().parse().ok());
            }
            "version" => {
                let num = |k: &str| -> Result<u32> {
                    let v = get(&a, k).unwrap_or("0");
                    v.trim()
                        .parse()
                        .map_err(|_| Error::Structure(format!("LSX: bad version {k}={v:?}")))
                };
                self.metadata.version = EngineVersion {
                    major: num("major")?,
                    minor: num("minor")?,
                    revision: num("revision")?,
                    build: num("build")?,
                };
                self.metadata.lslib_meta = get(&a, "lslib_meta").map(str::to_owned);
            }
            "region" => {
                self.region = Some(require(&a, "id", "region")?.to_owned());
            }
            "node" => {
                if self.region.is_none() {
                    return Err(Error::Structure("LSX: <node> outside <region>".into()));
                }
                let mut node = Node::new(require(&a, "id", "node")?);
                node.key = get(&a, "key").map(str::to_owned);
                self.stack.push(node);
            }
            "attribute" => {
                if self.stack.is_empty() {
                    return Err(Error::Structure("LSX: <attribute> outside <node>".into()));
                }
                let name = require(&a, "id", "attribute")?.to_owned();
                let ty_name = require(&a, "type", "attribute")?;
                let ty = AttributeType::from_name(ty_name)
                    .ok_or_else(|| Error::UnknownType(ty_name.to_owned()))?;
                let mut pending = PendingAttribute {
                    name,
                    ty,
                    value: None,
                    components: Vec::new(),
                    fs: Vec::new(),
                };
                match ty {
                    AttributeType::TranslatedString => {
                        pending.value = Some(Value::TranslatedString(translated(&a)?));
                    }
                    AttributeType::TranslatedFSString => {
                        pending.fs.push(FsFrame::String(TranslatedFSString {
                            string: translated(&a)?,
                            arguments: Vec::new(),
                        }));
                    }
                    _ => {
                        if let Some(text) = get(&a, "value") {
                            pending.value = Some(Value::from_text(ty, text)?);
                        }
                    }
                }
                self.attribute = Some(pending);
            }
            _ => return Err(Error::Structure(format!("LSX: unexpected <{tag}>"))),
        }
        if empty {
            self.end(tag)?;
        }
        Ok(())
    }

    fn end(&mut self, tag: &str) -> Result<()> {
        if let Some(pending) = &mut self.attribute {
            match tag {
                "attribute" => {
                    let pending = self.attribute.take().unwrap();
                    let value =
                        finish_attribute(pending.ty, pending.value, pending.components, pending.fs)
                            .map_err(|e| match e {
                                Error::Structure(m) => {
                                    Error::Structure(format!("{m} (attribute {})", pending.name))
                                }
                                e => e,
                            })?;
                    self.stack.last_mut().unwrap().attributes.push(Attribute {
                        name: pending.name,
                        value,
                    });
                }
                "string" => {
                    let Some(FsFrame::String(s)) = pending.fs.pop() else {
                        return Err(Error::Structure("LSX: unbalanced <string>".into()));
                    };
                    match pending.fs.last_mut() {
                        Some(FsFrame::Argument { string, .. }) => *string = Some(s),
                        _ => {
                            return Err(Error::Structure(
                                "LSX: <string> outside <argument>".into(),
                            ));
                        }
                    }
                }
                "argument" => {
                    let Some(FsFrame::Argument { key, value, string }) = pending.fs.pop() else {
                        return Err(Error::Structure("LSX: unbalanced <argument>".into()));
                    };
                    match pending.fs.last_mut() {
                        Some(FsFrame::String(parent)) => parent.arguments.push(FsArgument {
                            key,
                            value,
                            string: string.unwrap_or_default(),
                        }),
                        _ => return Err(Error::Structure("LSX: misplaced <argument>".into())),
                    }
                }
                _ => {}
            }
            return Ok(());
        }
        match tag {
            "node" => {
                let node = self
                    .stack
                    .pop()
                    .ok_or_else(|| Error::Structure("LSX: unbalanced </node>".into()))?;
                match self.stack.last_mut() {
                    Some(parent) => parent.children.push(node),
                    None => self.regions.push(Region {
                        name: self.region.clone().unwrap_or_default(),
                        node,
                    }),
                }
            }
            "region" => self.region = None,
            _ => {}
        }
        Ok(())
    }
}

fn finish_attribute(
    ty: AttributeType,
    value: Option<Value>,
    components: Vec<String>,
    mut fs: Vec<FsFrame>,
) -> Result<Value> {
    if ty == AttributeType::TranslatedFSString {
        return match (fs.pop(), fs.is_empty()) {
            (Some(FsFrame::String(s)), true) => Ok(Value::TranslatedFSString(s)),
            _ => Err(Error::Structure(
                "LSX: unbalanced TranslatedFSString".into(),
            )),
        };
    }
    if !components.is_empty() {
        let parts: Vec<&str> = components.iter().map(String::as_str).collect();
        return Value::from_components(ty, &parts);
    }
    match value {
        Some(v) => Ok(v),
        // A missing value attribute: treat as the empty text form (e.g. empty strings).
        None => Value::from_text(ty, ""),
    }
}

/// Parses an LSX document (UTF-8, optional BOM).
pub fn read(data: &[u8]) -> Result<Resource> {
    let data = data.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(data);
    let text = std::str::from_utf8(data).map_err(xml_err)?;
    let mut reader = Reader::from_str(text);
    let mut state = State {
        metadata: Metadata {
            format: SourceFormat::Lsx,
            ..Default::default()
        },
        ..Default::default()
    };
    loop {
        match reader
            .read_event()
            .map_err(|e| Error::Xml(format!("{e} at byte {}", reader.buffer_position())))?
        {
            Event::Start(e) => state.start(&e, false)?,
            Event::Empty(e) => state.start(&e, true)?,
            Event::End(e) => {
                let name = e.name();
                state.end(name.as_ref())?;
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !state.stack.is_empty() || state.attribute.is_some() {
        return Err(Error::Structure("LSX: unexpected end of document".into()));
    }
    Ok(Resource {
        metadata: state.metadata,
        regions: state.regions,
    })
}

/// Escapes text for a double-quoted XML attribute, keeping newlines/tabs as character
/// references so they survive attribute-value normalisation in strict parsers.
fn escape(out: &mut String, s: &str) {
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            '\t' => out.push_str("&#9;"),
            c => out.push(c),
        }
    }
}

fn attr(out: &mut String, key: &str, value: &str) {
    out.push(' ');
    out.push_str(key);
    out.push_str("=\"");
    escape(out, value);
    out.push('"');
}

fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("    ");
    }
}

fn write_translated(out: &mut String, t: &TranslatedString) {
    if let Some(v) = &t.value {
        attr(out, "value", v);
    }
    attr(out, "handle", &t.handle);
    attr(out, "version", &t.version.to_string());
}

fn write_fs_arguments(out: &mut String, t: &TranslatedFSString, depth: usize) {
    indent(out, depth);
    out.push_str("<arguments>\n");
    for arg in &t.arguments {
        indent(out, depth + 1);
        out.push_str("<argument");
        attr(out, "key", &arg.key);
        attr(out, "value", &arg.value);
        out.push_str(">\n");
        indent(out, depth + 2);
        out.push_str("<string");
        write_translated(out, &arg.string.string);
        attr(out, "arguments", &arg.string.arguments.len().to_string());
        if arg.string.arguments.is_empty() {
            out.push_str("/>\n");
        } else {
            out.push_str(">\n");
            write_fs_arguments(out, &arg.string, depth + 3);
            indent(out, depth + 2);
            out.push_str("</string>\n");
        }
        indent(out, depth + 1);
        out.push_str("</argument>\n");
    }
    indent(out, depth);
    out.push_str("</arguments>\n");
}

fn write_node(out: &mut String, node: &Node, depth: usize) {
    indent(out, depth);
    out.push_str("<node");
    attr(out, "id", &node.name);
    if let Some(key) = &node.key {
        attr(out, "key", key);
    }
    if node.attributes.is_empty() && node.children.is_empty() {
        out.push_str("/>\n");
        return;
    }
    out.push_str(">\n");
    for a in &node.attributes {
        indent(out, depth + 1);
        out.push_str("<attribute");
        attr(out, "id", &a.name);
        let ty = a.value.ty();
        attr(out, "type", ty.name());
        match &a.value {
            Value::TranslatedString(t) => {
                write_translated(out, t);
                out.push_str("/>\n");
            }
            Value::TranslatedFSString(t) => {
                write_translated(out, &t.string);
                attr(out, "arguments", &t.arguments.len().to_string());
                if t.arguments.is_empty() {
                    out.push_str("/>\n");
                } else {
                    out.push_str(">\n");
                    write_fs_arguments(out, t, depth + 2);
                    indent(out, depth + 1);
                    out.push_str("</attribute>\n");
                }
            }
            v => match v.components() {
                Some(parts) => {
                    out.push_str(">\n");
                    let (rows, width) = ty.rows();
                    let is_int = matches!(
                        ty,
                        AttributeType::IVec2 | AttributeType::IVec3 | AttributeType::IVec4
                    );
                    let element = format!("{}{width}", if is_int { "int" } else { "float" });
                    let mut depth = depth + 2;
                    if rows > 1 {
                        indent(out, depth);
                        let _ = writeln!(out, "<{}>", matrix_tag(ty));
                        depth += 1;
                    }
                    for row in parts.chunks(width) {
                        indent(out, depth);
                        out.push('<');
                        out.push_str(&element);
                        for (c, x) in ["x", "y", "z", "w"].iter().zip(row) {
                            attr(out, c, &format_component(*x, is_int));
                        }
                        out.push_str("/>\n");
                    }
                    if rows > 1 {
                        depth -= 1;
                        indent(out, depth);
                        let _ = writeln!(out, "</{}>", matrix_tag(ty));
                    }
                    indent(out, depth - 1);
                    out.push_str("</attribute>\n");
                }
                None => {
                    attr(out, "value", &v.to_string());
                    out.push_str("/>\n");
                }
            },
        }
    }
    if !node.children.is_empty() {
        indent(out, depth + 1);
        out.push_str("<children>\n");
        for child in &node.children {
            write_node(out, child, depth + 2);
        }
        indent(out, depth + 1);
        out.push_str("</children>\n");
    }
    indent(out, depth);
    out.push_str("</node>\n");
}

fn matrix_tag(ty: AttributeType) -> &'static str {
    match ty {
        AttributeType::Mat2 => "mat2",
        AttributeType::Mat3 => "mat3",
        AttributeType::Mat3x4 => "mat3x4",
        AttributeType::Mat4x3 => "mat4x3",
        _ => "mat4",
    }
}

fn format_component(x: f64, is_int: bool) -> String {
    if is_int {
        format!("{}", x as i64)
    } else {
        // Components came from f32; print the shortest f32 representation.
        format!("{}", x as f32)
    }
}

/// Serialises a resource as LSX (Larian's element style for vectors/matrices).
pub fn write(resource: &Resource) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<save>\n");
    let m = &resource.metadata;
    if let Some(time) = m.timestamp {
        let _ = writeln!(out, "    <header version=\"1\" time=\"{time}\"/>");
    }
    out.push_str("    <version");
    attr(&mut out, "major", &m.version.major.to_string());
    attr(&mut out, "minor", &m.version.minor.to_string());
    attr(&mut out, "revision", &m.version.revision.to_string());
    attr(&mut out, "build", &m.version.build.to_string());
    if let Some(meta) = &m.lslib_meta {
        attr(&mut out, "lslib_meta", meta);
    }
    out.push_str("/>\n");
    for region in &resource.regions {
        out.push_str("    <region");
        attr(&mut out, "id", &region.name);
        out.push_str(">\n");
        write_node(&mut out, &region.node, 2);
        out.push_str("    </region>\n");
    }
    out.push_str("</save>\n");
    out
}
