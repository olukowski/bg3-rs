//! `BG3_PICK=x,y[;x,y…]`: when the screenshot is taken, cast a ray through each point of the
//! window (fractions of its size, 0..1 from the top left) and print every level mesh it hits,
//! nearest first: the object's name, the hit point in BG3 space and the surface normal. A
//! debugging aid for finding which object draws (or fails to draw) a part of a frame.

use bevy::asset::RenderAssetUsages;
use bevy::picking::mesh_picking::ray_cast::{
    MeshRayCast, MeshRayCastSettings, RayCastBackfaces, RayCastVisibility,
};
use bevy::prelude::*;

/// The window points to pick, if `BG3_PICK` asks for any.
pub fn requested() -> Option<Vec<Vec2>> {
    let spec = std::env::var("BG3_PICK").ok()?;
    let points: Vec<Vec2> = spec
        .split(';')
        .filter_map(|p| {
            let (x, y) = p.split_once(',')?;
            Some(Vec2::new(x.trim().parse().ok()?, y.trim().parse().ok()?))
        })
        .collect();
    (!points.is_empty()).then_some(points)
}

/// Mesh usages for level geometry: picking needs the vertex data in the main world too.
pub fn mesh_usages() -> RenderAssetUsages {
    if requested().is_some() {
        RenderAssetUsages::default()
    } else {
        RenderAssetUsages::RENDER_WORLD
    }
}

#[derive(Resource)]
pub struct PickPoints(pub Vec<Vec2>);

/// Back faces count as hits: a mesh seen from behind still covers the pixel.
pub fn mark_backfaces(mut commands: Commands, meshes: Query<Entity, Added<Mesh3d>>) {
    for e in &meshes {
        commands.entity(e).insert(RayCastBackfaces);
    }
}

pub fn pick_at_shot(
    points: Res<PickPoints>,
    framing: Option<Res<crate::framing::FrameTarget>>,
    cameras: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    windows: Query<&Window>,
    names: Query<(Option<&Name>, Option<&ChildOf>)>,
    mut ray_cast: MeshRayCast,
    // (frames run, picked)
    mut state: Local<(u32, bool)>,
) {
    let (frame, done) = &mut *state;
    *frame += 1;
    let ready = match &framing {
        Some(f) => f.captured,
        None => *frame >= 120,
    };
    if *done || !ready {
        return;
    }
    let (Ok((camera, transform)), Ok(window)) = (cameras.single(), windows.single()) else {
        return;
    };
    *done = true;
    let size = window.size();
    let name_of = |mut e: Entity| -> String {
        let mut parts = Vec::new();
        while let Ok((name, parent)) = names.get(e) {
            if let Some(n) = name {
                parts.push(n.as_str().to_owned());
            }
            match parent {
                Some(p) => e = p.parent(),
                None => break,
            }
        }
        parts.reverse();
        parts.join(" / ")
    };
    for p in &points.0 {
        let Ok(ray) = camera.viewport_to_world(transform, *p * size) else {
            continue;
        };
        eprintln!("pick {p}:");
        let settings = MeshRayCastSettings::default()
            .with_visibility(RayCastVisibility::VisibleInView)
            .never_early_exit();
        for (e, hit) in ray_cast.cast_ray(ray, &settings).iter().take(6) {
            let at = crate::space::vec3(hit.point.to_array());
            let n = hit.normal;
            eprintln!(
                "  {:6.2} m  {}  at [{:.2}, {:.2}, {:.2}] normal [{:.2}, {:.2}, {:.2}] tri {:?}",
                hit.distance,
                name_of(*e),
                at[0],
                at[1],
                at[2],
                n.x,
                n.y,
                -n.z,
                hit.triangle_index
            );
        }
    }
}
