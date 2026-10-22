// The game's distance falloff for point and spot lights, shared by Bevy's lighting (patched in
// at run time, see `lights.rs`) and the character shader. Keep this file free of imports: its
// text is appended to `bevy_pbr::render::pbr_lighting`.
//
// `gain_fill` is the light's `Gain`, negated when the light is a fill light (`FlatFalloff`),
// as the game packs it (−max(Gain, 1.19e-7) or max(Gain, 0)). Returns the factor the light's
// candela-like colour is multiplied by (no N·L, no cone):
//   (1/max(d², 1e-4) + Gain) · sat(1 − (d²/R²)²)², or (4/R² + Gain) · … for fill lights;
// zero at and beyond R.
//
// bg3: ClusteredLighting_* point/spot light loop, ls::ClusteredAssignmentStage::SetLightData
// (Core.metallib / binary, 4.1.1.7631656)

fn bg3_light_falloff(distance_square: f32, inverse_range_squared: f32, gain_fill: f32) -> f32 {
    let factor = distance_square * inverse_range_squared;
    if factor >= 1.0 {
        return 0.0;
    }
    let gain = abs(gain_fill);
    var base = 1.0 / max(distance_square, 0.0001) + gain;
    if gain_fill < 0.0 {
        base = 4.0 * inverse_range_squared + gain;
    }
    let window = max(saturate(1.0 - factor * factor), 0.0001);
    return base * window * window;
}
