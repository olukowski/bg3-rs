//! Hair: `CHAR_Hair`.

use super::kinds::UNIFORM_LEN;
use super::params::ParamSet;

/// Hair uniform layout (see `character.wesl`).
pub fn uniform(v: &mut [[f32; 4]; UNIFORM_LEN], p: &ParamSet) {
    let [r, g, b] = p.rgb("Hair_Color", [0.1, 0.05, 0.02]);
    v[0] = [r, g, b, 0.0];
}
