//! Typed view of a GR2 scene, sufficient for rendering: meshes, skeletons and models.
//!
//! Built on the generic [`reflect`](crate::reflect) walker, so it only depends on member *names*
//! (Granny's standard `granny_file_info` layout) and decodes vertex components from whatever
//! types the file declares.

use crate::file::{Gr2File, Ptr};
use crate::reflect::{MemberType, Object, Transform, Types, numeric_to_f32, numeric_to_i64};
use crate::{Error, Result};

/// Everything renderable in one GR2 file.
#[derive(Debug, Clone, Default)]
pub struct Scene {
    /// Source file name recorded by the exporter.
    pub from_file_name: String,
    pub art_tool: Option<ArtToolInfo>,
    pub materials: Vec<String>,
    pub skeletons: Vec<Skeleton>,
    pub meshes: Vec<Mesh>,
    pub models: Vec<Model>,
    /// Number of track groups / animations present (decode them with [`crate::anim`]).
    pub track_group_count: usize,
    pub animation_count: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArtToolInfo {
    pub name: String,
    pub units_per_meter: f32,
    pub origin: [f32; 3],
    pub right: [f32; 3],
    pub up: [f32; 3],
    pub back: [f32; 3],
}

#[derive(Debug, Clone, Default)]
pub struct Skeleton {
    pub name: String,
    pub bones: Vec<Bone>,
    pub lod_type: i64,
}

#[derive(Debug, Clone)]
pub struct Bone {
    pub name: String,
    /// Index of the parent bone in the same skeleton; `None` for roots.
    pub parent: Option<usize>,
    /// Local (parent-relative) rest transform.
    pub transform: Transform,
    /// Inverse of the bone's world rest matrix, 4×4 row-major with translation in the last row
    /// (Granny / D3D row-vector convention).
    pub inverse_world: [f32; 16],
    pub lod_error: f32,
}

#[derive(Debug, Clone, Default)]
pub struct Model {
    pub name: String,
    /// Index into [`Scene::skeletons`].
    pub skeleton: Option<usize>,
    pub initial_placement: Transform,
    /// Indices into [`Scene::meshes`].
    pub meshes: Vec<usize>,
}

/// One declared vertex component (`name`, element type, element count).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VertexComponent {
    pub name: String,
    pub kind: MemberType,
    pub count: usize,
}

