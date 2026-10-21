//! The engine side of the VM: calls and queries the story makes into the game.
//!
//! In BG3 the engine registers its functions with libOsiris (`COsiris::RegisterDIVFunctions`)
//! and Osiris resolves the story's `call`/`query` declarations to handles
//! (`COsiris::_GetDIVHandles`); calls then go through one callback with the handle and an
//! argument list. Here functions are identified by name and arity instead. Events flow the
//! other way: the game calls [`crate::Vm::event`].

use crate::value::{Data, Root, StringTable, TypeId, TypeTable, Value, types};

/// Access to the VM's string and type tables for a host callback.
pub struct HostCtx<'a> {
    pub(crate) strings: &'a mut StringTable,
    pub(crate) types: &'a TypeTable,
}

impl HostCtx<'_> {
    pub fn string(&mut self, s: &str) -> Value {
        make_string(self.strings, types::STRING, Root::String, s)
    }

    pub fn guid(&mut self, s: &str) -> Value {
        make_string(self.strings, types::GUIDSTRING, Root::GuidString, s)
    }

    /// A string value of the given (alias) type.
    pub fn typed_string(&mut self, ty: TypeId, s: &str) -> Value {
        let root = self.types.root(ty);
        make_string(self.strings, ty, root, s)
    }

    pub fn text(&self, v: &Value) -> Option<&str> {
        v.as_str().map(|s| self.strings.text(s))
    }

    /// Render a value for logs/recordings.
    pub fn display(&self, v: &Value) -> String {
        display(self.strings, v)
    }
}

pub(crate) fn make_string(strings: &mut StringTable, ty: TypeId, root: Root, s: &str) -> Value {
    let is_guid = root == Root::GuidString;
    Value {
        ty,
        root,
        data: Data::Str(strings.intern(s, is_guid)),
    }
}

pub(crate) fn display(strings: &StringTable, v: &Value) -> String {
    match v.data {
        Data::Undef => "<unset>".to_owned(),
        Data::Int(i) => i.to_string(),
        Data::Int64(i) => i.to_string(),
        Data::Real(f) => format!("{f:?}"),
        Data::Str(s) => match v.root {
            Root::GuidString => strings.text(s).to_owned(),
            _ => format!("{:?}", strings.text(s)),
        },
    }
}

/// Engine-provided functions.
pub trait Host {
    /// A `call` (engine action) fired by a rule or goal section. The return value only matters
    /// for logging in Osiris ("call failed!").
    fn call(&mut self, ctx: &mut HostCtx<'_>, name: &str, args: &[Value]) -> bool;

    /// A `query` (engine condition). `args` holds the inputs; out-parameters (`out[i]`) come
    /// in unset (typed) and must be filled on success. Returning false fails the condition.
    fn query(
        &mut self,
        ctx: &mut HostCtx<'_>,
        name: &str,
        args: &mut [Value],
        out: &[bool],
    ) -> bool;
}

/// One recorded host interaction.
#[derive(Debug, Clone, PartialEq)]
pub struct Recorded {
    pub name: String,
    /// Rendered arguments (after the call, for queries).
    pub args: Vec<String>,
    pub result: bool,
}

type QueryFn = dyn FnMut(&mut HostCtx<'_>, &str, &mut [Value], &[bool]) -> Option<bool>;

/// A host that records every call and query. Queries fail unless a handler answers them.
#[derive(Default)]
pub struct RecordingHost {
    pub calls: Vec<Recorded>,
    pub queries: Vec<Recorded>,
    /// Optional query handler; `None` from it means "unknown query" (fails).
    pub query_handler: Option<Box<QueryFn>>,
}

impl RecordingHost {
    pub fn new() -> Self {
        Self::default()
    }

    /// Answer queries with `f` (returning `None` = unknown, fails).
    pub fn with_queries(
        f: impl FnMut(&mut HostCtx<'_>, &str, &mut [Value], &[bool]) -> Option<bool> + 'static,
    ) -> Self {
        Self {
            query_handler: Some(Box::new(f)),
            ..Self::default()
        }
    }

    /// Names of the recorded calls, in order.
    pub fn call_names(&self) -> Vec<&str> {
        self.calls.iter().map(|c| c.name.as_str()).collect()
    }
}

impl Host for RecordingHost {
    fn call(&mut self, ctx: &mut HostCtx<'_>, name: &str, args: &[Value]) -> bool {
        self.calls.push(Recorded {
            name: name.to_owned(),
            args: args.iter().map(|a| ctx.display(a)).collect(),
            result: true,
        });
        true
    }

    fn query(
        &mut self,
        ctx: &mut HostCtx<'_>,
        name: &str,
        args: &mut [Value],
        out: &[bool],
    ) -> bool {
        let result = match &mut self.query_handler {
            Some(f) => f(ctx, name, args, out).unwrap_or(false),
            None => false,
        };
        self.queries.push(Recorded {
            name: name.to_owned(),
            args: args.iter().map(|a| ctx.display(a)).collect(),
            result,
        });
        result
    }
}
