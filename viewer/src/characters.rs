//! Level characters: skinned GR2 meshes on a joint hierarchy, playing their idle animation.
//!
//! Resolution (level object → template → `CharacterVisualBank` → visuals, skeleton, idle) is
//! pure data in `bg3_world::characters`; see `docs/research/characters.md`. This module converts
//! it to Bevy:
//!
//! * One entity per skeleton bone (joint), parented like the skeleton, under the character's
//!   root entity, which carries the level transform.
//! * Mesh bone indices refer to the mesh's `BoneBindings` (bone *names*), so they are remapped
//!   by name to skeleton joint indices. Inverse bind matrices are the skeleton's
//!   `InverseWorldTransform`s.
//! * Everything is mirrored from BG3's left-handed space through `space.rs`: positions and
//!   rotations of joints, animation samples and the inverse bind matrices (`M · IBM · M` with
//!   `M = diag(1, 1, -1)`), so `joint · IBM · v` stays the mirror of the original skinning.
//! * The idle animation is sampled every frame (looping) into the joints' local transforms.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::mesh::VertexAttributeValues;
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::prelude::*;
use bg3_vfs::Vfs;
use bg3_world::characters::{
    DEFAULT_MOVEMENT_SPEED_RUN, DEFAULT_MOVEMENT_STEP_UP_HEIGHT, EquippedItem, RUN_PEACE,
    ResolvedCharacter, idle_simple_name, resolve_character, resolve_equipment,
};
use rayon::prelude::*;

use crate::level::LoadedLevel;
use crate::space;

/// Mirror matrix `diag(1, 1, -1, 1)` (see `space.rs`).
const MIRROR: Mat4 = Mat4::from_cols(Vec4::X, Vec4::Y, Vec4::NEG_Z, Vec4::W);

/// A skeleton converted to Bevy space.
#[derive(Clone)]
pub struct Skeleton {
    pub joints: Vec<Joint>,
    by_name: HashMap<String, usize>,
}

#[derive(Clone)]
pub struct Joint {
    pub name: String,
    pub parent: Option<usize>,
    /// Rest pose, parent-relative, in Bevy space.
    pub rest: Transform,
    /// Inverse bind matrix in Bevy space.
    pub inverse_bind: Mat4,
}

impl Skeleton {
    pub fn from_gr2(s: &bg3_gr2::Skeleton) -> Self {
        let joints: Vec<Joint> = s
            .bones
            .iter()
            .map(|b| Joint {
                name: b.name.clone(),
                parent: b.parent,
                rest: granny_transform(&b.transform),
                inverse_bind: inverse_bind(&b.inverse_world),
            })
            .collect();
        let by_name = joints
            .iter()
            .enumerate()
            .map(|(i, j)| (j.name.clone(), i))
            .collect();
        Self { joints, by_name }
    }

