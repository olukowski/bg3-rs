//! Recursive-descent parser for Thoth. Follows the structure and operator priorities of the
//! stock Lua 5.3 parser (`lparser.c`), plus the `try … catch Name then … end` statement.

use std::rc::Rc;

use super::SyntaxError;
use super::ast::*;
use super::lexer::{Tok, Token, tokenize};

/// Parse a whole chunk (a file, or a condition after rewriting).
pub fn parse_chunk(src: &str) -> Result<Block, SyntaxError> {
    let mut p = Parser {
        toks: tokenize(src)?,
        pos: 0,
    };
    let block = p.block()?;
    if p.peek() != &Tok::Eof {
        return p.err(format!("'<eof>' expected near {}", p.describe()));
    }
    Ok(block)
}

/// Parse a single expression (the whole input must be one expression).
pub fn parse_expr(src: &str) -> Result<Expr, SyntaxError> {
    let mut p = Parser {
        toks: tokenize(src)?,
        pos: 0,
    };
    let e = p.expr()?;
    if p.peek() != &Tok::Eof {
        return p.err(format!("'<eof>' expected near {}", p.describe()));
    }
    Ok(e)
}

struct Parser {
    toks: Vec<Token>,
    pos: usize,
}

/// Left/right priorities of binary operators (`lparser.c` `priority[]`).
fn binop(t: &Tok) -> Option<(BinOp, u8, u8)> {
    Some(match t {
        Tok::Plus => (BinOp::Add, 10, 10),
        Tok::Minus => (BinOp::Sub, 10, 10),
        Tok::Star => (BinOp::Mul, 11, 11),
        Tok::Percent => (BinOp::Mod, 11, 11),
        Tok::Caret => (BinOp::Pow, 14, 13),
        Tok::Slash => (BinOp::Div, 11, 11),
        Tok::DSlash => (BinOp::IDiv, 11, 11),
        Tok::Amp => (BinOp::BAnd, 6, 6),
        Tok::Pipe => (BinOp::BOr, 4, 4),
        Tok::Tilde => (BinOp::BXor, 5, 5),
        Tok::Shl => (BinOp::Shl, 7, 7),
        Tok::Shr => (BinOp::Shr, 7, 7),
        Tok::Concat => (BinOp::Concat, 9, 8),
        Tok::Eq => (BinOp::Eq, 3, 3),
        Tok::Lt => (BinOp::Lt, 3, 3),
        Tok::Le => (BinOp::Le, 3, 3),
        Tok::Ne => (BinOp::Ne, 3, 3),
        Tok::Gt => (BinOp::Gt, 3, 3),
        Tok::Ge => (BinOp::Ge, 3, 3),
        Tok::And => (BinOp::And, 2, 2),
        Tok::Or => (BinOp::Or, 1, 1),
        _ => return None,
    })
}

