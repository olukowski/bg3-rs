//! Replays a runtime log of the original game (see [`crate::trace`]) through the VM and compares
//! what the VM does with what libOsiris logged.
//!
//! The log is cut into *segments*: segment 0 is `COsiris::InitGame`, then one segment per
//! `>>> call COsiris::Event`. The VM runs `init_game` and then each logged event in order; a
//! [`ReplayHost`] answers engine queries (and call results) from the log, matched by name and
//! input arguments, preferring answers logged in the same segment. The VM's own log records
//! are compared with the game's segment by segment.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use bg3_story::Story;

use crate::host::{Host, HostCtx, make_string};
use crate::trace::{ActionKind, Line, LogData, LogValue, Record, parse_log};
use crate::value::{Root, TypeId, Value, types};
use crate::vm::Vm;

/// One logged event and everything Osiris did for it.
#[derive(Clone, Debug)]
pub struct Segment {
    /// Event name and arguments (`None` for `InitGame`).
    pub event: Option<(String, Vec<LogValue>)>,
    /// `event X/N ignored (no rules defined using it)`.
    pub ignored: bool,
    pub records: Vec<Line>,
}

/// Cut a parsed log into segments.
pub fn segments(lines: Vec<Line>) -> Vec<Segment> {
    let mut out = vec![Segment {
        event: None,
        ignored: false,
        records: Vec::new(),
    }];
    for l in lines {
        match &l.record {
            Record::EventCall => out.push(Segment {
                event: None,
                ignored: false,
                records: Vec::new(),
            }),
            Record::Event { name, args } if out.len() > 1 => {
                let seg = out.last_mut().expect("non-empty");
                if seg.event.is_none() {
                    seg.event = Some((name.clone(), args.clone()));
                }
            }
            Record::EventIgnored { name, arity } if out.len() > 1 => {
                let seg = out.last_mut().expect("non-empty");
                seg.ignored = true;
                seg.event = Some((name.clone(), vec![LogValue::unset(); *arity as usize]));
            }
            _ => {}
        }
        out.last_mut().expect("non-empty").records.push(l);
    }
    out
}

/// Comparison category of a record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    /// Database inserts/deletes and rejected inserts.
    Facts,
    /// Rule firings (`RuleActionPart … fires`).
    Rules,
    /// Engine calls (`[DIV call]`).
    Calls,
    /// Queries asked (`exec [...] Q(...)`), with their results.
    Queries,
    /// PROC and `Sys*` calls.
    Procs,
    /// Goal INIT/EXIT sections and `GoalCompleted`.
    Goals,
}

impl Category {
    pub const ALL: [Self; 6] = [
        Self::Facts,
        Self::Rules,
        Self::Calls,
        Self::Queries,
        Self::Procs,
        Self::Goals,
    ];

    pub fn of(r: &Record) -> Option<Self> {
        Some(match r {
            Record::Action { kind, .. } => match kind {
                ActionKind::AddFact | ActionKind::DeleteFact => Self::Facts,
                ActionKind::DivCall => Self::Calls,
                ActionKind::Proc | ActionKind::InternalCall => Self::Procs,
            },
            Record::InsertFailed { .. } => Self::Facts,
            Record::CallFailed => Self::Calls,
            Record::RuleFires { .. } | Record::RuleEnd { .. } => Self::Rules,
            Record::QueryExec { .. } | Record::QueryFailed | Record::QueryReturns { .. } => {
                Self::Queries
            }
            Record::GoalSection { .. }
            | Record::GoalSectionEnd { .. }
            | Record::GoalCompleted { .. }
            | Record::GlobalInit => Self::Goals,
            _ => return None,
        })
    }

    /// Records counted for match rates (one per logical item).
    fn counted(r: &Record) -> bool {
        !matches!(
            r,
            Record::RuleEnd { .. }
                | Record::QueryFailed
                | Record::QueryReturns { .. }
                | Record::CallFailed
                | Record::GoalSectionEnd { .. }
        )
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Facts => "facts",
            Self::Rules => "rules",
            Self::Calls => "calls",
            Self::Queries => "queries",
            Self::Procs => "procs",
            Self::Goals => "goals",
        }
    }
}

