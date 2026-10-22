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
//! * Weapons hang on their sheath bones or sit in the hands depending on the sim's weapon
//!   state, and each state has its own clip set (weapon animation subsets); see [`Unsheath`]
//!   and `bg3_world::unsheath`.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::mesh::VertexAttributeValues;
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::prelude::*;
use bg3_vfs::Vfs;
use bg3_world::characters::{
    CombatAnimation, DEFAULT_MOVEMENT_SPEED_RUN, DEFAULT_MOVEMENT_STEP_UP_HEIGHT, EquippedItem,
    IDLE_STILL_COMBAT, RUN_PEACE, ResolvedCharacter, animation_simple_names, idle_simple_name,
    resolve_character, resolve_equipment, resolve_equipment_items, weapon_items,
};
use bg3_world::unsheath::{
    UnsheathState, WeaponSet, WeaponSlot, placement, slot_shown, subset_for_state,
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
                let inverse_bind = inverse_bind(&bone.inverse_world);
                // The foreign rig's parents can sit elsewhere than ours (Lae'zel's hair rig has
                // `Head_M` 9 cm higher than her body rig), so its local rest would move the
                // mesh. Place the bone where its bind pose says, relative to our parent:
                // rest_world · inverse_bind = I, as for the base skeleton. Assumption: the
                // engine keeps attached meshes in their authored bind pose.
                let parent_world = parent.map_or(Mat4::IDENTITY, |p| self.world_of(p));
                let rest = Transform::from_matrix(parent_world.inverse() * inverse_bind.inverse());
                self.by_name.insert(bone.name.clone(), self.joints.len());
                self.joints.push(Joint {
                    name: bone.name.clone(),
                    parent,
                    rest,
                    inverse_bind,
                });
            }
        }
        unknown
    }

    /// Model-space rest matrix of joint `i`.
    fn world_of(&self, mut i: usize) -> Mat4 {
        let mut m = self.joints[i].rest.to_matrix();
        while let Some(p) = self.joints[i].parent {
            m = self.joints[p].rest.to_matrix() * m;
            i = p;
        }
        m
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
    /// Material overrides, vertex colours and cut support (`char_materials.rs`).
    pub look: crate::char_materials::PartLook,
    /// For weapon visuals: where they go when sheathed and when drawn.
    pub weapon: Option<WeaponPose>,
}

/// A weapon visual's two placements: `(joint, transform relative to it)`, `None` when the
/// equipment type names no bone (or the skeleton lacks it) and the visual is hidden.
#[derive(Clone, Debug)]
pub struct WeaponPose {
    pub slot: WeaponSlot,
    pub sheathed: Option<(usize, Transform)>,
    pub unsheathed: Option<(usize, Transform)>,
}

