//! Module (Larian "mod") load order. Content in later modules overrides earlier ones.

/// Module load order for the base game (no Honour mode), lowest priority first.
///
/// Derived from `Mods/*/meta.lsx` dependencies in the reference build: `Shared` and `SharedDev`
/// are not listed as anyone's dependency (always-loaded engine-level modules) and go first;
/// `Gustav` depends on the dice sets, `MainUI`, `ModBrowser`, `PhotoMode` and `CrossplayUI`;
/// `GustavDev` → `Gustav`; `GustavX` → `GustavDev`. Modules that ship no relevant content for a
/// given system are simply skipped by it. See `docs/formats/stats.md` for the uncertainty.
pub const BASE_LOAD_ORDER: &[&str] = &[
    "Shared",
    "SharedDev",
    "PhotoMode",
    "Gustav",
    "GustavDev",
    "GustavX",
];

/// Additional modules for Honour mode, appended after [`BASE_LOAD_ORDER`]
/// (`Honour` → `GustavDev`; `HonourX` → `GustavX`, `Honour`).
pub const HONOUR_MODULES: &[&str] = &["Honour", "HonourX"];
