//! Conversion from BG3's left-handed Y-up space to Bevy's right-handed Y-up space.
//!
//! Evidence (see `docs/research/coordinate-space.md`): in the human body mesh the character faces
//! −Z (toes ahead of ankles) with `*_L` bones at +X — only consistent with a left-handed system.
//! We mirror the Z axis. Everything that crosses from BG3 data into Bevy goes through here.

use bevy::prelude::*;

/// Mirror a BG3 position or direction into Bevy space.
pub fn vec3([x, y, z]: [f32; 3]) -> [f32; 3] {
    [x, y, -z]
}

/// Mirror a BG3 rotation (quaternion `x y z w`): reflecting across the XY plane negates the
/// rotation components about X and Y.
pub fn quat([x, y, z, w]: [f32; 4]) -> Quat {
    Quat::from_xyzw(-x, -y, z, w).normalize()
}

pub fn transform(t: &bg3_world::Transform) -> Transform {
    Transform {
        translation: Vec3::from_array(vec3(t.position)),
        rotation: quat(t.rotation),
        scale: Vec3::splat(t.scale),
    }
}

/// Tangent with handedness: mirroring flips the bitangent, so `w` changes sign.
pub fn tangent([x, y, z, w]: [f32; 4]) -> [f32; 4] {
    [x, y, -z, -w]
}

/// A mirror flips triangle winding; swap two indices of every triangle to restore it.
pub fn flip_winding(indices: &mut [u32]) {
    for tri in indices.as_chunks_mut::<3>().0 {
        tri.swap(1, 2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirrored_rotation_matches_mirrored_vectors() {
        // Rotating then mirroring must equal mirroring then rotating with the mirrored quaternion.
        let q = [0.1f32, 0.7, -0.3, 0.64];
        let qn = Quat::from_xyzw(q[0], q[1], q[2], q[3]).normalize();
        let v = Vec3::new(0.3, -1.2, 2.5);
        let a = Vec3::from_array(vec3((qn * v).to_array()));
        let b = quat(qn.to_array()) * Vec3::from_array(vec3(v.to_array()));
        assert!((a - b).length() < 1e-5, "{a} vs {b}");
    }
}
