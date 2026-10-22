//! Navigation in the viewer: an overlay of the level's AI grid (`--nav`) and click-to-move for
//! one character, path-found with `bg3-nav` and walked at its template run speed.
//!
//! All path finding happens in BG3 space; positions cross into Bevy through `space.rs`.

use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bg3_nav::aigrid::TILE_SIZE;
use bg3_nav::grid::column_of;
use bg3_nav::{Agent, NavGrid};

use crate::characters::{CharacterAnimator, CharacterRoot};
use crate::space;

/// The level's AI grid.
#[derive(Resource, Clone)]
pub struct Navigation {
    pub grid: Arc<NavGrid>,
}

/// Viewer options for navigation.
#[derive(Resource, Clone, Default)]
pub struct NavOptions {
    /// Draw the grid overlay.
    pub overlay: bool,
    /// Pick the controlled character by name (substring, case-insensitive).
    pub player: Option<String>,
    /// Pick the controlled character by level object id (wins over `player`; `--play` sets
    /// it to the avatar).
    pub player_id: Option<String>,
    /// Walk here on start (BG3 world coordinates), for screenshots.
    pub walk_to: Option<[f32; 3]>,
}

/// Overlay tiles further than this (metres, horizontally) from the camera focus are skipped on
/// levels with many tiles.
const OVERLAY_RADIUS: f32 = 120.0;
/// Below this many non-empty tiles, the whole grid is drawn.
const OVERLAY_ALL_BELOW: usize = 600_000;

/// The character that follows clicks.
#[derive(Component)]
pub struct Controlled {
    pub agent: Agent,
    /// m/s.
    pub speed: f32,
    /// Remaining waypoints (Bevy space), nearest first.
    pub waypoints: Vec<Vec3>,
    /// The whole current path, for drawing.
    pub path: Vec<Vec3>,
    pub moving: bool,
}

pub struct NavPlugin {
    pub navigation: Navigation,
    pub options: NavOptions,
}

impl Plugin for NavPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(self.navigation.clone())
            .insert_resource(self.options.clone())
            .add_systems(Startup, spawn_overlay)
            .add_systems(PostStartup, (select_character, walk_on_start).chain())
            .add_systems(
                Update,
                (click_to_move, follow_path, swap_animation, draw_path).chain(),
            );
    }
}

impl Navigation {
    pub fn load(vfs: &bg3_vfs::Vfs, level: &str) -> anyhow::Result<Self> {
        let started = std::time::Instant::now();
        let file = bg3_nav::load_aigrid(vfs, level)?;
        let grid = NavGrid::new(file);
        eprintln!(
            "nav: {} AI subgrids in {:.2?}",
            grid.file.subgrids.len(),
            started.elapsed()
        );
        Ok(Self {
            grid: Arc::new(grid),
        })
    }
}

fn to_bevy(p: [f32; 3]) -> Vec3 {
    Vec3::from_array(space::vec3(p))
}

fn to_bg3(v: Vec3) -> [f32; 3] {
    space::vec3(v.to_array())
}

/// The point the camera starts looking at (Bevy space), or the camera position.
fn focus(start: &crate::CameraStart) -> Option<Vec3> {
    start.look_at.or(start.position)
}

