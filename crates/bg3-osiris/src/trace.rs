//! libOsiris' built-in runtime log ("### Run time log"): a parser, and the record model the VM
//! uses to write the same log.
//!
//! The log is written by libOsiris when `COsiris::m_LogEnabled` is set and `_ParserFlags` bit 19
//! (0x80000) is on; bit 24 adds rejected inserts with a database dump. The format is described
//! in `docs/research/osiris-runtime-log.md`. Every line maps to one [`Record`]; multi-line
//! database dumps become one record.
//!
//! Values are kept as [`LogValue`]s: the log loses most type information (alias types are
//! printed as a `(TYPE)` prefix only in some places), so comparisons use [`LogValue::key`],
//! which ignores the prefix.

use std::fmt;

/// A value as printed in the log.
#[derive(Clone, Debug, PartialEq)]
pub struct LogValue {
    /// `(TYPE)` prefix, if printed.
    pub ty: Option<String>,
    pub v: LogData,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LogData {
    /// `(unknown)`, `(undef value)`, or a bare `(TYPE)`: no value.
    Unset,
    Int(i64),
    /// Printed with one decimal (`%.1f`).
    Real(f64),
    /// `"text"`.
    Str(String),
    /// Bare text (GUIDSTRING and aliases of it).
    Guid(String),
    /// `TYPE.LABEL` (enumerations).
    Enum(String, String),
}

impl LogValue {
    pub fn unset() -> Self {
        Self {
            ty: None,
            v: LogData::Unset,
        }
    }

    /// Normalised text for comparisons: no type prefix, every kind of unset value is `_`.
    pub fn key(&self) -> String {
        match &self.v {
            LogData::Unset => "_".to_owned(),
            LogData::Int(i) => i.to_string(),
            LogData::Real(f) => format!("{f:.1}"),
            LogData::Str(s) => format!("\"{s}\""),
            LogData::Guid(s) => s.clone(),
            LogData::Enum(t, l) => format!("{t}.{l}"),
        }
    }

    /// Parse one printed value.
    pub fn parse(s: &str) -> Self {
        let s = s.trim();
        if s == "(unknown)" || s == "(undef value)" {
            return Self::unset();
        }
        if let Some(rest) = s.strip_prefix('(')
            && !s.starts_with("(\"")
            && let Some(end) = rest.find(')')
        {
            let name = &rest[..end];
            if !name.is_empty()
                && name
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
            {
                let tail = &rest[end + 1..];
                let mut v = if tail.is_empty() {
                    Self::unset()
                } else {
                    Self::parse(tail)
                };
                v.ty = Some(name.to_owned());
                return v;
            }
        }
        let v = if let Some(inner) = s.strip_prefix('"').and_then(|t| t.strip_suffix('"')) {
            LogData::Str(inner.to_owned())
        } else if let Ok(i) = s.parse::<i64>() {
            LogData::Int(i)
        } else if is_real(s) {
            LogData::Real(s.parse().unwrap_or(0.0))
        } else if let Some((t, l)) = enum_parts(s) {
            LogData::Enum(t.to_owned(), l.to_owned())
        } else {
            LogData::Guid(s.to_owned())
        };
        Self { ty: None, v }
    }
}

fn is_real(s: &str) -> bool {
    let t = s.strip_prefix('-').unwrap_or(s);
    let Some((a, b)) = t.split_once('.') else {
        return false;
    };
    !a.is_empty()
        && !b.is_empty()
        && a.bytes().all(|c| c.is_ascii_digit())
        && b.bytes().all(|c| c.is_ascii_digit())
        || t == "inf"
        || t == "nan"
}

fn enum_parts(s: &str) -> Option<(&str, &str)> {
    let (t, l) = s.split_once('.')?;
    let ok_t = !t.is_empty()
        && t.as_bytes()[0].is_ascii_uppercase()
        && t.bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
    let ok_l = !l.is_empty() && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    (ok_t && ok_l).then_some((t, l))
}

impl fmt::Display for LogValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.ty, &self.v) {
            (None, LogData::Unset) => return f.write_str("(unknown)"),
            (Some(t), LogData::Unset) => return write!(f, "({t})"),
            (Some(t), _) => write!(f, "({t})")?,
            (None, _) => {}
        }
        match &self.v {
            LogData::Unset => Ok(()),
            LogData::Int(i) => write!(f, "{i}"),
            LogData::Real(r) => write!(f, "{r:.1}"),
            LogData::Str(s) => write!(f, "\"{s}\""),
            LogData::Guid(s) => f.write_str(s),
            LogData::Enum(t, l) => write!(f, "{t}.{l}"),
        }
    }
}