    /// Add the bones of `other` named in `wanted` that this skeleton lacks, with any missing
    /// ancestors, parented to the nearest ancestor this skeleton has (by name). Returns the names
    /// that `other` doesn't have either.
    ///
    /// Mesh GR2s carry their own skeleton, which extends the base skeleton with face, hair,
    /// cloth or wing bones the body animations don't drive; they then keep their rest pose
    /// relative to their parent.
    pub fn extend_from<'a>(
        &mut self,
        other: &bg3_gr2::Skeleton,
        wanted: impl IntoIterator<Item = &'a str>,
    ) -> Vec<String> {
        let mut unknown = Vec::new();
        for name in wanted {
            if self.by_name.contains_key(name) {
                continue;
            }
            let Some(mut i) = other.bones.iter().position(|b| b.name == name) else {
                unknown.push(name.to_owned());
                continue;
            };
            // Chain of missing ancestors, child first.
            let mut chain = vec![i];
            while let Some(p) = other.bones[i].parent {
                if self.by_name.contains_key(&other.bones[p].name) {
                    break;
                }
                chain.push(p);
                i = p;
            }
            for &b in chain.iter().rev() {
                let bone = &other.bones[b];
                let parent = bone
                    .parent
                    .and_then(|p| self.by_name.get(&other.bones[p].name).copied());
                self.by_name.insert(bone.name.clone(), self.joints.len());
                self.joints.push(Joint {
                    name: bone.name.clone(),
                    parent,
                    rest: granny_transform(&bone.transform),
                    inverse_bind: inverse_bind(&bone.inverse_world),
                });
            }
        }
        unknown
    }

    pub fn index(&self, name: &str) -> Option<usize> {
        self.by_name.get(name).copied()
    }

    /// Model-space rest matrices (parents precede children in Granny skeletons).
    pub fn rest_world(&self) -> Vec<Mat4> {
        let mut world: Vec<Mat4> = Vec::with_capacity(self.joints.len());
        for j in &self.joints {
            let local = j.rest.to_matrix();
            let w = match j.parent {
                Some(p) if p < world.len() => world[p] * local,
                _ => local,
            };
            world.push(w);
        }
        world
    }
}

/// A Granny transform (BG3 space) as a Bevy transform. Shear is dropped (only the diagonal of
/// the scale/shear matrix is kept; it is the identity on nearly every bone and track).
pub fn granny_transform(t: &bg3_gr2::Transform) -> Transform {
    let s = t.scale_shear;
    Transform {
        translation: Vec3::from_array(space::vec3(t.position)),
        rotation: space::quat(t.orientation),
        scale: Vec3::new(s[0][0], s[1][1], s[2][2]),
    }
}

/// Granny's `InverseWorldTransform` is row-major with the translation in the last row (row
/// vectors), i.e. the column-major layout of the column-vector matrix. Mirror it into Bevy space.
pub fn inverse_bind(m: &[f32; 16]) -> Mat4 {
    MIRROR * Mat4::from_cols_array(m) * MIRROR
}

/// A prepared (Bevy-side) animation: per-joint tracks of the idle clip.
pub struct PreparedAnimation {
    pub name: String,
    pub duration: f32,
    pub tracks: Vec<bg3_gr2::TransformTrack>,
}

impl PreparedAnimation {
    /// Map tracks to `skeleton` joints by bone name.
    fn bind(&self, skeleton: &Skeleton) -> Vec<(usize, usize)> {
        self.tracks
            .iter()
            .enumerate()
            .filter_map(|(t, track)| Some((skeleton.index(&track.name)?, t)))
            .collect()
    }
}

/// A mesh ready to spawn.
pub struct PreparedPart {
    pub mesh: Mesh,
    pub material_id: Option<String>,
    /// For rigid (unskinned) meshes: joint to attach to.
    pub attach_joint: Option<usize>,
    /// Transform relative to the attachment joint.
    pub local: Transform,
}

pub struct PreparedCharacter {
    pub id: String,
    pub name: String,
    pub transform: bg3_world::Transform,
    pub skeleton: Arc<Skeleton>,
    pub parts: Vec<PreparedPart>,
    pub animation: Option<Arc<PreparedAnimation>>,
    /// Forward run loop, for characters that walk (`nav.rs`).
    pub run: Option<Arc<PreparedAnimation>>,
    /// `MovementSpeedRun` (m/s) and `MovementStepUpHeight` (m) from the template chain.
    pub run_speed: f32,
    pub step_height: f32,
}

/// Every character of the level that could be resolved and converted.
#[derive(Resource, Default)]
pub struct LevelCharacters {
    pub characters: Vec<PreparedCharacter>,
    pub failed: Vec<(String, String)>,
}