/// A logged engine-query answer.
#[derive(Clone, Debug)]
pub struct Answer {
    /// Segment the answer was logged in.
    pub segment: usize,
    /// False if the log marks the query failed.
    pub ok: bool,
    /// Returned values, all parameters.
    pub values: Vec<LogValue>,
}

/// How a query was answered.
#[derive(Clone, Copy, Debug, Default)]
pub struct HostStats {
    /// Answered from the same segment.
    pub exact: u64,
    /// Answered from another segment (same name and inputs).
    pub borrowed: u64,
    /// Not in the log at all: failed.
    pub unanswered: u64,
    /// Calls whose result was taken from the log.
    pub calls: u64,
}

/// A [`Host`] that answers engine queries from a logged trace.
pub struct ReplayHost {
    answers: HashMap<String, Vec<Answer>>,
    used: HashSet<(String, usize)>,
    failed_calls: HashMap<String, Vec<usize>>,
    segment: usize,
    pub stats: HostStats,
    /// First few queries the log could not answer: `(segment, rendered query)`.
    pub unanswered: Vec<(usize, String)>,
}

fn query_key(name: &str, inputs: impl Iterator<Item = String>) -> String {
    let mut k = name.to_ascii_lowercase();
    k.push('(');
    for (i, a) in inputs.enumerate() {
        if i > 0 {
            k.push_str(", ");
        }
        k.push_str(&a);
    }
    k.push(')');
    k
}

impl ReplayHost {
    pub fn new(segments: &[Segment]) -> Self {
        let mut answers: HashMap<String, Vec<Answer>> = HashMap::new();
        let mut failed_calls: HashMap<String, Vec<usize>> = HashMap::new();
        for (si, seg) in segments.iter().enumerate() {
            let recs = &seg.records;
            for (i, l) in recs.iter().enumerate() {
                match &l.record {
                    Record::QueryExec { kind, name, args } if kind == "DIV query" => {
                        // Engine queries do not nest: the next records are the result.
                        let mut ok = true;
                        let mut values: Vec<LogValue> =
                            args.iter().map(|a| a.value.clone()).collect();
                        for n in recs.iter().skip(i + 1).take(2) {
                            match &n.record {
                                Record::QueryFailed => ok = false,
                                Record::QueryReturns { name: rn, args: ra } if rn == name => {
                                    values = ra.clone();
                                    break;
                                }
                                _ => break,
                            }
                        }
                        let key =
                            query_key(name, args.iter().filter(|a| !a.out).map(|a| a.value.key()));
                        answers.entry(key).or_default().push(Answer {
                            segment: si,
                            ok,
                            values,
                        });
                    }
                    Record::Action {
                        kind: ActionKind::DivCall,
                        name,
                        args,
                        ..
                    } if matches!(recs.get(i + 1).map(|n| &n.record), Some(Record::CallFailed)) => {
                        let key = query_key(name, args.iter().map(LogValue::key));
                        failed_calls.entry(key).or_default().push(si);
                    }
                    _ => {}
                }
            }
        }
        Self {
            answers,
            used: HashSet::new(),
            failed_calls,
            segment: 0,
            stats: HostStats::default(),
            unanswered: Vec::new(),
        }
    }

    pub fn set_segment(&mut self, s: usize) {
        self.segment = s;
    }

    fn find(&mut self, key: &str) -> Option<Answer> {
        let list = self.answers.get(key)?;
        let seg = self.segment;
        let mut best: Option<usize> = None;
        for (i, a) in list.iter().enumerate() {
            if a.segment == seg && !self.used.contains(&(key.to_owned(), i)) {
                self.used.insert((key.to_owned(), i));
                self.stats.exact += 1;
                return Some(a.clone());
            }
            // Otherwise the closest answer: the latest before this segment, else the first
            // after it.
            if a.segment < seg || best.is_none() {
                best = Some(i);
            }
        }
        self.stats.borrowed += 1;
        best.map(|i| list[i].clone())
    }
}

