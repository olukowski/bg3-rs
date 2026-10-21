//! Install-free tests on self-authored fixtures.

use super::*;
use crate::lsf::{Compression, WriteOptions};

fn guid(s: &str) -> Guid {
    Guid::parse(s).unwrap()
}

/// A resource that exercises every attribute type, keys, nesting and multiple regions.
fn sample() -> Resource {
    let fs = TranslatedFSString {
        string: TranslatedString {
            value: None,
            handle: "h00000000g0000g0000g0000g000000000001".into(),
            version: 2,
        },
        arguments: vec![FsArgument {
            key: "Amount".into(),
            value: "3".into(),
            string: TranslatedFSString {
                string: TranslatedString {
                    value: None,
                    handle: "h00000000g0000g0000g0000g000000000002".into(),
                    version: 1,
                },
                arguments: vec![],
            },
        }],
    };
    let all = Node::new("Everything")
        .with_attr("none", Value::None)
        .with_attr("u8", Value::Byte(200))
        .with_attr("i16", Value::Short(-2))
        .with_attr("u16", Value::UShort(65000))
        .with_attr("i32", Value::Int(-123456))
        .with_attr("u32", Value::UInt(4_000_000_000))
        .with_attr("f32", Value::Float(0.25))
        .with_attr("f64", Value::Double(-1.5e100))
        .with_attr("iv2", Value::IVec2([1, -2]))
        .with_attr("iv3", Value::IVec3([1, 2, 3]))
        .with_attr("iv4", Value::IVec4([1, 2, 3, -4]))
        .with_attr("v2", Value::Vec2([0.5, 1.0]))
        .with_attr("v3", Value::Vec3([0.5, -0.0, 3.0]))
        .with_attr("v4", Value::Vec4([1.0, 2.0, 3.0, 4.5]))
        .with_attr("m2", Value::Mat2([1.0, 2.0, 3.0, 4.0]))
        .with_attr("m3", Value::Mat3(std::array::from_fn(|i| i as f32)))
        .with_attr(
            "m34",
            Value::Mat3x4(std::array::from_fn(|i| i as f32 * 0.5)),
        )
        .with_attr("m43", Value::Mat4x3(std::array::from_fn(|i| -(i as f32))))
        .with_attr("m4", Value::Mat4(std::array::from_fn(|i| i as f32 + 0.125)))
        .with_attr("yes", Value::Bool(true))
        .with_attr("s", Value::String("plain".into()))
        .with_attr("p", Value::Path("Public/Foo/bar.lsf".into()))
        .with_attr("fs", Value::FixedString("Fixed".into()))
        .with_attr(
            "ls",
            Value::LSString("multi\nline \"quoted\" & <tagged>".into()),
        )
        .with_attr("u64", Value::ULongLong(u64::MAX))
        .with_attr("buf", Value::ScratchBuffer(vec![0, 1, 2, 250, 255]))
        .with_attr("old64", Value::OldInt64(-5))
        .with_attr("i8", Value::Int8(-7))
        .with_attr(
            "ts",
            Value::TranslatedString(TranslatedString {
                value: None,
                handle: "h12345678g1234g1234g1234g123456789abc".into(),
                version: 3,
            }),
        )
        .with_attr("ws", Value::WString("wide".into()))
        .with_attr("lws", Value::LSWString("ls wide".into()))
        .with_attr(
            "id",
            Value::Uuid(guid("00112233-4455-6677-8899-aabbccddeeff")),
        )
        .with_attr("i64", Value::Int64(i64::MIN))
        .with_attr("tfs", Value::TranslatedFSString(fs));
    let mut keyed = Node::new("Entry").with_attr("MapKey", Value::FixedString("k1".into()));
    keyed.key = Some("MapKey".into());
    let root = Node::new("root")
        .with_child(all)
        .with_child(keyed)
        .with_child(Node::new("Empty"))
        .with_child(Node::new("Entry").with_child(Node::new("Deep").with_attr("x", Value::Int(1))));
    Resource {
        metadata: Metadata {
            version: EngineVersion {
                major: 4,
                minor: 6,
                revision: 0,
                build: 900,
            },
            ..Default::default()
        },
        regions: vec![
            Region {
                name: "First".into(),
                node: Node {
                    name: "First".into(),
                    ..root
                },
            },
            Region {
                name: "Second".into(),
                node: Node::new("Second").with_attr("n", Value::Int(2)),
            },
        ],
    }
}

/// Compares trees and versions, ignoring container-specific metadata.
fn assert_same(a: &Resource, b: &Resource) {
    assert_eq!(a.metadata.version, b.metadata.version);
    assert_eq!(a.regions, b.regions);
}