/// What a `N> Name( args ) [kind]` action line did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ActionKind {
    /// `[add fact]`: database insert.
    AddFact,
    /// `[delete fact]`: `NOT DB(...)`.
    DeleteFact,
    /// `[Osiris procedure call]`: a PROC.
    Proc,
    /// `[DIV call]`: an engine call.
    DivCall,
    /// `[Osiris internal call]`: `Sys*` built-ins.
    InternalCall,
}

impl ActionKind {
    pub fn suffix(self) -> &'static str {
        match self {
            Self::AddFact => "[add fact]",
            Self::DeleteFact => "[delete fact]",
            Self::Proc => "[Osiris procedure call]",
            Self::DivCall => "[DIV call]",
            Self::InternalCall => "[Osiris internal call]",
        }
    }

    fn from_suffix(s: &str) -> Option<Self> {
        Some(match s {
            "[add fact]" => Self::AddFact,
            "[delete fact]" => Self::DeleteFact,
            "[Osiris procedure call]" => Self::Proc,
            "[DIV call]" => Self::DivCall,
            "[Osiris internal call]" => Self::InternalCall,
            _ => return None,
        })
    }
}

/// An argument of a query line: `[out]`-flagged or not.
#[derive(Clone, Debug, PartialEq)]
pub struct QueryArg {
    pub out: bool,
    pub value: LogValue,
}

/// One log record.
#[derive(Clone, Debug, PartialEq)]
pub enum Record {
    /// `### Run time log:` banner.
    Banner,
    /// `exec global INIT section:` (start of `COsiris::InitGame`).
    GlobalInit,
    /// `exec INIT|EXIT section of goal …:`. `woken` = the quoted form written by
    /// `COsiGoal::EnableRules` / `SetCompleted` (which also write an `END` line); the unquoted
    /// form is the `InitGame` walk.
    GoalSection {
        exit: bool,
        goal: String,
        id: u32,
        woken: bool,
    },
    /// `exec INIT|EXIT section of goal "…" (#n) END`.
    GoalSectionEnd { exit: bool, goal: String, id: u32 },
    /// `N> GoalCompleted; ("Goal") (GoalID == n)`.
    GoalCompleted { depth: u32, goal: String, id: u32 },
    /// `N> Name( args ) [kind]`.
    Action {
        depth: u32,
        kind: ActionKind,
        name: String,
        args: Vec<LogValue>,
    },
    /// `## insert (add fact) failed: tuple: …` (`start` = a database start node, i.e. an action
    /// inserted a known fact) or `## insert failed: tuple: …` (a materialised join already held
    /// the tuple).
    InsertFailed { start: bool, tuple: Vec<LogValue> },
    /// `*** call failed! ***` after a `[DIV call]`.
    CallFailed,
    /// `>>> call COsiris::Event`.
    EventCall,
    /// `>>> event Name(args)`.
    Event { name: String, args: Vec<LogValue> },
    /// `event Name/N ignored (no rules defined using it)`.
    EventIgnored { name: String, arity: u32 },
    /// `<<< event ignored: …` (an argument without a type).
    EventRejected(String),
    /// `N> RuleActionPart of THEN at line L fires. Rule variables: …`.
    RuleFires {
        depth: u32,
        line: u32,
        vars: Vec<LogValue>,
    },
    /// `N> [end of rule action part]`.
    RuleEnd { depth: u32 },
    /// `exec [DIV query] Name( args )` (kind = text in brackets).
    QueryExec {
        kind: String,
        name: String,
        args: Vec<QueryArg>,
    },
    /// `*** Query failed! ***`.
    QueryFailed,
    /// `     Query returns: Name( args )` (not written for queries without parameters).
    QueryReturns { name: String, args: Vec<LogValue> },
    /// `DBase contents of Name( (T)_, … )` plus its rows.
    DbContents {
        name: String,
        rows: Vec<Vec<LogValue>>,
    },
    /// Join/valid diagnostics (`*** Multiple match ***`, `*** Match n: …`, `## (node n) …`).
    Diagnostic(String),
    /// Anything else (kept verbatim).
    Other(String),
}