/// A logged value as a VM value of declared type `decl`.
pub fn to_value(vm: &mut Vm, lv: &LogValue, decl: TypeId) -> Value {
    let root = vm.prog.types.root(decl);
    match &lv.v {
        LogData::Unset => Value::undef(decl, root),
        LogData::Int(i) => match root {
            Root::Integer64 => Value::int64(*i),
            Root::Real => Value::real(*i as f32),
            _ => Value::int(*i as i32),
        },
        LogData::Real(f) => match root {
            Root::Integer => Value::int(*f as i32),
            Root::Integer64 => Value::int64(*f as i64),
            _ => Value::real(*f as f32),
        },
        LogData::Str(s) | LogData::Guid(s) => {
            // The engine hands over plain STRING/GUIDSTRING values (the log prints no alias).
            let (ty, root) = match (root, &lv.v) {
                (Root::String, _) => (types::STRING, Root::String),
                (Root::GuidString, _) => (types::GUIDSTRING, Root::GuidString),
                (_, LogData::Str(_)) => (types::STRING, Root::String),
                _ => (types::GUIDSTRING, Root::GuidString),
            };
            make_string(&mut vm.strings, ty, root, s)
        }
        LogData::Enum(t, l) => {
            let ty = vm
                .prog
                .type_names
                .iter()
                .position(|n| n == t)
                .map_or(decl, |i| i as TypeId);
            let raw = vm
                .prog
                .enums
                .get(&ty)
                .and_then(|e| e.iter().find(|(n, _)| n == l))
                .map_or(0, |(_, v)| *v);
            let root = vm.prog.types.root(ty);
            let mut v = match root {
                Root::Integer => Value::int(raw as i32),
                _ => Value::int64(raw as i64),
            };
            v.ty = ty;
            v
        }
    }
}

fn host_to_value(ctx: &mut HostCtx<'_>, lv: &LogValue, decl: &Value) -> Value {
    match &lv.v {
        LogData::Unset => *decl,
        LogData::Int(i) => match decl.root {
            Root::Integer64 => Value::int64(*i),
            Root::Real => Value::real(*i as f32),
            _ => Value {
                ty: decl.ty,
                ..Value::int(*i as i32)
            },
        },
        LogData::Real(f) => match decl.root {
            Root::Integer => Value::int(*f as i32),
            _ => Value::real(*f as f32),
        },
        LogData::Str(s) | LogData::Guid(s) => match decl.root {
            Root::String => ctx.string(s),
            _ => ctx.guid(s),
        },
        // Enumerations keep their numeric payload; the label is resolved by the caller's type.
        LogData::Enum(..) => *decl,
    }
}

impl ReplayHost {
    /// The logged answer for a query with these inputs (out-parameters ignored), preferring
    /// an unused one from the current segment; `None` if the log never asked it. Counts in
    /// [`HostStats`]. `Answer::segment` tells whether it came from the current segment.
    pub fn lookup(
        &mut self,
        ctx: &HostCtx<'_>,
        name: &str,
        args: &[Value],
        out: &[bool],
    ) -> Option<Answer> {
        let key = query_key(
            name,
            args.iter()
                .zip(out)
                .filter(|(_, o)| !**o)
                .map(|(a, _)| crate::vm::log_value_ctx(ctx, a).key()),
        );
        let ans = self.find(&key);
        if ans.is_none() {
            self.stats.unanswered += 1;
            if self.unanswered.len() < 50 {
                self.unanswered.push((self.segment, key));
            }
        }
        ans
    }

    /// Fill the out-parameters from a logged answer; returns the logged success.
    pub fn apply(ctx: &mut HostCtx<'_>, ans: &Answer, args: &mut [Value], out: &[bool]) -> bool {
        if ans.ok {
            for (i, a) in args.iter_mut().enumerate() {
                if out.get(i).copied().unwrap_or(false)
                    && let Some(v) = ans.values.get(i)
                {
                    *a = host_to_value(ctx, v, a);
                }
            }
        }
        ans.ok
    }
}

impl Host for ReplayHost {
    fn call(&mut self, ctx: &mut HostCtx<'_>, name: &str, args: &[Value]) -> bool {
        let key = query_key(
            name,
            args.iter().map(|a| crate::vm::log_value_ctx(ctx, a).key()),
        );
        match self.failed_calls.get(&key) {
            Some(segs) if segs.contains(&self.segment) => {
                self.stats.calls += 1;
                false
            }
            _ => true,
        }
    }

