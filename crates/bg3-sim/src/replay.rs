//! `sim-replay`: run a captured libOsiris log with the sim as the engine, and measure how
//! much of the story's behaviour the sim reproduces.
//!
//! The story VM boots and receives the logged events in order, like the plain replay
//! (`bg3_osiris::replay`), but:
//!
//! * **Queries** the sim implements are answered by the sim. Each answer is checked against
//!   the logged one (same segment, same inputs). Queries the sim does not implement fall back
//!   to the logged answer.
//! * **Calls** the sim implements change the sim's state and queue events. Others fall back
//!   to the log (only their success flag matters to Osiris).
//! * **Events** are split in two classes (see [`EventClass`] and
//!   `docs/research/osiris-engine-api.md`):
//!   - *sim-owned* (`FlagSet`, `TagSet`, `ObjectTimerFinished`, `StatusApplied`, …): the
//!     engine raises them as a consequence of state changes the sim models. When the log
//!     delivers one, the harness looks for the same event in the sim's queue; if the sim
//!     produced it, the sim's event is delivered ("generated"), else the logged one is fed
//!     and counted as a fallback ("fed").
//!   - *external* (level loading, character creation, perception, dialog, combat, …): fed
//!     from the log, and the sim observes them ([`Sim::observe`]) so its state follows.
//!
//!   The log gives the order; the sim decides whether the event happens. The log has no
//!   timestamps, so timers are not driven by the clock here: when the log delivers a timer
//!   event, the sim must have that timer running, and it fires then.
//!
//! Two [`Mode`]s: [`Mode::Sim`] lets the sim's answers drive the story (the honest end-to-end
//! number: a wrong answer changes what the story does next); [`Mode::Check`] keeps the
//! story on the logged answers and only grades the sim's (isolates each answer's
//! correctness from knock-on effects).

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt::Write as _;
use std::sync::Arc;

use bg3_osiris::replay::{
    Answer, Comparer, ReplayHost, Report, Segment, event_values, ignored_event, segments,
};
use bg3_osiris::trace::{LogData, LogValue, parse_log};
use bg3_osiris::{Host, HostCtx, Value, Vm};
use bg3_story::Story;

use bg3_dialog::Step;

use crate::dialog::{DialogBank, DialogRollRecord, DialogScript, InstanceState};
use crate::events::SimEvent;
use crate::host::SimFunctions;
use crate::sim::{GameData, ObservedArg, Sim};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The sim's answers drive the story.
    Sim,
    /// The logged answers drive the story; the sim's answers are only compared.
    Check,
}

#[derive(Clone, Debug)]
pub struct Options {
    pub mode: Mode,
    /// Level loaded at the start; empty = the module's character-creation level (a new
    /// campaign starts there).
    pub start_level: String,
    pub keep_vm_log: bool,
    /// Dialogs: with them the sim runs the story's dialogs itself (the player's choices are
    /// searched for, roll outcomes taken from the log); without, dialog events are fed from
    /// the log.
    pub dialogs: Option<Arc<DialogBank>>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            mode: Mode::Sim,
            start_level: String::new(),
            keep_vm_log: false,
            dialogs: None,
        }
    }
}