impl Record {
    /// Nesting depth (`g_RuleNesting`) for records that print one.
    pub fn depth(&self) -> Option<u32> {
        match self {
            Self::GoalCompleted { depth, .. }
            | Self::Action { depth, .. }
            | Self::RuleFires { depth, .. }
            | Self::RuleEnd { depth } => Some(*depth),
            _ => None,
        }
    }

    /// Normalised one-line form used to compare the VM's log with the game's (type prefixes
    /// dropped, unset values unified).
    pub fn key(&self) -> String {
        let vals = |v: &[LogValue]| v.iter().map(LogValue::key).collect::<Vec<_>>().join(", ");
        match self {
            Self::GoalSection {
                exit, goal, woken, ..
            } => format!(
                "{} {goal}{}",
                if *exit { "EXIT" } else { "INIT" },
                if *woken { " (woken)" } else { "" }
            ),
            Self::GoalSectionEnd { exit, goal, .. } => {
                format!("{} {goal} END", if *exit { "EXIT" } else { "INIT" })
            }
            Self::GoalCompleted { depth, goal, .. } => format!("{depth}> GoalCompleted {goal}"),
            Self::Action {
                depth,
                kind,
                name,
                args,
            } => format!("{depth}> {name}({}) {}", vals(args), kind.suffix()),
            Self::InsertFailed { start, tuple } => format!(
                "insert{} failed: {}",
                if *start { " (add fact)" } else { "" },
                vals(tuple)
            ),
            Self::Event { name, args } => format!("event {name}({})", vals(args)),
            Self::EventIgnored { name, arity } => format!("event {name}/{arity} ignored"),
            Self::RuleFires { depth, line, vars } => {
                format!("{depth}> rule@{line} fires: {}", vals(vars))
            }
            Self::RuleEnd { depth } => format!("{depth}> end rule"),
            Self::QueryExec { kind, name, args } => format!(
                "query [{kind}] {name}({})",
                args.iter()
                    .map(|a| if a.out {
                        format!("[out] {}", a.value.key())
                    } else {
                        a.value.key()
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::QueryReturns { name, args } => format!("returns {name}({})", vals(args)),
            other => other.to_string(),
        }
    }
}

fn join_values(f: &mut fmt::Formatter<'_>, v: &[LogValue], sep: &str) -> fmt::Result {
    for (i, x) in v.iter().enumerate() {
        if i > 0 {
            f.write_str(sep)?;
        }
        write!(f, "{x}")?;
    }
    Ok(())
}

fn depth_prefix(depth: u32) -> String {
    format!("{depth}{}>", "--".repeat(depth as usize))
}

/// Renders a record the way libOsiris writes it (one or more lines, no trailing newline).
impl fmt::Display for Record {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Banner => f.write_str("### Run time log:"),
            Self::GlobalInit => f.write_str("exec global INIT section:"),
            Self::GoalSection {
                exit,
                goal,
                id,
                woken,
            } => {
                let sec = if *exit { "EXIT" } else { "INIT" };
                if *woken {
                    write!(f, "exec {sec} section of goal \"{goal}\" (#{id}):")
                } else {
                    write!(f, "exec {sec} section of goal {goal} (#{id}):")
                }
            }
            Self::GoalSectionEnd { exit, goal, id } => {
                let sec = if *exit { "EXIT" } else { "INIT" };
                write!(f, "exec {sec} section of goal \"{goal}\" (#{id}) END")
            }
            Self::GoalCompleted { depth, goal, id } => write!(
                f,
                "{} GoalCompleted; (\"{goal}\") (GoalID == {id})",
                depth_prefix(*depth)
            ),
            Self::Action {
                depth,
                kind,
                name,
                args,
            } => {
                write!(f, "{} {name}( ", depth_prefix(*depth))?;
                join_values(f, args, ", ")?;
                write!(f, " ) {}", kind.suffix())
            }
            Self::InsertFailed { start, tuple } => {
                if *start {
                    f.write_str("## insert (add fact) failed: tuple: ")?;
                } else {
                    f.write_str("## insert failed: tuple: ")?;
                }
                join_values(f, tuple, ", ")
            }
            Self::CallFailed => f.write_str("*** call failed! ***"),
            Self::EventCall => f.write_str(">>> call COsiris::Event"),
            Self::Event { name, args } => {
                write!(f, ">>> event {name}(")?;
                join_values(f, args, ", ")?;
                f.write_str(")")
            }
            Self::EventIgnored { name, arity } => {
                write!(
                    f,
                    "event {name}/{arity} ignored (no rules defined using it)"
                )
            }
            Self::EventRejected(s) => write!(f, "<<< event ignored: {s}"),
            Self::RuleFires { depth, line, vars } => {
                write!(
                    f,
                    "{} RuleActionPart of THEN at line {line} fires. Rule variables: ",
                    depth_prefix(*depth)
                )?;
                join_values(f, vars, ", ")
            }
            Self::RuleEnd { depth } => {
                write!(f, "{} [end of rule action part]", depth_prefix(*depth))
            }
            Self::QueryExec { kind, name, args } => {
                write!(f, "exec [{kind}] {name}( ")?;
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    if a.out {
                        f.write_str("[out] ")?;
                    }
                    write!(f, "{}", a.value)?;
                }
                f.write_str(" )")
            }
            Self::QueryFailed => f.write_str("*** Query failed! ***"),
            Self::QueryReturns { name, args } => {
                write!(f, "     Query returns: {name}( ")?;
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    if a.v == LogData::Unset {
                        f.write_str("(undef value)")?;
                    } else {
                        write!(f, "{a}")?;
                    }
                }
                f.write_str(" )")
            }
            Self::DbContents { name, rows } => {
                write!(f, "DBase contents of {name}")?;
                for r in rows {
                    f.write_str("\n")?;
                    join_values(f, r, ", ")?;
                }
                Ok(())
            }
            Self::Diagnostic(s) | Self::Other(s) => f.write_str(s),
        }
    }
}