/// The idle, run and combat clips of one weapon state (unbound).
#[derive(Clone, Default)]
pub struct PreparedClips {
    pub idle: Option<Arc<PreparedAnimation>>,
    pub run: Option<Arc<PreparedAnimation>>,
    pub combat: HashMap<CombatAnimation, Arc<PreparedAnimation>>,
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
    pub combat: HashMap<CombatAnimation, Arc<PreparedAnimation>>,
    /// Clip sets of the drawn states whose weapon subset differs from the default
    /// (`animation`/`run`/`combat` are the sheathed set).
    pub drawn: HashMap<UnsheathState, PreparedClips>,
    /// The active weapon set: only its weapons are shown while sheathed.
    pub weapon_set: WeaponSet,
    /// `MovementSpeedRun` (m/s) and `MovementStepUpHeight` (m) from the template chain.
    pub run_speed: f32,
    pub step_height: f32,
    /// Body-region bytes cut from vertex-colour-masked meshes (`ResolvedCharacter::body_masks`).
    pub cut_bytes: Vec<u8>,
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
        avatar_equipment: Option<(String, &[String])>,
        weapon_sets: &HashMap<String, WeaponSet>,
    ) -> Self {
        let started = std::time::Instant::now();
        let idle = idle_simple_name(vfs);
        let simple_names = animation_simple_names(vfs);
        let stats = bg3_stats::install::load(vfs.packages(), bg3_vfs::modules::BASE_LOAD_ORDER)
            .map_err(|e| eprintln!("characters: no stats ({e}), skipping equipment"))
            .ok();
        let mut out = Self::default();
        let mut resolved = Vec::new();
        for obj in objects.filter(|o| o.kind == "character") {
            match resolve_character(obj, &level.templates, &level.banks, &idle) {
                Ok(mut c) => {
                    let items = if let (Some((id, names)), Some(stats)) =
                        (&avatar_equipment, &stats)
                        && *id == c.id
                    {
                        resolve_equipment_items(
                            names.iter().map(String::as_str),
                            c.props.equipment_race.as_deref(),
                            stats,
                            &level.templates,
                            &level.banks,
                        )
                    } else {
                        match (&c.props.equipment, &stats) {
                            (Some(e), Some(stats)) => resolve_equipment(
                                e,
                                c.props.equipment_race.as_deref(),
                                stats,
                                &level.templates,
                                &level.banks,
                            ),
                            _ => Vec::new(),
                        }
                    };
                    c.apply_equipment(&items, &level.banks.equipment);
                    if std::env::var_os("BG3_CHAR_DEBUG").is_some() {
                        for i in &items {
                            eprintln!(
                                "equipment {} {} ({}): {:?}",
                                c.name,
                                i.stats,
                                i.slot,
                                i.worn.iter().map(|p| &p.visual.name).collect::<Vec<_>>()
                            );
                        }
                        eprintln!(
                            "character {} at {:?}, equipment race {:?}, body masks {:?}",
                            c.name, obj.transform.position, c.props.equipment_race, c.body_masks
                        );
                    }
                    let run = c.animation(&level.banks, RUN_PEACE);
                    let weapons = weapon_items(&items);
                    let subsets: Vec<(UnsheathState, String)> =
                        [UnsheathState::Melee, UnsheathState::Ranged]
                            .into_iter()
                            .map(|st| {
                                let key = subset_for_state(&level.banks.weapons, st, &weapons);
                                (st, key.to_owned())
                            })
                            .filter(|(_, key)| !key.is_empty())
                            .collect();
                    if std::env::var_os("BG3_CHAR_DEBUG").is_some() {
                        eprintln!("weapon subsets {}: {subsets:?}", c.name);
                    }
                    // The sim's active set with `--play`; else the equipment's starting set.
                    let weapon_set =
                        weapon_sets.get(&c.id).copied().unwrap_or_else(|| {
                            WeaponSet::from_initial(
                                c.props.equipment.as_deref().zip(stats.as_ref()).and_then(
                                    |(e, s)| s.equipment.get(e)?.initial_weapon_set.as_deref(),
                                ),
                            )
                        });
                    resolved.push((obj.transform, c, items, run, subsets, weapon_set));
                }
                Err(e) => out.failed.push((obj.name.clone(), e.to_string())),
            }
        }
        let cache = GrannyCache::default();
        let results: Vec<_> = resolved
            .par_iter()
            .map(|(t, c, items, run, subsets, weapon_set)| {
                let mut p = prepare(
                    vfs,
                    &cache,
                    *t,
                    c,
                    items,
                    &level.banks.material_presets,
                    &level.banks.weapons,
                );
                if let Ok(p) = &mut p {
                    p.weapon_set = *weapon_set;
                    p.run = run
                        .as_ref()
                        .and_then(|a| a.source_file.as_deref())
                        .and_then(|f| cache.animation(vfs, f));
                }
                if let Ok(p) = &mut p {
                    for kind in CombatAnimation::ALL {
                        let clip = simple_names
                            .get(kind.simple_name())
                            .and_then(|id| c.animation(&level.banks, id))
                            .and_then(|a| a.source_file)
                            .and_then(|path| cache.animation(vfs, &path));
                        if let Some(clip) = clip {
                            p.combat.insert(kind, clip);
                        }
                    }
                    if std::env::var_os("BG3_CHAR_DEBUG").is_some() {
                        for kind in CombatAnimation::ALL {
                            eprintln!(
                                "combat clip {} {kind:?}: {}",
                                c.name,
                                p.combat.get(&kind).map_or("MISSING", |a| a.name.as_str())
                            );
                        }
                    }
                    // Drawn weapon states: the same animation types in the weapon subset.
                    let clip = |subset: &str, name: &str| {
                        c.animation_in_subset(&level.banks, subset, name)
                            .and_then(|a| a.source_file)
                            .and_then(|path| cache.animation(vfs, &path))
                    };
                    // Drawn weapons: the combat idle of the weapon subset (see
                    // `IDLE_STILL_COMBAT`), else the peace idle.
                    let combat_idle = simple_names.get(IDLE_STILL_COMBAT);
                    for (state, subset) in subsets {
                        let set = PreparedClips {
                            idle: combat_idle
                                .and_then(|id| clip(subset, id))
                                .or_else(|| clip(subset, &idle)),
                            run: clip(subset, RUN_PEACE),
                            combat: CombatAnimation::ALL
                                .into_iter()
                                .filter_map(|k| {
                                    let id = simple_names.get(k.simple_name())?;
                                    Some((k, clip(subset, id)?))
                                })
                                .collect(),
                        };
                        if std::env::var_os("BG3_CHAR_DEBUG").is_some() {
                            eprintln!(
                                "{state:?} clips {}: idle {:?}, run {:?}",
                                c.name,
                                set.idle.as_ref().map(|a| &a.name),
                                set.run.as_ref().map(|a| &a.name)
                            );
                        }
                        p.drawn.insert(*state, set);
                    }
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
    presets: &HashMap<String, bg3_world::characters::MaterialOverrides>,
    weapons: &bg3_world::unsheath::WeaponData,
) -> Result<PreparedCharacter, String> {
    let looks = crate::char_materials::LookOverrides::new(c, items, presets);
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
        for obj in part.visual.drawn_lod0_objects() {
            let Some(mesh) = scene.meshes.iter().find(|m| m.name == obj.mesh_name()) else {
                continue;
            };
            let bevy_mesh = if part.attach_bone.is_some() {
                crate::meshes::to_bevy_mesh(mesh)
            } else {
                skinned_mesh(mesh, &skeleton).0
            };
            parts.push(PreparedPart {
                mesh: bevy_mesh,
                material_id: obj.material_id.clone(),
                attach_joint,
                local: Transform::default(),
                look: looks.part(&part.visual, mesh),
                weapon: None,
            });
        }
    }
    if c.character_visual.show_equipment_visuals {
        parts.extend(rigid_equipment_parts(vfs, cache, &skeleton, items, weapons));
    }
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
        combat: HashMap::new(),
        drawn: HashMap::new(),
        weapon_set: WeaponSet::default(),
        run_speed: c
            .props
            .movement_speed_run
            .unwrap_or(DEFAULT_MOVEMENT_SPEED_RUN),
        step_height: c
            .props
            .movement_step_up_height
            .unwrap_or(DEFAULT_MOVEMENT_STEP_UP_HEIGHT),
        cut_bytes: c.body_masks.iter().filter_map(|m| m.value).collect(),
    })
}

