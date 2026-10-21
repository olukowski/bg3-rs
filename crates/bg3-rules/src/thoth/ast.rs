//! Syntax tree for Thoth (Lua 5.3 + `try … catch e then … end`).

use std::rc::Rc;

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub stats: Vec<Stat>,
    /// `return explist` at the end of the block.
    pub ret: Option<Vec<Expr>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stat {
    Assign {
        targets: Vec<Expr>,
        values: Vec<Expr>,
    },
    Call(Expr),
    Label(String),
    Break,
    Goto(String),
    Do(Block),
    While {
        cond: Expr,
        body: Block,
    },
    Repeat {
        body: Block,
        cond: Expr,
    },
    If {
        arms: Vec<(Expr, Block)>,
        otherwise: Option<Block>,
    },
    NumericFor {
        var: String,
        start: Expr,
        limit: Expr,
        step: Option<Expr>,
        body: Block,
    },
    GenericFor {
        vars: Vec<String>,
        exprs: Vec<Expr>,
        body: Block,
    },
    /// `function a.b.c:m() … end` (`path` = `[a, b, c]`, `method` = `m`).
    Function {
        path: Vec<String>,
        method: Option<String>,
        func: Rc<FuncBody>,
    },
    LocalFunction {
        name: String,
        func: Rc<FuncBody>,
    },
    Local {
        names: Vec<String>,
        values: Vec<Expr>,
    },
    /// Thoth extension: `try body catch var then handler end`.
    Try {
        body: Block,
        var: String,
        handler: Block,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct FuncBody {
    pub params: Vec<String>,
    pub vararg: bool,
    pub body: Block,
    pub line: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Or,
    And,
    Lt,
    Gt,
    Le,
    Ge,
    Ne,
    Eq,
    BOr,
    BXor,
    BAnd,
    Shl,
    Shr,
    Concat,
    Add,
    Sub,
    Mul,
    Div,
    IDiv,
    Mod,
    Pow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Not,
    Neg,
    Len,
    BNot,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Nil,
    True,
    False,
    Int(i64),
    Float(f64),
    Str(Rc<str>),
    Vararg,
    Function(Rc<FuncBody>),
    Name(Rc<str>),
    Index(Box<Expr>, Box<Expr>),
    Call(Box<Expr>, Vec<Expr>),
    Method(Box<Expr>, Rc<str>, Vec<Expr>),
    /// `(e)`: truncates multiple results to one.
    Paren(Box<Expr>),
    Table(Vec<TableField>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    Un(UnOp, Box<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum TableField {
    Positional(Expr),
    Named(Rc<str>, Expr),
    Keyed(Expr, Expr),
}

impl Expr {
    /// Visit this expression and every sub-expression (not descending into function bodies).
    pub fn walk(&self, f: &mut dyn FnMut(&Expr)) {
        f(self);
        match self {
            Expr::Index(a, b) | Expr::Bin(_, a, b) => {
                a.walk(f);
                b.walk(f);
            }
            Expr::Call(callee, args) => {
                callee.walk(f);
                args.iter().for_each(|a| a.walk(f));
            }
            Expr::Method(obj, _, args) => {
                obj.walk(f);
                args.iter().for_each(|a| a.walk(f));
            }
            Expr::Paren(e) | Expr::Un(_, e) => e.walk(f),
            Expr::Table(fields) => {
                for field in fields {
                    match field {
                        TableField::Positional(e) | TableField::Named(_, e) => e.walk(f),
                        TableField::Keyed(k, v) => {
                            k.walk(f);
                            v.walk(f);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Names of the global functions this expression calls directly (`Foo(...)`).
    pub fn called_names(&self) -> Vec<Rc<str>> {
        let mut out = Vec::new();
        self.walk(&mut |e| {
            if let Expr::Call(callee, _) = e
                && let Expr::Name(n) = &**callee
            {
                out.push(n.clone());
            }
        });
        out
    }
}