impl LevelCharacters {
    /// Resolve and convert the characters among `objects` (the level's, plus global ones the
    /// simulation placed in it with `--play`); non-characters are skipped.
    pub fn load_objects<'a>(
        vfs: &Vfs,
        level: &LoadedLevel,
        objects: impl Iterator<Item = &'a bg3_world::LevelObject>,
    ) -> Self {
        let started = std::time::Instant::now();
        let idle = idle_simple_name(vfs);
        let stats = bg3_stats::install::load(vfs.packages(), bg3_vfs::modules::BASE_LOAD_ORDER)
            .map_err(|e| eprintln!("characters: no stats ({e}), skipping equipment"))
            .ok();
        let mut out = Self::default();
        let mut resolved = Vec::new();
        for obj in objects.filter(|o| o.kind == "character") {
            match resolve_character(obj, &level.templates, &level.banks, &idle) {
                Ok(c) => {
                    let items = match (&c.props.equipment, &stats) {
                        (Some(e), Some(stats)) => {
                            resolve_equipment(e, stats, &level.templates, &level.banks)
                        }
                        _ => Vec::new(),
                    };
                    let run = c.animation(&level.banks, RUN_PEACE);
                    resolved.push((obj.transform, c, items, run));
                }
                Err(e) => out.failed.push((obj.name.clone(), e.to_string())),
            }
        }
        let cache = GrannyCache::default();
        let results: Vec<_> = resolved
            .par_iter()
            .map(|(t, c, items, run)| {
                let mut p = prepare(vfs, &cache, *t, c, items);
                if let Ok(p) = &mut p {
                    p.run = run
                        .as_ref()
                        .and_then(|a| a.source_file.as_deref())
                        .and_then(|f| cache.animation(vfs, f));
                }
                (c.name.clone(), p)
            })
            .collect();
        for (name, r) in results {
            match r {
                Ok(c) => out.characters.push(c),
                Err(e) => out.failed.push((name, e)),
            }
        }
        let animated = out
            .characters
            .iter()
            .filter(|c| c.animation.is_some())
            .count();
        eprintln!(
            "characters: {} prepared ({animated} animated), {} failed in {:.2?}",
            out.characters.len(),
            out.failed.len(),
            started.elapsed()
        );
        for (name, e) in out.failed.iter().take(8) {
            eprintln!("  {name}: {e}");
        }
        out
    }

    pub fn material_ids(&self) -> impl Iterator<Item = &str> {
        self.characters
            .iter()
            .flat_map(|c| &c.parts)
            .filter_map(|p| p.material_id.as_deref())
    }

    /// Whether a level object is drawn by this module (so the level shouldn't draw a box).
    pub fn handles(&self, id: &str) -> bool {
        self.characters.iter().any(|c| c.id == id)
    }
}

/// Parsed GR2 files shared between characters (skeletons and animations are reused a lot).
#[derive(Default)]
struct GrannyCache {
    skeletons: std::sync::Mutex<HashMap<String, Arc<Skeleton>>>,
    animations: std::sync::Mutex<HashMap<String, Option<Arc<PreparedAnimation>>>>,
    scenes: std::sync::Mutex<HashMap<String, Arc<bg3_gr2::Scene>>>,
}

impl GrannyCache {
    fn scene(&self, vfs: &Vfs, path: &str) -> Result<Arc<bg3_gr2::Scene>, String> {
        if let Some(s) = self.scenes.lock().unwrap().get(path) {
            return Ok(s.clone());
        }
        let data = vfs.read(path).map_err(|e| format!("{path}: {e}"))?;
        let scene =
            Arc::new(bg3_gr2::Scene::from_bytes(&data).map_err(|e| format!("{path}: {e}"))?);
        self.scenes
            .lock()
            .unwrap()
            .insert(path.to_owned(), scene.clone());
        Ok(scene)
    }