/// Weapons and rigid wearable attachments.
///
/// A visual in a weapon slot (shields included) gets both placements from its equipment type
/// (`bg3_world::unsheath::placement`): the weapon model's source bone (`Dummy_Sheath`,
/// `Dummy_Attachment`, …) is put on the character's sheath or hand bone. It is spawned
/// sheathed; [`Unsheath`] moves it. Other rigid items use their visual's `AttachBone`.
fn rigid_equipment_parts(
    vfs: &Vfs,
    cache: &GrannyCache,
    skeleton: &Skeleton,
    items: &[EquippedItem],
    weapons: &bg3_world::unsheath::WeaponData,
) -> Vec<PreparedPart> {
    let held = weapon_items(items);
    let mut out = Vec::new();
    for item in items {
        if !item.worn.is_empty() {
            continue;
        }
        let Some(visual) = &item.visual else { continue };
        let Some(path) = visual.source_file.as_deref() else {
            continue;
        };
        let Ok(scene) = cache.scene(vfs, path) else {
            continue;
        };
        // The source bone's rest pose is where the target bone goes: mesh = inverse(bone world).
        let source_local = |name: &str| {
            scene
                .skeletons
                .first()
                .and_then(|s| s.bones.iter().find(|b| b.name == name))
                .map_or(Transform::default(), |b| {
                    Transform::from_matrix(inverse_bind(&b.inverse_world))
                })
        };
        let (attach_joint, local, weapon) = if let Some(w) = item.weapon_item() {
            let p = placement(weapons, &w, &held);
            let mount = |a: &bg3_world::unsheath::Attachment| {
                let joint = skeleton.index(a.bone.as_deref()?)?;
                Some((joint, source_local(&a.source)))
            };
            let pose = WeaponPose {
                slot: p.slot,
                sheathed: mount(&p.sheathed),
                unsheathed: mount(&p.unsheathed),
            };
            if pose.sheathed.is_none() && pose.unsheathed.is_none() {
                continue;
            }
            let (joint, local) = pose.sheathed.unwrap_or_default();
            (Some(joint), local, Some(pose))
        } else {
            let Some(joint) = visual
                .attach_bone
                .as_deref()
                .and_then(|b| skeleton.index(b))
            else {
                continue;
            };
            (Some(joint), Transform::default(), None)
        };
        for obj in visual.drawn_lod0_objects() {
            if let Some(mesh) = scene.meshes.iter().find(|m| m.name == obj.mesh_name()) {
                out.push(PreparedPart {
                    mesh: crate::meshes::to_bevy_mesh(mesh),
                    material_id: obj.material_id.clone(),
                    attach_joint,
                    local,
                    look: Default::default(),
                    weapon: weapon.clone(),
                });
            }
        }
    }
    out
}