#[test]
fn lsf_roundtrip_every_variant() {
    let original = sample();
    for version in [2, 3, 4, 5, 6, 7] {
        for extended in [false, true] {
            for compression in [
                Compression::None,
                Compression::Lz4,
                Compression::Zstd,
                Compression::Zlib,
            ] {
                let mut res = original.clone();
                let keys_ok = version >= 6 && extended;
                if !keys_ok {
                    strip_keys(&mut res);
                }
                if version < 4 {
                    // Pre-BG3 translated strings carry inline text, no version.
                    set_inline_text(&mut res);
                }
                if version < 5 {
                    res.metadata.version = EngineVersion {
                        major: 3,
                        minor: 6,
                        revision: 9,
                        build: 1234,
                    };
                }
                let opts = WriteOptions {
                    version,
                    extended,
                    compression,
                };
                let bytes = to_lsf(&res, opts).unwrap();
                let back = parse(&bytes).unwrap_or_else(|e| panic!("{opts:?}: {e}"));
                assert_same(&res, &back);
                let info = back.metadata.lsf.unwrap();
                assert_eq!(info.version, version);
                assert_eq!(info.extended, extended);
            }
        }
    }
}

fn strip_keys(res: &mut Resource) {
    fn walk(n: &mut Node) {
        n.key = None;
        n.children.iter_mut().for_each(walk);
    }
    res.regions.iter_mut().for_each(|r| walk(&mut r.node));
}

fn set_inline_text(res: &mut Resource) {
    fn fix(t: &mut TranslatedString) {
        t.version = 0;
        t.value = Some(format!("text of {}", t.handle));
    }
    fn fix_fs(t: &mut TranslatedFSString) {
        fix(&mut t.string);
        t.arguments.iter_mut().for_each(|a| fix_fs(&mut a.string));
    }
    fn walk(n: &mut Node) {
        for a in &mut n.attributes {
            match &mut a.value {
                Value::TranslatedString(t) => fix(t),
                Value::TranslatedFSString(t) => fix_fs(t),
                _ => {}
            }
        }
        n.children.iter_mut().for_each(walk);
    }
    res.regions.iter_mut().for_each(|r| walk(&mut r.node));
}

/// Hand-assembled minimal v7 file, independent of our writer, to pin the on-disk layout.
#[test]
fn lsf_hand_built_layout() {
    let mut f = Vec::new();
    f.extend_from_slice(b"LSOF");
    f.extend_from_slice(&7u32.to_le_bytes());
    // Engine version 4.6.0.1 packed into 64 bits.
    f.extend_from_slice(&((4u64 << 55) | (6 << 47) | 1).to_le_bytes());
    // Strings: 1 bucket with "Reg", "Val", "Key".
    let mut strings = 1u32.to_le_bytes().to_vec();
    strings.extend_from_slice(&3u16.to_le_bytes());
    for s in ["Reg", "Val", "Key"] {
        strings.extend_from_slice(&(s.len() as u16).to_le_bytes());
        strings.extend_from_slice(s.as_bytes());
    }
    // Nodes (extended): Reg (root), Key (child of 0).
    let mut nodes = Vec::new();
    for v in [0i32, -1, -1, 0, 2, 0, -1, 1] {
        nodes.extend_from_slice(&v.to_le_bytes());
    }
    // Attributes: Val:int32=7 on Reg, Key:FixedString "a" on node 1.
    let mut attrs = Vec::new();
    for v in [1u32, 4 | 4 << 6, u32::MAX, 0, 2, 22 | 2 << 6, u32::MAX, 4] {
        attrs.extend_from_slice(&v.to_le_bytes());
    }
    let mut values = 7i32.to_le_bytes().to_vec();
    values.extend_from_slice(b"a\0");
    // Keys: node 1 keyed by "Key".
    let mut keys = 1u32.to_le_bytes().to_vec();
    keys.extend_from_slice(&2u32.to_le_bytes());
    // Header: strings, keys, nodes, attributes, values as (uncompressed, on-disk=0).
    for s in [&strings, &keys, &nodes, &attrs, &values] {
        f.extend_from_slice(&(s.len() as u32).to_le_bytes());
        f.extend_from_slice(&0u32.to_le_bytes());
    }
    f.extend_from_slice(&[0, 0, 0, 0]);
    f.extend_from_slice(&1u32.to_le_bytes());
    for s in [&strings, &nodes, &attrs, &values, &keys] {
        f.extend_from_slice(s);
    }

    let res = parse(&f).unwrap();
    assert_eq!(res.metadata.version.to_string(), "4.6.0.1");
    assert_eq!(res.regions.len(), 1);
    let reg = res.region("Reg").unwrap();
    assert_eq!(reg.attr("Val"), Some(&Value::Int(7)));
    let child = &reg.children[0];
    assert_eq!(child.name, "Key");
    assert_eq!(child.key.as_deref(), Some("Key"));
    assert_eq!(child.attr_str("Key"), Some("a"));
}