    fn skeleton(&self, vfs: &Vfs, path: &str) -> Result<Arc<Skeleton>, String> {
        if let Some(s) = self.skeletons.lock().unwrap().get(path) {
            return Ok(s.clone());
        }
        let scene = self.scene(vfs, path)?;
        let s = scene
            .skeletons
            .first()
            .ok_or_else(|| format!("{path}: no skeleton"))?;
        let skeleton = Arc::new(Skeleton::from_gr2(s));
        // Sanity check: the rest pose should be the bind pose.
        let err = skeleton
            .rest_world()
            .iter()
            .zip(&skeleton.joints)
            .map(|(w, j)| {
                (*w * j.inverse_bind - Mat4::IDENTITY)
                    .to_cols_array()
                    .iter()
                    .fold(0.0f32, |m, x| m.max(x.abs()))
            })
            .fold(0.0f32, f32::max);
        if err > 1e-3 {
            eprintln!("  skeleton {path}: rest pose differs from bind pose by {err}");
        }
        self.skeletons
            .lock()
            .unwrap()
            .insert(path.to_owned(), skeleton.clone());
        Ok(skeleton)
    }

    fn animation(&self, vfs: &Vfs, path: &str) -> Option<Arc<PreparedAnimation>> {
        if let Some(a) = self.animations.lock().unwrap().get(path) {
            return a.clone();
        }
        let anim = vfs
            .read(path)
            .ok()
            .and_then(|d| bg3_gr2::Animation::from_bytes(&d).ok())
            .and_then(|mut list| (!list.is_empty()).then(|| list.swap_remove(0)))
            .and_then(|a| {
                let duration = a.duration;
                let group = a.track_groups.into_iter().next()?;
                Some(Arc::new(PreparedAnimation {
                    name: a.name,
                    duration,
                    tracks: group.transform_tracks,
                }))
            });
        match &anim {
            Some(a) => eprintln!(
                "  animation {} ({:.2} s, {} tracks)",
                a.name,
                a.duration,
                a.tracks.len()
            ),
            None => eprintln!("  animation {path}: failed to decode"),
        }
        self.animations
            .lock()
            .unwrap()
            .insert(path.to_owned(), anim.clone());
        anim
    }
}

fn prepare(
    vfs: &Vfs,
    cache: &GrannyCache,
    transform: bg3_world::Transform,
    c: &ResolvedCharacter,
    items: &[EquippedItem],
) -> Result<PreparedCharacter, String> {
    let skel_path = c
        .skeleton
        .as_ref()
        .and_then(|s| s.source_file.as_deref())
        .ok_or("no skeleton resource")?;
    let base = cache.skeleton(vfs, skel_path)?;
    // Extend the base skeleton with bones only the meshes' own skeletons have.
    let mut extended = (*base).clone();
    let mut unmapped: Vec<String> = Vec::new();
    for part in &c.parts {
        let Some(path) = part.visual.source_file.as_deref() else {
            continue;
        };
        let scene = cache.scene(vfs, path)?;
        let names = scene
            .meshes
            .iter()
            .flat_map(|m| &m.bone_bindings)
            .map(|b| b.bone_name.as_str());
        match scene.skeletons.first() {
            Some(s) => unmapped.extend(extended.extend_from(s, names)),
            None => unmapped.extend(
                names
                    .filter(|n| extended.index(n).is_none())
                    .map(str::to_owned),
            ),
        }
    }
    let skeleton = if extended.joints.len() == base.joints.len() {
        base
    } else {
        Arc::new(extended)
    };
    let mut parts = Vec::new();
    for part in &c.parts {
        let Some(path) = part.visual.source_file.as_deref() else {
            continue;
        };
        let scene = cache.scene(vfs, path)?;
        let attach_joint = part.attach_bone.as_deref().and_then(|b| skeleton.index(b));
        for obj in part.visual.objects.iter().filter(|o| o.lod == 0) {
            let Some(mesh) = scene.meshes.iter().find(|m| m.name == obj.mesh_name()) else {
                continue;
            };
            let (bevy_mesh, _) = skinned_mesh(mesh, &skeleton);
            parts.push(PreparedPart {
                mesh: bevy_mesh,
                material_id: obj.material_id.clone(),
                attach_joint,
                local: Transform::default(),
            });
        }
    }
    parts.extend(weapon_parts(vfs, cache, &skeleton, items));
    if !unmapped.is_empty() {
        unmapped.sort();
        unmapped.dedup();
        eprintln!(
            "  {}: {} mesh bones not in skeleton {skel_path}, e.g. {:?}",
            c.name,
            unmapped.len(),
            &unmapped[..unmapped.len().min(4)]
        );
    }
    let animation = c
        .idle
        .as_ref()
        .and_then(|a| a.source_file.as_deref())
        .and_then(|p| cache.animation(vfs, p));
    Ok(PreparedCharacter {
        id: c.id.clone(),
        name: c.name.clone(),
        transform,
        skeleton,
        parts,
        animation,
        run: None,
        run_speed: c
            .props
            .movement_speed_run
            .unwrap_or(DEFAULT_MOVEMENT_SPEED_RUN),
        step_height: c
            .props
            .movement_step_up_height
            .unwrap_or(DEFAULT_MOVEMENT_STEP_UP_HEIGHT),
    })
}

