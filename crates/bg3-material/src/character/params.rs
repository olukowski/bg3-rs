//! Named scalar/vector parameter values of one material, with per-character overrides applied.

use std::collections::HashMap;

use crate::resolve::ResolvedMaterial;

/// Scalar and vector parameters by name (scalars are 1-element vectors).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParamSet {
    values: HashMap<String, Vec<f32>>,
}

impl ParamSet {
    /// The graph defaults with the instance's enabled overrides (see [`crate::resolve`]).
    pub fn from_material(m: &ResolvedMaterial) -> Self {
        let mut values = HashMap::new();
        for (n, v) in &m.scalars {
            values.insert(n.clone(), vec![*v]);
        }
        for (n, v) in &m.vectors {
            values.insert(n.clone(), v.clone());
        }
        Self { values }
    }

    /// Set a parameter, but only one the material declares: character overrides name
    /// parameters of all their materials at once (`MelaninColor` means nothing to an eye).
    /// Returns whether it applied.
    pub fn set_declared(&mut self, name: &str, value: &[f32]) -> bool {
        match self.values.get_mut(name) {
            Some(v) if !value.is_empty() => {
                // Keep the declared arity: a Vector3 override of a Vector4 parameter keeps w.
                for (dst, src) in v.iter_mut().zip(value) {
                    *dst = *src;
                }
                true
            }
            _ => false,
        }
    }

    pub fn set(&mut self, name: &str, value: Vec<f32>) {
        self.values.insert(name.to_owned(), value);
    }

    pub fn has(&self, name: &str) -> bool {
        self.values.contains_key(name)
    }

    /// A scalar, or `default` when the material doesn't declare it.
    pub fn scalar(&self, name: &str, default: f32) -> f32 {
        self.values
            .get(name)
            .and_then(|v| v.first().copied())
            .unwrap_or(default)
    }

    /// An RGB vector, or `default`.
    pub fn rgb(&self, name: &str, default: [f32; 3]) -> [f32; 3] {
        match self.values.get(name) {
            Some(v) if v.len() >= 3 => [v[0], v[1], v[2]],
            _ => default,
        }
    }

    /// A 4-vector, or `default`.
    pub fn vec4(&self, name: &str, default: [f32; 4]) -> [f32; 4] {
        match self.values.get(name) {
            Some(v) if v.len() >= 4 => [v[0], v[1], v[2], v[3]],
            _ => default,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_only_apply_to_declared_parameters() {
        let mut p = ParamSet::default();
        p.set("MelaninAmount", vec![0.6]);
        p.set("TattooIntensity", vec![0.0; 4]);
        assert!(p.set_declared("MelaninAmount", &[0.2]));
        assert!(!p.set_declared("Eyes_IrisColour", &[1.0, 0.0, 0.0]));
        assert!(p.set_declared("TattooIntensity", &[1.0, 0.5, 0.25]));
        assert_eq!(p.scalar("MelaninAmount", 0.0), 0.2);
        assert_eq!(p.vec4("TattooIntensity", [9.0; 4]), [1.0, 0.5, 0.25, 0.0]);
        assert_eq!(p.scalar("Missing", 7.0), 7.0);
        assert!(!p.has("Eyes_IrisColour"));
    }
}
