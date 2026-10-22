//! Decals: box-projected materials placed in a level (`Decals/_merged.lsf`, `Type=decal`),
//! sparse over their root template (`ls::DecalTemplate`). See `docs/research/decals.md`.
//!
//! A decal is a unit box (−½…½ on every axis) scaled by `Dimensions`, rotated and placed by its
//! transform. It projects along its local −Y onto whatever receives decals inside the box: the
//! texture's u runs along local +X, v along local −Z.

use std::collections::HashMap;

use bg3_lsf::Node;
use bg3_vfs::Vfs;
use bg3_vfs::modules::BASE_LOAD_ORDER;

use crate::level::read_transform;
use crate::{Result, RootTemplates, Transform, parse, str_attr};

/// Decals fade out with the horizontal distance from the camera: fully gone at this many
/// metres, the fade taking the last ~1.5 m (see [`distance_fade`]). `ls::DecalObject::PrepareDraw`
/// passes the constant as `_DecalDeferredProperties.y`.
// bg3: ls::DecalObject::PrepareDraw (4.1.1.7631656)
pub const FADE_DISTANCE: f32 = 60.0;

/// Decal fields as stored on one node; `None` = inherit (value/is-set pairs in
/// `ls::DecalTemplate::Visit`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DecalProps {
    /// `MaterialBank` resource ID.
    pub material: Option<String>,
    /// Box size (m) along local X, Y (projection axis) and Z.
    pub dimensions: Option<[f32; 3]>,
    pub tiling: Option<[f32; 2]>,
    /// `offset` (lower case in the data).
    pub offset: Option<[f32; 2]>,
    /// How much of the receiver's own normal (vs. its flat geometric normal) the decal's normal
    /// map bends: 0 flat, 1 the receiver's normal-mapped normal.
    pub normal_blending: Option<f32>,
    /// Minimum cosine between the receiver's normal and the decal's up axis; 0 = off.
    pub angle_cutoff: Option<f32>,
    /// Draw-order layer: higher layers draw over lower ones.
    pub layer: Option<u32>,
    pub opacity: Option<f32>,
}

fn f32_attr(n: &Node, name: &str) -> Option<f32> {
    n.attr(name).and_then(|v| v.as_f64()).map(|v| v as f32)
}

fn vec_attr<const N: usize>(n: &Node, name: &str) -> Option<[f32; N]> {
    let c = n.attr(name)?.components()?;
    (c.len() == N).then(|| std::array::from_fn(|i| c[i] as f32))
}

impl DecalProps {
    // bg3: ls::DecalTemplate::Visit (4.1.1.7631656)
    pub fn read(n: &Node) -> Self {
        Self {
            material: str_attr(n, "Material"),
            dimensions: vec_attr(n, "Dimensions"),
            tiling: vec_attr(n, "Tiling"),
            offset: vec_attr(n, "offset"),
            normal_blending: f32_attr(n, "NormalBlendingFactor"),
            angle_cutoff: f32_attr(n, "AngleCutoff"),
            layer: n
                .attr("Layer")
                .and_then(|v| v.as_i64())
                .and_then(|v| u32::try_from(v).ok()),
            opacity: f32_attr(n, "Opacity"),
        }
    }

    fn inherit(self, p: &Self) -> Self {
        Self {
            material: self.material.or_else(|| p.material.clone()),
            dimensions: self.dimensions.or(p.dimensions),
            tiling: self.tiling.or(p.tiling),
            offset: self.offset.or(p.offset),
            normal_blending: self.normal_blending.or(p.normal_blending),
            angle_cutoff: self.angle_cutoff.or(p.angle_cutoff),
            layer: self.layer.or(p.layer),
            opacity: self.opacity.or(p.opacity),
        }
    }
}

/// A placed decal with its fields resolved (defaults as in `ls::DecalTemplate::Visit`).
#[derive(Debug, Clone, PartialEq)]
pub struct Decal {
    pub id: String,
    pub name: String,
    pub transform: Transform,
    pub material: Option<String>,
    pub dimensions: [f32; 3],
    pub tiling: [f32; 2],
    pub offset: [f32; 2],
    pub normal_blending: f32,
    pub angle_cutoff: f32,
    pub layer: u32,
    pub opacity: f32,
}