/// Convert a GR2 mesh and remap its bone indices (into the mesh's `BoneBindings`) to joints of
/// `skeleton` by bone name. Returns the bone names the skeleton lacks; their weights are moved to
/// the root joint so the vertices stay attached to the character.
///
/// A rigid mesh (no per-vertex bone data: the separate ears, horns, circlets and caps of heads)
/// follows its first bone binding, so every vertex is bound to that bone with full weight.
pub fn skinned_mesh(m: &bg3_gr2::Mesh, skeleton: &Skeleton) -> (Mesh, Vec<String>) {
    let mut mesh = crate::meshes::to_bevy_mesh(m);
    let mut missing = Vec::new();
    let n = m.positions.len();
    // bg3: gr::FactoryGr2::Parse → GrannyMeshIsRigid (4.1.1.7631656)
    let rigid = m.bone_indices.is_empty() && m.bone_weights.is_empty();
    let weighted = m.bone_indices.len() == n && m.bone_weights.len() == n;
    if !(rigid && !m.bone_bindings.is_empty() || weighted) {
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
    let (joints, weights): (Vec<[u16; 4]>, Vec<[f32; 4]>) = if rigid {
        (vec![[remap[0]; 4]; n], vec![[1.0, 0.0, 0.0, 0.0]; n])
    } else {
        let joints = m
            .bone_indices
            .iter()
            .map(|idx| idx.map(|i| remap.get(usize::from(i)).copied().unwrap_or(0)))
            .collect();
        let weights = m
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
        (joints, weights)
    };
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

/// Shared CLI name matching for `--player`, `--talk` and `--frame`.
pub fn name_matches(name: &str, substring: &str) -> bool {
    name.to_lowercase().contains(&substring.to_lowercase())
}

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
    pub combat: HashMap<CombatAnimation, BoundClip>,
}

/// A character's weapon state as shown: weapon visuals and clip sets follow `state`.
///
/// The sim owns the state (`bg3_sim::Sim::unsheath_state`); the combat UI sets `state` when it
/// presents a `CombatEvent::Unsheathed`, and [`apply_unsheath`] re-attaches the weapons and
/// swaps [`CharacterRoot`]'s idle/run/combat clips. The switch is instant: the game's
/// sheathe/unsheathe animation (`ecl::CharacterTask_SheathUnsheath`) is not played.
#[derive(Component)]
pub struct Unsheath {
    pub state: UnsheathState,
    applied: UnsheathState,
    /// The active weapon set: a weapon that is not drawn is shown only if it is in this set.
    weapon_set: WeaponSet,
    weapons: Vec<(Entity, WeaponMount)>,
    /// Bound clip sets per state; the sheathed set is always present.
    clips: HashMap<UnsheathState, ClipSet>,
}

#[derive(Clone)]
struct WeaponMount {
    slot: WeaponSlot,
    sheathed: Option<(Entity, Transform)>,
    unsheathed: Option<(Entity, Transform)>,
}

#[derive(Clone, Default)]
struct ClipSet {
    idle: Option<BoundClip>,
    run: Option<BoundClip>,
    combat: HashMap<CombatAnimation, BoundClip>,
}

impl ClipSet {
    fn bind(c: &PreparedClips, skeleton: &Skeleton) -> Self {
        let b = |a: &Arc<PreparedAnimation>| (a.clone(), a.bind(skeleton));
        Self {
            idle: c.idle.as_ref().map(b),
            run: c.run.as_ref().map(b),
            combat: c.combat.iter().map(|(&k, a)| (k, b(a))).collect(),
        }
    }
}

/// Re-attach weapon visuals and swap clip sets when a character's weapon state changes.
pub fn apply_unsheath(
    mut commands: Commands,
    mut chars: Query<(
        &mut Unsheath,
        &mut CharacterRoot,
        Option<&mut CharacterAnimator>,
    )>,
) {
    for (mut u, mut root, animator) in &mut chars {
        if u.state == u.applied {
            continue;
        }
        let state = u.state;
        u.applied = state;
        for (entity, m) in &u.weapons {
            let mount = if !slot_shown(m.slot, state, u.weapon_set) {
                None
            } else if state.draws(m.slot) {
                m.unsheathed
            } else {
                m.sheathed
            };
            match mount {
                Some((joint, local)) => {
                    commands
                        .entity(*entity)
                        .insert((ChildOf(joint), local, Visibility::Inherited));
                }
                None => {
                    commands.entity(*entity).insert(Visibility::Hidden);
                }
            }
        }
        // A clip missing from the state's set falls back to the sheathed (default) one.
        let base = u
            .clips
            .get(&UnsheathState::Sheathed)
            .cloned()
            .unwrap_or_default();
        let set = u.clips.get(&state).cloned().unwrap_or_default();
        let old = (root.idle.clone(), root.run.clone());
        root.idle = set.idle.or(base.idle);
        root.run = set.run.or(base.run);
        root.combat = base.combat;
        root.combat.extend(set.combat);
        if let Some(mut a) = animator {
            a.swap_loops(&old.0, &root.idle);
            a.swap_loops(&old.1, &root.run);
        }
    }
}

/// Joint entities of a spawned character, plus its animation state.
#[derive(Component)]
pub struct CharacterAnimator {
    pub joints: Vec<Entity>,
    pub animation: Arc<PreparedAnimation>,
    /// `(joint index, track index)`.
    pub bindings: Vec<(usize, usize)>,
    pub time: f32,
    playback: Playback,
    /// Death/downed poses must not be replaced by navigation or a late hit event.
    pub incapacitated: bool,
    rest: Vec<Transform>,
}

enum Playback {
    Loop,
    Once { then: Option<BoundClip> },
    Hold,
}

/// Advance without wrapping one-shots back to their first frame.
fn advance_clip(time: f32, dt: f32, duration: f32, looping: bool) -> (f32, bool) {
    let end = duration.max(0.0);
    let next = time + dt.max(0.0);
    if looping && end > 0.0 {
        (next % end, false)
    } else {
        (next.min(end), next >= end)
    }
}

impl CharacterAnimator {
    fn select(&mut self, clip: &BoundClip, playback: Playback) {
        self.animation = clip.0.clone();
        self.bindings.clone_from(&clip.1);
        self.time = 0.0;
        self.playback = playback;
    }

    /// Replace `old` with `new` where it plays as a loop or is the clip a one-shot returns
    /// to, keeping the loop's phase.
    pub fn swap_loops(&mut self, old: &Option<BoundClip>, new: &Option<BoundClip>) {
        let (Some(old), Some(new)) = (old, new) else {
            return;
        };
        if Arc::ptr_eq(&old.0, &new.0) {
            return;
        }
        match &mut self.playback {
            Playback::Loop if Arc::ptr_eq(&self.animation, &old.0) => {
                let phase = self.time / old.0.duration.max(1e-3);
                self.select(new, Playback::Loop);
                self.time = phase * new.0.duration;
            }
            Playback::Once { then: Some(then) } if Arc::ptr_eq(&then.0, &old.0) => {
                *then = new.clone();
            }
            _ => {}
        }
    }

    pub fn locomotion(&mut self, clip: &Option<BoundClip>) {
        if self.incapacitated || !matches!(self.playback, Playback::Loop) {
            return;
        }
        if let Some(clip) = clip
            && !Arc::ptr_eq(&self.animation, &clip.0)
        {
            self.select(clip, Playback::Loop);
        }
    }

    pub fn once(&mut self, clip: &BoundClip, then: Option<BoundClip>) -> f32 {
        self.select(clip, Playback::Once { then });
        clip.0.duration.max(0.0)
    }

    pub fn downed(&mut self, clip: &BoundClip) {
        self.select(clip, Playback::Loop);
        self.incapacitated = true;
    }

    pub fn freeze(&mut self) {
        self.incapacitated = true;
        self.playback = Playback::Hold;
    }

    pub fn revive(&mut self, idle: &Option<BoundClip>) {
        self.incapacitated = false;
        if let Some(clip) = idle {
            self.select(clip, Playback::Loop);
        }
    }

    fn advance(&mut self, dt: f32) {
        if matches!(self.playback, Playback::Hold) {
            return;
        }
        let remaining = (self.time + dt - self.animation.duration).max(0.0);
        let (t, ended) = advance_clip(
            self.time,
            dt,
            self.animation.duration,
            matches!(self.playback, Playback::Loop),
        );
        self.time = t;
        if ended {
            let old = std::mem::replace(&mut self.playback, Playback::Hold);
            if let Playback::Once { then: Some(clip) } = old {
                // Carry the unused time into the return loop, independent of frame rate.
                self.select(&clip, Playback::Loop);
                self.time = advance_clip(0.0, remaining, clip.0.duration, true).0;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_characters(
    mut commands: Commands,
    mut characters: ResMut<LevelCharacters>,
    level_materials: Res<crate::materials::LevelMaterials>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    templates: Option<Res<crate::char_materials::CharacterTemplates>>,
    framing: Option<Res<crate::framing::FrameTarget>>,
    mut char_materials: ResMut<Assets<crate::char_materials::CharacterMaterial>>,
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
        let mut weapons: Vec<(Entity, WeaponMount)> = Vec::new();
        for mut part in c.parts {
            let Some(material) = crate::char_materials::part_material(
                &mut part,
                &c.cut_bytes,
                templates.as_deref(),
                &mut char_materials,
                &level_materials,
                &fallback,
            ) else {
                continue; // helper material the game doesn't draw
            };
            let skinned = part.mesh.attribute(Mesh::ATTRIBUTE_JOINT_INDEX).is_some();
            let mesh = meshes.add(part.mesh);
            if skinned {
                let mut e = commands.spawn((
                    Mesh3d(mesh),
                    SkinnedMesh {
                        inverse_bindposes: ibm.clone(),
                        joints: joints.clone(),
                    },
                    Transform::default(),
                    ChildOf(root),
                ));
                if framing.is_some() {
                    e.insert(bevy::camera::visibility::DynamicSkinnedMeshBounds);
                }
                material.insert(&mut e);
            } else {
                let parent = part
                    .attach_joint
                    .and_then(|j| joints.get(j).copied())
                    .unwrap_or(root);
                let mut e = commands.spawn((Mesh3d(mesh), part.local, ChildOf(parent)));
                if let Some(pose) = &part.weapon {
                    // Spawned sheathed; hidden when the sheathed type names no bone or the
                    // weapon is not in the active weapon set.
                    if pose.sheathed.is_none()
                        || !slot_shown(pose.slot, UnsheathState::Sheathed, c.weapon_set)
                    {
                        e.insert(Visibility::Hidden);
                    }
                    let joint = |m: Option<(usize, Transform)>| {
                        m.and_then(|(j, t)| Some((joints.get(j).copied()?, t)))
                    };
                    weapons.push((
                        e.id(),
                        WeaponMount {
                            slot: pose.slot,
                            sheathed: joint(pose.sheathed),
                            unsheathed: joint(pose.unsheathed),
                        },
                    ));
                }
                material.insert(&mut e);
            }
        }
        let mut clips: HashMap<UnsheathState, ClipSet> = c
            .drawn
            .iter()
            .map(|(&st, set)| (st, ClipSet::bind(set, &c.skeleton)))
            .collect();
        clips.insert(
            UnsheathState::Sheathed,
            ClipSet::bind(
                &PreparedClips {
                    idle: c.animation.clone(),
                    run: c.run.clone(),
                    combat: c.combat.clone(),
                },
                &c.skeleton,
            ),
        );
        commands.entity(root).insert(Unsheath {
            state: UnsheathState::Sheathed,
            applied: UnsheathState::Sheathed,
            weapon_set: c.weapon_set,
            weapons,
            clips,
        });
        commands.entity(root).insert(CharacterRoot {
            id: c.id.clone(),
            name: c.name.clone(),
            run_speed: c.run_speed,
            step_height: c.step_height,
            idle: c.animation.clone().map(|a| {
                let b = a.bind(&c.skeleton);
                (a, b)
            }),
            combat: c
                .combat
                .iter()
                .map(|(&kind, a)| (kind, (a.clone(), a.bind(&c.skeleton))))
                .collect(),
            run: c.run.clone().map(|a| {
                let b = a.bind(&c.skeleton);
                (a, b)
            }),
        });
        let has_idle = c.animation.is_some();
        let animation = c
            .animation
            .or(c.run)
            .or_else(|| {
                CombatAnimation::ALL
                    .iter()
                    .find_map(|k| c.combat.get(k).cloned())
            })
            .filter(|_| std::env::var_os("BG3_NO_ANIM").is_none());
        if let Some(animation) = animation {
            let bindings = animation.bind(&c.skeleton);
            // Desynchronise identical idles a little, deterministically.
            rng_phase = (rng_phase + 0.37) % 1.0;
            commands.entity(root).insert(CharacterAnimator {
                joints,
                time: if has_idle {
                    rng_phase * animation.duration
                } else {
                    0.0
                },
                animation,
                bindings,
                playback: if has_idle {
                    Playback::Loop
                } else {
                    Playback::Hold
                },
                incapacitated: false,
                rest: c.skeleton.joints.iter().map(|j| j.rest).collect(),
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
        a.advance(dt);
        let duration = a.animation.duration.max(1e-3);
        // Reset unbound bones when clips have different track coverage.
        for (&joint, rest) in a.joints.iter().zip(&a.rest) {
            if let Ok(mut tr) = transforms.get_mut(joint) {
                *tr = *rest;
            }
        }
        let t = a.time;
        for &(joint, track) in &a.bindings {
            let sample = if matches!(a.playback, Playback::Loop) {
                a.animation.tracks[track].sample_looped(t, duration)
            } else {
                a.animation.tracks[track].sample(t)
            };
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
            .add_systems(Update, (apply_unsheath, animate_characters).chain());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(name: &str, duration: f32) -> BoundClip {
        (
            Arc::new(PreparedAnimation {
                name: name.into(),
                duration,
                tracks: vec![],
            }),
            vec![],
        )
    }

    fn animator(clip: &BoundClip) -> CharacterAnimator {
        CharacterAnimator {
            animation: clip.0.clone(),
            bindings: vec![],
            joints: vec![],
            rest: vec![],
            time: 0.0,
            playback: Playback::Loop,
            incapacitated: false,
        }
    }

    #[test]
    fn weapon_state_swaps_loops_keeping_phase_and_return_clips() {
        let (idle, run) = (clip("idle", 2.0), clip("run", 1.0));
        let (drawn_idle, drawn_run) = (clip("drawn_idle", 4.0), clip("drawn_run", 1.0));
        let mut a = animator(&idle);
        a.time = 0.5;
        a.swap_loops(&Some(idle.clone()), &Some(drawn_idle.clone()));
        a.swap_loops(&Some(run.clone()), &Some(drawn_run.clone()));
        assert_eq!(a.animation.name, "drawn_idle");
        assert!((a.time - 1.0).abs() < 1e-6, "same phase: {}", a.time);
        // A one-shot returns to the new idle.
        let attack = clip("attack", 0.5);
        a.once(&attack, Some(drawn_idle.clone()));
        a.swap_loops(&Some(drawn_idle), &Some(idle.clone()));
        assert_eq!(a.animation.name, "attack");
        a.advance(0.6);
        assert_eq!(a.animation.name, "idle");
    }

    #[test]
    fn one_shot_returns_to_idle_and_carries_remaining_time() {
        let idle = clip("idle", 2.0);
        let attack = clip("attack", 0.7);
        let mut a = animator(&idle);
        a.once(&attack, Some(idle.clone()));
        a.locomotion(&Some(idle.clone())); // navigation cannot cancel the attack
        a.advance(0.5);
        assert_eq!(a.animation.name, "attack");
        assert_eq!(a.time, 0.5);
        a.advance(0.3);
        assert_eq!(a.animation.name, "idle");
        assert!((a.time - 0.1).abs() < 1e-5);
        a.advance(4.0);
        assert!((a.time - 0.1).abs() < 1e-5);
    }

    #[test]
    fn death_holds_final_frame_and_downed_survives_locomotion_until_revived() {
        let idle = clip("idle", 2.0);
        let death = clip("death", 1.0);
        let mut a = animator(&idle);
        a.incapacitated = true;
        a.once(&death, None);
        a.advance(1.0);
        a.advance(50.0);
        a.locomotion(&Some(idle.clone()));
        assert_eq!(a.animation.name, "death");
        assert_eq!(a.time, 1.0);
        a.downed(&clip("downed", 3.0));
        a.advance(4.5);
        a.locomotion(&Some(idle.clone()));
        assert_eq!(a.animation.name, "downed");
        assert_eq!(a.time, 1.5);
        a.revive(&Some(idle));
        assert!(!a.incapacitated);
        assert_eq!(a.animation.name, "idle");
    }

    #[test]
    fn zero_duration_and_overshoot_do_not_wrap_a_one_shot() {
        assert_eq!(advance_clip(0.0, 1.0, 0.0, false), (0.0, true));
        assert_eq!(advance_clip(0.5, 8.0, 1.0, false), (1.0, true));
        let mut a = animator(&clip("zero", 0.0));
        a.once(&clip("zero", 0.0), None);
        a.advance(1.0);
        assert_eq!(a.time, 0.0);
        assert!(matches!(a.playback, Playback::Hold));
    }

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

    fn bindings(names: &[&str]) -> Vec<bg3_gr2::BoneBinding> {
        names
            .iter()
            .map(|n| bg3_gr2::BoneBinding {
                bone_name: (*n).into(),
                obb_min: [0.0; 3],
                obb_max: [0.0; 3],
            })
            .collect()
    }

    #[test]
    fn rigid_mesh_follows_its_first_bone_binding() {
        let s = Skeleton::from_gr2(&test_skeleton());
        let rigid = |names: &[&str]| bg3_gr2::Mesh {
            positions: vec![[0.0; 3]; 3],
            normals: vec![[0.0, 1.0, 0.0]; 3],
            indices: vec![0, 1, 2],
            bone_bindings: bindings(names),
            ..Default::default()
        };
        let skin = |m: &bg3_gr2::Mesh| {
            let (mesh, missing) = skinned_mesh(m, &s);
            let joints = match mesh.attribute(Mesh::ATTRIBUTE_JOINT_INDEX) {
                Some(VertexAttributeValues::Uint16x4(j)) => Some(j.clone()),
                _ => None,
            };
            let weights = match mesh.attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT) {
                Some(VertexAttributeValues::Float32x4(w)) => Some(w.clone()),
                _ => None,
            };
            (joints, weights, missing)
        };

        let (j, w, missing) = skin(&rigid(&["Spine"]));
        assert_eq!(j, Some(vec![[1; 4]; 3]));
        assert_eq!(w, Some(vec![[1.0, 0.0, 0.0, 0.0]; 3]));
        assert!(missing.is_empty());

        // Only the first binding counts; an unknown bone falls back to the root and is reported.
        let (j, _, missing) = skin(&rigid(&["Missing", "Hand_L"]));
        assert_eq!(j, Some(vec![[0; 4]; 3]));
        assert_eq!(missing, vec!["Missing".to_string()]);

        // Without any binding there is nothing to follow: the mesh stays unskinned.
        let (j, w, _) = skin(&rigid(&[]));
        assert_eq!((j, w), (None, None));
    }

    /// Lae'zel's ears are a rigid mesh bound to `Head_M`, which her body rig animates.
    #[test]
    #[ignore = "needs BG3 install"]
    fn laezel_ears_follow_the_head() {
        let install = match bg3_install::Install::locate() {
            Ok(i) => i,
            Err(e) => {
                eprintln!("skipping: no BG3 install ({e})");
                return;
            }
        };
        let vfs = Vfs::from_install(&install).expect("vfs");
        let read =
            |path: &str| bg3_gr2::Scene::from_bytes(&vfs.read(path).expect(path)).expect(path);
        let dir = "Public/Shared/Assets/Characters/_Anims/Githyanki/_Female";
        let base = read(&format!("{dir}/GTY_F_Base.GR2"));
        let head = read(&format!(
            "Generated/{dir}/Resources/GTY_F_NKD_Head_Laezel.GR2"
        ));
        let mut skeleton = Skeleton::from_gr2(&base.skeletons[0]);
        let names = head
            .meshes
            .iter()
            .flat_map(|m| &m.bone_bindings)
            .map(|b| b.bone_name.as_str());
        skeleton.extend_from(&head.skeletons[0], names);
        let ears = head
            .meshes
            .iter()
            .find(|m| m.name == "GTY_F_NKD_Head_Laezel_Ears_Mesh")
            .expect("ears mesh");
        assert!(ears.bone_indices.is_empty() && ears.bone_weights.is_empty());
        let head_m = skeleton.index("Head_M").expect("Head_M") as u16;
        let (mesh, missing) = skinned_mesh(ears, &skeleton);
        assert!(missing.is_empty(), "{missing:?}");
        let Some(VertexAttributeValues::Uint16x4(j)) = mesh.attribute(Mesh::ATTRIBUTE_JOINT_INDEX)
        else {
            panic!("ears not skinned");
        };
        assert!(j.iter().all(|v| *v == [head_m; 4]));
    }
}
