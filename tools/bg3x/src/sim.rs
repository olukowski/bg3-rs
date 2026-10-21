//! `bg3x sim-replay`: replay a libOsiris runtime log with `bg3-sim` as the engine.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use bg3_install::Install;
use bg3_pak::Package;
use bg3_sim::replay::{Mode, Options, sim_replay};

pub struct Args {
    pub trace: Option<PathBuf>,
    pub pak: String,
    pub path: String,
    pub check: bool,
    pub both: bool,
    pub levels: Vec<String>,
    pub vm_log: Option<PathBuf>,
    pub dump: Option<PathBuf>,
    pub inventory: bool,
    pub no_dialogs: bool,
    pub no_cc: bool,
}

/// Load the sim's game data from the install.
pub fn load_data(levels: &[String]) -> Result<bg3_sim::GameData> {
    let install = Install::locate()?;
    let vfs = bg3_vfs::Vfs::from_install(&install)?;
    let paks: Vec<Package> = install
        .paks()?
        .iter()
        .map(Package::open)
        .collect::<Result<_, _>>()?;
    let stats = bg3_stats::install::load(&paks, bg3_stats::install::BASE_LOAD_ORDER)?;
    let levels: Vec<&str> = levels.iter().map(String::as_str).collect();
    Ok(bg3_sim::GameData::load(&vfs, &stats, &levels)?)
}

/// The install's dialogs for the sim.
pub fn load_dialogs() -> Result<Arc<bg3_sim::DialogBank>> {
    let install = Install::locate()?;
    let vfs = Arc::new(bg3_vfs::Vfs::from_install(&install)?);
    let lib = Arc::new(bg3_dialog::library::DialogLibrary::new(vfs)?);
    Ok(Arc::new(bg3_sim::DialogBank::from_library(lib)))
}

pub fn run(a: Args) -> Result<()> {
    let trace = a
        .trace
        .unwrap_or_else(|| PathBuf::from(".cache/traces/full-nautiloid.log"));
    let log = std::fs::read(&trace).with_context(|| trace.display().to_string())?;
    let log = String::from_utf8_lossy(&log).into_owned();
    let story = crate::read_file(&a.pak, &a.path)?;
    if a.inventory {
        let story = bg3_story::Story::parse(&story)?;
        print!(
            "{}",
            bg3_sim::inventory::Inventory::from_log(&story, &log).markdown()
        );
        return Ok(());
    }
    let t = Instant::now();
    let data = load_data(&a.levels)?;
    let dialogs = if a.no_dialogs {
        None
    } else {
        Some(load_dialogs()?)
    };
    eprintln!(
        "loaded {} flags, {} tags, {} statuses, {} templates, {} global objects, levels {:?} in {:.1?}",
        data.flags.len(),
        data.tags.len(),
        data.statuses.len(),
        data.templates.len(),
        data.globals.len(),
        data.levels.keys().collect::<Vec<_>>(),
        t.elapsed()
    );
    let modes: Vec<Mode> = if a.both {
        vec![Mode::Check, Mode::Sim]
    } else if a.check {
        vec![Mode::Check]
    } else {
        vec![Mode::Sim]
    };
    let vm_log = a.vm_log;
    let no_cc = a.no_cc;
    let dump = a.dump;
    let start = a.levels.first().cloned().unwrap_or_default();
    std::thread::Builder::new()
        .stack_size(1 << 30)
        .spawn(move || -> Result<()> {
            let story = bg3_story::Story::parse(&story)?;
            for mode in modes {
                let opts = Options {
                    mode,
                    start_level: start.clone(),
                    keep_vm_log: vm_log.is_some(),
                    dialogs: dialogs.clone(),
                    character: (!no_cc).then(bg3_sim::start::trace_choice),
                };
                let t = Instant::now();
                let report = sim_replay(&story, &log, data.clone(), &opts);
                print!("{}", report.summary());
                println!("({:.1?})\n", t.elapsed());
                if let (Some(p), Some(text)) = (&vm_log, &report.vm.vm_log) {
                    std::fs::write(p, text).with_context(|| p.display().to_string())?;
                }
                if let Some(p) = &dump {
                    std::fs::write(p, report.details()).with_context(|| p.display().to_string())?;
                }
            }
            Ok(())
        })?
        .join()
        .map_err(|_| anyhow::anyhow!("replay thread panicked"))?
}
