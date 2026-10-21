//! Loading the condition helper scripts from the install.

use bg3_vfs::Vfs;

use crate::condition::{ConditionEngine, ConditionEvalError};
use crate::context::RulesContext;
use crate::dice::DiceRng;

/// The helper scripts of `modules` (`Mods/<Module>/Scripts/thoth/helpers/*.khn`), in module
/// order and then by path: `(path, source)`.
// bg3: ls::thoth::Machine::CreateEnvironment (0x1020cd834) loads every helpers script of each
// module into the condition environment (4.1.1.7631656).
pub fn helper_scripts(vfs: &Vfs, modules: &[&str]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for m in modules {
        let mut files: Vec<_> = vfs
            .files_under(&format!("Mods/{m}/Scripts/thoth/helpers/"))
            .filter(|f| f.entry.name.to_ascii_lowercase().ends_with(".khn"))
            .collect();
        files.sort_by(|a, b| a.entry.name.cmp(&b.entry.name));
        for f in files {
            if let Ok(bytes) = f.read() {
                out.push((
                    f.entry.name.clone(),
                    String::from_utf8_lossy(&bytes).into_owned(),
                ));
            }
        }
    }
    out
}

/// Load the helper scripts of `modules` into `engine`. Returns how many were loaded.
pub fn load_helpers(
    engine: &mut ConditionEngine,
    vfs: &Vfs,
    modules: &[&str],
    ctx: &mut dyn RulesContext,
    rng: &mut dyn DiceRng,
) -> Result<usize, ConditionEvalError> {
    let scripts = helper_scripts(vfs, modules);
    for (path, src) in &scripts {
        engine.load_helpers(path, src, ctx, rng)?;
    }
    Ok(scripts.len())
}
