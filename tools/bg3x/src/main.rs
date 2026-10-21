//! `bg3x` — inspect a Baldur's Gate 3 install. Output stays local; see ADR 0001.

mod dialog;
mod materials;
mod nav;
mod rules;
mod sim;

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use bg3_install::Install;
use bg3_pak::Package;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(about, version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show the located install (layout, root, version) and its paks.
    Info {
        /// List every location probed for the install and whether it is valid.
        #[arg(long)]
        candidates: bool,
    },
    /// List files in a pak. PAK is a path or a pak name in the install (e.g. `Gustav`).
    List {
        pak: String,
        /// Only list entries whose path contains this substring.
        #[arg(short, long)]
        filter: Option<String>,
    },
    /// Extract files from a pak into a directory (default: ./extracted/<pak>).
    Extract {
        pak: String,
        #[arg(short, long)]
        filter: Option<String>,
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Write a single file from a pak to stdout.
    Cat { pak: String, path: String },
    /// Dump an LSF/LSFX/LSX/LSJ resource as LSX. PATH is looked up across all paks if PAK is `-`.
    Lsx { pak: String, path: String },
    /// Look up a localization handle (`h…` or `h…;version`) in a language pak.
    Loca {
        handle: String,
        /// Language pak name in the install.
        #[arg(short, long, default_value = "English")]
        language: String,
    },
    /// Print a stats object with `using` inheritance resolved.
    Stat {
        name: String,
        /// Also load the Honour-mode modules (Honour, HonourX).
        #[arg(long)]
        honour: bool,
        /// Print only the object's own `data` lines instead of the effective fields.
        #[arg(long)]
        own: bool,
    },
    /// Find resources (LSF/LSX/LSJ) under PREFIX with an attribute whose value equals VALUE.
    Find {
        value: String,
        #[arg(short, long, default_value = "")]
        prefix: String,
    },
    /// Dump a compiled Osiris story as pseudo-Osiris source (full dump, or one goal).
    Story {
        /// Pak name or path containing the story (`-` searches all paks).
        #[arg(default_value = "Gustav")]
        pak: String,
        /// Story path inside the pak.
        #[arg(default_value = "Mods/GustavDev/Story/story.div.osi")]
        path: String,
        /// Only dump this goal (exact name).
        #[arg(short, long)]
        goal: Option<String>,
        /// Only print the summary.
        #[arg(short, long)]
        summary: bool,
    },
    /// Summarise a GR2 model file (meshes, skeletons, models). `--tree` dumps the raw object tree.
    Gr2 {
        /// Pak name or path (`-` searches all paks).
        pak: String,
        path: String,
        #[arg(long)]
        tree: bool,
        /// Print the mean position of the vertices dominated by each bone binding.
        #[arg(long)]
        bones: bool,
    },
    /// Survey the materials a level's meshes use: shader graphs, texture slots, DDS formats.
    Materials {
        #[arg(default_value = "TUT_Avernus_C")]
        level: String,
        /// Print every material with its resolved parameters.
        #[arg(short, long)]
        verbose: bool,
    },
    /// Replay a libOsiris runtime log (see docs/research/osiris-runtime-log.md) through the VM
    /// and compare: prints match rates and the first divergence.
    OsirisReplay {
        /// The log (default: .cache/traces/full-nautiloid.log, else snapshot-early.log).
        trace: Option<PathBuf>,
        /// Pak with the story the game ran.
        #[arg(long, default_value = "GustavX")]
        pak: String,
        #[arg(long, default_value = "Mods/GustavX/Story/story.div.osi")]
        path: String,
        /// Also write the VM's own log, in libOsiris' format, to this file.
        #[arg(long)]
        vm_log: Option<PathBuf>,
        /// Print how many records of each kind the log contains.
        #[arg(long)]
        kinds: bool,
    },
    /// Print a compiled dialog (`Story/DialogsBinary`) as a tree with its text resolved from
    /// loca, or walk it interactively. NAME is the file stem (e.g. `TUT_Start_Brinepool`) or an
    /// Osiris DIALOGRESOURCE (`Name_<uuid>`). Output is from your install; don't commit it.
    Dialog {
        name: String,
        /// Walk the dialog with the runner: pick choices by number, flags kept in memory.
        #[arg(long)]
        walk: bool,
        /// List dialog resources whose name contains NAME instead.
        #[arg(long)]
        list: bool,
        /// Localization folder to read text from.
        #[arg(short, long, default_value = "English")]
        language: String,
        /// With --walk: choices to take, 1-based (e.g. `2,1,1`); then interactive.
        #[arg(long, value_delimiter = ',')]
        choices: Vec<usize>,
        /// With --walk: roll outcomes, `s`/`f` (e.g. `s,f`); then interactive.
        #[arg(long, value_delimiter = ',')]
        rolls: Vec<String>,
        /// With --walk: print the Osiris events the dialog raises.
        #[arg(long)]
        events: bool,
        /// With --walk: play the dialog on bg3-sim's world (flags, tags, character stats from
        /// the install; rolls through the rules engine) instead of an empty memory world.
        #[arg(long)]
        sim: bool,
        /// With --sim: the speakers, by slot (object names like `S_Player_ShadowHeart` or
        /// UUIDs); default: the first object of each slot's speaker list that exists.
        #[arg(long, value_delimiter = ',')]
        speakers: Vec<String>,
        /// With --sim: seed of the dice.
        #[arg(long, default_value_t = 0)]
        seed: u64,
    },
    /// Replay a libOsiris runtime log with bg3-sim answering engine queries and raising engine
    /// events (see docs/research/osiris-engine-api.md); prints sim vs fallback rates.
    SimReplay {
        /// The log (default: .cache/traces/full-nautiloid.log).
        trace: Option<PathBuf>,
        #[arg(long, default_value = "GustavX")]
        pak: String,
        #[arg(long, default_value = "Mods/GustavX/Story/story.div.osi")]
        path: String,
        /// Drive the story with the logged answers and only grade the sim's.
        #[arg(long)]
        check: bool,
        /// Run both modes (check, then sim).
        #[arg(long)]
        both: bool,
        /// Levels whose objects to load; the first is loaded at the start (default: the
        /// module's character-creation and startup levels).
        #[arg(long, value_delimiter = ',')]
        levels: Vec<String>,
        /// Also write the VM's own log, in libOsiris' format, to this file.
        #[arg(long)]
        vm_log: Option<PathBuf>,
        /// Write every disagreement / missing / extra event to this file (`.cache/` only).
        #[arg(long)]
        dump: Option<PathBuf>,
        /// Print the inventory of engine calls/queries/events in the log (Markdown) instead.
        #[arg(long)]
        inventory: bool,
        /// Feed dialog events from the log instead of running the dialogs on the sim.
        #[arg(long)]
        no_dialogs: bool,
    },
    /// Stats expression languages: evaluate an expression, survey the install, cast a spell.
    Rules {
        #[command(subcommand)]
        cmd: rules::RulesCmd,
    },
    /// A level's navigation data: AI grid summary, tile probes and path finding (BG3 world
    /// coordinates, e.g. `bg3x nav TUT_Avernus_C --path=-60,17.8,-389 --path=-20,17.6,-400`).
    Nav {
        #[arg(default_value = "TUT_Avernus_C")]
        level: String,
        /// Path end points `x,y,z`, given in pairs (start, goal, start, goal, …).
        #[arg(long, value_parser = nav::parse_vec3, allow_hyphen_values = true)]
        path: Vec<[f32; 3]>,
        /// Print the tiles of the column containing `x,y,z`.
        #[arg(long, value_parser = nav::parse_vec3, allow_hyphen_values = true)]
        at: Option<[f32; 3]>,
        /// Agent bound radius in metres (0.499 = humanoid).
        #[arg(long, default_value_t = 0.499)]
        radius: f32,
        /// List the subgrids.
        #[arg(long)]
        subgrids: bool,
        /// List connected walkable regions.
        #[arg(long)]
        regions: bool,
    },
    /// Decompress every entry of every pak (or one pak) and check sizes.
    Verify { pak: Option<String> },
    /// Report archive paths that occur in more than one pak, and whether their contents differ.
    Dups,
}

/// Let `bg3x … | head` end quietly instead of panicking on a closed stdout: restore the default
/// SIGPIPE action, which the Rust runtime sets to "ignore".
fn restore_sigpipe() {
    #[cfg(unix)]
    // SAFETY: called once at startup before any threads exist; resetting a signal disposition to
    // SIG_DFL has no memory-safety implications.
    #[allow(unsafe_code)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

fn main() -> Result<()> {
    restore_sigpipe();
    match Cli::parse().command {
        Command::Info { candidates } => info(candidates),
        Command::List { pak, filter } => {
            let pak = open(&pak)?;
            for e in matching(&pak, filter.as_deref()) {
                println!("{:>12}  {:?}  {}", e.size(), e.compression()?, e.name);
            }
            Ok(())
        }
        Command::Extract { pak, filter, out } => {
            let pak = open(&pak)?;
            let stem = pak
                .path()
                .file_stem()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let out = out.unwrap_or_else(|| PathBuf::from("extracted").join(stem));
            let mut n = 0;
            for e in matching(&pak, filter.as_deref()) {
                let dest = out.join(safe_relative(&e.name)?);
                std::fs::create_dir_all(dest.parent().unwrap())?;
                std::fs::write(&dest, pak.read(e)?).with_context(|| dest.display().to_string())?;
                n += 1;
            }
            eprintln!("extracted {n} files to {}", out.display());
            Ok(())
        }
        Command::Cat { pak, path } => {
            use std::io::Write;
            let pak = open(&pak)?;
            let entry = pak
                .find(&path)
                .with_context(|| format!("{path} not found"))?;
            std::io::stdout().write_all(&pak.read(entry)?)?;
            Ok(())
        }
        Command::Verify { pak } => verify(pak),
        Command::Dialog {
            name,
            walk,
            list,
            language,
            choices,
            rolls,
            events,
            sim,
            speakers,
            seed,
        } => dialog::run(dialog::Options {
            name,
            walk,
            list,
            language,
            choices,
            rolls: rolls.iter().map(|r| r.starts_with('s')).collect(),
            events,
            sim,
            speakers,
            seed,
        }),
        Command::Dups => dups(),
        Command::Gr2 {
            pak,
            path,
            tree,
            bones,
        } => gr2(&read_file(&pak, &path)?, tree, bones),
        Command::Story {
            pak,
            path,
            goal,
            summary,
        } => story(&pak, &path, goal.as_deref(), summary),
        Command::Find { value, prefix } => find(&value, &prefix),
        Command::OsirisReplay {
            trace,
            pak,
            path,
            vm_log,
            kinds,
        } => osiris_replay(trace, &pak, &path, vm_log, kinds),
        Command::SimReplay {
            trace,
            pak,
            path,
            check,
            both,
            levels,
            vm_log,
            dump,
            inventory,
            no_dialogs,
        } => sim::run(sim::Args {
            trace,
            pak,
            path,
            check,
            both,
            levels,
            vm_log,
            dump,
            inventory,
            no_dialogs,
        }),
        Command::Materials { level, verbose } => materials::survey(&level, verbose),
        Command::Loca { handle, language } => loca(&handle, &language),
        Command::Stat { name, honour, own } => stat(&name, honour, own),
        Command::Rules { cmd } => rules::run(cmd),
        Command::Nav {
            level,
            path,
            at,
            radius,
            subgrids,
            regions,
        } => nav::run(nav::Options {
            level,
            path,
            at,
            radius,
            subgrids,
            regions,
        }),
        Command::Lsx { pak, path } => {
            use std::io::Write;
            let data = read_file(&pak, &path)?;
            let res = bg3_lsf::parse(&data).with_context(|| path.clone())?;
            std::io::stdout().write_all(bg3_lsf::to_lsx(&res).as_bytes())?;
            Ok(())
        }
    }
}

fn info(show_candidates: bool) -> Result<()> {
    if show_candidates {
        print_candidates();
    }
    let install = match Install::locate() {
        Ok(install) => install,
        Err(e) => {
            if !show_candidates && matches!(e, bg3_install::Error::NotFound(_)) {
                eprintln!("(run `bg3x info --candidates` for details)");
            }
            return Err(e.into());
        }
    };
    println!("layout:  {}", install.layout);
    println!("root:    {}", install.root.display());
    let note = if install.is_reference_version() {
        String::new()
    } else {
        format!(
            " (not the reference build {})",
            bg3_install::REFERENCE_VERSION
        )
    };
    println!("version: {}{note}", install.version);
    println!("binary:  {}", install.binary().display());
    match install.osiris_library() {
        Some(p) => println!("osiris:  {}", p.display()),
        None => println!("osiris:  (not found)"),
    }
    for w in &install.warnings {
        eprintln!("warning: {w}");
    }
    for path in install.paks()? {
        let pak = Package::open(&path)?;
        let h = pak.header();
        println!(
            "  {:<28} files={:<7} parts={:<3} flags={:#04x} priority={}",
            path.file_name().unwrap().to_string_lossy(),
            pak.entries().len(),
            h.num_parts,
            h.flags,
            h.priority
        );
    }
    Ok(())
}

/// Print the env overrides and every default location, with whether each holds a valid install.
fn print_candidates() {
    for var in [bg3_install::ENV_INSTALL, bg3_install::ENV_APP] {
        match std::env::var_os(var) {
            Some(v) => println!("${var} = {} (overrides the search)", v.to_string_lossy()),
            None => println!("${var} not set"),
        }
    }
    println!("default locations, in search order:");
    for c in Install::candidates() {
        let status = match Install::open(&c.path) {
            Ok(i) => format!("OK: {} {}", i.layout, i.version),
            Err(bg3_install::Error::Invalid(_, why)) => why,
            Err(e) => e.to_string(),
        };
        println!("  {}\n    [{}] {status}", c.path.display(), c.reason);
    }
    println!();
}

fn verify(only: Option<String>) -> Result<()> {
    let paks = match only {
        Some(name) => vec![resolve(&name)?],
        None => Install::locate()?.paks()?,
    };
    let mut failures = 0;
    for path in paks {
        let pak = Package::open(&path)?;
        let (mut files, mut bytes) = (0u64, 0u64);
        for e in pak.entries() {
            match pak.read(e) {
                Ok(data) => {
                    files += 1;
                    bytes += data.len() as u64;
                }
                Err(err) => {
                    failures += 1;
                    eprintln!("FAIL {}: {err}", path.display());
                }
            }
        }
        println!(
            "{:<28} {files:>7} files {:>10.1} MiB",
            path.file_name().unwrap().to_string_lossy(),
            bytes as f64 / 1048576.0
        );
    }
    if failures > 0 {
        bail!("{failures} entries failed");
    }
    Ok(())
}

fn dups() -> Result<()> {
    use std::collections::BTreeMap;
    use std::hash::{DefaultHasher, Hash, Hasher};

    let paks: Vec<Package> = bg3_vfs::open_paks(&Install::locate()?)?;
    let mut by_name: BTreeMap<&str, Vec<(usize, &bg3_pak::Entry)>> = BTreeMap::new();
    for (i, pak) in paks.iter().enumerate() {
        for e in pak.entries() {
            by_name.entry(&e.name).or_default().push((i, e));
        }
    }
    let (mut same, mut differ) = (0, 0);
    for (name, hits) in by_name.iter().filter(|(_, hits)| hits.len() > 1) {
        let mut hashes = Vec::new();
        for &(i, e) in hits {
            let mut h = DefaultHasher::new();
            paks[i].read(e)?.hash(&mut h);
            hashes.push(h.finish());
        }
        let identical = hashes.windows(2).all(|w| w[0] == w[1]);
        if identical {
            same += 1;
        } else {
            differ += 1;
        }
        let owners: Vec<_> = hits
            .iter()
            .map(|&(i, _)| paks[i].path().file_stem().unwrap().to_string_lossy())
            .collect();
        println!(
            "{}\t{}\t{name}",
            if identical { "same" } else { "DIFF" },
            owners.join(",")
        );
    }
    eprintln!("{same} identical duplicates, {differ} with differing contents");
    Ok(())
}

/// Accept either a filesystem path or a pak stem in the install (`Gustav` → `.../Data/Gustav.pak`).
fn resolve(name: &str) -> Result<PathBuf> {
    let path = PathBuf::from(name);
    if path.exists() {
        return Ok(path);
    }
    let want = name.trim_end_matches(".pak");
    Install::locate()?
        .paks()?
        .into_iter()
        .find(|p| p.file_stem().is_some_and(|s| s.eq_ignore_ascii_case(want)))
        .with_context(|| format!("no pak named {name} in the install"))
}

fn loca(handle: &str, language: &str) -> Result<()> {
    let pak = open(language)?;
    let mut table = bg3_loca::Loca::new();
    for e in pak.entries().iter().filter(|e| e.name.ends_with(".loca")) {
        table
            .merge_bytes(&pak.read(e)?)
            .with_context(|| e.name.clone())?;
    }
    let entry = table
        .get(handle)
        .with_context(|| format!("{handle} not found in {language}"))?;
    println!("{}", entry.text);
    eprintln!("(version {})", entry.version);
    Ok(())
}

fn stat(name: &str, honour: bool, own: bool) -> Result<()> {
    use bg3_stats::install::{BASE_LOAD_ORDER, HONOUR_MODULES, load};

    let paks: Vec<Package> = bg3_vfs::open_paks(&Install::locate()?)?;
    let mut order = BASE_LOAD_ORDER.to_vec();
    if honour {
        order.extend_from_slice(HONOUR_MODULES);
    }
    let stats = load(&paks, &order)?;
    let obj = stats
        .get(name)
        .or_else(|| stats.kinds().find_map(|(_, t)| t.get(name)))
        .with_context(|| format!("no stats object named {name}"))?;
    println!("new {} \"{}\"", obj.kind, obj.name);
    if let Some(t) = &obj.type_name {
        println!("type \"{t}\"");
    }
    if let Some(u) = &obj.using {
        println!("using \"{u}\"");
    }
    let fields = if own { &obj.own } else { &obj.fields };
    for (k, v) in fields {
        let inherited = !own && !obj.own.iter().any(|(ok, _)| ok == k);
        let mark = if inherited { "  // inherited" } else { "" };
        println!("data \"{k}\" \"{v}\"{mark}");
    }
    eprintln!("// defined at {} ({})", obj.source, obj.source.module);
    for prev in &obj.previous {
        eprintln!("// replaces {} ({})", prev, prev.module);
    }
    Ok(())
}

fn find(value: &str, prefix: &str) -> Result<()> {
    fn walk(node: &bg3_lsf::Node, path: &mut Vec<String>, value: &str, hits: &mut Vec<String>) {
        path.push(node.name.clone());
        for attr in &node.attributes {
            if attr.value.to_string() == value {
                hits.push(format!("{} @{}", path.join("/"), attr.name));
            }
        }
        for child in &node.children {
            walk(child, path, value, hits);
        }
        path.pop();
    }

    let vfs = bg3_vfs::Vfs::from_install(&Install::locate()?)?;
    let resource_ext = [".lsf", ".lsx", ".lsj", ".lsfx"];
    for file in vfs.files_under(prefix) {
        let name = file.entry.name.to_ascii_lowercase();
        if !resource_ext.iter().any(|e| name.ends_with(e)) {
            continue;
        }
        let Ok(res) = bg3_lsf::parse(&file.read()?) else {
            eprintln!("skipping unparsable {}", file.entry.name);
            continue;
        };
        let mut hits = Vec::new();
        for region in &res.regions {
            walk(&region.node, &mut Vec::new(), value, &mut hits);
        }
        for hit in hits {
            println!("{}: {hit}", file.entry.name);
        }
    }
    Ok(())
}

fn story(pak: &str, path: &str, goal: Option<&str>, summary: bool) -> Result<()> {
    use std::io::Write;
    let story = bg3_story::Story::parse(&read_file(pak, path)?).with_context(|| path.to_owned())?;
    let mut out = std::io::stdout().lock();
    if summary {
        out.write_all(story.dump_summary().as_bytes())?;
    } else if let Some(name) = goal {
        let g = story
            .goal_by_name(name)
            .with_context(|| format!("goal {name} not found"))?;
        out.write_all(story.dump_goal(g).as_bytes())?;
    } else {
        write!(out, "{story}")?;
    }
    Ok(())
}

fn osiris_replay(
    trace: Option<PathBuf>,
    pak: &str,
    path: &str,
    vm_log: Option<PathBuf>,
    kinds: bool,
) -> Result<()> {
    let trace = trace.unwrap_or_else(|| {
        let dir = PathBuf::from(".cache").join("traces");
        let full = dir.join("full-nautiloid.log");
        if full.exists() {
            full
        } else {
            dir.join("snapshot-early.log")
        }
    });
    let log = std::fs::read(&trace).with_context(|| trace.display().to_string())?;
    let log = String::from_utf8_lossy(&log).into_owned();
    if kinds {
        let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
        for l in bg3_osiris::trace::parse_log(&log) {
            if let bg3_osiris::trace::Record::Other(text) = &l.record {
                println!("unparsed line {}: {text}", l.line);
            }
            let k = format!("{:?}", l.record);
            let k = k.split([' ', '(', '{']).next().unwrap_or("").to_owned();
            *counts.entry(k).or_default() += 1;
        }
        for (k, n) in counts {
            println!("{n:>9}  {k}");
        }
    }
    let data = read_file(pak, path)?;
    eprintln!("replaying {} against {pak}:{path}", trace.display());
    // Rule chains recurse deeply; run on a big stack.
    let keep = vm_log.is_some();
    let report = std::thread::Builder::new()
        .stack_size(1 << 30)
        .spawn(move || -> Result<bg3_osiris::replay::Report> {
            let story = bg3_story::Story::parse(&data)?;
            Ok(bg3_osiris::replay::replay_with(&story, &log, keep))
        })?
        .join()
        .map_err(|_| anyhow::anyhow!("replay thread panicked"))??;
    if let (Some(p), Some(t)) = (&vm_log, &report.vm_log) {
        std::fs::write(p, t).with_context(|| p.display().to_string())?;
    }
    print!("{}", report.summary());
    Ok(())
}

fn gr2(data: &[u8], tree: bool, bones: bool) -> Result<()> {
    let file = bg3_gr2::Gr2File::parse(data)?;
    if tree {
        let types = bg3_gr2::Types::new(&file);
        let mut out = String::new();
        types.root()?.to_value(8, 4).write_tree(&mut out, 0);
        print!("{out}");
        return Ok(());
    }
    let h = &file.header;
    let codecs: std::collections::BTreeSet<_> = h.sections.iter().map(|s| s.compression).collect();
    println!(
        "format {:?} rev {} sections {} ({codecs:?})",
        h.format,
        h.revision,
        h.sections.len()
    );
    let scene = bg3_gr2::Scene::from_file(&file)?;
    println!("from {:?}", scene.from_file_name);
    for s in &scene.skeletons {
        println!("skeleton {:?}: {} bones", s.name, s.bones.len());
    }
    for (i, m) in scene.meshes.iter().enumerate() {
        let fmt: Vec<String> = m
            .format
            .iter()
            .map(|c| format!("{}:{:?}x{}", c.name, c.kind, c.count))
            .collect();
        println!(
            "mesh {i} {:?}: {} verts, {} tris ({:?}), {} groups, {} bone bindings, lod {:?}",
            m.name,
            m.vertex_count(),
            m.triangle_count(),
            m.index_width,
            m.groups.len(),
            m.bone_bindings.len(),
            m.properties.as_ref().and_then(|p| p.lod)
        );
        println!("    {}", fmt.join(" "));
    }
    for m in &scene.models {
        println!(
            "model {:?}: skeleton {:?}, meshes {:?}",
            m.name, m.skeleton, m.meshes
        );
    }
    if bones {
        for m in &scene.meshes {
            let mut sums = vec![([0f64; 3], 0usize); m.bone_bindings.len()];
            for (pos, (idx, w)) in m
                .positions
                .iter()
                .zip(m.bone_indices.iter().zip(&m.bone_weights))
            {
                let dominant = (0..4).max_by(|&a, &b| w[a].total_cmp(&w[b])).unwrap();
                if let Some((sum, n)) = sums.get_mut(idx[dominant] as usize) {
                    for k in 0..3 {
                        sum[k] += pos[k] as f64;
                    }
                    *n += 1;
                }
            }
            println!("mesh {:?}:", m.name);
            for (b, (sum, n)) in m.bone_bindings.iter().zip(sums) {
                if n > 0 {
                    let c = sum.map(|v| v / n as f64);
                    println!(
                        "  {:<28} {n:>6} verts  mean ({:7.3}, {:7.3}, {:7.3})",
                        b.bone_name, c[0], c[1], c[2]
                    );
                }
            }
        }
    }
    if scene.animation_count > 0 {
        println!(
            "{} animations, {} track groups (not decoded)",
            scene.animation_count, scene.track_group_count
        );
    }
    Ok(())
}

/// Read `path` from a named pak, or from the whole VFS when `pak` is `-`.
fn read_file(pak: &str, path: &str) -> Result<Vec<u8>> {
    if pak == "-" {
        let vfs = bg3_vfs::Vfs::from_install(&Install::locate()?)?;
        return Ok(vfs.read(path)?);
    }
    let pak = open(pak)?;
    let entry = pak
        .find(path)
        .with_context(|| format!("{path} not found"))?;
    Ok(pak.read(entry)?)
}

fn open(name: &str) -> Result<Package> {
    let path = resolve(name)?;
    Package::open(&path).with_context(|| path.display().to_string())
}

fn matching<'a>(
    pak: &'a Package,
    filter: Option<&'a str>,
) -> impl Iterator<Item = &'a bg3_pak::Entry> {
    pak.entries()
        .iter()
        .filter(move |e| filter.is_none_or(|f| e.name.contains(f)))
}

/// Turn an archive path (always `/`-separated) into a relative filesystem path, rejecting
/// anything that could escape the output directory or isn't a plain name on every OS
/// (`..`, absolute paths, drive prefixes, backslashes, empty segments).
fn safe_relative(name: &str) -> Result<PathBuf> {
    let mut path = PathBuf::new();
    for seg in name.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." || seg.contains(['\\', ':']) {
            bail!("refusing unsafe archive path {name:?}");
        }
        path.push(seg);
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::safe_relative;
    use std::path::PathBuf;

    #[test]
    fn safe_relative_builds_native_paths() {
        assert_eq!(
            safe_relative("Public/Shared/x.lsf").unwrap(),
            PathBuf::from("Public").join("Shared").join("x.lsf")
        );
        for bad in ["/abs", "a/../b", "a//b", "C:/x", "a\\b", "", "./a"] {
            assert!(safe_relative(bad).is_err(), "{bad:?}");
        }
    }
}