/// A vertex component this crate does not map to a typed field, decoded to f32.
#[derive(Debug, Clone)]
pub struct VertexAttribute {
    pub name: String,
    pub components: usize,
    pub data: Vec<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TopologyGroup {
    pub material_index: i64,
    /// First triangle (not index) of the group.
    pub tri_first: u32,
    pub tri_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IndexWidth {
    #[default]
    U16,
    U32,
}

#[derive(Debug, Clone)]
pub struct BoneBinding {
    pub bone_name: String,
    pub obb_min: [f32; 3],
    pub obb_max: [f32; 3],
}

/// Larian's per-mesh `ExtendedData` (`UserMeshProperties`), when present.
#[derive(Debug, Clone, Default)]
pub struct MeshProperties {
    pub lsm_version: Option<i64>,
    pub lod: Option<i64>,
    /// `UserMeshProperties.Flags` (4 words; bit meanings are Larian's `MeshFlags`).
    pub flags: [u32; 4],
    pub lod_distance: Option<f32>,
    pub is_impostor: bool,
}

impl MeshProperties {
    /// `flags[0]`: the mesh is a proxy (`rf::ModelProxy`), not a renderable model.
    pub const MESH_PROXY: u32 = 0x01;
    /// `flags[0]`: the mesh is cloth (simulated or skinned to a simulation).
    pub const CLOTH: u32 = 0x02;
    /// `flags[0]`: the mesh is an occluder (`rf::OcclusionModel`), not a renderable model.
    pub const OCCLUDER: u32 = 0x80;
    /// `flags[2]` bits that say which cloth mesh this is; [`Self::CLOTH_PHYSICS`] is the
    /// simulation mesh.
    pub const CLOTH_KIND_MASK: u32 = 0x300;
    pub const CLOTH_PHYSICS: u32 = 0x100;

    /// Whether the game draws this mesh. Proxies and occluders get no renderable model, and
    /// the cloth simulation mesh's renderable object is created without its drawn flag (the
    /// visible cloth is a separate mesh driven by it).
    // bg3: gr::FactoryGr2::Parse, ls::RenderableObjectTemplate::CreateInstance (4.1.1.7631656)
    pub fn is_drawn(&self) -> bool {
        let [kind, _, cloth, _] = self.flags;
        let cloth_physics =
            kind & Self::CLOTH != 0 && cloth & Self::CLOTH_KIND_MASK == Self::CLOTH_PHYSICS;
        kind & (Self::MESH_PROXY | Self::OCCLUDER) == 0 && !cloth_physics
    }
}

#[derive(Debug, Clone, Default)]
pub struct Mesh {
    pub name: String,
    /// Declared vertex layout, in file order.
    pub format: Vec<VertexComponent>,
    pub positions: Vec<[f32; 3]>,
    /// Unit normals (from `Normal` or decoded from `QTangent`).
    pub normals: Vec<[f32; 3]>,
    /// Tangent xyz + handedness w (±1); empty if the format has no tangent frame.
    pub tangents: Vec<[f32; 4]>,
    /// One `Vec` per `TextureCoordinatesN` set, in N order.
    pub uvs: Vec<Vec<[f32; 2]>>,
    /// One `Vec` per `DiffuseColorN` set (RGBA, 0..1).
    pub colors: Vec<Vec<[f32; 4]>>,
    /// Indices into [`Mesh::bone_bindings`]; unused slots are 0 with weight 0.
    pub bone_indices: Vec<[u16; 4]>,
    /// Raw weights scaled to 0..1 (Granny stores u8s that sum to 255).
    pub bone_weights: Vec<[f32; 4]>,
    pub other_attributes: Vec<VertexAttribute>,
    pub indices: Vec<u32>,
    pub index_width: IndexWidth,
    pub groups: Vec<TopologyGroup>,
    /// Material name per binding (index = `TopologyGroup::material_index`).
    pub material_bindings: Vec<Option<String>>,
    pub bone_bindings: Vec<BoneBinding>,
    pub morph_target_count: usize,
    pub properties: Option<MeshProperties>,
}

impl Mesh {
    /// Whether the game draws this mesh ([`MeshProperties::is_drawn`]; meshes without
    /// Larian properties are drawn).
    pub fn is_drawn(&self) -> bool {
        self.properties
            .as_ref()
            .is_none_or(MeshProperties::is_drawn)
    }

    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

fn vec3(v: &[f32]) -> [f32; 3] {
    [
        v.first().copied().unwrap_or(0.0),
        v.get(1).copied().unwrap_or(0.0),
        v.get(2).copied().unwrap_or(0.0),
    ]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = dot(v, v).sqrt();
    if l > 0.0 {
        [v[0] / l, v[1] / l, v[2] / l]
    } else {
        v
    }
}

/// Decode a BG3 `QTangent` (quaternion `x, y, z, w` as 4 × snorm16) into
/// `(normal, tangent, handedness)`.
///
/// Let `X, Y, Z` be the quaternion's rotated basis axes and `s = sign(w)` (`q` and `-q` are the
/// same rotation, so the sign is free to carry the reflection bit):
///
/// * normal  = `s · Z`
/// * tangent = `X` (points along +U)
/// * bitangent along +V (UVs as stored in the file) = `-Y`
///
/// `handedness` is the glTF/Bevy `tangent.w`, i.e. `cross(normal, tangent) · w` points along +V,
/// which works out to `-s`.
///
/// Verified empirically over ~80M triangles of `Models.pak` (example `gr2_scan`): `s·Z` agrees
/// with the face winding normal on 97.8% of triangles (plain `Z`: 28%), `X` with dP/du on 99%,
/// and `-Y` with dP/dv on 98.7%. The residue is degenerate / smoothed geometry.
pub fn decode_qtangent(q: [f32; 4]) -> ([f32; 3], [f32; 3], f32) {
    let [x, y, z, w] = q;
    let s = if w < 0.0 { -1.0 } else { 1.0 };
    let tangent = normalize([
        1.0 - 2.0 * (y * y + z * z),
        2.0 * (x * y + w * z),
        2.0 * (x * z - w * y),
    ]);
    let z_axis = normalize([
        2.0 * (x * z + w * y),
        2.0 * (y * z - w * x),
        1.0 - 2.0 * (x * x + y * y),
    ]);
    let normal = [z_axis[0] * s, z_axis[1] * s, z_axis[2] * s];
    (normal, tangent, -s)
}

/// Rotated Y axis of a quaternion (exposed for convention checks).
pub fn quat_y_axis(q: [f32; 4]) -> [f32; 3] {
    let [x, y, z, w] = q;
    normalize([
        2.0 * (x * y - w * z),
        1.0 - 2.0 * (x * x + z * z),
        2.0 * (y * z + w * x),
    ])
}

/// Members that are optional in the root: missing → empty list.
fn opt_refs<'t, 'f>(o: &Object<'t, 'f>, name: &str) -> Result<Vec<Option<Object<'t, 'f>>>> {
    if o.has(name) {
        o.references(name)
    } else {
        Ok(Vec::new())
    }
}

impl Scene {
    /// Parse a whole file and extract the scene.
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        let file = Gr2File::parse(data)?;
        Self::from_file(&file)
    }

    pub fn from_file(file: &Gr2File) -> Result<Self> {
        let types = Types::new(file);
        let root = types.root()?;
        let mut scene = Scene {
            from_file_name: if root.has("FromFileName") {
                root.string_or_empty("FromFileName")
            } else {
                String::new()
            },
            ..Default::default()
        };
        if root.has("ArtToolInfo")
            && let Some(a) = root.reference("ArtToolInfo")?
        {
            scene.art_tool = Some(ArtToolInfo {
                name: a.string_or_empty("FromArtToolName"),
                units_per_meter: a.f32("UnitsPerMeter").unwrap_or(1.0),
                origin: vec3(&a.f32s("Origin").unwrap_or_default()),
                right: vec3(&a.f32s("RightVector").unwrap_or_default()),
                up: vec3(&a.f32s("UpVector").unwrap_or_default()),
                back: vec3(&a.f32s("BackVector").unwrap_or_default()),
            });
        }

        let materials = opt_refs(&root, "Materials")?;
        scene.materials = materials
            .iter()
            .map(|m| {
                m.as_ref()
                    .map(|m| m.string_or_empty("Name"))
                    .unwrap_or_default()
            })
            .collect();

        let skeletons = opt_refs(&root, "Skeletons")?;
        let skeleton_ptrs: Vec<Ptr> = skeletons.iter().flatten().map(|s| s.ptr).collect();
        for s in skeletons.iter().flatten() {
            scene.skeletons.push(read_skeleton(s)?);
        }

        let meshes = opt_refs(&root, "Meshes")?;
        let mesh_ptrs: Vec<Ptr> = meshes.iter().flatten().map(|m| m.ptr).collect();
        for m in meshes.iter().flatten() {
            scene.meshes.push(read_mesh(m)?);
        }

        for m in opt_refs(&root, "Models")?.iter().flatten() {
            let skeleton = m
                .reference("Skeleton")?
                .and_then(|s| skeleton_ptrs.iter().position(|&p| p == s.ptr));
            let mut model = Model {
                name: m.string_or_empty("Name"),
                skeleton,
                initial_placement: m.transform("InitialPlacement").unwrap_or_default(),
                meshes: Vec::new(),
            };
            for b in m.array("MeshBindings")?.iter() {
                if let Some(mesh) = b.reference("Mesh")?
                    && let Some(i) = mesh_ptrs.iter().position(|&p| p == mesh.ptr)
                {
                    model.meshes.push(i);
                }
            }
            scene.models.push(model);
        }

        scene.track_group_count = opt_refs(&root, "TrackGroups")?.len();
        scene.animation_count = opt_refs(&root, "Animations")?.len();
        Ok(scene)
    }
}

fn read_skeleton(s: &Object<'_, '_>) -> Result<Skeleton> {
    let bones_arr = s.array("Bones")?;
    let mut bones = Vec::with_capacity(bones_arr.len);
    for b in bones_arr.iter() {
        let parent = b.int("ParentIndex")?;
        let iw = b.f32s("InverseWorldTransform")?;
        let mut inverse_world = [0.0; 16];
        for (d, s) in inverse_world.iter_mut().zip(&iw) {
            *d = *s;
        }
        bones.push(Bone {
            name: b.string_or_empty("Name"),
            parent: usize::try_from(parent).ok(),
            transform: b.transform("Transform")?,
            inverse_world,
            lod_error: b.f32("LODError").unwrap_or(0.0),
        });
    }
    let n = bones.len();
    if bones.iter().any(|b| b.parent.is_some_and(|p| p >= n)) {
        return Err(Error::Corrupt("bone parent index out of range"));
    }
    Ok(Skeleton {
        name: s.string_or_empty("Name"),
        bones,
        lod_type: s.int("LODType").unwrap_or(0),
    })
}

/// Parse a trailing decimal set index (`TextureCoordinates3` → 3).
fn set_index(name: &str, prefix: &str) -> Option<usize> {
    name.strip_prefix(prefix)?.parse().ok()
}

fn read_mesh(m: &Object<'_, '_>) -> Result<Mesh> {
    let mut mesh = Mesh {
        name: m.string_or_empty("Name"),
        ..Default::default()
    };
    if let Some(vd) = m.reference("PrimaryVertexData")? {
        read_vertices(&vd, &mut mesh)?;
    }
    if let Some(topo) = m.reference("PrimaryTopology")? {
        for g in topo.array("Groups")?.iter() {
            mesh.groups.push(TopologyGroup {
                material_index: g.int("MaterialIndex")?,
                tri_first: g.int("TriFirst")? as u32,
                tri_count: g.int("TriCount")? as u32,
            });
        }
        let wide = topo.int_array("Indices")?;
        if !wide.is_empty() {
            mesh.index_width = IndexWidth::U32;
            mesh.indices = wide.into_iter().map(|i| i as u32).collect();
        } else {
            mesh.index_width = IndexWidth::U16;
            mesh.indices = topo
                .int_array("Indices16")?
                .into_iter()
                .map(|i| i as u16 as u32)
                .collect();
        }
        let nv = mesh.positions.len() as u32;
        if mesh.indices.iter().any(|&i| i >= nv) {
            return Err(Error::Corrupt("vertex index out of range"));
        }
    }
    for b in m.array("MaterialBindings")?.iter() {
        mesh.material_bindings.push(
            b.reference("Material")?
                .map(|mat| mat.string_or_empty("Name")),
        );
    }
    for b in m.array("BoneBindings")?.iter() {
        mesh.bone_bindings.push(BoneBinding {
            bone_name: b.string_or_empty("BoneName"),
            obb_min: vec3(&b.f32s("OBBMin").unwrap_or_default()),
            obb_max: vec3(&b.f32s("OBBMax").unwrap_or_default()),
        });
    }
    if m.has("MorphTargets") {
        mesh.morph_target_count = m.array("MorphTargets")?.len;
    }
    if m.has("ExtendedData")
        && let Some(ext) = m.variant("ExtendedData")?
    {
        mesh.properties = Some(read_mesh_properties(&ext));
    }
    Ok(mesh)
}

fn read_mesh_properties(ext: &Object<'_, '_>) -> MeshProperties {
    let mut p = MeshProperties {
        lsm_version: ext.int("LSMVersion").ok(),
        lod: ext.int("LOD").ok(),
        ..Default::default()
    };
    let user = if ext.has("UserMeshProperties") {
        match ext.member("UserMeshProperties").map(|m| m.kind) {
            Some(MemberType::Reference) => ext.reference("UserMeshProperties").ok().flatten(),
            Some(MemberType::Inline) => ext.inline("UserMeshProperties").ok(),
            _ => None,
        }
    } else {
        None
    };
    if let Some(u) = user {
        if let Ok(flags) = u.f32s("Flags") {
            for (d, s) in p.flags.iter_mut().zip(flags) {
                *d = s as u32;
            }
        }
        if p.lod.is_none() {
            p.lod = u.int("Lod").ok();
        }
        p.lod_distance = u.f32("LodDistance").ok();
        p.is_impostor = u.int("IsImpostor").is_ok_and(|v| v != 0);
    }
    p
}

fn read_vertices(vd: &Object<'_, '_>, mesh: &mut Mesh) -> Result<()> {
    let verts = vd.array("Vertices")?;
    let n = verts.len;
    if n == 0 {
        return Ok(());
    }
    let bytes = verts.bytes()?;
    let stride = verts.ty.size;
    let mut binormals: Vec<[f32; 3]> = Vec::new();
    let mut scratch = Vec::with_capacity(16);
    for c in &verts.ty.members {
        let count = c.count();
        mesh.format.push(VertexComponent {
            name: c.name.clone(),
            kind: c.kind,
            count,
        });
        if !c.kind.is_numeric() {
            return Err(Error::UnsupportedVertexComponent {
                name: c.name.clone(),
                kind: c.kind,
                count,
            });
        }
        let mut each = |f: &mut dyn FnMut(&[f32], &[u8])| {
            for v in 0..n {
                let b = &bytes[v * stride + c.offset..v * stride + c.offset + c.size];
                scratch.clear();
                numeric_to_f32(c.kind, b, count, &mut scratch);
                f(&scratch, b);
            }
        };
        let name = c.name.as_str();
        match name {
            "Position" => each(&mut |s, _| mesh.positions.push(vec3(s))),
            "Normal" => each(&mut |s, _| mesh.normals.push(normalize(vec3(s)))),
            "Tangent" => each(&mut |s, _| {
                let w = if count >= 4 && s[3] < 0.0 { -1.0 } else { 1.0 };
                let t = normalize(vec3(s));
                mesh.tangents.push([t[0], t[1], t[2], w]);
            }),
            "Binormal" => each(&mut |s, _| binormals.push(normalize(vec3(s)))),
            "QTangent" if count == 4 => {
                // Some files declare QTangent as UInt16 instead of BinormalInt16; the bits are
                // still snorm16.
                let snorm = matches!(c.kind, MemberType::BinormalInt16 | MemberType::UInt16);
                each(&mut |s, b| {
                    let q = if snorm && c.kind == MemberType::UInt16 {
                        let mut q = [0.0; 4];
                        for (i, d) in q.iter_mut().enumerate() {
                            let v = i16::from_le_bytes([b[2 * i], b[2 * i + 1]]);
                            *d = (f32::from(v) / 32767.0).max(-1.0);
                        }
                        q
                    } else {
                        [s[0], s[1], s[2], s[3]]
                    };
                    let (nrm, tan, w) = decode_qtangent(q);
                    mesh.normals.push(nrm);
                    mesh.tangents.push([tan[0], tan[1], tan[2], w]);
                })
            }
            "BoneWeights" if count <= 4 => each(&mut |_, b| {
                let mut w = [0.0; 4];
                for (i, d) in w.iter_mut().enumerate().take(count) {
                    *d = match c.kind {
                        MemberType::UInt8 | MemberType::NormalUInt8 => f32::from(b[i]) / 255.0,
                        MemberType::UInt16 | MemberType::NormalUInt16 => {
                            f32::from(u16::from_le_bytes([b[2 * i], b[2 * i + 1]])) / 65535.0
                        }
                        _ => scratch_value(c.kind, b, i),
                    };
                }
                mesh.bone_weights.push(w);
            }),
            "BoneIndices" if count <= 4 => each(&mut |_, b| {
                let mut idx = [0u16; 4];
                for (i, d) in idx.iter_mut().enumerate().take(count) {
                    *d = numeric_to_i64(c.kind, b, i).unwrap_or(0) as u16;
                }
                mesh.bone_indices.push(idx);
            }),
            _ => {
                if let Some(set) = set_index(name, "TextureCoordinates") {
                    if mesh.uvs.len() <= set {
                        mesh.uvs.resize_with(set + 1, Vec::new);
                    }
                    let dst = &mut mesh.uvs[set];
                    each(&mut |s, _| {
                        dst.push([s[0], s.get(1).copied().unwrap_or(0.0)]);
                    });
                } else if let Some(set) = set_index(name, "DiffuseColor") {
                    if mesh.colors.len() <= set {
                        mesh.colors.resize_with(set + 1, Vec::new);
                    }
                    let dst = &mut mesh.colors[set];
                    each(&mut |s, _| {
                        let mut col = [0.0, 0.0, 0.0, 1.0];
                        for (d, v) in col.iter_mut().zip(s) {
                            *d = *v;
                        }
                        dst.push(col);
                    });
                } else {
                    let mut data = Vec::with_capacity(n * count);
                    each(&mut |s, _| data.extend_from_slice(s));
                    mesh.other_attributes.push(VertexAttribute {
                        name: name.to_owned(),
                        components: count,
                        data,
                    });
                }
            }
        }
    }
    // Separate Tangent + Binormal: derive handedness.
    if !binormals.is_empty() && mesh.tangents.len() == n && mesh.normals.len() == n {
        for ((t, b), nrm) in mesh.tangents.iter_mut().zip(&binormals).zip(&mesh.normals) {
            let w = if dot(cross(*nrm, [t[0], t[1], t[2]]), *b) < 0.0 {
                -1.0
            } else {
                1.0
            };
            t[3] = w;
        }
    } else if !binormals.is_empty() {
        mesh.other_attributes.push(VertexAttribute {
            name: "Binormal".into(),
            components: 3,
            data: binormals.into_iter().flatten().collect(),
        });
    }
    for (what, len) in [
        ("Normal", mesh.normals.len()),
        ("Tangent", mesh.tangents.len()),
        ("BoneWeights", mesh.bone_weights.len()),
        ("BoneIndices", mesh.bone_indices.len()),
    ] {
        if len != 0 && len != n {
            let _ = what;
            return Err(Error::Corrupt("vertex attribute length mismatch"));
        }
    }
    if mesh.positions.len() != n {
        return Err(Error::MissingMember("Position".into()));
    }
    Ok(())
}

fn scratch_value(kind: MemberType, b: &[u8], i: usize) -> f32 {
    let mut v = Vec::with_capacity(i + 1);
    numeric_to_f32(kind, b, i + 1, &mut v);
    v[i]
}