/// A record and the (1-based) line it starts on.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub line: usize,
    pub record: Record,
}

/// Split a printed argument list on top-level `", "` (quotes respected).
pub fn split_args(s: &str) -> Vec<&str> {
    let s = s.trim();
    if s.is_empty() {
        return Vec::new();
    }
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut start = 0;
    let mut in_quote = false;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'"' => {
                // A quote closes a string only if followed by a separator or the end.
                if in_quote {
                    let rest = &s[i + 1..];
                    if rest.is_empty() || rest.starts_with(", ") {
                        in_quote = false;
                    }
                } else if i == start || s[start..i].ends_with(')') || s[start..i].ends_with(' ') {
                    in_quote = true;
                }
            }
            b',' if !in_quote && b.get(i + 1) == Some(&b' ') => {
                out.push(s[start..i].trim());
                start = i + 2;
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    out.push(s[start..].trim());
    out
}

fn parse_values(s: &str) -> Vec<LogValue> {
    split_args(s).into_iter().map(LogValue::parse).collect()
}

/// `Name( a, b ) tail` → (`Name`, `a, b`, `tail`). The argument list ends at the last `)`
/// before `tail_hint` (or the end).
fn call_parts<'a>(s: &'a str, tail_hint: Option<&str>) -> Option<(&'a str, &'a str, &'a str)> {
    let open = s.find('(')?;
    let name = s[..open].trim();
    let body_end = match tail_hint {
        Some(t) => s.rfind(t)?,
        None => s.len(),
    };
    let close = s[..body_end].rfind(')')?;
    if close < open {
        return None;
    }
    Some((name, &s[open + 1..close], &s[body_end..]))
}

