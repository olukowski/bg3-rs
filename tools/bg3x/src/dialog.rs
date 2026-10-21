//! `bg3x dialog`: print a compiled dialog as a tree, or walk it interactively with the runner.
//! Text is resolved from the user's install at run time and only printed to the terminal.

use std::collections::HashSet;
use std::io::{BufRead, Write};
use std::sync::Arc;

use anyhow::{Context, Result};
use bg3_dialog::defs::Definitions;
use bg3_dialog::library::DialogLibrary;
use bg3_dialog::runner::placeholder_speakers;
use bg3_dialog::{
    Dialog, DialogRunner, FlagGroup, Guid, MemoryWorld, Node, NodeId, NodeKind, RollOutcome,
    StartParams, Step,
};
use bg3_install::Install;
use bg3_loca::Loca;
use bg3_vfs::Vfs;

struct Ctx {
    lib: Arc<DialogLibrary>,
    defs: Definitions,
    loca: Loca,
}

fn load_ctx(language: &str) -> Result<Ctx> {
    let install = Install::locate()?;
    let vfs = Arc::new(Vfs::from_install(&install)?);
    let defs = Definitions::load(&vfs)?;
    let loca = load_loca(&vfs, language)?;
    Ok(Ctx {
        lib: Arc::new(DialogLibrary::new(vfs)?),
        defs,
        loca,
    })
}

/// Merges every `.loca` file under `Localization/<language>/` in the VFS.
pub fn load_loca(vfs: &Vfs, language: &str) -> Result<Loca> {
    let mut loca = Loca::new();
    let prefix = format!("Localization/{language}/");
    let mut files: Vec<_> = vfs
        .files_under(&prefix)
        .filter(|f| f.entry.name.to_ascii_lowercase().ends_with(".loca"))
        .collect();
    files.sort_by(|a, b| a.entry.name.cmp(&b.entry.name));
    for f in files {
        loca.merge_bytes(&f.read()?)
            .with_context(|| f.entry.name.clone())?;
    }
    Ok(loca)
}

/// Options of `bg3x dialog`.
pub struct Options {
    pub name: String,
    pub walk: bool,
    pub list: bool,
    pub language: String,
    /// Scripted choices for `--walk` (1-based, as displayed); interactive once used up.
    pub choices: Vec<usize>,
    /// Scripted roll outcomes for `--walk`; interactive once used up.
    pub rolls: Vec<bool>,
    /// Print the Osiris events the walk raises.
    pub events: bool,
}

pub fn run(opts: Options) -> Result<()> {
    let ctx = load_ctx(&opts.language)?;
    if opts.list {
        let needle = opts.name.to_ascii_lowercase();
        for r in ctx.lib.resources() {
            if r.name.to_ascii_lowercase().contains(&needle) {
                println!("{}\t{}", r.osiris_name(), r.binary_path());
            }
        }
        return Ok(());
    }
    let resource = ctx
        .lib
        .find(&opts.name)
        .with_context(|| {
            format!(
                "no dialog resource named {} (try `bg3x dialog --list {}`)",
                opts.name, opts.name
            )
        })?
        .clone();
    let dialog = ctx.lib.load_resource(&resource.id)?;
    eprintln!("{}  {}", resource.osiris_name(), resource.binary_path());
    if opts.walk {
        walk_dialog(&ctx, &dialog, &opts)
    } else {
        print_tree(&ctx, &dialog);
        Ok(())
    }
}

fn print_tree(ctx: &Ctx, d: &Dialog) {
    println!(
        "dialog {}  category={:?}  timeline={}  automated={}",
        d.uuid,
        d.category,
        d.timeline_id.map(|t| t.to_string()).unwrap_or("-".into()),
        d.options.automated
    );
    for s in &d.speakers {
        let list: Vec<String> = s.list.iter().map(|g| ctx.defs.name_of(g)).collect();
        println!(
            "  speaker {}: {}{}",
            s.index,
            list.join(";"),
            if s.is_peanut { " (peanut)" } else { "" }
        );
    }
    let mut seen = HashSet::new();
    for &r in &d.roots {
        print_node(ctx, d, r, 0, &mut seen);
    }
}