fn rotate([x, y, z, w]: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    // v + 2w(q×v) + 2q×(q×v)
    let q = [x, y, z];
    let cross = |a: [f32; 3], b: [f32; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let t = cross(q, v).map(|c| 2.0 * c);
    let u = cross(q, t);
    std::array::from_fn(|i| v[i] + w * t[i] + u[i])
}

fn normalized(q: [f32; 4]) -> [f32; 4] {
    let len = q.iter().map(|c| c * c).sum::<f32>().sqrt();
    if len > 0.0 {
        q.map(|c| c / len)
    } else {
        [0.0, 0.0, 0.0, 1.0]
    }
}

impl Decal {
    /// World size of the box along each local axis: `Dimensions` × the transform's scale.
    /// **Assumption**: the uniform `Scale` multiplies the dimensions (every Nautiloid decal has
    /// scale 1, so it is not exercised).
    pub fn size(&self) -> [f32; 3] {
        self.dimensions.map(|d| d * self.transform.scale)
    }

    /// The box's local axes (X, Y = up, Z) in world space, unit length.
    pub fn axes(&self) -> [[f32; 3]; 3] {
        let q = normalized(self.transform.rotation);
        [
            rotate(q, [1.0, 0.0, 0.0]),
            rotate(q, [0.0, 1.0, 0.0]),
            rotate(q, [0.0, 0.0, 1.0]),
        ]
    }

    /// World → unit-box coordinates (inside when every component is within ±½), as a
    /// column-major 4×4 matrix (game space).
    pub fn local_from_world(&self) -> [[f32; 4]; 4] {
        let axes = self.axes();
        let size = self.size();
        let p = self.transform.position;
        // Row i of the 3×3 part: axis i / size i; translation: −row · position.
        let row = |i: usize| axes[i].map(|a| a / size[i].max(1e-6));
        let rows = [row(0), row(1), row(2)];
        let t = rows.map(|r| -(r[0] * p[0] + r[1] * p[1] + r[2] * p[2]));
        [
            [rows[0][0], rows[1][0], rows[2][0], 0.0],
            [rows[0][1], rows[1][1], rows[2][1], 0.0],
            [rows[0][2], rows[1][2], rows[2][2], 0.0],
            [t[0], t[1], t[2], 1.0],
        ]
    }

    /// Unit-box coordinates of a world point.
    pub fn to_local(&self, p: [f32; 3]) -> [f32; 3] {
        let m = self.local_from_world();
        std::array::from_fn(|i| m[0][i] * p[0] + m[1][i] * p[1] + m[2][i] * p[2] + m[3][i])
    }

    /// Texture coordinates of a unit-box point: u along +X, v along −Z, then tiling and offset.
    // bg3: Engine_Materials_Engine_Default_Decal_Deferred_ST_DEF_fragmentMain (4.1.1.7631656)
    pub fn uv(&self, local: [f32; 3]) -> [f32; 2] {
        [
            (local[0] + 0.5) * self.tiling[0] + self.offset[0],
            (0.5 - local[2]) * self.tiling[1] + self.offset[1],
        ]
    }

    /// World-space bounding box (min, max) of the decal's box.
    pub fn world_bounds(&self) -> ([f32; 3], [f32; 3]) {
        let axes = self.axes();
        let size = self.size();
        let p = self.transform.position;
        let mut min = p;
        let mut max = p;
        for corner in 0..8 {
            let s = [corner & 1, (corner >> 1) & 1, (corner >> 2) & 1]
                .map(|b| if b == 1 { 0.5 } else { -0.5 });
            let c: [f32; 3] = std::array::from_fn(|k| {
                p[k] + (0..3).map(|i| axes[i][k] * size[i] * s[i]).sum::<f32>()
            });
            for k in 0..3 {
                min[k] = min[k].min(c[k]);
                max[k] = max[k].max(c[k]);
            }
        }
        (min, max)
    }

    /// Half the diagonal of the world bounds: the size key of [`sort_draw_order`].
    pub fn bound_radius(&self) -> f32 {
        let (min, max) = self.world_bounds();
        0.5 * (0..3)
            .map(|k| (max[k] - min[k]).powi(2))
            .sum::<f32>()
            .sqrt()
    }
}

/// Fade with the camera's horizontal distance `d` (m): 1 near, 0 from [`FADE_DISTANCE`] on, a
/// smoothstep of `(F − d) / (F + 1.5 − d)` in between.
// bg3: Engine_Materials_Engine_Default_Decal_Deferred_ST_DEF_fragmentMain (4.1.1.7631656)
pub fn distance_fade(d: f32) -> f32 {
    let f = FADE_DISTANCE;
    if d >= f {
        return 0.0;
    }
    let t = ((f - d) / (f + 1.5 - d)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Order decals as the renderer draws them: by `Layer` (lower first, so higher layers end up on
/// top), then larger world bounds first (small decals over big ones); ties keep their order.
// bg3: ls::CullingHelpers::SortDecalObjects (4.1.1.7631656)
pub fn sort_draw_order(decals: &mut [Decal]) {
    decals.sort_by(|a, b| {
        a.layer.cmp(&b.layer).then_with(|| {
            let (ra, rb) = (a.bound_radius(), b.bound_radius());
            if (ra - rb).abs() <= f32::EPSILON {
                std::cmp::Ordering::Equal
            } else {
                rb.total_cmp(&ra)
            }
        })
    });
}

fn template_props(templates: &RootTemplates, id: &str) -> DecalProps {
    let mut acc = DecalProps::default();
    let mut current = templates.get(id);
    for _ in 0..32 {
        let Some(t) = current else { break };
        if let Some(p) = &t.decal {
            acc = acc.inherit(p);
        }
        current = t.parent.as_deref().and_then(|p| templates.get(p));
    }
    acc
}

/// Read the decals of one `Decals/_merged.lsf`.
pub fn read_decals(res: &bg3_lsf::Resource, templates: &RootTemplates) -> Vec<Decal> {
    res.region("Templates")
        .into_iter()
        .flat_map(|n| n.children_named("GameObjects"))
        .filter(|n| str_attr(n, "Type").as_deref() == Some("decal"))
        .filter_map(|n| {
            let inherited = str_attr(n, "TemplateName")
                .map(|t| template_props(templates, &t))
                .unwrap_or_default();
            let p = DecalProps::read(n).inherit(&inherited);
            Some(Decal {
                id: str_attr(n, "MapKey")?,
                name: str_attr(n, "Name").unwrap_or_default(),
                transform: n.child("Transform").map(read_transform).unwrap_or_default(),
                material: p.material,
                dimensions: p.dimensions.unwrap_or([1.0; 3]),
                tiling: p.tiling.unwrap_or([1.0; 2]),
                offset: p.offset.unwrap_or([0.0; 2]),
                normal_blending: p.normal_blending.unwrap_or(0.0),
                angle_cutoff: p.angle_cutoff.unwrap_or(0.0),
                layer: p.layer.unwrap_or(0),
                opacity: p.opacity.unwrap_or(1.0),
            })
        })
        .collect()
}

/// All decals of a level, merged across modules by `MapKey`, in file order.
pub fn load_decals(vfs: &Vfs, level: &str, templates: &RootTemplates) -> Result<Vec<Decal>> {
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<Decal> = Vec::new();
    for module in BASE_LOAD_ORDER {
        let path = format!("Mods/{module}/Levels/{level}/Decals/_merged.lsf");
        if !vfs.exists(&path) {
            continue;
        }
        for d in read_decals(&parse(vfs, &path)?, templates) {
            match index.get(&d.id) {
                Some(&i) => out[i] = d,
                None => {
                    index.insert(d.id.clone(), out.len());
                    out.push(d);
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decal(rotation: [f32; 4], dimensions: [f32; 3]) -> Decal {
        Decal {
            id: "d".into(),
            name: "d".into(),
            transform: Transform {
                position: [10.0, 2.0, -5.0],
                rotation,
                scale: 1.0,
            },
            material: None,
            dimensions,
            tiling: [1.0, 1.0],
            offset: [0.0, 0.0],
            normal_blending: 0.0,
            angle_cutoff: 0.0,
            layer: 0,
            opacity: 1.0,
        }
    }

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
    }

    #[test]
    fn unit_box_spans_the_dimensions() {
        let d = decal([0.0, 0.0, 0.0, 1.0], [4.0, 2.0, 8.0]);
        assert!(close(d.to_local([10.0, 2.0, -5.0]), [0.0; 3]));
        assert!(close(d.to_local([12.0, 3.0, -1.0]), [0.5, 0.5, 0.5]));
        assert!(close(d.to_local([8.0, 1.0, -9.0]), [-0.5, -0.5, -0.5]));
    }

    #[test]
    fn rotation_turns_the_box() {
        // 90° about Y: local X → world −Z, local Z → world +X.
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let d = decal([0.0, h, 0.0, h], [4.0, 2.0, 8.0]);
        let [x, y, z] = d.axes();
        assert!(close(x, [0.0, 0.0, -1.0]));
        assert!(close(y, [0.0, 1.0, 0.0]));
        assert!(close(z, [1.0, 0.0, 0.0]));
        // 2 m along world −Z is half the 4 m X extent.
        assert!(close(d.to_local([10.0, 2.0, -7.0]), [0.5, 0.0, 0.0]));
        assert!(close(d.to_local([14.0, 2.0, -5.0]), [0.0, 0.0, 0.5]));
        let (min, max) = d.world_bounds();
        assert!(close(min, [6.0, 1.0, -7.0]) && close(max, [14.0, 3.0, -3.0]));
    }

    #[test]
    fn uv_runs_along_x_and_minus_z_with_tiling() {
        let mut d = decal([0.0, 0.0, 0.0, 1.0], [1.0; 3]);
        assert_eq!(d.uv([-0.5, 0.0, 0.5]), [0.0, 0.0]);
        assert_eq!(d.uv([0.5, 0.0, -0.5]), [1.0, 1.0]);
        d.tiling = [0.35, 0.62];
        d.offset = [0.65, 0.0];
        let [u, v] = d.uv([0.5, 0.0, -0.5]);
        assert!((u - 1.0).abs() < 1e-6 && (v - 0.62).abs() < 1e-6);
    }

    #[test]
    fn distance_fade_ends_at_sixty_metres() {
        assert!(distance_fade(0.0) > 0.99);
        assert!(distance_fade(40.0) > 0.95);
        assert!((distance_fade(58.5) - 0.5).abs() < 1e-6);
        assert_eq!(distance_fade(60.0), 0.0);
        assert_eq!(distance_fade(100.0), 0.0);
    }

    #[test]
    fn draw_order_is_layer_then_larger_first() {
        let small = Decal {
            id: "small".into(),
            ..decal([0.0, 0.0, 0.0, 1.0], [1.0; 3])
        };
        let big = Decal {
            id: "big".into(),
            ..decal([0.0, 0.0, 0.0, 1.0], [8.0; 3])
        };
        let top = Decal {
            id: "top".into(),
            layer: 1,
            ..decal([0.0, 0.0, 0.0, 1.0], [10.0; 3])
        };
        let mut list = vec![top, small, big];
        sort_draw_order(&mut list);
        let ids: Vec<_> = list.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, ["big", "small", "top"]);
    }

    #[test]
    fn instance_fields_override_the_template_chain() {
        let base = DecalProps {
            material: Some("m".into()),
            dimensions: Some([4.0, 4.0, 8.0]),
            layer: Some(5),
            ..Default::default()
        };
        let own = DecalProps {
            dimensions: Some([4.0, 1.0, 8.0]),
            layer: Some(1),
            ..Default::default()
        };
        let p = own.inherit(&base);
        assert_eq!(p.material.as_deref(), Some("m"));
        assert_eq!(p.dimensions, Some([4.0, 1.0, 8.0]));
        assert_eq!(p.layer, Some(1));
        assert_eq!(p.opacity, None);
    }

    #[test]
    #[ignore = "needs BG3 install"]
    fn nautiloid_decals_resolve() {
        let Ok(install) = bg3_install::Install::locate() else {
            eprintln!("skipping: no BG3 install");
            return;
        };
        let vfs = Vfs::from_install(&install).unwrap();
        let templates = RootTemplates::load(&vfs).unwrap();
        let banks = crate::ResourceBanks::load(&vfs).unwrap();
        let decals = load_decals(&vfs, "TUT_Avernus_C", &templates).unwrap();
        eprintln!("{} decals", decals.len());
        assert!(decals.len() > 50);
        for d in &decals {
            let m = d.material.as_deref().expect("material");
            assert!(banks.materials.contains_key(m), "{}: {m}", d.name);
            assert!(d.dimensions.iter().all(|&x| x > 0.0), "{}", d.name);
        }
        // Instance dimensions override the template's (Destruction_B: template 4×4×8).
        assert!(decals.iter().any(|d| {
            d.name
                .starts_with("DCL_MF_Nautiloid_Interior_Destruction_B")
                && d.dimensions == [4.0, 1.0, 8.0]
        }));
    }
}