/// `12-----> rest` → (12, rest).
fn split_depth(l: &str) -> Option<(u32, &str)> {
    let digits = l.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let rest = l[digits..].trim_start_matches('-');
    let rest = rest.strip_prefix("> ").or_else(|| rest.strip_prefix('>'))?;
    Some((l[..digits].parse().ok()?, rest))
}

fn parse_goal_section(rest: &str, exit: bool) -> Option<Record> {
    // rest: `goal NAME (#n):` | `goal "NAME" (#n):` | `goal "NAME" (#n) END`
    let rest = rest.strip_prefix("goal ")?;
    let hash = rest.rfind("(#")?;
    let close = hash + rest[hash..].find(')')?;
    let id: u32 = rest[hash + 2..close].parse().ok()?;
    let raw_name = rest[..hash].trim();
    let tail = rest[close + 1..].trim();
    let (goal, quoted) = match raw_name.strip_prefix('"').and_then(|n| n.strip_suffix('"')) {
        Some(n) => (n.to_owned(), true),
        None => (raw_name.to_owned(), false),
    };
    if tail == "END" {
        Some(Record::GoalSectionEnd { exit, goal, id })
    } else {
        Some(Record::GoalSection {
            exit,
            goal,
            id,
            woken: quoted,
        })
    }
}