    fn query(
        &mut self,
        ctx: &mut HostCtx<'_>,
        name: &str,
        args: &mut [Value],
        out: &[bool],
    ) -> bool {
        match self.lookup(ctx, name, args, out) {
            Some(ans) => Self::apply(ctx, &ans, args, out),
            None => false,
        }
    }
}

/// Matched / total for one category.
#[derive(Clone, Copy, Debug, Default)]
pub struct Rate {
    pub trace: u64,
    pub vm: u64,
    /// Records in both, in order (longest common subsequence per segment).
    pub matched: u64,
}

impl Rate {
    pub fn percent(&self) -> f64 {
        if self.trace == 0 && self.vm == 0 {
            100.0
        } else {
            100.0 * self.matched as f64 / self.trace.max(self.vm) as f64
        }
    }
}

/// The first point where the VM and the log disagree.
#[derive(Clone, Debug)]
pub struct Divergence {
    pub segment: usize,
    /// Event of the segment (`None` = InitGame).
    pub event: Option<String>,
    /// Index into the segment's compared records.
    pub index: usize,
    /// Log line of the first differing trace record (or of the segment end).
    pub line: usize,
    pub trace: Option<String>,
    pub vm: Option<String>,
    /// A few compared records before and after, from both sides.
    pub trace_context: Vec<String>,
    pub vm_context: Vec<String>,
}

/// Result of a replay.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub segments: usize,
    pub segments_matched: usize,
    pub rates: Vec<(Category, Rate)>,
    pub first: Option<Divergence>,
    /// Every diverging segment: `(segment, event, first trace record, first vm record)`.
    pub diverging: Vec<(usize, String, Option<String>, Option<String>)>,
    pub databases: usize,
    pub databases_matched: usize,
    /// Databases whose fact history differs: `(name, trace ops, vm ops)`.
    pub databases_differing: Vec<(String, usize, usize)>,
    pub host: HostStats,
    pub unanswered: Vec<(usize, String)>,
    /// Records that match but print a different `(TYPE)` annotation somewhere (type ids of
    /// values differ while the values agree).
    pub type_mismatches: usize,
    /// First few `(log line, trace text, vm text)` type mismatches.
    pub type_mismatch_samples: Vec<(usize, String, String)>,
    /// Type mismatches per record kind.
    pub type_mismatch_kinds: Vec<(String, usize)>,
    pub vm_errors: usize,
    pub vm_error_samples: Vec<String>,
    /// Maximum `g_RuleNesting` printed in the log.
    pub max_depth_trace: u32,
    pub max_depth_vm: u32,
    /// The VM's own runtime log in libOsiris' format (only with [`replay_with`]`(.., true)`).
    pub vm_log: Option<String>,
}

impl Report {
    pub fn rate(&self, c: Category) -> Rate {
        self.rates
            .iter()
            .find(|(k, _)| *k == c)
            .map_or_else(Rate::default, |(_, r)| *r)
    }

    pub fn divergences(&self) -> usize {
        self.diverging.len()
    }