const UNARY_PRIORITY: u8 = 12;

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }

    fn peek2(&self) -> &Tok {
        &self.toks[(self.pos + 1).min(self.toks.len() - 1)].tok
    }

    fn line(&self) -> u32 {
        self.toks[self.pos].line
    }

    fn next(&mut self) -> Tok {
        let t = self.toks[self.pos].tok.clone();
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        t
    }

    fn describe(&self) -> String {
        match self.peek() {
            Tok::Name(n) => format!("'{n}'"),
            Tok::Str(s) => format!("'{s}'"),
            Tok::Eof => "<eof>".into(),
            t => format!("{t:?}"),
        }
    }

    fn err<T>(&self, message: String) -> Result<T, SyntaxError> {
        Err(SyntaxError {
            line: self.line(),
            message,
        })
    }

    fn check(&mut self, t: Tok) -> bool {
        if *self.peek() == t {
            self.next();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, t: Tok, what: &str) -> Result<(), SyntaxError> {
        if self.check(t) {
            Ok(())
        } else {
            self.err(format!("'{what}' expected near {}", self.describe()))
        }
    }

    fn name(&mut self) -> Result<String, SyntaxError> {
        match self.next() {
            Tok::Name(n) => Ok(n),
            _ => {
                self.pos -= 1;
                self.err(format!("<name> expected near {}", self.describe()))
            }
        }
    }

    fn block_follow(&self) -> bool {
        matches!(
            self.peek(),
            Tok::Else | Tok::Elseif | Tok::End | Tok::Until | Tok::Catch | Tok::Eof
        )
    }

    fn block(&mut self) -> Result<Block, SyntaxError> {
        let mut stats = Vec::new();
        loop {
            if self.block_follow() {
                return Ok(Block { stats, ret: None });
            }
            if *self.peek() == Tok::Return {
                self.next();
                let mut ret = Vec::new();
                if !self.block_follow() && *self.peek() != Tok::Semi {
                    ret = self.exprlist()?;
                }
                self.check(Tok::Semi);
                if !self.block_follow() {
                    return self.err(format!("'<eof>' expected near {}", self.describe()));
                }
                return Ok(Block {
                    stats,
                    ret: Some(ret),
                });
            }
            if let Some(s) = self.statement()? {
                stats.push(s);
            }
        }
    }

    fn statement(&mut self) -> Result<Option<Stat>, SyntaxError> {
        let line = self.line();
        Ok(Some(match self.peek().clone() {
            Tok::Semi => {
                self.next();
                return Ok(None);
            }
            Tok::If => self.if_stat()?,
            Tok::While => {
                self.next();
                let cond = self.expr()?;
                self.expect(Tok::Do, "do")?;
                let body = self.block()?;
                self.expect(Tok::End, "end")?;
                Stat::While { cond, body }
            }
            Tok::Do => {
                self.next();
                let b = self.block()?;
                self.expect(Tok::End, "end")?;
                Stat::Do(b)
            }
            Tok::For => self.for_stat()?,
            Tok::Repeat => {
                self.next();
                let body = self.block()?;
                self.expect(Tok::Until, "until")?;
                let cond = self.expr()?;
                Stat::Repeat { body, cond }
            }
            Tok::Function => {
                self.next();
                let mut path = vec![self.name()?];
                while self.check(Tok::Dot) {
                    path.push(self.name()?);
                }
                let method = if self.check(Tok::Colon) {
                    Some(self.name()?)
                } else {
                    None
                };
                let mut func = self.funcbody(line)?;
                if method.is_some() {
                    func.params.insert(0, "self".into());
                }
                Stat::Function {
                    path,
                    method,
                    func: Rc::new(func),
                }
            }
            Tok::Local => {
                self.next();
                if self.check(Tok::Function) {
                    let name = self.name()?;
                    Stat::LocalFunction {
                        name,
                        func: Rc::new(self.funcbody(line)?),
                    }
                } else {
                    let mut names = vec![self.name()?];
                    self.attrib()?;
                    while self.check(Tok::Comma) {
                        names.push(self.name()?);
                        self.attrib()?;
                    }
                    let values = if self.check(Tok::Assign) {
                        self.exprlist()?
                    } else {
                        Vec::new()
                    };
                    Stat::Local { names, values }
                }
            }
            Tok::DColon => {
                self.next();
                let n = self.name()?;
                self.expect(Tok::DColon, "::")?;
                Stat::Label(n)
            }
            Tok::Break => {
                self.next();
                Stat::Break
            }
            Tok::Goto => {
                self.next();
                Stat::Goto(self.name()?)
            }
            Tok::Try => {
                self.next();
                let body = self.block()?;
                self.expect(Tok::Catch, "catch")?;
                let var = self.name()?;
                self.expect(Tok::Then, "then")?;
                let handler = self.block()?;
                self.expect(Tok::End, "end")?;
                Stat::Try { body, var, handler }
            }
            _ => self.expr_stat()?,
        }))
    }

    /// Lua 5.4 attributes (`<const>`) are not Lua 5.3; we accept none.
    fn attrib(&mut self) -> Result<(), SyntaxError> {
        Ok(())
    }

    fn if_stat(&mut self) -> Result<Stat, SyntaxError> {
        self.next();
        let mut arms = Vec::new();
        let cond = self.expr()?;
        self.expect(Tok::Then, "then")?;
        arms.push((cond, self.block()?));
        let mut otherwise = None;
        loop {
            match self.peek() {
                Tok::Elseif => {
                    self.next();
                    let c = self.expr()?;
                    self.expect(Tok::Then, "then")?;
                    arms.push((c, self.block()?));
                }
                Tok::Else => {
                    self.next();
                    otherwise = Some(self.block()?);
                    self.expect(Tok::End, "end")?;
                    break;
                }
                _ => {
                    self.expect(Tok::End, "end")?;
                    break;
                }
            }
        }
        Ok(Stat::If { arms, otherwise })
    }

    fn for_stat(&mut self) -> Result<Stat, SyntaxError> {
        self.next();
        let first = self.name()?;
        if self.check(Tok::Assign) {
            let start = self.expr()?;
            self.expect(Tok::Comma, ",")?;
            let limit = self.expr()?;
            let step = if self.check(Tok::Comma) {
                Some(self.expr()?)
            } else {
                None
            };
            self.expect(Tok::Do, "do")?;
            let body = self.block()?;
            self.expect(Tok::End, "end")?;
            return Ok(Stat::NumericFor {
                var: first,
                start,
                limit,
                step,
                body,
            });
        }
        let mut vars = vec![first];
        while self.check(Tok::Comma) {
            vars.push(self.name()?);
        }
        self.expect(Tok::In, "in")?;
        let exprs = self.exprlist()?;
        self.expect(Tok::Do, "do")?;
        let body = self.block()?;
        self.expect(Tok::End, "end")?;
        Ok(Stat::GenericFor { vars, exprs, body })
    }

    fn expr_stat(&mut self) -> Result<Stat, SyntaxError> {
        let e = self.suffixedexp()?;
        if matches!(self.peek(), Tok::Assign | Tok::Comma) {
            let mut targets = vec![e];
            while self.check(Tok::Comma) {
                targets.push(self.suffixedexp()?);
            }
            self.expect(Tok::Assign, "=")?;
            for t in &targets {
                if !matches!(t, Expr::Name(_) | Expr::Index(..)) {
                    return self.err("syntax error: cannot assign to this expression".into());
                }
            }
            let values = self.exprlist()?;
            return Ok(Stat::Assign { targets, values });
        }
        if !matches!(e, Expr::Call(..) | Expr::Method(..)) {
            return self.err(format!("syntax error near {}", self.describe()));
        }
        Ok(Stat::Call(e))
    }

    fn funcbody(&mut self, line: u32) -> Result<FuncBody, SyntaxError> {
        self.expect(Tok::LParen, "(")?;
        let mut params = Vec::new();
        let mut vararg = false;
        if *self.peek() != Tok::RParen {
            loop {
                if self.check(Tok::Dots) {
                    vararg = true;
                    break;
                }
                params.push(self.name()?);
                if !self.check(Tok::Comma) {
                    break;
                }
            }
        }
        self.expect(Tok::RParen, ")")?;
        let body = self.block()?;
        self.expect(Tok::End, "end")?;
        Ok(FuncBody {
            params,
            vararg,
            body,
            line,
        })
    }

    fn exprlist(&mut self) -> Result<Vec<Expr>, SyntaxError> {
        let mut v = vec![self.expr()?];
        while self.check(Tok::Comma) {
            v.push(self.expr()?);
        }
        Ok(v)
    }

    pub fn expr(&mut self) -> Result<Expr, SyntaxError> {
        self.subexpr(0)
    }

    fn subexpr(&mut self, limit: u8) -> Result<Expr, SyntaxError> {
        let unop = match self.peek() {
            Tok::Not => Some(UnOp::Not),
            Tok::Minus => Some(UnOp::Neg),
            Tok::Hash => Some(UnOp::Len),
            Tok::Tilde => Some(UnOp::BNot),
            _ => None,
        };
        let mut left = if let Some(op) = unop {
            self.next();
            let e = self.subexpr(UNARY_PRIORITY)?;
            Expr::Un(op, Box::new(e))
        } else {
            self.simpleexp()?
        };
        while let Some((op, lp, rp)) = binop(self.peek()) {
            if lp <= limit {
                break;
            }
            self.next();
            let right = self.subexpr(rp)?;
            left = Expr::Bin(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn simpleexp(&mut self) -> Result<Expr, SyntaxError> {
        let line = self.line();
        Ok(match self.peek().clone() {
            Tok::Int(v) => {
                self.next();
                Expr::Int(v)
            }
            Tok::Float(v) => {
                self.next();
                Expr::Float(v)
            }
            Tok::Str(s) => {
                self.next();
                Expr::Str(s.into())
            }
            Tok::Nil => {
                self.next();
                Expr::Nil
            }
            Tok::True => {
                self.next();
                Expr::True
            }
            Tok::False => {
                self.next();
                Expr::False
            }
            Tok::Dots => {
                self.next();
                Expr::Vararg
            }
            Tok::LBrace => self.table()?,
            Tok::Function => {
                self.next();
                Expr::Function(Rc::new(self.funcbody(line)?))
            }
            _ => self.suffixedexp()?,
        })
    }

    fn primaryexp(&mut self) -> Result<Expr, SyntaxError> {
        match self.peek().clone() {
            Tok::Name(n) => {
                self.next();
                Ok(Expr::Name(n.into()))
            }
            Tok::LParen => {
                self.next();
                let e = self.expr()?;
                self.expect(Tok::RParen, ")")?;
                Ok(Expr::Paren(Box::new(e)))
            }
            _ => self.err(format!("unexpected symbol near {}", self.describe())),
        }
    }

    fn suffixedexp(&mut self) -> Result<Expr, SyntaxError> {
        let mut e = self.primaryexp()?;
        loop {
            match self.peek().clone() {
                Tok::Dot => {
                    self.next();
                    let n = self.name()?;
                    e = Expr::Index(Box::new(e), Box::new(Expr::Str(n.into())));
                }
                Tok::LBracket => {
                    self.next();
                    let k = self.expr()?;
                    self.expect(Tok::RBracket, "]")?;
                    e = Expr::Index(Box::new(e), Box::new(k));
                }
                Tok::Colon => {
                    // `a:b(...)` – but a `::label` can't follow an expression, so `:` is a method.
                    self.next();
                    let n = self.name()?;
                    let args = self.callargs()?;
                    e = Expr::Method(Box::new(e), n.into(), args);
                }
                Tok::LParen | Tok::Str(_) | Tok::LBrace => {
                    let args = self.callargs()?;
                    e = Expr::Call(Box::new(e), args);
                }
                _ => return Ok(e),
            }
        }
    }

    fn callargs(&mut self) -> Result<Vec<Expr>, SyntaxError> {
        match self.peek().clone() {
            Tok::Str(s) => {
                self.next();
                Ok(vec![Expr::Str(s.into())])
            }
            Tok::LBrace => Ok(vec![self.table()?]),
            Tok::LParen => {
                self.next();
                if self.check(Tok::RParen) {
                    return Ok(Vec::new());
                }
                let args = self.exprlist()?;
                self.expect(Tok::RParen, ")")?;
                Ok(args)
            }
            _ => self.err(format!(
                "function arguments expected near {}",
                self.describe()
            )),
        }
    }

    fn table(&mut self) -> Result<Expr, SyntaxError> {
        self.expect(Tok::LBrace, "{")?;
        let mut fields = Vec::new();
        while *self.peek() != Tok::RBrace {
            match (self.peek().clone(), self.peek2().clone()) {
                (Tok::Name(n), Tok::Assign) => {
                    self.next();
                    self.next();
                    fields.push(TableField::Named(n.into(), self.expr()?));
                }
                (Tok::LBracket, _) => {
                    self.next();
                    let k = self.expr()?;
                    self.expect(Tok::RBracket, "]")?;
                    self.expect(Tok::Assign, "=")?;
                    fields.push(TableField::Keyed(k, self.expr()?));
                }
                _ => fields.push(TableField::Positional(self.expr()?)),
            }
            if !self.check(Tok::Comma) && !self.check(Tok::Semi) {
                break;
            }
        }
        self.expect(Tok::RBrace, "}")?;
        Ok(Expr::Table(fields))
    }
}
