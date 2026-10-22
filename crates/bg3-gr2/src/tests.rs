//! Synthetic, self-authored GR2 fixtures: a tiny writer for the 64-bit layout, then round-trips
//! through the reader, the generic walker and the typed scene extraction.

use crate::header::{Format, MAGIC_BLOCK_SIZE, SECTION_HEADER_SIZE};
use crate::reflect::MEMBER_DEF_SIZE;
use crate::*;

const INLINE: u32 = 1;
const REFERENCE: u32 = 2;
const REF_TO_ARRAY: u32 = 3;
const ARRAY_OF_REFS: u32 = 4;
const VARIANT_REF: u32 = 5;
const REF_TO_VARIANT_ARRAY: u32 = 7;
const STRING: u32 = 8;
const TRANSFORM: u32 = 9;
const REAL32: u32 = 10;
const UINT8: u32 = 12;
const NORMAL_UINT8: u32 = 14;
const INT16: u32 = 15;
const BINORMAL_INT16: u32 = 17;
const INT32: u32 = 19;
const REAL16: u32 = 21;

/// Pointer target: (section, offset).
type Target = (u32, u32);

#[derive(Default)]
struct Section {
    data: Vec<u8>,
    /// (slot offset, target)
    relocs: Vec<(u32, Target)>,
}