    /// Human-readable summary.
    pub fn summary(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(
            s,
            "segments: {} ({} matched, {} diverging)",
            self.segments,
            self.segments_matched,
            self.diverging.len()
        );
        for (c, r) in &self.rates {
            let _ = writeln!(
                s,
                "  {:<8} trace {:>7}  vm {:>7}  matched {:>7}  ({:.2}%)",
                c.name(),
                r.trace,
                r.vm,
                r.matched,
                r.percent()
            );
        }
        let _ = writeln!(
            s,
            "databases touched: {} ({} with identical fact history)",
            self.databases, self.databases_matched
        );
        for (n, t, v) in self.databases_differing.iter().take(10) {
            let _ = writeln!(s, "  differs: {n} (trace {t} ops, vm {v} ops)");
        }
        let _ = writeln!(
            s,
            "engine queries: {} answered from the same event, {} borrowed, {} not in the log; {} failed calls replayed",
            self.host.exact, self.host.borrowed, self.host.unanswered, self.host.calls
        );
        for (seg, q) in self.unanswered.iter().take(10) {
            let _ = writeln!(s, "  unanswered in segment {seg}: {q}");
        }
        let _ = writeln!(
            s,
            "max rule nesting: trace {}, vm {}",
            self.max_depth_trace, self.max_depth_vm
        );
        let _ = writeln!(
            s,
            "type annotations: {} matching records print a different (TYPE) prefix",
            self.type_mismatches
        );
        for (k, n) in &self.type_mismatch_kinds {
            let _ = writeln!(s, "  {k}: {n}");
        }
        for (l, a, b) in self.type_mismatch_samples.iter().take(20) {
            let _ = writeln!(s, "  line {l}:\n    trace: {a}\n    vm:    {b}");
        }
        let _ = writeln!(s, "VM errors: {}", self.vm_errors);
        for e in self.vm_error_samples.iter().take(10) {
            let _ = writeln!(s, "  {e}");
        }
        if let Some(d) = &self.first {
            let _ = writeln!(
                s,
                "first divergence: segment {} ({}), record {}, log line {}",
                d.segment,
                d.event.as_deref().unwrap_or("InitGame"),
                d.index,
                d.line
            );
            let _ = writeln!(s, "  trace: {}", d.trace.as_deref().unwrap_or("<end>"));
            let _ = writeln!(s, "  vm:    {}", d.vm.as_deref().unwrap_or("<end>"));
            let _ = writeln!(s, "  trace context:");
            for c in &d.trace_context {
                let _ = writeln!(s, "    {c}");
            }
            let _ = writeln!(s, "  vm context:");
            for c in &d.vm_context {
                let _ = writeln!(s, "    {c}");
            }
        }
        if self.diverging.len() > 1 {
            let _ = writeln!(s, "diverging segments (first 20):");
            for (seg, ev, t, v) in self.diverging.iter().take(20) {
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

fn record_kind(r: &Record) -> &'static str {
    match r {
        Record::Action { kind, .. } => kind.suffix(),
        Record::RuleFires { .. } => "rule variables",
        Record::QueryExec { .. } => "query exec",
        Record::QueryReturns { .. } => "query returns",
        Record::InsertFailed { .. } => "insert failed",
        _ => "other",
    }
}

/// Records that take part in the comparison.
fn compared(r: &Record) -> bool {
    Category::of(r).is_some()
}

/// Length of the longest common subsequence (prefix/suffix trimmed; falls back to a multiset
/// intersection for very long differing middles).
fn lcs(a: &[&str], b: &[&str]) -> usize {
    let pre = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let (a, b) = (&a[pre..], &b[pre..]);
    let suf = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (a, b) = (&a[..a.len() - suf], &b[..b.len() - suf]);
    if a.is_empty() || b.is_empty() {
        return pre + suf;
    }
    if a.len() * b.len() > 4_000_000 {
        let mut counts: HashMap<&str, i64> = HashMap::new();
        for x in a {
            *counts.entry(x).or_default() += 1;
        }
        let mut m = 0;
        for y in b {
            if let Some(c) = counts.get_mut(y)
                && *c > 0
            {
                *c -= 1;
                m += 1;
            }
        }
        return pre + suf + m;
    }
    let mut prev = vec![0u32; b.len() + 1];
    let mut cur = vec![0u32; b.len() + 1];
    for x in a {
        for (j, y) in b.iter().enumerate() {
            cur[j + 1] = if x == y {
                prev[j] + 1
            } else {
                cur[j].max(prev[j + 1])
            };
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    pre + suf + prev[b.len()] as usize
}

/// Run the VM over a logged trace and compare. Needs a deep stack for big stories (rule
/// chains recurse); run it on a thread with a large stack.
pub fn replay(story: &Story, log: &str) -> Report {
    replay_with(story, log, false)
}

/// [`replay`], optionally keeping the VM's log text (for diffing against the game's).
pub fn replay_with(story: &Story, log: &str, keep_vm_log: bool) -> Report {
    let segs = segments(parse_log(log));
    let mut host = ReplayHost::new(&segs);
    let mut vm = Vm::new(story);
    vm.set_logging(true);
    let mut cmp = Comparer::new(segs.len(), keep_vm_log);
    for (si, seg) in segs.iter().enumerate() {
        host.set_segment(si);
        let records = run_segment(&mut vm, &mut host, seg, si);
        cmp.segment(si, seg, &records);
    }
    let mut report = cmp.finish(&vm);
    report.host = host.stats;
    report.unanswered = host.unanswered;
    report
}

/// The logged arguments of an event as VM values of the declared parameter types.
pub fn event_values(vm: &mut Vm, name: &str, args: &[LogValue]) -> Vec<Value> {
    let params = vm.function_params(name, args.len()).unwrap_or_default();
    args.iter()
        .enumerate()
        .map(|(i, a)| {
            let decl = params.get(i).copied().unwrap_or(0);
            to_value(vm, a, decl)
        })
        .collect()
}

/// Run one segment's event (segment 0: `InitGame`) with `host` and return the VM's records.
/// For an event the log marks as ignored, the VM only checks that it has no rules for it.
pub fn run_segment(vm: &mut Vm, host: &mut dyn Host, seg: &Segment, index: usize) -> Vec<Record> {
    if index == 0 {
        vm.init_game(host);
        return vm.take_log();
    }
    let Some((name, args)) = &seg.event else {
        return Vec::new();
    };
    if seg.ignored {
        return ignored_event(vm, name, args.len());
    }
    let values = event_values(vm, name, args);
    vm.event(host, name, &values);
    vm.take_log()
}

/// The records for an event the game logged as ignored (no rules use it): the VM only checks
/// that it has no rules for it either.
pub fn ignored_event(vm: &Vm, name: &str, arity: usize) -> Vec<Record> {
    let mut out = vec![Record::EventCall];
    if vm.event_has_rules(name, arity) != Some(false) {
        out.push(Record::Other(format!(
            "VM has rules for ignored event {name}/{arity}"
        )));
    }
    out
}

/// Compares the VM's records with the log, segment by segment, and builds the [`Report`].
pub struct Comparer {
    report: Report,
    rates: HashMap<Category, Rate>,
    db_trace: HashMap<String, Vec<String>>,
    db_vm: HashMap<String, Vec<String>>,
    vm_text: Option<String>,
}

impl Comparer {
    pub fn new(segments: usize, keep_vm_log: bool) -> Self {
        Self {
            report: Report {
                segments,
                ..Report::default()
            },
            rates: HashMap::new(),
            db_trace: HashMap::new(),
            db_vm: HashMap::new(),
            vm_text: keep_vm_log.then(String::new),
        }
    }

    /// Compare one segment; returns whether it matched.
    pub fn segment(&mut self, si: usize, seg: &Segment, vm_records: &[Record]) -> bool {
        let report = &mut self.report;
        if let Some(text) = self.vm_text.as_mut() {
            text.push_str(&crate::trace::render_log(vm_records));
        }
        let t: Vec<&Line> = seg.records.iter().filter(|l| compared(&l.record)).collect();
        let v: Vec<&Record> = vm_records.iter().filter(|r| compared(r)).collect();
        for l in &t {
            if let Some(d) = l.record.depth() {
                report.max_depth_trace = report.max_depth_trace.max(d);
            }
        }
        for r in &v {
            if let Some(d) = r.depth() {
                report.max_depth_vm = report.max_depth_vm.max(d);
            }
        }
        let tk: Vec<String> = t.iter().map(|l| l.record.key()).collect();
        let vk: Vec<String> = v.iter().map(|r| r.key()).collect();
        let ignored_mismatch = vm_records.iter().any(|r| matches!(r, Record::Other(_)));
        let matched = tk == vk && !ignored_mismatch;

        if matched {
            report.segments_matched += 1;
            for (l, r) in t.iter().zip(&v) {
                let (a, b) = (l.record.to_string(), r.to_string());
                if a != b {
                    report.type_mismatches += 1;
                    let kind = record_kind(&l.record);
                    match report
                        .type_mismatch_kinds
                        .iter_mut()
                        .find(|(k, _)| *k == kind)
                    {
                        Some((_, n)) => *n += 1,
                        None => report.type_mismatch_kinds.push((kind.to_owned(), 1)),
                    }
                    if report
                        .type_mismatch_kinds
                        .iter()
                        .find(|(k, _)| *k == kind)
                        .map_or(0, |x| x.1)
                        <= 2
                    {
                        report.type_mismatch_samples.push((l.line, a, b));
                    }
                }
            }
        } else {
            let i = tk.iter().zip(&vk).take_while(|(a, b)| a == b).count();
            let event = seg.event.as_ref().map(|(n, _)| n.clone());
            report.diverging.push((
                si,
                event.clone().unwrap_or_else(|| "InitGame".to_owned()),
                tk.get(i).cloned(),
                vk.get(i)
                    .cloned()
                    .or_else(|| ignored_mismatch.then(|| "rules exist".to_owned())),
            ));
            if report.first.is_none() {
                let lo = i.saturating_sub(8);
                report.first = Some(Divergence {
                    segment: si,
                    event,
                    index: i,
                    line: t
                        .get(i)
                        .or(t.last())
                        .map_or_else(|| seg.records.first().map_or(0, |l| l.line), |l| l.line),
                    trace: t.get(i).map(|l| l.record.to_string()),
                    vm: v.get(i).map(|r| r.to_string()),
                    trace_context: t[lo..(i + 6).min(t.len())]
                        .iter()
                        .map(|l| format!("{:>8}: {}", l.line, l.record))
                        .collect(),
                    vm_context: v[lo.min(v.len())..(i + 6).min(v.len())]
                        .iter()
                        .map(|r| r.to_string())
                        .collect(),
                });
            }
        }

        // Per-category in-order match counts.
        for c in Category::ALL {
            let tc: Vec<&str> = t
                .iter()
                .zip(&tk)
                .filter(|(l, _)| Category::of(&l.record) == Some(c) && Category::counted(&l.record))
                .map(|(_, k)| k.as_str())
                .collect();
            let vc: Vec<&str> = v
                .iter()
                .zip(&vk)
                .filter(|(r, _)| Category::of(r) == Some(c) && Category::counted(r))
                .map(|(_, k)| k.as_str())
                .collect();
            let rate = self.rates.entry(c).or_default();
            rate.trace += tc.len() as u64;
            rate.vm += vc.len() as u64;
            rate.matched += lcs(&tc, &vc) as u64;
        }

        // Per-database fact history.
        let mut last_db: Option<String> = None;
        for (r, side) in t
            .iter()
            .map(|l| (&l.record, true))
            .chain(v.iter().map(|r| (*r, false)))
        {
            let map = if side {
                &mut self.db_trace
            } else {
                &mut self.db_vm
            };
            match r {
                Record::Action {
                    kind: kind @ (ActionKind::AddFact | ActionKind::DeleteFact),
                    name,
                    args,
                    ..
                } => {
                    last_db = Some(name.clone());
                    map.entry(name.clone()).or_default().push(format!(
                        "{}{}",
                        if *kind == ActionKind::DeleteFact {
                            "-"
                        } else {
                            "+"
                        },
                        args.iter()
                            .map(LogValue::key)
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                Record::InsertFailed { start: true, .. } => {
                    if let Some(db) = &last_db
                        && let Some(ops) = map.get_mut(db)
                        && let Some(op) = ops.last_mut()
                    {
                        op.push_str(" (known)");
                    }
                }
                _ => {}
            }
        }
        matched
    }

    /// The report (host statistics are left to the caller).
    pub fn finish(self, vm: &Vm) -> Report {
        let mut report = self.report;
        let mut names: Vec<&String> = self.db_trace.keys().chain(self.db_vm.keys()).collect();
        names.sort();
        names.dedup();
        report.databases = names.len();
        for n in names {
            let (a, b) = (self.db_trace.get(n), self.db_vm.get(n));
            if a == b {
                report.databases_matched += 1;
            } else {
                report.databases_differing.push((
                    n.clone(),
                    a.map_or(0, Vec::len),
                    b.map_or(0, Vec::len),
                ));
            }
        }
        report
            .databases_differing
            .sort_by_key(|(_, a, b)| std::cmp::Reverse(a.abs_diff(*b)));
        let mut rates: Vec<(Category, Rate)> = self.rates.into_iter().collect();
        rates.sort_by_key(|(c, _)| *c);
        report.rates = rates;
        report.vm_log = self.vm_text;
        let (errs, n) = vm.errors();
        report.vm_errors = n;
        report.vm_error_samples = errs.iter().take(20).map(|e| e.0.clone()).collect();
        report
    }
}