fn spawn_overlay(
    mut commands: Commands,
    nav: Res<Navigation>,
    options: Res<NavOptions>,
    start: Res<crate::CameraStart>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if !options.overlay {
        return;
    }
    let started = std::time::Instant::now();
    let grid = &nav.grid;
    let agent = Agent::humanoid();
    let non_empty: usize = grid
        .file
        .subgrids
        .iter()
        .map(|s| s.tiles.iter().filter(|t| !t.is_empty()).count())
        .sum();
    let center = focus(&start).map(to_bg3);
    let limit = (non_empty >= OVERLAY_ALL_BELOW).then_some(center).flatten();
    let (mut positions, mut colors, mut indices) = (Vec::new(), Vec::new(), Vec::new());
    let half = TILE_SIZE * 0.45;
    let (mut standable, mut blocked) = (0usize, 0usize);
    for (i, s) in grid.file.subgrids.iter().enumerate() {
        for y in 0..s.height {
            for x in 0..s.width {
                let t = bg3_nav::TileRef {
                    subgrid: i as u32,
                    x: x as u16,
                    y: y as u16,
                };
                if grid.tile(t).is_empty() {
                    continue;
                }
                let c = grid.world_pos(t);
                if let Some(f) = limit
                    && (c[0] - f[0]).hypot(c[2] - f[2]) > OVERLAY_RADIUS
                {
                    continue;
                }
                let color = if grid.is_colliding(t, &agent) {
                    blocked += 1;
                    [0.9, 0.15, 0.1, 0.35]
                } else if grid.is_area_unstable(t, &agent) {
                    blocked += 1;
                    [0.95, 0.75, 0.1, 0.35]
                } else {
                    standable += 1;
                    [0.1, 0.9, 0.3, 0.4]
                };
                let base = positions.len() as u32;
                for (dx, dz) in [(-half, -half), (half, -half), (half, half), (-half, half)] {
                    positions.push(space::vec3([c[0] + dx, c[1] + 0.04, c[2] + dz]));
                    colors.push(color);
                }
                indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
            }
        }
    }
    let count = positions.len() / 4;
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(indices));
    commands.spawn((
        Name::new("AI grid overlay"),
        Mesh3d(meshes.add(mesh)),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::WHITE,
            unlit: true,
            cull_mode: None,
            alpha_mode: AlphaMode::Blend,
            depth_bias: 50.0,
            ..default()
        })),
        Transform::default(),
    ));
    eprintln!(
        "nav overlay: {count} tiles ({standable} standable for a humanoid, {blocked} blocked) in {:.2?}",
        started.elapsed()
    );
}

/// Make one character controllable: `--player NAME`, else the one nearest the camera focus;
/// without characters, a placeholder capsule on the grid.
fn select_character(
    mut commands: Commands,
    nav: Res<Navigation>,
    options: Res<NavOptions>,
    start: Res<crate::CameraStart>,
    roots: Query<(Entity, &CharacterRoot, &Transform)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let focus = focus(&start).unwrap_or(Vec3::ZERO);
    let by_id = options
        .player_id
        .as_ref()
        .and_then(|id| roots.iter().find(|(_, r, _)| &r.id == id));
    let chosen = match &options.player {
        _ if by_id.is_some() => by_id,
        Some(name) => {
            let name = name.to_lowercase();
            roots
                .iter()
                .find(|(_, r, _)| crate::characters::name_matches(&r.name, &name))
        }
        None => roots
            .iter()
            .filter(|(_, _, t)| {
                nav.grid
                    .project(to_bg3(t.translation), 1.0)
                    .is_some_and(|tile| nav.grid.is_standable(tile, &Agent::humanoid()))
            })
            .min_by(|a, b| {
                a.2.translation
                    .distance_squared(focus)
                    .total_cmp(&b.2.translation.distance_squared(focus))
            }),
    };
    if let Some((entity, root, t)) = chosen {
        eprintln!(
            "nav: controlling {} at {:?} (run {} m/s, step {} m)",
            root.name,
            to_bg3(t.translation),
            root.run_speed,
            root.step_height
        );
        commands.entity(entity).insert(Controlled {
            agent: Agent {
                step_height: root.step_height,
                ..Agent::humanoid()
            },
            speed: root.run_speed,
            waypoints: Vec::new(),
            path: Vec::new(),
            moving: false,
        });
        return;
    }
    // Placeholder: the standable tile nearest the focus.
    let agent = Agent::humanoid();
    let Some(tile) = nav.grid.project_standable(to_bg3(focus), 50.0, 20, &agent) else {
        eprintln!("nav: no character and no standable tile near the camera focus");
        return;
    };
    let p = to_bevy(nav.grid.world_pos(tile));
    eprintln!("nav: controlling a placeholder at {:?}", to_bg3(p));
    let body = commands
        .spawn((
            Name::new("Placeholder character"),
            Transform::from_translation(p),
            Visibility::default(),
            Controlled {
                agent,
                speed: bg3_world::characters::DEFAULT_MOVEMENT_SPEED_RUN,
                waypoints: Vec::new(),
                path: Vec::new(),
                moving: false,
            },
        ))
        .id();
    commands.spawn((
        Mesh3d(meshes.add(Capsule3d::new(0.3, 1.2))),
        MeshMaterial3d(materials.add(Color::srgb(0.2, 0.5, 0.95))),
        Transform::from_xyz(0.0, 0.9, 0.0),
        ChildOf(body),
    ));
}