/// Weapons from the starting equipment, drawn in hand (the drawn/sheathed state isn't modelled).
///
/// Assumption: a weapon GR2's `Dummy_Attachment` bone is placed on the character's
/// `Dummy_R_Hand` (main hand) or `Dummy_L_Hand` (off hand); both names occur in the game
/// binary and in every humanoid skeleton. A ranged main weapon is shown only when there is no
/// melee main weapon.
fn weapon_parts(
    vfs: &Vfs,
    cache: &GrannyCache,
    skeleton: &Skeleton,
    items: &[EquippedItem],
) -> Vec<PreparedPart> {
    let has_melee = items.iter().any(|i| i.slot == "Melee Main Weapon");
    let mut out = Vec::new();
    for item in items {
        let bone = match item.slot.as_str() {
            "Melee Main Weapon" => "Dummy_R_Hand",
            "Ranged Main Weapon" if !has_melee => "Dummy_R_Hand",
            "Melee Offhand Weapon" => "Dummy_L_Hand",
            _ => continue,
        };
        let Some(joint) = skeleton.index(bone) else {
            continue;
        };
        let Some(path) = item.visual.source_file.as_deref() else {
            continue;
        };
        let Ok(scene) = cache.scene(vfs, path) else {
            continue;
        };
        // The attachment bone's rest pose is where the hand goes: mesh = inverse(attach world).
        let local = scene
            .skeletons
            .first()
            .and_then(|s| s.bones.iter().find(|b| b.name == "Dummy_Attachment"))
            .map_or(Transform::default(), |b| {
                Transform::from_matrix(inverse_bind(&b.inverse_world))
            });
        for obj in item.visual.objects.iter().filter(|o| o.lod == 0) {
            if let Some(mesh) = scene.meshes.iter().find(|m| m.name == obj.mesh_name()) {
                out.push(PreparedPart {
                    mesh: crate::meshes::to_bevy_mesh(mesh),
                    material_id: obj.material_id.clone(),
                    attach_joint: Some(joint),
                    local,
                });
            }
        }
    }
    out
}