#[test]
fn lsf_rejects_garbage() {
    assert!(matches!(parse_lsf(b"LSOX\0\0\0\0"), Err(Error::BadMagic)));
    assert!(matches!(
        parse_lsf(b"LSOF\x09\0\0\0"),
        Err(Error::UnsupportedVersion(9))
    ));
    let mut bytes = to_lsf(&sample(), WriteOptions::default()).unwrap();
    bytes.truncate(bytes.len() - 10);
    assert!(parse(&bytes).is_err());
}

#[test]
fn guid_byte_order() {
    let g = guid("00112233-4455-6677-8899-AABBCCDDEEFF");
    assert_eq!(g.to_string(), "00112233-4455-6677-8899-aabbccddeeff");
    assert_eq!(
        g.to_lsf_bytes(),
        [
            0x33, 0x22, 0x11, 0x00, 0x55, 0x44, 0x77, 0x66, 0x99, 0x88, 0xbb, 0xaa, 0xdd, 0xcc,
            0xff, 0xee
        ]
    );
    assert_eq!(Guid::from_lsf_bytes(g.to_lsf_bytes()), g);
    assert!(Guid::parse("0011").is_none());
}

const LSX: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<save>
    <header version="1" time="12"/>
    <version major="4" minor="0" revision="9" build="331"/>
    <region id="Config">
        <node id="root">
            <attribute id="Name" type="FixedString" value="Thing"/>
            <attribute id="Text" type="LSString" value="line one
line two &amp; &quot;more&quot;"/>
            <attribute id="Old" value="5" type="4"/>
            <attribute id="Id" type="guid" value="01234567-89ab-cdef-0123-456789abcdef"/>
            <attribute id="Pos" type="fvec3">
                <float3 x="1" y="-0" z="2.5"/>
            </attribute>
            <attribute id="Flat" type="fvec2" value="0.5 1"/>
            <attribute id="Rot" type="mat3x3">
                <mat3>
                    <float3 x="1" y="0" z="0"/>
                    <float3 x="0" y="1" z="0"/>
                    <float3 x="0" y="0" z="1"/>
                </mat3>
            </attribute>
            <attribute id="Label" type="TranslatedString" handle="habc" version="4"/>
            <attribute id="Fmt" type="TranslatedFSString" handle="hfmt" version="1" arguments="1">
                <arguments>
                    <argument key="X" value="1">
                        <string handle="harg" version="2" arguments="0"/>
                    </argument>
                </arguments>
            </attribute>
            <children>
                <node id="Item" key="Name">
                    <attribute id="Name" type="FixedString" value="a"/>
                </node>
                <node id="Empty"/>
            </children>
        </node>
    </region>
</save>
"#;

#[test]
fn lsx_reads_larian_style() {
    let res = parse(LSX.as_bytes()).unwrap();
    assert_eq!(res.metadata.format, SourceFormat::Lsx);
    assert_eq!(res.metadata.version.to_string(), "4.0.9.331");
    assert_eq!(res.metadata.timestamp, Some(12));
    assert_eq!(res.regions[0].name, "Config");
    let root = res.region("Config").unwrap();
    assert_eq!(root.name, "root");
    assert_eq!(root.attr_str("Name"), Some("Thing"));
    assert_eq!(root.attr_str("Text"), Some("line one\nline two & \"more\""));
    assert_eq!(root.attr("Old"), Some(&Value::Int(5)));
    assert_eq!(
        root.attr("Id").unwrap().as_guid().unwrap().to_string(),
        "01234567-89ab-cdef-0123-456789abcdef"
    );
    assert_eq!(root.attr("Pos"), Some(&Value::Vec3([1.0, -0.0, 2.5])));
    assert_eq!(root.attr("Flat"), Some(&Value::Vec2([0.5, 1.0])));
    assert_eq!(
        root.attr("Rot"),
        Some(&Value::Mat3([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]))
    );
    let Some(Value::TranslatedString(t)) = root.attr("Label") else {
        panic!()
    };
    assert_eq!((t.handle.as_str(), t.version), ("habc", 4));
    let Some(Value::TranslatedFSString(f)) = root.attr("Fmt") else {
        panic!()
    };
    assert_eq!(f.arguments[0].key, "X");
    assert_eq!(f.arguments[0].string.string.handle, "harg");
    assert_eq!(root.children.len(), 2);
    assert_eq!(root.children[0].key.as_deref(), Some("Name"));
}