fn print_node(ctx: &Ctx, d: &Dialog, id: NodeId, depth: usize, seen: &mut HashSet<NodeId>) {
    let indent = "  ".repeat(depth);
    let n = &d.nodes[id];
    if !seen.insert(id) {
        println!("{indent}#{id} (see above)");
        return;
    }
    println!("{indent}{}", describe(ctx, d, id));
    for line in flag_lines(ctx, n) {
        println!("{indent}    {line}");
    }
    for &c in &n.children {
        print_node(ctx, d, c, depth + 1, seen);
    }
}

/// One-line description of a node: id, constructor, speaker, attributes and its first line.
fn describe(ctx: &Ctx, d: &Dialog, id: NodeId) -> String {
    let n = &d.nodes[id];
    let uuid = n.uuid.to_string();
    let mut s = format!("#{id} {{{}}} {}", &uuid[..8], n.kind.constructor());
    match &n.kind {
        NodeKind::Alias { source } => s += &format!(" of #{}", opt(source)),
        NodeKind::Jump {
            target,
            target_point,
        } => s += &format!(" to #{} (point {target_point})", opt(target)),
        NodeKind::Pop { level } => s += &format!(" level {level}"),
        NodeKind::NestedDialog { dialog, .. } => {
            let name = ctx
                .lib
                .resource(dialog)
                .map(|r| r.name.clone())
                .unwrap_or_else(|| dialog.to_string());
            s += &format!(" {name}");
        }
        NodeKind::ActiveRoll(r) | NodeKind::PassiveRoll(r) => {
            s += &format!(" {} {} {}", r.roll_type, r.ability, r.skill);
            if let Some(dc) = r.difficulty_class {
                s += &format!(" DC {dc}");
            }
            if r.advantage != 0 {
                s += &format!(" adv={}", r.advantage);
            }
        }
        NodeKind::RollResult { success } => s += &format!(" success={success}"),
        NodeKind::FallibleQuestionResult { result } => s += &format!(" result={result}"),
        _ => {}
    }
    if let Some(sp) = n.speaker {
        s += &format!(" [spk {sp}]");
    }
    let mut marks = Vec::new();
    for (on, m) in [
        (n.root, "root"),
        (n.end_node, "end"),
        (n.show_once, "once"),
        (n.optional, "optional"),
        (n.exclusive, "exclusive"),
    ] {
        if on {
            marks.push(m);
        }
    }
    if !marks.is_empty() {
        s += &format!(" ({})", marks.join(","));
    }
    if let Some(text) = node_text(ctx, d, id) {
        s += &format!(" \"{text}\"");
    }
    s
}

fn opt(v: &Option<NodeId>) -> String {
    v.map(|v| v.to_string()).unwrap_or("?".into())
}

/// The first line of a node (following aliases), resolved through loca.
fn node_text(ctx: &Ctx, d: &Dialog, id: NodeId) -> Option<String> {
    let mut n: &Node = &d.nodes[id];
    if let NodeKind::Alias { source: Some(s) } = n.kind {
        n = &d.nodes[s];
    }
    let line = n.texts.first()?.lines.first()?;
    Some(
        ctx.loca
            .text(&line.handle)
            .map(|t| t.replace('\n', " "))
            .unwrap_or_else(|| format!("<{}>", line.handle)),
    )
}

fn flag_lines(ctx: &Ctx, n: &Node) -> Vec<String> {
    let mut out = Vec::new();
    let fmt = |prefix: &str, groups: &[FlagGroup]| -> Option<String> {
        let parts: Vec<String> = groups
            .iter()
            .flat_map(|g| {
                g.flags.iter().map(move |f| {
                    let param = f.param.map(|p| format!("({p})")).unwrap_or_default();
                    format!(
                        "{}{param}:{}={}",
                        g.kind.name(),
                        ctx.defs.name_of(&f.uuid),
                        f.value as u8
                    )
                })
            })
            .collect();
        (!parts.is_empty()).then(|| format!("{prefix} {}", parts.join("  ")))
    };
    if let Some(l) = fmt("check", &n.check_flags) {
        out.push(l);
    }
    if let Some(l) = fmt("set  ", &n.set_flags) {
        out.push(l);
    }
    if let Some(v) = &n.validated_flags
        && let Some(l) = fmt("valid", &v.groups)
    {
        out.push(l);
    }
    out
}