/// Convert a GR2 mesh and remap its bone indices (into the mesh's `BoneBindings`) to joints of
/// `skeleton` by bone name. Returns the bone names the skeleton lacks; their weights are moved to
/// the root joint so the vertices stay attached to the character.
pub fn skinned_mesh(m: &bg3_gr2::Mesh, skeleton: &Skeleton) -> (Mesh, Vec<String>) {
    let mut mesh = crate::meshes::to_bevy_mesh(m);
    let mut missing = Vec::new();
    if m.bone_indices.len() != m.positions.len() || m.bone_weights.len() != m.positions.len() {
        return (mesh, missing);
    }
    let remap: Vec<u16> = m
        .bone_bindings
        .iter()
        .map(|b| match skeleton.index(&b.bone_name) {
            Some(i) => i as u16,
            None => {
                missing.push(b.bone_name.clone());
                0
            }
        })
        .collect();
    let joints: Vec<[u16; 4]> = m
        .bone_indices
        .iter()
        .map(|idx| idx.map(|i| remap.get(usize::from(i)).copied().unwrap_or(0)))
        .collect();
    let weights: Vec<[f32; 4]> = m
        .bone_weights
        .iter()
        .map(|w| {
            let sum: f32 = w.iter().sum();
            if sum > 0.0 {
                w.map(|x| x / sum)
            } else {
                [1.0, 0.0, 0.0, 0.0]
            }
        })
        .collect();
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_JOINT_INDEX,
        VertexAttributeValues::Uint16x4(joints),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, weights);
    let _ = mesh.generate_skinned_mesh_bounds();
    (mesh, missing)
}

/// An animation with its `(joint index, track index)` bindings.
pub type BoundClip = (Arc<PreparedAnimation>, Vec<(usize, usize)>);

/// Root entity of a spawned level character.
#[derive(Component)]
pub struct CharacterRoot {
    /// Level object id (`MapKey`).
    pub id: String,
    pub name: String,
    pub run_speed: f32,
    pub step_height: f32,
    /// Idle and run clips with their joint bindings, for swapping into [`CharacterAnimator`].
    pub idle: Option<BoundClip>,
    pub run: Option<BoundClip>,
}

/// Joint entities of a spawned character, plus its animation state.
#[derive(Component)]
pub struct CharacterAnimator {
    pub joints: Vec<Entity>,
    pub animation: Arc<PreparedAnimation>,
    /// `(joint index, track index)`.
    pub bindings: Vec<(usize, usize)>,
    pub time: f32,
}

pub fn spawn_characters(
    mut commands: Commands,
    mut characters: ResMut<LevelCharacters>,
    level_materials: Res<crate::materials::LevelMaterials>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
) {
    let fallback = materials.add(Color::srgb(0.75, 0.6, 0.55));
    let mut ibm_cache: HashMap<usize, Handle<SkinnedMeshInverseBindposes>> = HashMap::new();
    let mut rng_phase = 0.0f32;
    for c in std::mem::take(&mut characters.characters) {
        let root = commands
            .spawn((
                Name::new(c.name.clone()),
                space::transform(&c.transform),
                Visibility::default(),
            ))
            .id();
        let mut joints: Vec<Entity> = Vec::with_capacity(c.skeleton.joints.len());
        for j in &c.skeleton.joints {
            let parent = j
                .parent
                .and_then(|p| joints.get(p).copied())
                .unwrap_or(root);
            joints.push(
                commands
                    .spawn((
                        Name::new(j.name.clone()),
                        j.rest,
                        Visibility::default(),
                        ChildOf(parent),
                    ))
                    .id(),
            );
        }
        let ibm = ibm_cache
            .entry(Arc::as_ptr(&c.skeleton) as usize)
            .or_insert_with(|| {
                bindposes.add(SkinnedMeshInverseBindposes::from(
                    c.skeleton
                        .joints
                        .iter()
                        .map(|j| j.inverse_bind)
                        .collect::<Vec<_>>(),
                ))
            })
            .clone();
        for part in c.parts {
            let material = match part
                .material_id
                .as_ref()
                .map(|m| level_materials.by_id.get(m))
            {
                Some(Some(Some(m))) => m.clone(),
                Some(Some(None)) => continue, // helper material the game doesn't draw
                _ => fallback.clone(),
            };
            let skinned = part.mesh.attribute(Mesh::ATTRIBUTE_JOINT_INDEX).is_some();
            let mesh = meshes.add(part.mesh);
            if skinned {
                commands.spawn((
                    Mesh3d(mesh),
                    MeshMaterial3d(material),
                    SkinnedMesh {
                        inverse_bindposes: ibm.clone(),
                        joints: joints.clone(),
                    },
                    Transform::default(),
                    ChildOf(root),
                ));
            } else {
                let parent = part
                    .attach_joint
                    .and_then(|j| joints.get(j).copied())
                    .unwrap_or(root);
                commands.spawn((
                    Mesh3d(mesh),
                    MeshMaterial3d(material),
                    part.local,
                    ChildOf(parent),
                ));
            }
        }
        commands.entity(root).insert(CharacterRoot {
            id: c.id.clone(),
            name: c.name.clone(),
            run_speed: c.run_speed,
            step_height: c.step_height,
            idle: c.animation.clone().map(|a| {
                let b = a.bind(&c.skeleton);
                (a, b)
            }),
            run: c.run.clone().map(|a| {
                let b = a.bind(&c.skeleton);
                (a, b)
            }),
        });
        let animation = c
            .animation
            .filter(|_| std::env::var_os("BG3_NO_ANIM").is_none());
        if let Some(animation) = animation {
            let bindings = animation.bind(&c.skeleton);
            // Desynchronise identical idles a little, deterministically.
            rng_phase = (rng_phase + 0.37) % 1.0;
            commands.entity(root).insert(CharacterAnimator {
                joints,
                time: rng_phase * animation.duration,
                animation,
                bindings,
            });
        }
    }
}