impl Section {
    fn here(&self) -> u32 {
        self.data.len() as u32
    }
    fn u32(&mut self, v: u32) {
        self.data.extend_from_slice(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.data.extend_from_slice(&v.to_le_bytes());
    }
    fn ptr(&mut self, t: Option<Target>) {
        if let Some(t) = t {
            self.relocs.push((self.here(), t));
        }
        self.data.extend_from_slice(&[0; 8]);
    }
    fn str(&mut self, s: &str) -> u32 {
        let at = self.here();
        self.data.extend_from_slice(s.as_bytes());
        self.data.push(0);
        at
    }
    /// Write a type definition (in this section, which must be section `me`).
    fn ty(&mut self, me: u32, members: &[(u32, &str, Option<Target>, u32)]) -> Target {
        let names: Vec<u32> = members.iter().map(|m| self.str(m.1)).collect();
        while !self.data.len().is_multiple_of(4) {
            self.data.push(0);
        }
        let at = self.here();
        for (m, name) in members.iter().zip(names) {
            self.u32(m.0);
            self.ptr(Some((me, name)));
            self.ptr(m.2);
            self.u32(m.3);
            self.data.extend_from_slice(&[0; 12 + 8]);
        }
        self.data.extend_from_slice(&[0; MEMBER_DEF_SIZE]);
        (me, at)
    }
    fn transform(&mut self, pos: [f32; 3]) {
        self.u32(1);
        for v in pos {
            self.f32(v);
        }
        for v in [
            0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0,
        ] {
            self.f32(v);
        }
    }
}

/// Wrap `data` as a BitKnit 2 stream made of stored quanta.
fn bitknit_stored(data: &[u8]) -> Vec<u8> {
    let mut s = bitknit::MAGIC.to_le_bytes().to_vec();
    for chunk in data.chunks(0x10000) {
        s.extend_from_slice(&[0, 0]);
        s.extend_from_slice(chunk);
    }
    if !s.len().is_multiple_of(2) {
        s.push(0);
    }
    s
}

fn write_file(sections: &[Section], root_type: Target, root: Target, compress: bool) -> Vec<u8> {
    let n = sections.len();
    let header_size = MAGIC_BLOCK_SIZE + 0x48 + n * SECTION_HEADER_SIZE;
    let mut out = vec![0u8; header_size];
    out[..16].copy_from_slice(&Format::LittleEndian64.magic());
    out[0x10..0x14].copy_from_slice(&(header_size as u32).to_le_bytes());
    let mut sec_headers = Vec::new();
    for s in sections {
        let payload = if compress {
            bitknit_stored(&s.data)
        } else {
            s.data.clone()
        };
        let data_offset = out.len() as u32;
        out.extend_from_slice(&payload);
        let reloc_offset = out.len() as u32;
        let mut table = Vec::new();
        for (at, (sec, off)) in &s.relocs {
            for v in [*at, *sec, *off] {
                table.extend_from_slice(&v.to_le_bytes());
            }
        }
        if compress && !table.is_empty() {
            let packed = bitknit_stored(&table);
            out.extend_from_slice(&(packed.len() as u32).to_le_bytes());
            out.extend_from_slice(&packed);
        } else {
            out.extend_from_slice(&table);
        }
        let fields = [
            if compress { 4 } else { 0 },
            data_offset,
            payload.len() as u32,
            s.data.len() as u32,
            4,
            s.data.len() as u32,
            s.data.len() as u32,
            reloc_offset,
            s.relocs.len() as u32,
            out.len() as u32,
            0,
        ];
        sec_headers.push(fields);
    }
    let total = out.len() as u32;
    let fi = MAGIC_BLOCK_SIZE;
    let info = [
        7,
        total,
        0,
        0x48,
        n as u32,
        root_type.0,
        root_type.1,
        root.0,
        root.1,
        0x8000_0039,
    ];
    for (i, v) in info.iter().enumerate() {
        out[fi + 4 * i..fi + 4 * i + 4].copy_from_slice(&v.to_le_bytes());
    }
    for (i, h) in sec_headers.iter().enumerate() {
        let base = fi + 0x48 + i * SECTION_HEADER_SIZE;
        for (k, v) in h.iter().enumerate() {
            out[base + 4 * k..base + 4 * k + 4].copy_from_slice(&v.to_le_bytes());
        }
    }
    out
}

fn f16(v: f32) -> u16 {
    // Exact for the simple values used here (0, 0.5, 1).
    match v {
        0.0 => 0,
        0.5 => 0x3800,
        1.0 => 0x3C00,
        _ => unreachable!(),
    }
}

/// A one-triangle skinned mesh, a two-bone skeleton and a model binding both. Types live in
/// section 1, data in section 0, so relocations cross sections.
fn build_scene(compress: bool) -> Vec<u8> {
    let mut t = Section::default();
    let mut d = Section::default();
    const T: u32 = 1;
    const D: u32 = 0;

    let bone_t = t.ty(
        T,
        &[
            (STRING, "Name", None, 0),
            (INT32, "ParentIndex", None, 0),
            (TRANSFORM, "Transform", None, 0),
            (REAL32, "InverseWorldTransform", None, 16),
            (REAL32, "LODError", None, 0),
        ],
    );
    let skel_t = t.ty(
        T,
        &[
            (STRING, "Name", None, 0),
            (REF_TO_ARRAY, "Bones", Some(bone_t), 0),
            (INT32, "LODType", None, 0),
        ],
    );
    let vert_t = t.ty(
        T,
        &[
            (REAL32, "Position", None, 3),
            (NORMAL_UINT8, "BoneWeights", None, 4),
            (UINT8, "BoneIndices", None, 4),
            (BINORMAL_INT16, "QTangent", None, 4),
            (REAL16, "TextureCoordinates0", None, 2),
        ],
    );
    let vdata_t = t.ty(T, &[(REF_TO_VARIANT_ARRAY, "Vertices", None, 0)]);
    let group_t = t.ty(
        T,
        &[
            (INT32, "MaterialIndex", None, 0),
            (INT32, "TriFirst", None, 0),
            (INT32, "TriCount", None, 0),
        ],
    );
    let i32_t = t.ty(T, &[(INT32, "Int32", None, 0)]);
    let i16_t = t.ty(T, &[(INT16, "Int16", None, 0)]);
    let topo_t = t.ty(
        T,
        &[
            (REF_TO_ARRAY, "Groups", Some(group_t), 0),
            (REF_TO_ARRAY, "Indices", Some(i32_t), 0),
            (REF_TO_ARRAY, "Indices16", Some(i16_t), 0),
        ],
    );
    let mat_t = t.ty(T, &[(STRING, "Name", None, 0)]);
    let matbind_t = t.ty(T, &[(REFERENCE, "Material", Some(mat_t), 0)]);
    let bonebind_t = t.ty(
        T,
        &[
            (STRING, "BoneName", None, 0),
            (REAL32, "OBBMin", None, 3),
            (REAL32, "OBBMax", None, 3),
        ],
    );
    let props_t = t.ty(T, &[(REAL32, "Flags", None, 4), (INT32, "Lod", None, 1)]);
    let ext_t = t.ty(
        T,
        &[
            (REFERENCE, "UserMeshProperties", Some(props_t), 0),
            (INT32, "LSMVersion", None, 0),
        ],
    );
    let mesh_t = t.ty(
        T,
        &[
            (STRING, "Name", None, 0),
            (REFERENCE, "PrimaryVertexData", Some(vdata_t), 0),
            (REFERENCE, "PrimaryTopology", Some(topo_t), 0),
            (REF_TO_ARRAY, "MaterialBindings", Some(matbind_t), 0),
            (REF_TO_ARRAY, "BoneBindings", Some(bonebind_t), 0),
            (VARIANT_REF, "ExtendedData", None, 0),
        ],
    );
    let meshbind_t = t.ty(T, &[(REFERENCE, "Mesh", Some(mesh_t), 0)]);
    let model_t = t.ty(
        T,
        &[
            (STRING, "Name", None, 0),
            (REFERENCE, "Skeleton", Some(skel_t), 0),
            (TRANSFORM, "InitialPlacement", None, 0),
            (REF_TO_ARRAY, "MeshBindings", Some(meshbind_t), 0),
        ],
    );
    let art_t = t.ty(
        T,
        &[
            (STRING, "FromArtToolName", None, 0),
            (REAL32, "UnitsPerMeter", None, 0),
            (REAL32, "Origin", None, 3),
            (REAL32, "RightVector", None, 3),
            (REAL32, "UpVector", None, 3),
            (REAL32, "BackVector", None, 3),
        ],
    );
    let pair_t = t.ty(T, &[(INT32, "A", None, 0), (INT32, "B", None, 0)]);
    let root_t = t.ty(
        T,
        &[
            (REFERENCE, "ArtToolInfo", Some(art_t), 0),
            (STRING, "FromFileName", None, 0),
            (ARRAY_OF_REFS, "Materials", Some(mat_t), 0),
            (ARRAY_OF_REFS, "Skeletons", Some(skel_t), 0),
            (ARRAY_OF_REFS, "Meshes", Some(mesh_t), 0),
            (ARRAY_OF_REFS, "Models", Some(model_t), 0),
            (INLINE, "Pair", Some(pair_t), 2),
        ],
    );

    // --- data (section 0) ---
    let s_file = d.str("synthetic.ma");
    let s_tool = d.str("TestTool");
    let s_root_bone = d.str("Root");
    let s_child_bone = d.str("Child");
    let s_skel = d.str("Rig");
    let s_mesh = d.str("Tri");
    let s_model = d.str("Thing");
    let s_mat = d.str("Mat0");

    let art = d.here();
    d.ptr(Some((D, s_tool)));
    for v in [
        100.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, -1.0,
    ] {
        d.f32(v);
    }

    let bones = d.here();
    for (name, parent, z) in [(s_root_bone, -1i32, 0.0f32), (s_child_bone, 0, 2.0)] {
        d.ptr(Some((D, name)));
        d.u32(parent as u32);
        d.transform([0.0, 0.0, z]);
        for i in 0..16 {
            d.f32(match i {
                0 | 5 | 10 | 15 => 1.0,
                14 => -z,
                _ => 0.0,
            });
        }
        d.f32(0.5);
    }
    let skel = d.here();
    d.ptr(Some((D, s_skel)));
    d.u32(2);
    d.ptr(Some((D, bones)));
    d.u32(1);

    // Three vertices. Identity quaternion with w > 0, and one with w < 0 (mirrored frame).
    let verts = d.here();
    let quats: [[i16; 4]; 3] = [[0, 0, 0, 32767], [0, 0, 0, 32767], [0, 0, 0, -32767]];
    let pos = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    let uv = [[0.0, 0.0], [1.0, 0.0], [0.0, 0.5]];
    for v in 0..3 {
        for p in pos[v] {
            d.f32(p);
        }
        d.data.extend_from_slice(&[255, 0, 0, 0]);
        d.data.extend_from_slice(&[v as u8 % 2, 0, 0, 0]);
        for q in quats[v] {
            d.data.extend_from_slice(&q.to_le_bytes());
        }
        for c in uv[v] {
            d.data.extend_from_slice(&f16(c).to_le_bytes());
        }
    }
    let vdata = d.here();
    d.ptr(Some(vert_t));
    d.u32(3);
    d.ptr(Some((D, verts)));

    let groups = d.here();
    for v in [0, 0, 1] {
        d.u32(v);
    }
    let idx16 = d.here();
    for i in [0u16, 1, 2] {
        d.data.extend_from_slice(&i.to_le_bytes());
    }
    let topo = d.here();
    d.u32(1);
    d.ptr(Some((D, groups)));
    d.u32(0);
    d.ptr(None);
    d.u32(3);
    d.ptr(Some((D, idx16)));

    let mat = d.here();
    d.ptr(Some((D, s_mat)));
    let matbind = d.here();
    d.ptr(Some((D, mat)));
    let bonebind = d.here();
    d.ptr(Some((D, s_child_bone)));
    for v in [-1.0, -1.0, -1.0, 1.0, 1.0, 1.0] {
        d.f32(v);
    }
    let props = d.here();
    for v in [1.0, 2.0, 0.0, 0.0] {
        d.f32(v);
    }
    d.u32(3);
    let ext = d.here();
    d.ptr(Some((D, props)));
    d.u32(3);

    let mesh = d.here();
    d.ptr(Some((D, s_mesh)));
    d.ptr(Some((D, vdata)));
    d.ptr(Some((D, topo)));
    d.u32(1);
    d.ptr(Some((D, matbind)));
    d.u32(1);
    d.ptr(Some((D, bonebind)));
    d.ptr(Some(ext_t));
    d.ptr(Some((D, ext)));

    let meshbind = d.here();
    d.ptr(Some((D, mesh)));
    let model = d.here();
    d.ptr(Some((D, s_model)));
    d.ptr(Some((D, skel)));
    d.transform([1.0, 2.0, 3.0]);
    d.u32(1);
    d.ptr(Some((D, meshbind)));

    // Arrays of references.
    let mats = d.here();
    d.ptr(Some((D, mat)));
    let skels = d.here();
    d.ptr(Some((D, skel)));
    let meshes = d.here();
    d.ptr(None); // a null entry is skipped
    d.ptr(Some((D, mesh)));
    let models = d.here();
    d.ptr(Some((D, model)));

    let root = d.here();
    d.ptr(Some((D, art)));
    d.ptr(Some((D, s_file)));
    for (n, at) in [(1, mats), (1, skels), (2, meshes), (1, models)] {
        d.u32(n);
        d.ptr(Some((D, at)));
    }
    for v in [10, 20, 30, 40] {
        d.u32(v);
    }

    write_file(&[d, t], root_t, (D, root), compress)
}

#[test]
fn header_roundtrip_and_errors() {
    let bytes = build_scene(false);
    let h = Header::parse(&bytes).unwrap();
    assert_eq!(h.format, Format::LittleEndian64);
    assert_eq!(h.revision, 7);
    assert_eq!(h.sections.len(), 2);
    assert_eq!(h.sections[0].compression, Compression::None);

    let mut bad = bytes.clone();
    bad[0] ^= 0xFF;
    assert!(matches!(Header::parse(&bad), Err(Error::BadMagic)));
    let mut be = bytes.clone();
    be[..16].copy_from_slice(&Format::BigEndian64.magic());
    assert!(matches!(
        Header::parse(&be),
        Err(Error::UnsupportedFormat(Format::BigEndian64))
    ));
    let mut rev = bytes.clone();
    rev[0x20] = 6;
    assert!(matches!(
        Header::parse(&rev),
        Err(Error::UnsupportedRevision(6))
    ));
    assert!(Gr2File::parse(&bytes[..0x40]).is_err());
}

#[test]
fn generic_walk() {
    for compress in [false, true] {
        let bytes = build_scene(compress);
        let file = Gr2File::parse(&bytes).unwrap();
        let types = Types::new(&file);
        let root = types.root().unwrap();
        assert_eq!(
            root.string("FromFileName").unwrap().unwrap(),
            "synthetic.ma"
        );
        let pair = root.member("Pair").unwrap();
        assert_eq!(
            (pair.kind, pair.size, pair.count()),
            (MemberType::Inline, 16, 2)
        );
        let v = root.to_value(8, 8);
        assert_eq!(
            v.get("Pair"),
            Some(&Value::Array(
                vec![
                    Value::Struct(vec![
                        ("A".into(), Value::Int(10)),
                        ("B".into(), Value::Int(20))
                    ]),
                    Value::Struct(vec![
                        ("A".into(), Value::Int(30)),
                        ("B".into(), Value::Int(40))
                    ]),
                ],
                2
            ))
        );
        let Some(Value::Array(meshes, 2)) = v.get("Meshes") else {
            panic!("meshes: {:?}", v.get("Meshes"));
        };
        assert_eq!(meshes[0], Value::Null);
        assert_eq!(meshes[1].get("Name"), Some(&Value::String("Tri".into())));
        let mut dump = String::new();
        v.write_tree(&mut dump, 0);
        assert!(dump.contains("TextureCoordinates0: [0, 0]"), "{dump}");
        // Errors on missing / mistyped members.
        assert!(matches!(root.int("Nope"), Err(Error::MissingMember(_))));
        assert!(matches!(
            root.string("Meshes"),
            Err(Error::WrongMemberType { .. })
        ));
    }
}

#[test]
fn typed_scene() {
    for compress in [false, true] {
        let scene = Scene::from_bytes(&build_scene(compress)).unwrap();
        assert_eq!(scene.from_file_name, "synthetic.ma");
        let art = scene.art_tool.as_ref().unwrap();
        assert_eq!(
            (art.name.as_str(), art.units_per_meter),
            ("TestTool", 100.0)
        );
        assert_eq!(art.back, [0.0, 0.0, -1.0]);
        assert_eq!(scene.materials, ["Mat0"]);

        let skel = &scene.skeletons[0];
        assert_eq!(skel.name, "Rig");
        assert_eq!(skel.bones.len(), 2);
        assert_eq!(skel.bones[0].parent, None);
        assert_eq!(skel.bones[1].parent, Some(0));
        assert_eq!(skel.bones[1].transform.position, [0.0, 0.0, 2.0]);
        assert_eq!(skel.bones[1].inverse_world[14], -2.0);

        assert_eq!(scene.meshes.len(), 1);
        let m = &scene.meshes[0];
        assert_eq!(m.name, "Tri");
        assert_eq!(m.vertex_count(), 3);
        assert_eq!(m.positions[1], [1.0, 0.0, 0.0]);
        assert_eq!(m.indices, [0, 1, 2]);
        assert_eq!(m.index_width, IndexWidth::U16);
        assert_eq!(
            m.groups,
            [TopologyGroup {
                material_index: 0,
                tri_first: 0,
                tri_count: 1
            }]
        );
        assert_eq!(m.uvs.len(), 1);
        assert_eq!(m.uvs[0][2], [0.0, 0.5]);
        assert_eq!(m.bone_weights[0], [1.0, 0.0, 0.0, 0.0]);
        assert_eq!(m.bone_indices[1], [1, 0, 0, 0]);
        assert_eq!(m.material_bindings, [Some("Mat0".to_owned())]);
        assert_eq!(m.bone_bindings[0].bone_name, "Child");
        // Identity quaternion: normal +Z, tangent +X; w < 0 flips the normal.
        assert_eq!(m.normals[0], [0.0, 0.0, 1.0]);
        assert_eq!(m.tangents[0], [1.0, 0.0, 0.0, -1.0]);
        assert_eq!(m.normals[2], [0.0, 0.0, -1.0]);
        assert_eq!(m.tangents[2][3], 1.0);
        let props = m.properties.as_ref().unwrap();
        assert_eq!(props.lsm_version, Some(3));
        assert_eq!(props.lod, Some(3));
        assert_eq!(props.flags, [1, 2, 0, 0]);

        let model = &scene.models[0];
        assert_eq!(model.name, "Thing");
        assert_eq!(model.skeleton, Some(0));
        assert_eq!(model.meshes, [0]);
        assert_eq!(model.initial_placement.position, [1.0, 2.0, 3.0]);
    }
}

#[test]
fn bad_relocation_is_rejected() {
    let mut s = Section::default();
    s.u32(0);
    s.ptr(Some((5, 0))); // section 5 does not exist
    let bytes = write_file(&[s], (0, 0), (0, 0), false);
    assert!(matches!(Gr2File::parse(&bytes), Err(Error::Corrupt(_))));
}

#[test]
fn half_floats() {
    use crate::reflect::f16_to_f32;
    assert_eq!(f16_to_f32(0x3C00), 1.0);
    assert_eq!(f16_to_f32(0xC000), -2.0);
    assert_eq!(f16_to_f32(0x3800), 0.5);
    assert_eq!(f16_to_f32(0x0001), 2f32.powi(-24));
    assert_eq!(f16_to_f32(0x7C00), f32::INFINITY);
    assert!(f16_to_f32(0x7E00).is_nan());
}

/// A file with one animation: a bone with a quantised quaternion curve, a constant position and
/// an identity scale/shear, plus a float vector track. Types in section 1, data in section 0.
fn build_animation() -> Vec<u8> {
    const UINT16: u32 = 16;
    let mut t = Section::default();
    let mut d = Section::default();
    const T: u32 = 1;
    const D: u32 = 0;

    let header_t = t.ty(T, &[(UINT8, "Format", None, 0), (UINT8, "Degree", None, 0)]);
    let u16_t = t.ty(T, &[(UINT16, "UInt16", None, 0)]);
    let f32_t = t.ty(T, &[(REAL32, "Real32", None, 0)]);
    let d4n_t = t.ty(
        T,
        &[
            (INLINE, "CurveDataHeader_D4nK16uC15u", Some(header_t), 0),
            (UINT16, "ScaleOffsetTableEntries", None, 0),
            (REAL32, "OneOverKnotScale", None, 0),
            (REF_TO_ARRAY, "KnotsControls", Some(u16_t), 0),
        ],
    );
    let d3c_t = t.ty(
        T,
        &[
            (INLINE, "CurveDataHeader_D3Constant32f", Some(header_t), 0),
            (INT16, "Padding", None, 0),
            (REAL32, "Controls", None, 3),
        ],
    );
    let ident_t = t.ty(
        T,
        &[
            (INLINE, "CurveDataHeader_DaIdentity", Some(header_t), 0),
            (INT16, "Dimension", None, 0),
        ],
    );
    let float_t = t.ty(
        T,
        &[
            (INLINE, "CurveDataHeader_DaK32fC32f", Some(header_t), 0),
            (INT16, "Padding", None, 0),
            (REF_TO_ARRAY, "Knots", Some(f32_t), 0),
            (REF_TO_ARRAY, "Controls", Some(f32_t), 0),
        ],
    );
    let curve_t = t.ty(T, &[(VARIANT_REF, "CurveData", None, 0)]);
    let track_t = t.ty(
        T,
        &[
            (STRING, "Name", None, 0),
            (INT32, "Flags", None, 0),
            (INLINE, "OrientationCurve", Some(curve_t), 0),
            (INLINE, "PositionCurve", Some(curve_t), 0),
            (INLINE, "ScaleShearCurve", Some(curve_t), 0),
        ],
    );
    let vtrack_t = t.ty(
        T,
        &[
            (STRING, "Name", None, 0),
            (INT32, "TrackKey", None, 0),
            (INT32, "Dimension", None, 0),
            (INLINE, "ValueCurve", Some(curve_t), 0),
        ],
    );
    let group_t = t.ty(
        T,
        &[
            (STRING, "Name", None, 0),
            (REF_TO_ARRAY, "VectorTracks", Some(vtrack_t), 0),
            (REF_TO_ARRAY, "TransformTracks", Some(track_t), 0),
            (TRANSFORM, "InitialPlacement", None, 0),
            (INT32, "AccumulationFlags", None, 0),
            (REAL32, "LoopTranslation", None, 3),
        ],
    );
    let anim_t = t.ty(
        T,
        &[
            (STRING, "Name", None, 0),
            (REAL32, "Duration", None, 0),
            (REAL32, "TimeStep", None, 0),
            (REAL32, "Oversampling", None, 0),
            (ARRAY_OF_REFS, "TrackGroups", Some(group_t), 0),
            (INT32, "DefaultLoopCount", None, 0),
            (INT32, "Flags", None, 0),
        ],
    );
    let root_t = t.ty(T, &[(ARRAY_OF_REFS, "Animations", Some(anim_t), 0)]);

    let s_anim = d.str("Idle");
    let s_group = d.str("Dummy_Root");
    let s_bone = d.str("Spine");
    let s_vt = d.str("Textkeys");

    // Quaternion curve: 2 knots (0 s and 1 s), both controls the identity quaternion
    // (omitted component w = index 3, positive; x, y, z = 0 → word = max/2 for entry 0).
    let kc = d.here();
    let mid = 16384u16; // 16384 / 32767 * sqrt2 - 1/sqrt2 ≈ 0
    for w in [
        0u16,
        30,
        mid,
        mid | 0x8000,
        mid | 0x8000,
        mid,
        mid | 0x8000,
        mid | 0x8000,
    ] {
        d.data.extend_from_slice(&w.to_le_bytes());
    }
    let d4n = d.here();
    d.data.extend_from_slice(&[8, 2]);
    d.data.extend_from_slice(&0u16.to_le_bytes());
    d.f32(30.0);
    d.u32(8);
    d.ptr(Some((D, kc)));
    let d3c = d.here();
    d.data.extend_from_slice(&[4, 0, 0, 0]);
    for v in [0.0, 1.5, 0.0] {
        d.f32(v);
    }
    let ident = d.here();
    d.data.extend_from_slice(&[2, 0]);
    d.data.extend_from_slice(&9i16.to_le_bytes());
    let fknots = d.here();
    for v in [0.0, 1.0] {
        d.f32(v);
    }
    let fctrl = d.here();
    for v in [0.0, 2.0] {
        d.f32(v);
    }
    let fcurve = d.here();
    d.data.extend_from_slice(&[1, 1, 0, 0]);
    d.u32(2);
    d.ptr(Some((D, fknots)));
    d.u32(2);
    d.ptr(Some((D, fctrl)));

    let track = d.here();
    d.ptr(Some((D, s_bone)));
    d.u32(0);
    for (ty, at) in [(d4n_t, d4n), (d3c_t, d3c), (ident_t, ident)] {
        d.ptr(Some(ty));
        d.ptr(Some((D, at)));
    }
    let vtrack = d.here();
    d.ptr(Some((D, s_vt)));
    d.u32(7);
    d.u32(1);
    d.ptr(Some(float_t));
    d.ptr(Some((D, fcurve)));

    let group = d.here();
    d.ptr(Some((D, s_group)));
    d.u32(1);
    d.ptr(Some((D, vtrack)));
    d.u32(1);
    d.ptr(Some((D, track)));
    d.transform([0.0, 0.0, 0.0]);
    d.u32(2);
    for v in [0.0, 0.0, 1.0] {
        d.f32(v);
    }
    let groups = d.here();
    d.ptr(Some((D, group)));

    let anim = d.here();
    d.ptr(Some((D, s_anim)));
    for v in [1.0, 1.0 / 30.0, 2.0] {
        d.f32(v);
    }
    d.u32(1);
    d.ptr(Some((D, groups)));
    d.u32(0);
    d.u32(0);
    let anims = d.here();
    d.ptr(Some((D, anim)));
    let root = d.here();
    d.u32(1);
    d.ptr(Some((D, anims)));

    write_file(&[d, t], root_t, (D, root), true)
}

#[test]
fn synthetic_animation_decodes() {
    let anims = Animation::from_bytes(&build_animation()).unwrap();
    assert_eq!(anims.len(), 1);
    let a = &anims[0];
    assert_eq!(a.name, "Idle");
    assert_eq!((a.duration, a.oversampling), (1.0, 2.0));
    let g = &a.track_groups[0];
    assert_eq!(g.name, "Dummy_Root");
    assert_eq!(
        (g.accumulation_flags, g.loop_translation),
        (2, [0.0, 0.0, 1.0])
    );
    let tr = g.track("Spine").unwrap();
    assert_eq!(tr.orientation.format, CurveFormat::D4nK16uC15u);
    assert_eq!(tr.orientation.knots, vec![0.0, 1.0]);
    assert_eq!(tr.position.format, CurveFormat::D3Constant32f);
    assert!(tr.scale_shear.is_identity());
    let x = tr.sample(0.5);
    for (v, want) in x.orientation.iter().zip([0.0, 0.0, 0.0, 1.0]) {
        assert!((v - want).abs() < 1e-3, "{:?}", x.orientation);
    }
    assert_eq!(x.position, [0.0, 1.5, 0.0]);
    let v = &g.vector_tracks[0];
    assert_eq!(
        (v.name.as_str(), v.track_key, v.dimension),
        ("Textkeys", 7, 1)
    );
    let mut out = [0.0];
    v.curve.sample(0.25, &mut out);
    assert!((out[0] - 0.5).abs() < 1e-6);
}

#[test]
fn proxies_occluders_and_cloth_physics_meshes_are_not_drawn() {
    let props = |flags| MeshProperties {
        flags,
        ..Default::default()
    };
    assert!(props([0, 0, 0, 0]).is_drawn());
    // Rigid, skinned, coloured: drawn.
    assert!(props([0x20 | 0x10 | 0x08, 0, 0, 0]).is_drawn());
    assert!(!props([MeshProperties::MESH_PROXY, 0, 0, 0]).is_drawn());
    assert!(!props([MeshProperties::OCCLUDER, 0, 0, 0]).is_drawn());
    // Cloth: the simulation mesh is hidden, the meshes it drives are drawn.
    assert!(!props([MeshProperties::CLOTH | 0x08, 0, 0x100, 0]).is_drawn());
    assert!(props([MeshProperties::CLOTH | 0x08, 0, 0, 0]).is_drawn());
    assert!(props([MeshProperties::CLOTH, 0, 0x200, 0]).is_drawn());
    // The cloth-kind bits only count on cloth meshes.
    assert!(props([0, 0, 0x100, 0]).is_drawn());
    let mut mesh = Mesh::default();
    assert!(mesh.is_drawn());
    mesh.properties = Some(props([MeshProperties::OCCLUDER, 0, 0, 0]));
    assert!(!mesh.is_drawn());
}