/// Walks a dialog with the runner: lines are printed, choices read from `--choices` or stdin,
/// rolls from `--rolls` or stdin. Speakers are placeholders (`SPEAKER_<slot>`); flags live in
/// memory for the walk only.
fn walk_dialog(ctx: &Ctx, d: &Arc<Dialog>, opts: &Options) -> Result<()> {
    let speakers = placeholder_speakers(d);
    let slot_of: std::collections::HashMap<Guid, usize> = speakers
        .iter()
        .enumerate()
        .filter_map(|(i, a)| Some(((*a)?, i)))
        .collect();
    let name = |g: &Guid| match slot_of.get(g) {
        Some(i) => format!("SPEAKER_{i}"),
        None => match ctx.defs.flags.get(g) {
            Some(f) => f.osiris_name(),
            None => format!("{}_{g}", ctx.defs.name_of(g)),
        },
    };
    let mut world = MemoryWorld::with_definitions(&ctx.defs);
    let rolls = std::sync::Mutex::new(opts.rolls.clone().into_iter());
    world.set_roller(move |req| {
        let what = format!(
            "{} roll: {} {} {}",
            if req.passive { "passive" } else { "active" },
            req.roll.roll_type,
            req.roll.ability,
            req.roll.skill
        );
        let scripted = rolls.lock().unwrap().next();
        let success = match scripted {
            Some(s) => s,
            None => {
                prompt(&format!("{what} — success? [Y/n] ")).is_none_or(|a| !a.starts_with('n'))
            }
        };
        println!(
            "  [{what} → {}]",
            if success { "success" } else { "failure" }
        );
        RollOutcome {
            success,
            critical: None,
        }
    });
    let mut run = DialogRunner::new(
        d.clone(),
        ctx.lib.clone(),
        StartParams {
            instance: 1,
            speakers,
        },
    );
    let mut choices = opts.choices.iter().copied();
    run.start(&mut world);
    loop {
        if opts.events {
            for e in run.take_events() {
                for line in e.osiris(&name) {
                    println!("    >>> {line}");
                }
            }
        }
        match run.step().clone() {
            Step::Line(l) => {
                let text = l
                    .text
                    .as_ref()
                    .map(|t| loca_text(ctx, &t.handle))
                    .unwrap_or_else(|| format!("[{}]", l.constructor));
                println!("{}: {text}", speaker_label(l.speaker));
                run.advance(&mut world)?;
            }
            Step::Choices(options) => {
                for (i, c) in options.iter().enumerate() {
                    let text = c
                        .text
                        .as_ref()
                        .map(|t| loca_text(ctx, &t.handle))
                        .unwrap_or_else(|| format!("[{}]", c.constructor));
                    let roll = c
                        .roll
                        .as_ref()
                        .map(|r| format!(" [{} {}]", r.ability, r.skill))
                        .unwrap_or_default();
                    let disabled = if c.enabled { "" } else { " (unavailable)" };
                    println!("  {}. {text}{roll}{disabled}", i + 1);
                }
                let pick = match choices.next() {
                    Some(n) => n,
                    None => match prompt("> ").and_then(|a| a.trim().parse::<usize>().ok()) {
                        Some(n) => n,
                        None => return Ok(()),
                    },
                };
                if let Err(e) = run.choose(&mut world, pick.wrapping_sub(1)) {
                    eprintln!("{e}");
                }
            }
            Step::Ended => {
                println!("(dialog ended)");
                return Ok(());
            }
        }
    }
}

fn speaker_label(speaker: Option<i32>) -> String {
    match speaker {
        Some(bg3_dialog::model::SPEAKER_NARRATOR) => "Narrator".into(),
        Some(s) if s >= 0 => format!("Speaker {s}"),
        _ => "-".into(),
    }
}

fn loca_text(ctx: &Ctx, handle: &str) -> String {
    ctx.loca
        .text(handle)
        .map(|t| t.replace('\n', " "))
        .unwrap_or_else(|| format!("<{handle}>"))
}

/// Reads one line from stdin after printing `msg`; `None` at end of input or on `q`.
fn prompt(msg: &str) -> Option<String> {
    print!("{msg}");
    std::io::stdout().flush().ok()?;
    let mut line = String::new();
    if std::io::stdin().lock().read_line(&mut line).ok()? == 0 {
        return None;
    }
    let line = line.trim().to_owned();
    (line != "q").then_some(line)
}