pub fn animate_characters(
    time: Res<Time>,
    mut animators: Query<&mut CharacterAnimator>,
    mut transforms: Query<&mut Transform>,
) {
    let dt = time.delta_secs();
    for mut a in &mut animators {
        let duration = a.animation.duration.max(1e-3);
        a.time = (a.time + dt) % duration;
        let t = a.time;
        for &(joint, track) in &a.bindings {
            let sample = a.animation.tracks[track].sample_looped(t, duration);
            if let Ok(mut tr) = transforms.get_mut(a.joints[joint]) {
                *tr = granny_transform(&sample);
            }
        }
    }
}

pub struct CharactersPlugin;

impl Plugin for CharactersPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_characters.after(crate::materials::upload))
            .add_systems(Update, animate_characters);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bone(name: &str, parent: Option<usize>, pos: [f32; 3], rot: [f32; 4]) -> bg3_gr2::Bone {
        bg3_gr2::Bone {
            name: name.into(),
            parent,
            transform: bg3_gr2::Transform {
                flags: 3,
                position: pos,
                orientation: rot,
                ..Default::default()
            },
            inverse_world: [0.0; 16],
            lod_error: 0.0,
        }
    }

    /// Granny-style inverse world matrix (row vectors, translation in the last row) computed in
    /// BG3 space from the bones' local transforms.
    fn fill_inverse_world(bones: &mut [bg3_gr2::Bone]) {
        let mut world: Vec<Mat4> = Vec::new();
        for b in bones.iter() {
            let t = &b.transform;
            let local = Mat4::from_rotation_translation(
                Quat::from_array(t.orientation).normalize(),
                Vec3::from_array(t.position),
            );
            let w = b.parent.map_or(local, |p| world[p] * local);
            world.push(w);
        }
        for (b, w) in bones.iter_mut().zip(world) {
            // Column-major of the column-vector inverse == row-major of the row-vector one.
            b.inverse_world = w.inverse().to_cols_array();
        }
    }

    fn test_skeleton() -> bg3_gr2::Skeleton {
        let mut bones = vec![
            bone("Root", None, [0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]),
            bone("Spine", Some(0), [0.0, 0.5, 0.1], [0.2, 0.0, 0.0, 0.98]),
            bone("Hand_L", Some(1), [0.4, 0.0, -0.2], [0.0, 0.3, 0.1, 0.95]),
        ];
        fill_inverse_world(&mut bones);
        bg3_gr2::Skeleton {
            name: "Test".into(),
            bones,
            lod_type: 0,
        }
    }

    #[test]
    fn rest_pose_times_inverse_bind_is_identity_after_mirroring() {
        let s = Skeleton::from_gr2(&test_skeleton());
        for (w, j) in s.rest_world().iter().zip(&s.joints) {
            let p = *w * j.inverse_bind;
            assert!(p.abs_diff_eq(Mat4::IDENTITY, 1e-5), "{}: {p}", j.name);
        }
    }

    #[test]
    fn skinned_vertex_is_mirror_of_original() {
        // Pose the hand away from rest in BG3 space, skin a vertex in BG3 space, and compare with
        // the same pose skinned in Bevy space.
        let gs = test_skeleton();
        let s = Skeleton::from_gr2(&gs);
        let pose_local = |i: usize| -> (Vec3, Quat) {
            let t = &gs.bones[i].transform;
            let extra = if i == 2 {
                Quat::from_rotation_y(0.7)
            } else {
                Quat::IDENTITY
            };
            (
                Vec3::from_array(t.position),
                Quat::from_array(t.orientation).normalize() * extra,
            )
        };
        let mut bg3_world = Vec::new();
        let mut bevy_world = Vec::new();
        for (i, b) in gs.bones.iter().enumerate() {
            let (p, q) = pose_local(i);
            let local = Mat4::from_rotation_translation(q, p);
            let w = b.parent.map_or(local, |pi| bg3_world[pi] * local);
            bg3_world.push(w);
            let bt = Transform {
                translation: Vec3::from_array(space::vec3(p.to_array())),
                rotation: space::quat(q.to_array()),
                scale: Vec3::ONE,
            }
            .to_matrix();
            let bw = b.parent.map_or(bt, |pi| bevy_world[pi] * bt);
            bevy_world.push(bw);
        }
        let v = Vec3::new(0.45, 1.6, -0.05);
        let ibm_bg3 = Mat4::from_cols_array(&gs.bones[2].inverse_world);
        let skinned_bg3 = (bg3_world[2] * ibm_bg3).transform_point3(v);
        let skinned_bevy = (bevy_world[2] * s.joints[2].inverse_bind)
            .transform_point3(Vec3::from_array(space::vec3(v.to_array())));
        let expect = Vec3::from_array(space::vec3(skinned_bg3.to_array()));
        assert!(
            skinned_bevy.abs_diff_eq(expect, 1e-5),
            "{skinned_bevy} vs {expect}"
        );
        // Left stays left: a +X (left, in BG3) bone stays at +X after mirroring.
        assert!(bevy_world[2].w_axis.x > 0.0);
    }

    #[test]
    fn bone_indices_are_remapped_by_name() {
        let s = Skeleton::from_gr2(&test_skeleton());
        let m = bg3_gr2::Mesh {
            positions: vec![[0.0; 3]; 2],
            normals: vec![[0.0, 1.0, 0.0]; 2],
            indices: vec![0, 1, 1],
            bone_bindings: ["Hand_L", "Missing", "Root"]
                .iter()
                .map(|n| bg3_gr2::BoneBinding {
                    bone_name: (*n).into(),
                    obb_min: [0.0; 3],
                    obb_max: [0.0; 3],
                })
                .collect(),
            bone_indices: vec![[0, 2, 0, 0], [1, 0, 0, 0]],
            bone_weights: vec![[0.5, 0.5, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0]],
            ..Default::default()
        };
        let (mesh, missing) = skinned_mesh(&m, &s);
        assert_eq!(missing, vec!["Missing".to_string()]);
        let Some(VertexAttributeValues::Uint16x4(j)) = mesh.attribute(Mesh::ATTRIBUTE_JOINT_INDEX)
        else {
            panic!("no joints");
        };
        assert_eq!(j, &vec![[2, 0, 2, 2], [0, 2, 2, 2]]);
    }
}
