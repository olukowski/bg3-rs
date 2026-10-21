//! Level maps (`LevelMapValue(Name)` in stats expressions): per-level values, usually dice,
//! from `Public/<Module>/Levelmaps/LevelMapValues.lsx`.

use std::collections::{BTreeMap, HashMap};

use bg3_vfs::Vfs;

use crate::dice::RollDefinition;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LevelMapSeries {
    pub name: String,
    pub uuid: String,
    /// `LevelN` → value text (`1d10`, `3`).
    pub levels: BTreeMap<u32, String>,
    pub fallback: Option<String>,
    /// Set on series that scale with one class's level rather than the character level.
    pub preferred_class: Option<String>,
}

impl LevelMapSeries {
    /// The value for `level`: the exact `LevelN`, else the highest defined level below it,
    /// else `FallbackValue`. Only the exact lookup is known to be what the engine does
    /// (`eoc::GetLevelMapValueFromStatsEntity` 0x101bab6a0 was not traced in full).
    pub fn value(&self, level: i32) -> Option<&str> {
        let level = level.max(0) as u32;
        self.levels
            .get(&level)
            .or_else(|| self.levels.range(..level).next_back().map(|(_, v)| v))
            .or(self.fallback.as_ref())
            .map(String::as_str)
    }
}

/// All level map series by name; later modules replace earlier series of the same name.
#[derive(Debug, Clone, Default)]
pub struct LevelMaps {
    pub series: HashMap<String, LevelMapSeries>,
}

impl LevelMaps {
    /// Add the series of one `LevelMapValues` resource.
    pub fn add_resource(&mut self, res: &bg3_lsf::Resource) {
        let Some(root) = res.region("LevelMapValues") else {
            return;
        };
        let series_nodes = root
            .children_named("root")
            .flat_map(|r| r.children_named("LevelMapSeries"))
            .chain(root.children_named("LevelMapSeries"));
        for n in series_nodes {
            let mut s = LevelMapSeries::default();
            for a in &n.attributes {
                let v = n.attr_str(&a.name).unwrap_or_default().to_owned();
                match a.name.as_str() {
                    "Name" => s.name = v,
                    "UUID" => s.uuid = v,
                    "FallbackValue" => s.fallback = Some(v),
                    "PreferredClassUUID" => s.preferred_class = Some(v),
                    l => {
                        if let Some(n) = l.strip_prefix("Level").and_then(|n| n.parse().ok()) {
                            s.levels.insert(n, v);
                        }
                    }
                }
            }
            if !s.name.is_empty() {
                self.series.insert(s.name.clone(), s);
            }
        }
    }

    /// Load `Public/<Module>/Levelmaps/LevelMapValues.lsx` of each module, in order.
    pub fn load(vfs: &Vfs, modules: &[&str]) -> Self {
        let mut maps = Self::default();
        for m in modules {
            let path = format!("Public/{m}/Levelmaps/LevelMapValues.lsx");
            if let Ok(bytes) = vfs.read(&path)
                && let Ok(res) = bg3_lsf::parse(&bytes)
            {
                maps.add_resource(&res);
            }
        }
        maps
    }

    /// The value of series `name` at `level`, as a roll (a plain number is a roll with no
    /// dice).
    pub fn roll(&self, name: &str, level: i32) -> Option<RollDefinition> {
        let text = self.series.get(name)?.value(level)?;
        RollDefinition::parse(text)
            .or_else(|| text.trim().parse::<i32>().ok().map(RollDefinition::flat))
    }
}