fn parse_line(l: &str) -> Record {
    if let Some((depth, rest)) = split_depth(l) {
        if let Some(r) = rest.strip_prefix("RuleActionPart of THEN at line ") {
            let (num, vars) = r.split_once(" fires. Rule variables:").unwrap_or((r, ""));
            return Record::RuleFires {
                depth,
                line: num.trim().parse().unwrap_or(0),
                vars: parse_values(vars),
            };
        }
        if rest.trim() == "[end of rule action part]" {
            return Record::RuleEnd { depth };
        }
        if let Some(r) = rest.strip_prefix("GoalCompleted; (\"") {
            let (goal, tail) = r.split_once("\")").unwrap_or((r, ""));
            let id = tail
                .split("GoalID ==")
                .nth(1)
                .and_then(|t| t.trim().trim_end_matches(')').trim().parse().ok())
                .unwrap_or(0);
            return Record::GoalCompleted {
                depth,
                goal: goal.to_owned(),
                id,
            };
        }
        if let Some(open) = rest.rfind(" [")
            && let Some(kind) = ActionKind::from_suffix(rest[open + 1..].trim_end())
            && let Some((name, args, _)) = call_parts(&rest[..open], None)
        {
            return Record::Action {
                depth,
                kind,
                name: name.to_owned(),
                args: parse_values(args),
            };
        }
        return Record::Other(l.to_owned());
    }
    if let Some(r) = l.strip_prefix(">>> event ") {
        if let Some((name, args, _)) = call_parts(r, None) {
            return Record::Event {
                name: name.to_owned(),
                args: parse_values(args),
            };
        }
        return Record::Other(l.to_owned());
    }
    if l == ">>> call COsiris::Event" {
        return Record::EventCall;
    }
    if let Some(r) = l.strip_prefix("<<< event ignored: ") {
        return Record::EventRejected(r.to_owned());
    }
    if let Some(r) = l.strip_prefix("event ")
        && let Some(r) = r.strip_suffix(" ignored (no rules defined using it)")
        && let Some((name, arity)) = r.rsplit_once('/')
    {
        return Record::EventIgnored {
            name: name.to_owned(),
            arity: arity.parse().unwrap_or(0),
        };
    }
    if l == "exec global INIT section:" {
        return Record::GlobalInit;
    }
    if let Some(r) = l.strip_prefix("exec INIT section of ")
        && let Some(rec) = parse_goal_section(r, false)
    {
        return rec;
    }
    if let Some(r) = l.strip_prefix("exec EXIT section of ")
        && let Some(rec) = parse_goal_section(r, true)
    {
        return rec;
    }
    if let Some(r) = l.strip_prefix("exec [")
        && let Some((kind, call)) = r.split_once("] ")
        && let Some((name, args, _)) = call_parts(call, None)
    {
        let args = split_args(args)
            .into_iter()
            .map(|a| {
                let (out, v) = match a.strip_prefix("[out] ") {
                    Some(v) => (true, v),
                    None => (false, a.strip_prefix("[in] ").unwrap_or(a)),
                };
                QueryArg {
                    out,
                    value: LogValue::parse(v),
                }
            })
            .collect();
        return Record::QueryExec {
            kind: kind.to_owned(),
            name: name.to_owned(),
            args,
        };
    }
    if l.trim() == "*** Query failed! ***" {
        return Record::QueryFailed;
    }
    // Written through the error channel, which may prefix `line N: error: `.
    if l.trim_end().ends_with("*** call failed! ***") {
        return Record::CallFailed;
    }
    if let Some(r) = l.trim_start().strip_prefix("Query returns: ")
        && let Some((name, args, _)) = call_parts(r, None)
    {
        return Record::QueryReturns {
            name: name.to_owned(),
            args: parse_values(args),
        };
    }
    if let Some(r) = l.strip_prefix("## insert (add fact) failed: tuple: ") {
        return Record::InsertFailed {
            start: true,
            tuple: parse_values(r),
        };
    }
    if let Some(r) = l.strip_prefix("## insert failed: tuple: ") {
        return Record::InsertFailed {
            start: false,
            tuple: parse_values(r),
        };
    }
    if l.starts_with("### Run time log") {
        return Record::Banner;
    }
    if l.starts_with("***") || l.starts_with("## ") || l.starts_with("##") {
        return Record::Diagnostic(l.to_owned());
    }
    Record::Other(l.to_owned())
}

/// Parse a whole log. Blank lines and separator lines are dropped.
pub fn parse_log(text: &str) -> Vec<Line> {
    let mut out = Vec::new();
    let mut lines = text.lines().enumerate().peekable();
    while let Some((i, raw)) = lines.next() {
        let l = raw.trim_end_matches('\r');
        if l.trim().is_empty() || l.bytes().all(|b| b == b'-') {
            continue;
        }
        if let Some(r) = l.strip_prefix("DBase contents of ") {
            // Header, a dashed rule, then one row per line up to a blank line.
            let mut rows = Vec::new();
            if lines
                .peek()
                .is_some_and(|(_, n)| !n.is_empty() && n.bytes().all(|b| b == b'-'))
            {
                lines.next();
            }
            while let Some((_, n)) = lines.peek() {
                if n.trim().is_empty() {
                    break;
                }
                rows.push(parse_values(n));
                lines.next();
            }
            out.push(Line {
                line: i + 1,
                record: Record::DbContents {
                    name: r.to_owned(),
                    rows,
                },
            });
            continue;
        }
        out.push(Line {
            line: i + 1,
            record: parse_line(l),
        });
    }
    out
}