/// Plan a path for the controlled character to `goal` (BG3 space).
fn plan(nav: &Navigation, c: &mut Controlled, from: Vec3, goal: [f32; 3]) {
    let started = std::time::Instant::now();
    match nav.grid.find_path(to_bg3(from), goal, &c.agent) {
        Ok(path) => {
            eprintln!(
                "nav: path to {goal:?}: {} tiles, {:.1} m, {} waypoints, {} expanded in {:.2?}",
                path.tiles.len(),
                path.length(),
                path.waypoints.len(),
                path.expanded,
                started.elapsed()
            );
            c.path = path.waypoints.iter().map(|&w| to_bevy(w)).collect();
            c.waypoints = c.path.iter().skip(1).copied().collect();
            c.moving = !c.waypoints.is_empty();
        }
        Err(e) => eprintln!("nav: no path to {goal:?}: {e}"),
    }
}

fn walk_on_start(
    nav: Res<Navigation>,
    options: Res<NavOptions>,
    mut controlled: Query<(&mut Controlled, &Transform)>,
) {
    let Some(goal) = options.walk_to else {
        return;
    };
    for (mut c, t) in &mut controlled {
        plan(&nav, &mut c, t.translation, goal);
    }
}

pub fn click_to_move(
    buttons: Res<ButtonInput<MouseButton>>,
    nav: Res<Navigation>,
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform)>,
    mut controlled: Query<(&mut Controlled, &Transform)>,
) {
    if !buttons.just_pressed(MouseButton::Left) {
        return;
    }
    let Some(cursor) = windows.iter().find_map(Window::cursor_position) else {
        return;
    };
    let Some(ray) = cameras
        .iter()
        .find_map(|(cam, gt)| cam.viewport_to_world(gt, cursor).ok())
    else {
        return;
    };
    let Some((_, hit)) = nav
        .grid
        .raycast(to_bg3(ray.origin), to_bg3(*ray.direction), 500.0)
    else {
        eprintln!("nav: click missed the AI grid");
        return;
    };
    for (mut c, t) in &mut controlled {
        plan(&nav, &mut c, t.translation, hit);
    }
}

/// Move along the waypoints at the character's speed, facing the direction of travel and
/// keeping the feet on the grid surface.
fn follow_path(
    time: Res<Time>,
    nav: Res<Navigation>,
    mut controlled: Query<(&mut Controlled, &mut Transform)>,
) {
    let dt = time.delta_secs();
    for (mut c, mut t) in &mut controlled {
        let mut budget = c.speed * dt;
        while budget > 0.0 {
            let Some(&next) = c.waypoints.first() else {
                c.moving = false;
                break;
            };
            let to = next - t.translation;
            let flat = Vec3::new(to.x, 0.0, to.z);
            let dist = flat.length();
            if dist > 1e-4 {
                // The model faces −Z in BG3 space, i.e. +Z after mirroring.
                t.rotation = Quat::from_rotation_y(flat.x.atan2(flat.z));
            }
            if dist <= budget {
                t.translation = next;
                budget -= dist;
                c.waypoints.remove(0);
            } else {
                t.translation += flat / dist * budget;
                budget = 0.0;
            }
        }
        if c.moving {
            // Follow the surface between waypoints (stairs, ramps).
            let p = to_bg3(t.translation);
            if let Some(tile) = nav.grid.closest_in_column(column_of(p), p[1])
                && (nav.grid.height(tile) - p[1]).abs() < 1.0
            {
                t.translation.y = nav.grid.height(tile);
            }
        }
    }
}

/// Run while moving, idle while standing.
fn swap_animation(
    mut q: Query<
        (&Controlled, &CharacterRoot, &mut CharacterAnimator),
        Without<crate::combat_ui::CombatWalk>,
    >,
) {
    for (c, root, mut anim) in &mut q {
        let want = if c.moving { &root.run } else { &root.idle };
        anim.locomotion(want);
    }
}

fn draw_path(mut gizmos: Gizmos, controlled: Query<(&Controlled, &Transform)>) {
    for (c, t) in &controlled {
        if c.moving {
            let lift = Vec3::Y * 0.1;
            gizmos.linestrip(
                std::iter::once(t.translation + lift).chain(c.waypoints.iter().map(|w| *w + lift)),
                Color::srgb(0.2, 0.7, 1.0),
            );
            if let Some(goal) = c.path.last() {
                gizmos.sphere(
                    Isometry3d::from_translation(*goal + lift),
                    0.25,
                    Color::srgb(0.2, 0.7, 1.0),
                );
            }
        }
    }
}
