//! Per-character material parameter overrides: `MaterialOverrides` and `MaterialPresetBank`.
//!
//! A character visual (and a worn item's visual set) carries `MaterialOverrides`: presets by
//! category (`02Skin Properties`, `03Hair`, `06Eyes`, `02 Colour`, …) pointing at
//! `MaterialPresetBank` resources, plus its own parameter list. Each entry names a material
//! parameter; it applies to every material of the character that declares a parameter of that
//! name (the category is not a filter: skin presets set the eyelash card's
//! `LashesAlphaIntensity`, hair presets set the skin's `Eyebrow_Color`). Only `Enabled`
//! entries count. See `docs/research/character-shaders.md`.
//!
//! Layering, lowest first (**assumption** where marked, inferred from the data):
//! 1. graph defaults, 2. material instance (`Enabled` values) — both in `bg3-material`;
//! 3. presets without `ForcePresetValues` (**assumption**), 4. the visual's own parameters,
//! 5. presets with `ForcePresetValues` (**assumption**: "force" lets a preset beat 4).
//!
//! bg3: ecl::visual::ApplyMaterialPreset (4.1.1.7631656) — name only; not read.

use std::collections::HashMap;

use crate::characters::MaterialOverrides;

/// One parameter assignment, applied in order (later wins).
#[derive(Debug, Clone, PartialEq)]
pub struct ParamAssignment {
    pub parameter: String,
    pub value: Vec<f32>,
}

/// Presets can reference further presets (`ColorPreset`, nested `MaterialPresets`); this bounds
/// the recursion against cycles.
const MAX_PRESET_DEPTH: usize = 4;

/// The enabled parameter assignments of `ov`, in application order.
pub fn assignments(
    ov: &MaterialOverrides,
    presets: &HashMap<String, MaterialOverrides>,
) -> Vec<ParamAssignment> {
    let mut out = Vec::new();
    collect(ov, presets, 0, &mut out);
    out
}

fn collect(
    ov: &MaterialOverrides,
    presets: &HashMap<String, MaterialOverrides>,
    depth: usize,
    out: &mut Vec<ParamAssignment>,
) {
    let preset_refs = || {
        ov.presets
            .iter()
            .map(|p| (p.preset.as_str(), p.force))
            .chain(ov.color_preset.as_deref().map(|p| (p, true)))
            .filter(|(id, _)| !crate::characters::is_null_guid(id))
    };
    let apply_presets = |force: bool, out: &mut Vec<ParamAssignment>| {
        if depth >= MAX_PRESET_DEPTH {
            return;
        }
        for (id, f) in preset_refs() {
            if f == force
                && let Some(p) = presets.get(id)
            {
                collect(p, presets, depth + 1, out);
            }
        }
    };
    apply_presets(false, out);
    out.extend(
        ov.params
            .iter()
            .filter(|p| p.enabled && !p.value.is_empty())
            .map(|p| ParamAssignment {
                parameter: p.parameter.clone(),
                value: p.value.clone(),
            }),
    );
    apply_presets(true, out);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::characters::{MaterialPresetRef, ParamOverride};

    fn param(name: &str, v: f32, enabled: bool) -> ParamOverride {
        ParamOverride {
            parameter: name.into(),
            value: vec![v],
            enabled,
        }
    }

    fn preset_ref(id: &str, force: bool) -> MaterialPresetRef {
        MaterialPresetRef {
            group: "g".into(),
            preset: id.into(),
            force,
        }
    }

    #[test]
    fn layering_order_and_enabled_filter() {
        let mut presets = HashMap::new();
        presets.insert(
            "weak".to_owned(),
            MaterialOverrides {
                params: vec![param("A", 1.0, true), param("B", 1.0, true)],
                ..Default::default()
            },
        );
        presets.insert(
            "strong".to_owned(),
            MaterialOverrides {
                params: vec![param("B", 3.0, true), param("C", 3.0, false)],
                ..Default::default()
            },
        );
        let ov = MaterialOverrides {
            presets: vec![preset_ref("strong", true), preset_ref("weak", false)],
            params: vec![
                param("A", 2.0, true),
                param("B", 2.0, true),
                param("C", 2.0, false),
            ],
            ..Default::default()
        };
        let a = assignments(&ov, &presets);
        let last = |n: &str| {
            a.iter()
                .rev()
                .find(|x| x.parameter == n)
                .map(|x| x.value[0])
        };
        assert_eq!(last("A"), Some(2.0), "own params beat unforced presets");
        assert_eq!(last("B"), Some(3.0), "forced presets beat own params");
        assert_eq!(last("C"), None, "disabled entries are ignored");
    }

    #[test]
    fn preset_cycles_terminate() {
        let mut presets = HashMap::new();
        presets.insert(
            "loop".to_owned(),
            MaterialOverrides {
                presets: vec![preset_ref("loop", true)],
                params: vec![param("A", 1.0, true)],
                ..Default::default()
            },
        );
        let ov = MaterialOverrides {
            presets: vec![preset_ref("loop", true)],
            ..Default::default()
        };
        assert_eq!(assignments(&ov, &presets).len(), MAX_PRESET_DEPTH);
    }
}