/// Events the sim is responsible for raising.
pub const SIM_OWNED_EVENTS: &[&str] = &[
    "FlagSet",
    "FlagCleared",
    "TagSet",
    "TagCleared",
    "TimerFinished",
    "ObjectTimerFinished",
    "StatusAttempt",
    "StatusApplied",
    "StatusRemoved",
    "BaseFactionChanged",
    // Raised by the sim's dialogs (from `DIALOG_EVENTS_FROM` on).
    "DialogStarted",
    "AutomatedDialogStarted",
    "DialogActorJoined",
    "DialogActorLeft",
    "DialogEnded",
    "AutomatedDialogEnded",
    "RollResult",
    "DialogRollResult",
    "NestedDialogPlayed",
    "InstanceDialogChanged",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventClass {
    SimOwned,
    External,
}

pub fn classify(event: &str) -> EventClass {
    if SIM_OWNED_EVENTS.contains(&event) {
        EventClass::SimOwned
    } else {
        EventClass::External
    }
}

/// Per-query statistics.
#[derive(Clone, Copy, Debug, Default)]
pub struct QueryStats {
    /// Answered by the sim.
    pub sim: u64,
    /// Sim answer equal to the logged one of the same event.
    pub agree: u64,
    pub disagree: u64,
    /// Sim answered, but the log has no answer for these inputs in the same event (the story
    /// had already diverged, so there is nothing to grade against).
    pub ungraded: u64,
    /// Implemented, but the sim lacked the data (an object it never saw, a status without
    /// stats entry): answered from the log.
    pub no_data: u64,
    /// Not implemented: answered from the log.
    pub fallback: u64,
}

impl QueryStats {
    /// Answered from the log, for whatever reason.
    pub fn from_log(&self) -> u64 {
        self.no_data + self.fallback
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CallStats {
    pub sim: u64,
    /// Sim reports the call failed.
    pub sim_failed: u64,
    /// Not implemented: result taken from the log.
    pub fallback: u64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct EventStats {
    /// In the log.
    pub expected: u64,
    /// Sim-owned and produced by the sim, identical text.
    pub generated: u64,
    /// Produced by the sim with the same UUIDs but a different rendering (names).
    pub generated_renamed: u64,
    /// Sim-owned but not produced by the sim: fed from the log (`fed_outside + fed_gap`).
    pub fed: u64,
    /// ... because its cause is outside what the sim models: a flag set by a dialog (non-zero
    /// dialog instance), a tag/status on an object the sim never loaded (created at runtime by
    /// treasure generation, character creation, …).
    pub fed_outside: u64,
    /// ... although the sim knows the object: a gap in the sim's rules.
    pub fed_gap: u64,
    /// External: fed from the log by design.
    pub external: u64,
    /// Produced by the sim but never in the log.
    pub extra: u64,
}

#[derive(Clone, Debug)]
pub struct SimReport {
    pub mode: Mode,
    /// The VM comparison (records, databases, …) as in the plain replay.
    pub vm: Report,
    pub queries: BTreeMap<String, QueryStats>,
    pub calls: BTreeMap<String, CallStats>,
    pub events: BTreeMap<String, EventStats>,
    /// `(segment, query, sim answer, logged answer)`.
    pub disagreements: Vec<(usize, String, String, String)>,
    /// Sim-owned events the sim did not produce: `(segment, event)`.
    pub missing: Vec<(usize, String)>,
    /// Events the sim produced that the log never delivered.
    pub extras: Vec<String>,
    pub entities: usize,
    /// Every dialog instance of the log.
    pub dialogs: Vec<DialogReport>,
    /// The sim at the end of the replay.
    pub sim: Sim,
}

/// How one logged dialog instance went.
#[derive(Clone, Debug)]
pub struct DialogReport {
    pub instance: u32,
    pub dialog: String,
    /// Run by the sim (requested by the story through `StartDialog_Internal`).
    pub owned: bool,
    /// Events of the instance in the log (start, actors, flags, rolls, end).
    pub logged: usize,
    /// Of those, produced by the sim.
    pub generated: usize,
    /// The choices the search found (1-based as shown to a player), if any.
    pub choices: Option<Vec<usize>>,
    pub rolls: Vec<DialogRollRecord>,
    /// The log ends before the instance does.
    pub cut_off: bool,
}

impl SimReport {
    fn sum_q(&self) -> QueryStats {
        let mut t = QueryStats::default();
        for q in self.queries.values() {
            t.sim += q.sim;
            t.agree += q.agree;
            t.disagree += q.disagree;
            t.ungraded += q.ungraded;
            t.no_data += q.no_data;
            t.fallback += q.fallback;
        }
        t
    }

    fn sum_e(&self, f: impl Fn(&str) -> bool) -> EventStats {
        let mut t = EventStats::default();
        for (n, e) in &self.events {
            if f(n) {
                t.expected += e.expected;
                t.generated += e.generated;
                t.generated_renamed += e.generated_renamed;
                t.fed += e.fed;
                t.fed_outside += e.fed_outside;
                t.fed_gap += e.fed_gap;
                t.external += e.external;
                t.extra += e.extra;
            }
        }
        t
    }

    /// Queries answered by the sim / all queries.
    pub fn sim_query_share(&self) -> f64 {
        let t = self.sum_q();
        pct(t.sim, t.sim + t.from_log())
    }

    /// Every disagreement, missing and extra event, one per line (for scratch analysis).
    pub fn details(&self) -> String {
        let mut s = String::new();
        for (seg, q, a, b) in &self.disagreements {
            let _ = writeln!(s, "disagree\t{seg}\t{q}\tsim {a}\tlog {b}");
        }
        for (seg, e) in &self.missing {
            let _ = writeln!(s, "missing\t{seg}\t{e}");
        }
        for e in &self.extras {
            let _ = writeln!(s, "extra\t{e}");
        }
        s
    }

    pub fn summary(&self) -> String {
        let mut s = String::new();
        let mode = match self.mode {
            Mode::Sim => "sim answers drive the story",
            Mode::Check => "logged answers drive the story; sim answers graded only",
        };
        let _ = writeln!(s, "== sim-replay ({mode})");
        let r = &self.vm;
        let _ = writeln!(
            s,
            "segments: {} ({} matched = {:.2}%, {} diverging)",
            r.segments,
            r.segments_matched,
            pct(r.segments_matched as u64, r.segments as u64),
            r.diverging.len()
        );
        for (c, rate) in &r.rates {
            let _ = writeln!(
                s,
                "  {:<8} trace {:>7}  vm {:>7}  matched {:>7}  ({:.2}%)",
                c.name(),
                rate.trace,
                rate.vm,
                rate.matched,
                rate.percent()
            );
        }
        let _ = writeln!(
            s,
            "databases: {} touched, {} with identical fact history; VM errors {}",
            r.databases, r.databases_matched, r.vm_errors
        );
        let q = self.sum_q();
        let _ = writeln!(
            s,
            "queries: {} total, {} answered by the sim ({:.2}%), {} from the log ({} not implemented, {} implemented but without the data)",
            q.sim + q.from_log(),
            q.sim,
            self.sim_query_share(),
            q.from_log(),
            q.fallback,
            q.no_data
        );
        let _ = writeln!(
            s,
            "  sim answers: {} agree with the log, {} disagree, {} ungraded ({:.2}% agreement of graded)",
            q.agree,
            q.disagree,
            q.ungraded,
            pct(q.agree, q.agree + q.disagree)
        );
        let _ = writeln!(
            s,
            "  {:<28} {:>6} {:>6} {:>6} {:>6} {:>7} {:>8}",
            "query", "sim", "agree", "differ", "ungr.", "no data", "not impl"
        );
        let mut qs: Vec<(&String, &QueryStats)> = self.queries.iter().collect();
        qs.sort_by_key(|(_, q)| std::cmp::Reverse(q.sim + q.from_log()));
        for (n, q) in qs.iter().take(40) {
            let _ = writeln!(
                s,
                "  {:<28} {:>6} {:>6} {:>6} {:>6} {:>7} {:>8}",
                n, q.sim, q.agree, q.disagree, q.ungraded, q.no_data, q.fallback
            );
        }
        let rest: Vec<String> = qs
            .iter()
            .skip(40)
            .map(|(n, q)| format!("{n} {}", q.sim + q.from_log()))
            .collect();
        if !rest.is_empty() {
            let _ = writeln!(s, "  others: {}", rest.join(", "));
        }
        let (cs, cf): (u64, u64) = self
            .calls
            .values()
            .fold((0, 0), |(a, b), c| (a + c.sim, b + c.fallback));
        let _ = writeln!(
            s,
            "calls: {} total, {} run by the sim ({:.2}%), {} not implemented",
            cs + cf,
            cs,
            pct(cs, cs + cf),
            cf
        );
        for (n, c) in self.calls.iter().filter(|(_, c)| c.sim > 0) {
            let _ = writeln!(s, "  {:<28} {:>6} (failed {})", n, c.sim, c.sim_failed);
        }
        let own = self.sum_e(|n| classify(n) == EventClass::SimOwned);
        let ext = self.sum_e(|n| classify(n) == EventClass::External);
        let _ = writeln!(
            s,
            "events: {} in the log; external {} (fed by design); sim-owned {}: {} generated by the sim ({} with a different rendering), {} fed from the log ({} caused outside the sim, {} gaps); {} produced by the sim but not in the log",
            own.expected + ext.expected,
            ext.expected,
            own.expected,
            own.generated + own.generated_renamed,
            own.generated_renamed,
            own.fed,
            own.fed_outside,
            own.fed_gap,
            own.extra
        );
        let _ = writeln!(
            s,
            "  {:<22} {:>7} {:>9} {:>8} {:>8} {:>6} {:>6}",
            "sim-owned event", "log", "generated", "renamed", "outside", "gap", "extra"
        );
        for (n, e) in self
            .events
            .iter()
            .filter(|(n, _)| classify(n) == EventClass::SimOwned)
        {
            let _ = writeln!(
                s,
                "  {:<22} {:>7} {:>9} {:>8} {:>8} {:>6} {:>6}",
                n, e.expected, e.generated, e.generated_renamed, e.fed_outside, e.fed_gap, e.extra
            );
        }
        let _ = writeln!(s, "sim entities at the end: {}", self.entities);
        if !self.dialogs.is_empty() {
            let _ = writeln!(s, "dialogs:");
            for d in &self.dialogs {
                let rolls: Vec<String> = d
                    .rolls
                    .iter()
                    .map(|r| match &r.check {
                        Some(c) => format!(
                            "{} {}{}",
                            if r.active { "active" } else { "passive" },
                            c,
                            if r.consistent { "" } else { " (inconsistent)" }
                        ),
                        None => format!("roll without rules data: {}", r.success),
                    })
                    .collect();
                let _ = writeln!(
                    s,
                    "  #{} {}: {}/{} events by the sim{}{}{}",
                    d.instance,
                    d.dialog,
                    d.generated,
                    d.logged,
                    if d.owned {
                        ""
                    } else {
                        " (not started by the sim)"
                    },
                    match &d.choices {
                        Some(c) if !c.is_empty() => format!(", choices {c:?}"),
                        _ => String::new(),
                    },
                    if d.cut_off {
                        ", log ends mid-dialog"
                    } else {
                        ""
                    },
                );
                for r in rolls {
                    let _ = writeln!(s, "      {r}");
                }
            }
        }
        if !self.disagreements.is_empty() {
            let _ = writeln!(s, "disagreements (first 3 per query):");
            let mut shown: BTreeMap<&str, usize> = BTreeMap::new();
            for (seg, q, a, b) in &self.disagreements {
                let name = q.split('(').next().unwrap_or("");
                let n = shown.entry(name).or_default();
                *n += 1;
                if *n <= 3 {
                    let _ = writeln!(s, "  #{seg} {q}: sim {a} | log {b}");
                }
            }
        }
        if !self.missing.is_empty() {
            let _ = writeln!(s, "first sim-owned events the sim did not produce:");
            for (seg, e) in self.missing.iter().take(25) {
                let _ = writeln!(s, "  #{seg} {e}");
            }
        }
        if !self.extras.is_empty() {
            let _ = writeln!(s, "first events produced by the sim but not in the log:");
            for e in self.extras.iter().take(15) {
                let _ = writeln!(s, "  {e}");
            }
        }
        if let Some(d) = &r.first {
            let _ = writeln!(
                s,
                "first divergence: segment {} ({}), log line {}\n  trace: {}\n  vm:    {}",
                d.segment,
                d.event.as_deref().unwrap_or("InitGame"),
                d.line,
                d.trace.as_deref().unwrap_or("<end>"),
                d.vm.as_deref().unwrap_or("<end>")
            );
        }
        if r.diverging.len() > 1 {
            let _ = writeln!(s, "diverging segments (first 15):");
            for (seg, ev, t, v) in r.diverging.iter().take(15) {
                let _ = writeln!(
                    s,
                    "  #{seg} {ev}: trace {} | vm {}",
                    t.as_deref().unwrap_or("<end>"),
                    v.as_deref().unwrap_or("<end>")
                );
            }
        }
        s
    }
}

fn pct(a: u64, b: u64) -> f64 {
    if b == 0 {
        100.0
    } else {
        100.0 * a as f64 / b as f64
    }
}

/// The host during a sim replay.
struct HarnessHost<'a> {
    sim: &'a mut Sim,
    log: &'a mut ReplayHost,
    mode: Mode,
    segment: usize,
    queries: BTreeMap<String, QueryStats>,
    calls: BTreeMap<String, CallStats>,
    disagreements: Vec<(usize, String, String, String)>,
}

fn render(ctx: &HostCtx<'_>, ok: bool, args: &[Value], out: &[bool]) -> String {
    if !ok {
        return "failed".to_owned();
    }
    let vals: Vec<String> = args
        .iter()
        .zip(out)
        .filter(|(_, o)| **o)
        .map(|(a, _)| ctx.log_value(a).key())
        .collect();
    vals.join(", ")
}

fn render_logged(ans: &Answer, out: &[bool]) -> String {
    if !ans.ok {
        return "failed".to_owned();
    }
    let vals: Vec<String> = ans
        .values
        .iter()
        .zip(out)
        .filter(|(_, o)| **o)
        .map(|(v, _)| v.key())
        .collect();
    vals.join(", ")
}

impl Host for HarnessHost<'_> {
    fn call(&mut self, ctx: &mut HostCtx<'_>, name: &str, args: &[Value]) -> bool {
        let st = self.calls.entry(name.to_owned()).or_default();
        match SimFunctions::new(self.sim).call(ctx, name, args) {
            Some(ok) => {
                st.sim += 1;
                if !ok {
                    st.sim_failed += 1;
                }
                // The log only tells failed calls; in check mode its verdict wins.
                let logged = self.log.call(ctx, name, args);
                match self.mode {
                    Mode::Sim => ok,
                    Mode::Check => logged,
                }
            }
            None => {
                st.fallback += 1;
                self.log.call(ctx, name, args)
            }
        }
    }

    fn query(
        &mut self,
        ctx: &mut HostCtx<'_>,
        name: &str,
        args: &mut [Value],
        out: &[bool],
    ) -> bool {
        let mut sim_args = args.to_vec();
        let sim_ok = SimFunctions::new(self.sim).query(ctx, name, &mut sim_args, out);
        let logged = self.log.lookup(ctx, name, args, out);
        let st = self.queries.entry(name.to_owned()).or_default();
        let Some(sim_ok) = sim_ok else {
            if crate::host::implements(name, args.len()) {
                st.no_data += 1;
            } else {
                st.fallback += 1;
            }
            return match logged {
                Some(ans) => ReplayHost::apply(ctx, &ans, args, out),
                None => false,
            };
        };
        st.sim += 1;
        let sim_text = render(ctx, sim_ok, &sim_args, out);
        match &logged {
            Some(ans) if ans.segment == self.segment => {
                let log_text = render_logged(ans, out);
                if sim_text == log_text {
                    st.agree += 1;
                } else {
                    st.disagree += 1;
                    if self.disagreements.len() < 20_000 {
                        let inputs: Vec<String> = args
                            .iter()
                            .zip(out)
                            .filter(|(_, o)| !**o)
                            .map(|(a, _)| ctx.log_value(a).key())
                            .collect();
                        self.disagreements.push((
                            self.segment,
                            format!("{name}({})", inputs.join(", ")),
                            sim_text,
                            log_text,
                        ));
                    }
                }
            }
            _ => st.ungraded += 1,
        }
        match (self.mode, logged) {
            (Mode::Check, Some(ans)) => ReplayHost::apply(ctx, &ans, args, out),
            _ => {
                args.copy_from_slice(&sim_args);
                sim_ok
            }
        }
    }
}

/// Normalised event key: GUIDSTRINGs reduced to their UUID.
fn uuid_key(name: &str, args: &[String]) -> String {
    let a: Vec<String> = args
        .iter()
        .map(|t| match crate::guid::Guid::from_osiris(t) {
            Some(g) if !t.starts_with('"') => g.to_string(),
            _ => t.clone(),
        })
        .collect();
    format!("{name}({})", a.join(", "))
}

/// Sim events waiting for the log to deliver them.
#[derive(Default)]
struct Pending {
    events: Vec<Option<SimEvent>>,
    by_key: HashMap<String, VecDeque<usize>>,
    by_uuid: HashMap<String, VecDeque<usize>>,
}

impl Pending {
    fn push(&mut self, e: SimEvent) {
        let i = self.events.len();
        let texts: Vec<String> = e.args.iter().map(ToString::to_string).collect();
        self.by_key.entry(e.key()).or_default().push_back(i);
        self.by_uuid
            .entry(uuid_key(&e.name, &texts))
            .or_default()
            .push_back(i);
        self.events.push(Some(e));
    }

    fn take(
        map: &mut HashMap<String, VecDeque<usize>>,
        events: &mut [Option<SimEvent>],
        key: &str,
    ) -> Option<SimEvent> {
        let q = map.get_mut(key)?;
        while let Some(i) = q.pop_front() {
            if let Some(e) = events[i].take() {
                return Some(e);
            }
        }
        None
    }

    /// Whether the sim has the event queued (exact text or same UUIDs).
    fn contains(&self, key: &str, uuid: &str) -> bool {
        let live = |m: &HashMap<String, VecDeque<usize>>, k: &str| {
            m.get(k)
                .is_some_and(|q| q.iter().any(|&i| self.events[i].is_some()))
        };
        live(&self.by_key, key) || live(&self.by_uuid, uuid)
    }

    /// `(event, exact text)` for a logged event, if the sim has it queued.
    fn take_match(&mut self, key: &str, uuid: &str) -> Option<(SimEvent, bool)> {
        if let Some(e) = Self::take(&mut self.by_key, &mut self.events, key) {
            return Some((e, true));
        }
        Self::take(&mut self.by_uuid, &mut self.events, uuid).map(|e| (e, false))
    }

    fn remaining(&self) -> impl Iterator<Item = &SimEvent> {
        self.events.iter().flatten()
    }
}

/// Index of the first dialog event in [`SIM_OWNED_EVENTS`].
const DIALOG_EVENTS_FROM: usize = 10;

/// The dialog instance an event belongs to. `by_dialog` finds the instance of a dialog by its
/// `Name_<uuid>` (for `DialogRollResult`, which names no instance).
fn dialog_instance_of(
    name: &str,
    texts: &[String],
    by_dialog: impl Fn(&str) -> Option<u32>,
) -> Option<u32> {
    let int = |i: usize| texts.get(i)?.parse::<u32>().ok();
    match name {
        "DialogStarted"
        | "AutomatedDialogStarted"
        | "DialogActorJoined"
        | "DialogActorLeft"
        | "DialogEnded"
        | "AutomatedDialogEnded"
        | "NestedDialogPlayed" => int(1),
        "InstanceDialogChanged" => int(0),
        "FlagSet" | "FlagCleared" => int(2).filter(|i| *i != 0),
        "RollResult" => texts
            .first()?
            .trim_matches('"')
            .rsplit_once('_')?
            .1
            .parse()
            .ok(),
        "DialogRollResult" => by_dialog(texts.get(2)?),
        _ => None,
    }
}

/// A dialog instance as the log shows it: its events in order and its roll outcomes.
#[derive(Clone, Debug, Default)]
struct LoggedDialog {
    dialog: String,
    expected: Vec<String>,
    rolls: Vec<bool>,
}

fn event_key(name: &str, texts: &[String]) -> String {
    format!("{name}({})", texts.join(", "))
}

fn logged_dialogs(segs: &[Segment]) -> BTreeMap<u32, LoggedDialog> {
    let mut out: BTreeMap<u32, LoggedDialog> = BTreeMap::new();
    let mut by_name: HashMap<String, u32> = HashMap::new();
    for seg in segs {
        let Some((name, args)) = &seg.event else {
            continue;
        };
        let texts: Vec<String> = args.iter().map(LogValue::key).collect();
        if matches!(name.as_str(), "DialogStarted" | "AutomatedDialogStarted")
            && let (Some(d), Some(i)) = (texts.first(), texts.get(1).and_then(|t| t.parse().ok()))
        {
            by_name.insert(d.clone(), i);
            out.entry(i).or_default().dialog = d.clone();
        }
        let Some(inst) = dialog_instance_of(name, &texts, |d| by_name.get(d).copied()) else {
            continue;
        };
        let e = out.entry(inst).or_default();
        e.expected.push(event_key(name, &texts));
        if name == "RollResult" {
            e.rolls.push(texts.get(3).is_some_and(|s| s == "1"));
        }
    }
    out
}

/// One attempt of a choice path in the search.
enum Probe {
    /// Every logged event of the instance reproduced.
    Match,
    /// Diverged after this many matching events.
    Mismatch(usize),
    /// Choices on offer after this many matching events: how many, and a state signature.
    Choose(usize, usize, u64),
}

/// Play an instance on a copy of the sim with the given choices; compare what it raises with
/// the log.
fn probe(sim: &Sim, inst: u32, logged: &LoggedDialog, path: &[usize]) -> Probe {
    use std::hash::{Hash, Hasher};
    let mut s = sim.clone();
    s.take_events();
    s.set_dialog_script(
        inst,
        DialogScript {
            choices: path.iter().copied().collect(),
            rolls: logged.rolls.iter().copied().collect(),
        },
    );
    let name = s.dialog(inst).map(|i| i.name.clone()).unwrap_or_default();
    let mut produced: Vec<String> = Vec::new();
    for _ in 0..5_000 {
        let progressed = s.dialog_autostep(inst);
        for e in s.take_events() {
            let texts: Vec<String> = e.args.iter().map(ToString::to_string).collect();
            if dialog_instance_of(&e.name, &texts, |d| (d == name).then_some(inst)) == Some(inst) {
                produced.push(e.key());
            }
        }
        let matched = produced
            .iter()
            .zip(&logged.expected)
            .take_while(|(a, b)| a == b)
            .count();
        if matched < produced.len() {
            return Probe::Mismatch(matched);
        }
        if matched == logged.expected.len() {
            return Probe::Match;
        }
        if !progressed {
            return match s.dialog(inst).and_then(|i| i.step()) {
                Some(Step::Choices(c)) => {
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    produced.hash(&mut h);
                    c.iter().for_each(|o| o.node_uuid.hash(&mut h));
                    Probe::Choose(c.len(), matched, h.finish())
                }
                _ => Probe::Mismatch(matched),
            };
        }
    }
    Probe::Mismatch(0)
}

/// Search the player's choices for a walk that raises the logged events (depth first, like
/// `bg3-dialog`'s trace test); the best walk if none does.
fn find_choices(sim: &Sim, inst: u32, logged: &LoggedDialog) -> Option<Vec<usize>> {
    let mut stack: Vec<Vec<usize>> = vec![Vec::new()];
    let mut seen: HashSet<u64> = HashSet::new();
    let mut best: (usize, Option<Vec<usize>>) = (0, None);
    let mut budget = 20_000;
    while let Some(path) = stack.pop() {
        if budget == 0 {
            break;
        }
        budget -= 1;
        match probe(sim, inst, logged, &path) {
            Probe::Match => return Some(path),
            Probe::Mismatch(n) => {
                if n > best.0 {
                    best = (n, Some(path));
                }
            }
            Probe::Choose(n, matched, sig) => {
                if !seen.insert(sig) {
                    continue;
                }
                if matched > best.0 {
                    best = (matched, Some(path.clone()));
                }
                for i in (0..n).rev() {
                    let mut p = path.clone();
                    p.push(i);
                    stack.push(p);
                }
            }
        }
    }
    best.1
}

/// Drives the sim's dialogs during a replay: when the log delivers a dialog's event the sim
/// has not raised yet, the dialog plays on (lines finish, scripted choices are taken) until it
/// raises it.
struct DialogDriver {
    logged: BTreeMap<u32, LoggedDialog>,
    choices: BTreeMap<u32, Option<Vec<usize>>>,
    generated: BTreeMap<u32, usize>,
}

impl DialogDriver {
    fn pump(&mut self, sim: &mut Sim, pending: &mut Pending, inst: u32, key: &str, uuid: &str) {
        let Some(i) = sim.dialog(inst) else {
            return;
        };
        if !i.owned {
            return;
        }
        if i.state == InstanceState::Requested && !self.choices.contains_key(&inst) {
            let logged = self.logged.get(&inst).cloned().unwrap_or_default();
            let path = find_choices(sim, inst, &logged);
            sim.set_dialog_script(
                inst,
                DialogScript {
                    choices: path.iter().flatten().copied().collect(),
                    rolls: logged.rolls.iter().copied().collect(),
                },
            );
            self.choices.insert(inst, path);
        }
        for _ in 0..5_000 {
            if !sim.dialog_autostep(inst) {
                break;
            }
            let mut found = false;
            for e in sim.take_events() {
                let texts: Vec<String> = e.args.iter().map(ToString::to_string).collect();
                found |= e.key() == key || uuid_key(&e.name, &texts) == uuid;
                pending.push(e);
            }
            if found {
                break;
            }
        }
    }

    fn report(&self, sim: &Sim) -> Vec<DialogReport> {
        self.logged
            .iter()
            .map(|(&id, l)| {
                let inst = sim.dialog(id);
                DialogReport {
                    instance: id,
                    dialog: l.dialog.clone(),
                    owned: inst.is_some_and(|i| i.owned),
                    logged: l.expected.len(),
                    generated: self.generated.get(&id).copied().unwrap_or(0),
                    choices: self
                        .choices
                        .get(&id)
                        .cloned()
                        .flatten()
                        .map(|c| c.iter().map(|i| i + 1).collect()),
                    rolls: inst.map(|i| i.rolls.clone()).unwrap_or_default(),
                    cut_off: !l
                        .expected
                        .last()
                        .is_some_and(|e| e.contains("DialogEnded(")),
                }
            })
            .collect()
    }
}

/// The running sim instance a dialog name refers to (latest first).
fn sim_instance_by_name(sim: &Sim, name: &str) -> Option<u32> {
    sim.dialogs
        .instances
        .iter()
        .rev()
        .find(|i| i.name == name)
        .map(|i| i.id)
}

/// Run the replay. Needs a deep stack (run on a thread with a large stack, like the plain
/// replay).
pub fn sim_replay(story: &Story, log: &str, data: GameData, opts: &Options) -> SimReport {
    let segs: Vec<Segment> = segments(parse_log(log));
    let mut log_host = ReplayHost::new(&segs);
    let mut vm = Vm::new(story);
    vm.set_logging(true);
    let mut sim = Sim::new(data);
    if let Some(bank) = &opts.dialogs {
        sim.set_dialog_bank(bank.clone());
    }
    let mut dialogs = DialogDriver {
        logged: logged_dialogs(&segs),
        choices: BTreeMap::new(),
        generated: BTreeMap::new(),
    };
    let start = if opts.start_level.is_empty() {
        sim.data.module_levels.character_creation.clone()
    } else {
        opts.start_level.clone()
    };
    sim.load_level(&start);
    let mut cmp = Comparer::new(segs.len(), opts.keep_vm_log);
    let mut events: BTreeMap<String, EventStats> = BTreeMap::new();
    let mut pending = Pending::default();
    let mut missing: Vec<(usize, String)> = Vec::new();
    let mut queries = BTreeMap::new();
    let mut calls = BTreeMap::new();
    let mut disagreements = Vec::new();

    for (si, seg) in segs.iter().enumerate() {
        log_host.set_segment(si);
        // What the log delivers, and what we deliver instead.
        let mut deliver: Option<(String, Vec<LogValue>)> = None;
        if si > 0
            && let Some((name, args)) = &seg.event
        {
            let st = events.entry(name.clone()).or_default();
            st.expected += 1;
            let texts: Vec<String> = args.iter().map(LogValue::key).collect();
            if classify(name) == EventClass::SimOwned && !seg.ignored {
                let key = format!("{name}({})", texts.join(", "));
                let uuid = uuid_key(name, &texts);
                // A dialog plays on until it raises the event.
                let inst = dialog_instance_of(name, &texts, |d| sim_instance_by_name(&sim, d));
                if let Some(inst) = inst
                    && !pending.contains(&key, &uuid)
                {
                    dialogs.pump(&mut sim, &mut pending, inst, &key, &uuid);
                }
                // A timer the sim has running fires when the log says it did.
                if matches!(name.as_str(), "TimerFinished" | "ObjectTimerFinished") {
                    force_timer(&mut sim, name, args);
                    for e in sim.take_events() {
                        pending.push(e);
                    }
                }
                match pending.take_match(&key, &uuid) {
                    Some((e, exact)) => {
                        if exact {
                            st.generated += 1;
                        } else {
                            st.generated_renamed += 1;
                        }
                        if let Some(i) = inst {
                            *dialogs.generated.entry(i).or_default() += 1;
                        }
                        let vals = e
                            .args
                            .iter()
                            .map(|a| LogValue::parse(&a.to_string()))
                            .collect();
                        deliver = Some((e.name.clone(), vals));
                    }
                    None => {
                        st.fed += 1;
                        if caused_outside(&sim, name, &texts) {
                            st.fed_outside += 1;
                        } else {
                            st.fed_gap += 1;
                        }
                        if missing.len() < 50_000 {
                            missing.push((si, annotate(&sim, name, &texts, key)));
                        }
                        observe(&mut sim, &vm, name, args);
                        deliver = Some((name.clone(), args.clone()));
                    }
                }
            } else {
                if classify(name) == EventClass::SimOwned {
                    // Logged as ignored (no rules use it): nothing to compare.
                    st.fed += 1;
                    st.fed_outside += 1;
                } else {
                    st.external += 1;
                }
                observe(&mut sim, &vm, name, args);
                deliver = Some((name.clone(), args.clone()));
            }
        }

        let mut host = HarnessHost {
            sim: &mut sim,
            log: &mut log_host,
            mode: opts.mode,
            segment: si,
            queries: std::mem::take(&mut queries),
            calls: std::mem::take(&mut calls),
            disagreements: std::mem::take(&mut disagreements),
        };
        let records = if si == 0 {
            vm.init_game(&mut host);
            vm.take_log()
        } else if let Some((name, args)) = deliver {
            if seg.ignored {
                ignored_event(&vm, &name, args.len())
            } else {
                let values = event_values(&mut vm, &name, &args);
                vm.event(&mut host, &name, &values);
                vm.take_log()
            }
        } else {
            Vec::new()
        };
        queries = host.queries;
        calls = host.calls;
        disagreements = host.disagreements;
        cmp.segment(si, seg, &records);
        for e in sim.take_events() {
            pending.push(e);
        }
    }

    let mut extras = Vec::new();
    for e in pending.remaining() {
        events.entry(e.name.clone()).or_default().extra += 1;
        if extras.len() < 50_000 {
            extras.push(e.key());
        }
    }
    let mut vm_report = cmp.finish(&vm);
    vm_report.host = log_host.stats;
    vm_report.unanswered = log_host.unanswered;
    SimReport {
        mode: opts.mode,
        vm: vm_report,
        queries,
        calls,
        events,
        disagreements,
        missing,
        extras,
        entities: sim.entities.len(),
        dialogs: dialogs.report(&sim),
        sim,
    }
}

/// Whether a sim-owned event the sim did not raise has a cause outside the sim's model: a
/// flag set by a dialog, or a tag/status on an object the sim did not load from the game data.
fn caused_outside(sim: &Sim, name: &str, texts: &[String]) -> bool {
    use crate::entity::Origin;
    use crate::guid::Guid;
    // A dialog event is the sim's to raise only for dialogs the sim runs.
    if let Some(inst) = dialog_instance_of(name, texts, |d| sim_instance_by_name(sim, d)) {
        return !sim.dialog(inst).is_some_and(|i| i.owned);
    }
    if SIM_OWNED_EVENTS[DIALOG_EVENTS_FROM..].contains(&name) {
        return true;
    }
    match name {
        "FlagSet" | "FlagCleared" => texts.get(2).is_some_and(|d| d != "0"),
        _ => texts
            .first()
            .and_then(|t| Guid::from_osiris(t))
            .and_then(|g| sim.entity(g))
            .is_none_or(|e| e.origin != Origin::Data),
    }
}

/// What the sim knows about a missing event's object (and tag), for the report.
fn annotate(sim: &Sim, name: &str, texts: &[String], key: String) -> String {
    use crate::guid::Guid;
    let Some(obj) = texts.first().and_then(|t| Guid::from_osiris(t)) else {
        return key;
    };
    let who = match sim.entity(obj) {
        Some(e) => format!("{:?} {:?}", e.origin, e.kind),
        None => "unknown object".to_owned(),
    };
    let tag = if matches!(name, "TagSet" | "TagCleared") {
        texts
            .get(1)
            .and_then(|t| Guid::from_osiris(t))
            .and_then(|t| sim.data.tags.get(t))
            .map_or(String::new(), |d| format!("; tag {}", d.name))
    } else {
        String::new()
    };
    format!("{key} [{who}{tag}]")
}

fn arg_text(v: &LogValue) -> String {
    match &v.v {
        LogData::Str(s) | LogData::Guid(s) => s.clone(),
        _ => v.key(),
    }
}

fn force_timer(sim: &mut Sim, name: &str, args: &[LogValue]) {
    match (name, args) {
        ("TimerFinished", [n]) => {
            sim.force_timer(&arg_text(n), crate::guid::Guid::NULL);
        }
        ("ObjectTimerFinished", [o, n]) => {
            if let Some(g) = crate::guid::Guid::from_osiris(&arg_text(o)) {
                sim.force_timer(&arg_text(n), g);
            }
        }
        _ => {}
    }
}

fn observe(sim: &mut Sim, vm: &Vm, name: &str, args: &[LogValue]) {
    let params = vm.function_params(name, args.len()).unwrap_or_default();
    let obs: Vec<ObservedArg> = args
        .iter()
        .enumerate()
        .map(|(i, a)| ObservedArg {
            ty: params
                .get(i)
                .and_then(|&t| vm.type_name(t))
                .unwrap_or("")
                .to_owned(),
            text: arg_text(a),
        })
        .collect();
    sim.observe(name, &obs);
}