/// Render records as log text (one record per line, blank lines like libOsiris around rule
/// firings).
pub fn render_log(records: &[Record]) -> String {
    let mut s = String::new();
    for r in records {
        if matches!(r, Record::RuleFires { .. }) {
            s.push('\n');
        }
        s.push_str(&r.to_string());
        s.push('\n');
        if matches!(r, Record::RuleEnd { .. } | Record::DbContents { .. }) {
            s.push('\n');
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values() {
        assert_eq!(LogValue::parse("12").v, LogData::Int(12));
        assert_eq!(LogValue::parse("-1.0").v, LogData::Real(-1.0));
        assert_eq!(LogValue::parse("\"a, b\"").v, LogData::Str("a, b".into()));
        let v = LogValue::parse("(CHARACTER)S_Foo_00000000-0000-0000-0000-000000000001");
        assert_eq!(v.ty.as_deref(), Some("CHARACTER"));
        assert_eq!(
            v.v,
            LogData::Guid("S_Foo_00000000-0000-0000-0000-000000000001".into())
        );
        assert_eq!(LogValue::parse("(INTEGER)").v, LogData::Unset);
        assert_eq!(LogValue::parse("(unknown)").key(), "_");
        assert_eq!(LogValue::parse("(undef value)").key(), "_");
        assert_eq!(
            LogValue::parse("TAGCATEGORY.RACE").v,
            LogData::Enum("TAGCATEGORY".into(), "RACE".into())
        );
        assert_eq!(
            LogValue::parse("Approval_AtLeast_-40_For_Sp1").v,
            LogData::Guid("Approval_AtLeast_-40_For_Sp1".into())
        );
    }

    #[test]
    fn args_respect_quotes() {
        assert_eq!(
            split_args(r#""a, b", 1, (FLAG)X, "", "c""#),
            vec![r#""a, b""#, "1", "(FLAG)X", r#""""#, r#""c""#]
        );
        assert!(split_args(" ").is_empty());
    }

    #[test]
    fn lines() {
        let log = "\n### Run time log:\n-----\n\nexec global INIT section:\n\
exec INIT section of goal Start (#1):\n\
0> DB_A( 1, \"x\" ) [add fact]\n\
\n0> RuleActionPart of THEN at line 42 fires. Rule variables: 1, (unknown)\n\
1--> PROC_B(  ) [Osiris procedure call]\n\
1--> GoalCompleted; (\"Start\") (GoalID == 1)\n\
exec EXIT section of goal \"Start\" (#1):\n\
exec EXIT section of goal \"Start\" (#1) END\n\
0> [end of rule action part]\n\n\
>>> call COsiris::Event\n>>> event Ev(G_1, 2.5)\n\
exec [DIV query] Q( (T)G, [out] (INTEGER) )\n\
*** Query failed! ***\n     Query returns: Q( G, (undef value) )\n\
## insert (add fact) failed: tuple: 1\n\nDBase contents of DB_A( (INTEGER)_ )\n----\n1\n2\n\n\
event Foo/2 ignored (no rules defined using it)\n\
line 1: error: *** call failed! ***\n";
        let recs: Vec<Record> = parse_log(log).into_iter().map(|l| l.record).collect();
        let keys: Vec<String> = recs.iter().map(Record::key).collect();
        assert_eq!(
            keys,
            vec![
                "### Run time log:",
                "exec global INIT section:",
                "INIT Start",
                "0> DB_A(1, \"x\") [add fact]",
                "0> rule@42 fires: 1, _",
                "1> PROC_B() [Osiris procedure call]",
                "1> GoalCompleted Start",
                "EXIT Start (woken)",
                "EXIT Start END",
                "0> end rule",
                ">>> call COsiris::Event",
                "event Ev(G_1, 2.5)",
                "query [DIV query] Q(G, [out] _)",
                "*** Query failed! ***",
                "returns Q(G, _)",
                "insert (add fact) failed: 1",
                "DBase contents of DB_A( (INTEGER)_ )\n1\n2",
                "event Foo/2 ignored",
                "*** call failed! ***",
            ]
        );
        // Rendering round-trips through the parser.
        let again: Vec<Record> = parse_log(&render_log(&recs))
            .into_iter()
            .map(|l| l.record)
            .collect();
        assert_eq!(again, recs);
    }
}