#[test]
fn lsx_writer_roundtrips() {
    let original = sample();
    let text = to_lsx(&original);
    let back = parse_lsx(text.as_bytes()).unwrap();
    assert_same(&original, &back);
    // And the parsed fixture survives write → read too.
    let fixture = parse(LSX.as_bytes()).unwrap();
    let again = parse(to_lsx(&fixture).as_bytes()).unwrap();
    assert_eq!(fixture, again);
}

#[test]
fn lsx_lsf_agree() {
    let res = parse(LSX.as_bytes()).unwrap();
    let lsf = parse(&to_lsf(&res, WriteOptions::default()).unwrap()).unwrap();
    // LSF names the region after its node; otherwise the trees match.
    assert_eq!(
        lsf.regions[0].node.attributes,
        res.regions[0].node.attributes
    );
    assert_eq!(lsf.regions[0].node.children, res.regions[0].node.children);
}

const LSJ: &str = r#"{
   "save" : {
      "header" : { "time" : 99, "version" : "4.6.0.900" },
      "regions" : {
         "dialog" : {
            "Children" : [
               { "Value" : {"type" : "int32", "value" : -1} },
               { "Value" : {"type" : 4, "value" : 2} }
            ],
            "Flag" : {"type" : "bool", "value" : true},
            "Name" : {"type" : 22, "value" : "Any"},
            "Id" : {"type" : "guid", "value" : "01234567-89ab-cdef-0123-456789abcdef"},
            "Speed" : {"type" : "float", "value" : 1.5},
            "Big" : {"type" : "uint64", "value" : 18446744073709551615},
            "Text" : {"handle" : "hxyz", "type" : "TranslatedString", "version" : 1},
            "Fmt" : {"type" : "TranslatedFSString", "handle" : "hf", "version" : 1,
                     "arguments" : [{"key" : "K", "value" : "V", "string" : {"handle" : "ha", "version" : 2}}]},
            "Pos" : {"type" : "fvec3", "value" : "1 2 3"}
         }
      }
   }
}"#;

#[test]
fn lsj_reads() {
    let res = parse(LSJ.as_bytes()).unwrap();
    assert_eq!(res.metadata.format, SourceFormat::Lsj);
    assert_eq!(res.metadata.version.to_string(), "4.6.0.900");
    assert_eq!(res.metadata.timestamp, Some(99));
    let d = res.region("dialog").unwrap();
    assert_eq!(d.name, "dialog");
    let names: Vec<_> = d.attributes.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(
        names,
        ["Flag", "Name", "Id", "Speed", "Big", "Text", "Fmt", "Pos"]
    );
    assert_eq!(d.children.len(), 2);
    assert_eq!(d.children[1].attr("Value"), Some(&Value::Int(2)));
    assert_eq!(d.attr("Flag"), Some(&Value::Bool(true)));
    assert_eq!(d.attr("Name"), Some(&Value::FixedString("Any".into())));
    assert_eq!(d.attr("Speed"), Some(&Value::Float(1.5)));
    assert_eq!(d.attr("Big"), Some(&Value::ULongLong(u64::MAX)));
    assert_eq!(d.attr("Pos"), Some(&Value::Vec3([1.0, 2.0, 3.0])));
    let Some(Value::TranslatedFSString(f)) = d.attr("Fmt") else {
        panic!()
    };
    assert_eq!(f.arguments[0].string.string.version, 2);
}

#[test]
fn format_detection() {
    assert!(matches!(parse(b"   "), Err(Error::UnknownFormat)));
    assert!(parse(b"\xEF\xBB\xBF<save></save>").is_ok());
    assert!(parse(b"{\"save\":{}}").is_ok());
    assert!(parse(b"<save><bogus/></save>").is_err());
}

#[test]
fn value_text_forms() {
    assert_eq!(
        Value::from_text(AttributeType::Bool, "True").unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        Value::from_text(AttributeType::Float, "6.1035156e-05").unwrap(),
        Value::Float(6.1035156e-05)
    );
    assert!(Value::from_text(AttributeType::Int, "x").is_err());
    assert_eq!(
        Value::from_text(AttributeType::ScratchBuffer, "AAEC").unwrap(),
        Value::ScratchBuffer(vec![0, 1, 2])
    );
    for &ty in AttributeType::ALL {
        assert_eq!(AttributeType::from_name(ty.name()), Some(ty));
        assert_eq!(AttributeType::from_id(ty.id()), Some(ty));
    }
}
