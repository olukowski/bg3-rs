//! Tokenizer for Thoth: the Lua 5.3 lexical grammar plus the reserved words `try` and `catch`.
//!
//! The engine embeds Lua 5.3 (the binary carries `Lua 5.3` and the stock `llex.c` token names;
//! `catch` sits in that token-name table between `break` and `else`).

use super::SyntaxError;

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Name(String),
    Str(String),
    Int(i64),
    Float(f64),
    // keywords
    And,
    Break,
    Catch,
    Do,
    Else,
    Elseif,
    End,
    False,
    For,
    Function,
    Goto,
    If,
    In,
    Local,
    Nil,
    Not,
    Or,
    Repeat,
    Return,
    Then,
    True,
    Try,
    Until,
    While,
    // symbols
    Plus,
    Minus,
    Star,
    Slash,
    DSlash,
    Percent,
    Caret,
    Hash,
    Amp,
    Tilde,
    Pipe,
    Shl,
    Shr,
    Eq,
    Ne,
    Le,
    Ge,
    Lt,
    Gt,
    Assign,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    DColon,
    Semi,
    Colon,
    Comma,
    Dot,
    Concat,
    Dots,
    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub tok: Tok,
    pub line: u32,
}

fn keyword(s: &str) -> Option<Tok> {
    Some(match s {
        "and" => Tok::And,
        "break" => Tok::Break,
        "catch" => Tok::Catch,
        "do" => Tok::Do,
        "else" => Tok::Else,
        "elseif" => Tok::Elseif,
        "end" => Tok::End,
        "false" => Tok::False,
        "for" => Tok::For,
        "function" => Tok::Function,
        "goto" => Tok::Goto,
        "if" => Tok::If,
        "in" => Tok::In,
        "local" => Tok::Local,
        "nil" => Tok::Nil,
        "not" => Tok::Not,
        "or" => Tok::Or,
        "repeat" => Tok::Repeat,
        "return" => Tok::Return,
        "then" => Tok::Then,
        "true" => Tok::True,
        "try" => Tok::Try,
        "until" => Tok::Until,
        "while" => Tok::While,
        _ => return None,
    })
}

pub fn tokenize(src: &str) -> Result<Vec<Token>, SyntaxError> {
    Lexer {
        s: src.as_bytes(),
        i: 0,
        line: 1,
    }
    .run()
}

struct Lexer<'a> {
    s: &'a [u8],
    i: usize,
    line: u32,
}

impl Lexer<'_> {
    fn err<T>(&self, msg: impl Into<String>) -> Result<T, SyntaxError> {
        Err(SyntaxError {
            line: self.line,
            message: msg.into(),
        })
    }

    fn peek(&self, k: usize) -> u8 {
        self.s.get(self.i + k).copied().unwrap_or(0)
    }

    fn run(mut self) -> Result<Vec<Token>, SyntaxError> {
        let mut out = Vec::new();
        // A first line starting with `#` is skipped, as in stock Lua.
        if self.s.starts_with(b"#") {
            while self.i < self.s.len() && self.s[self.i] != b'\n' {
                self.i += 1;
            }
        }
        loop {
            self.skip_space_and_comments()?;
            let line = self.line;
            let Some(&c) = self.s.get(self.i) else {
                out.push(Token {
                    tok: Tok::Eof,
                    line,
                });
                return Ok(out);
            };
            let tok = match c {
                b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                    let start = self.i;
                    while self.peek(0).is_ascii_alphanumeric() || self.peek(0) == b'_' {
                        self.i += 1;
                    }
                    let word = std::str::from_utf8(&self.s[start..self.i]).unwrap();
                    keyword(word).unwrap_or_else(|| Tok::Name(word.to_owned()))
                }
                b'0'..=b'9' => self.number()?,
                b'.' if self.peek(1).is_ascii_digit() => self.number()?,
                b'"' | b'\'' => Tok::Str(self.short_string(c)?),
                b'[' if matches!(self.peek(1), b'[' | b'=') && self.long_level().is_some() => {
                    Tok::Str(self.long_string()?)
                }
                _ => self.symbol()?,
            };
            out.push(Token { tok, line });
        }
    }

    fn skip_space_and_comments(&mut self) -> Result<(), SyntaxError> {
        loop {
            match self.peek(0) {
                b'\n' => {
                    self.line += 1;
                    self.i += 1;
                }
                b' ' | b'\t' | b'\r' | 0x0b | 0x0c => self.i += 1,
                b'-' if self.peek(1) == b'-' => {
                    self.i += 2;
                    if self.peek(0) == b'[' && self.long_level().is_some() {
                        self.long_string()?;
                    } else {
                        while self.i < self.s.len() && self.s[self.i] != b'\n' {
                            self.i += 1;
                        }
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    /// At `[`: the level of a long bracket `[==[`, if this is one.
    fn long_level(&self) -> Option<usize> {
        let mut k = 1;
        while self.peek(k) == b'=' {
            k += 1;
        }
        (self.peek(k) == b'[').then_some(k - 1)
    }

    fn long_string(&mut self) -> Result<String, SyntaxError> {
        let level = self.long_level().unwrap();
        self.i += level + 2;
        // A newline right after the opening bracket is skipped.
        if self.peek(0) == b'\r' {
            self.i += 1;
        }
        if self.peek(0) == b'\n' {
            self.i += 1;
            self.line += 1;
        }
        let start = self.i;
        loop {
            match self.s.get(self.i) {
                None => return self.err("unfinished long string/comment"),
                Some(b']') => {
                    let mut k = 1;
                    while self.peek(k) == b'=' {
                        k += 1;
                    }
                    if k - 1 == level && self.peek(k) == b']' {
                        let text = String::from_utf8_lossy(&self.s[start..self.i]).into_owned();
                        self.i += k + 1;
                        return Ok(text);
                    }
                    self.i += 1;
                }
                Some(b'\n') => {
                    self.line += 1;
                    self.i += 1;
                }
                Some(_) => self.i += 1,
            }
        }
    }

    fn short_string(&mut self, quote: u8) -> Result<String, SyntaxError> {
        self.i += 1;
        let mut buf: Vec<u8> = Vec::new();
        loop {
            let Some(&c) = self.s.get(self.i) else {
                return self.err("unfinished string");
            };
            self.i += 1;
            match c {
                b'\n' => return self.err("unfinished string"),
                c if c == quote => break,
                b'\\' => {
                    let e = self.peek(0);
                    self.i += 1;
                    match e {
                        b'n' => buf.push(b'\n'),
                        b't' => buf.push(b'\t'),
                        b'r' => buf.push(b'\r'),
                        b'a' => buf.push(7),
                        b'b' => buf.push(8),
                        b'f' => buf.push(12),
                        b'v' => buf.push(11),
                        b'\\' | b'"' | b'\'' => buf.push(e),
                        b'\n' => {
                            self.line += 1;
                            buf.push(b'\n');
                        }
                        b'x' => {
                            let h =
                                std::str::from_utf8(self.s.get(self.i..self.i + 2).unwrap_or(&[]))
                                    .ok()
                                    .and_then(|h| u8::from_str_radix(h, 16).ok());
                            let Some(h) = h else {
                                return self.err("hexadecimal digit expected");
                            };
                            self.i += 2;
                            buf.push(h);
                        }
                        b'z' => {
                            while self.peek(0).is_ascii_whitespace() {
                                if self.peek(0) == b'\n' {
                                    self.line += 1;
                                }
                                self.i += 1;
                            }
                        }
                        b'u' => {
                            if self.peek(0) != b'{' {
                                return self.err("missing '{'");
                            }
                            self.i += 1;
                            let start = self.i;
                            while self.peek(0).is_ascii_hexdigit() {
                                self.i += 1;
                            }
                            let v = std::str::from_utf8(&self.s[start..self.i])
                                .ok()
                                .and_then(|h| u32::from_str_radix(h, 16).ok())
                                .and_then(char::from_u32);
                            if self.peek(0) != b'}' {
                                return self.err("missing '}'");
                            }
                            self.i += 1;
                            let Some(ch) = v else {
                                return self.err("UTF-8 value too large");
                            };
                            let mut tmp = [0u8; 4];
                            buf.extend_from_slice(ch.encode_utf8(&mut tmp).as_bytes());
                        }
                        b'0'..=b'9' => {
                            let mut v: u32 = (e - b'0') as u32;
                            for _ in 0..2 {
                                if self.peek(0).is_ascii_digit() {
                                    v = v * 10 + (self.peek(0) - b'0') as u32;
                                    self.i += 1;
                                }
                            }
                            if v > 255 {
                                return self.err("decimal escape too large");
                            }
                            buf.push(v as u8);
                        }
                        _ => return self.err("invalid escape sequence"),
                    }
                }
                _ => buf.push(c),
            }
        }
        Ok(String::from_utf8_lossy(&buf).into_owned())
    }

    fn number(&mut self) -> Result<Tok, SyntaxError> {
        let start = self.i;
        let hex = self.peek(0) == b'0' && matches!(self.peek(1), b'x' | b'X');
        if hex {
            self.i += 2;
        }
        let (exp_a, exp_b) = if hex { (b'p', b'P') } else { (b'e', b'E') };
        loop {
            let c = self.peek(0);
            if c == exp_a || c == exp_b {
                self.i += 1;
                if matches!(self.peek(0), b'+' | b'-') {
                    self.i += 1;
                }
            } else if c.is_ascii_hexdigit() || c == b'.' {
                self.i += 1;
            } else {
                break;
            }
        }
        let text = std::str::from_utf8(&self.s[start..self.i]).unwrap();
        parse_number(text).map_or_else(|| self.err(format!("malformed number near '{text}'")), Ok)
    }

    fn symbol(&mut self) -> Result<Tok, SyntaxError> {
        let c = self.peek(0);
        let n = self.peek(1);
        let (tok, len) = match (c, n) {
            (b'=', b'=') => (Tok::Eq, 2),
            (b'~', b'=') => (Tok::Ne, 2),
            (b'<', b'=') => (Tok::Le, 2),
            (b'>', b'=') => (Tok::Ge, 2),
            (b'<', b'<') => (Tok::Shl, 2),
            (b'>', b'>') => (Tok::Shr, 2),
            (b'/', b'/') => (Tok::DSlash, 2),
            (b':', b':') => (Tok::DColon, 2),
            (b'.', b'.') if self.peek(2) == b'.' => (Tok::Dots, 3),
            (b'.', b'.') => (Tok::Concat, 2),
            (b'+', _) => (Tok::Plus, 1),
            (b'-', _) => (Tok::Minus, 1),
            (b'*', _) => (Tok::Star, 1),
            (b'/', _) => (Tok::Slash, 1),
            (b'%', _) => (Tok::Percent, 1),
            (b'^', _) => (Tok::Caret, 1),
            (b'#', _) => (Tok::Hash, 1),
            (b'&', _) => (Tok::Amp, 1),
            (b'~', _) => (Tok::Tilde, 1),
            (b'|', _) => (Tok::Pipe, 1),
            (b'<', _) => (Tok::Lt, 1),
            (b'>', _) => (Tok::Gt, 1),
            (b'=', _) => (Tok::Assign, 1),
            (b'(', _) => (Tok::LParen, 1),
            (b')', _) => (Tok::RParen, 1),
            (b'{', _) => (Tok::LBrace, 1),
            (b'}', _) => (Tok::RBrace, 1),
            (b'[', _) => (Tok::LBracket, 1),
            (b']', _) => (Tok::RBracket, 1),
            (b';', _) => (Tok::Semi, 1),
            (b':', _) => (Tok::Colon, 1),
            (b',', _) => (Tok::Comma, 1),
            (b'.', _) => (Tok::Dot, 1),
            _ => {
                let ch = String::from_utf8_lossy(&self.s[self.i..(self.i + 1).min(self.s.len())])
                    .into_owned();
                return self.err(format!("unexpected symbol near '{ch}'"));
            }
        };
        self.i += len;
        Ok(tok)
    }
}

/// Lua 5.3 numeral conversion: decimal or hex, integer unless it has a fraction/exponent or
/// (decimal only) overflows, in which case it is a float.
pub fn parse_number(text: &str) -> Option<Tok> {
    let t = text.trim();
    if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        if h.contains(['.', 'p', 'P']) {
            return parse_hex_float(h).map(Tok::Float);
        }
        if h.is_empty() || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        // Hex integers wrap around (Lua 5.3 semantics).
        let mut v: u64 = 0;
        for b in h.bytes() {
            v = v
                .wrapping_mul(16)
                .wrapping_add((b as char).to_digit(16).unwrap() as u64);
        }
        return Some(Tok::Int(v as i64));
    }
    if t.is_empty()
        || !t
            .bytes()
            .all(|b| b.is_ascii_digit() || b"+-.eE".contains(&b))
    {
        return None;
    }
    if t.bytes().all(|b| b.is_ascii_digit())
        && let Ok(v) = t.parse::<i64>()
    {
        return Some(Tok::Int(v));
    }
    t.parse::<f64>().ok().map(Tok::Float)
}

fn parse_hex_float(h: &str) -> Option<f64> {
    let (mant, exp) = match h.find(['p', 'P']) {
        Some(p) => (&h[..p], h[p + 1..].parse::<i32>().ok()?),
        None => (h, 0),
    };
    let (int, frac) = mant.split_once('.').unwrap_or((mant, ""));
    if int.is_empty() && frac.is_empty() {
        return None;
    }
    let mut v = 0f64;
    for c in int.chars() {
        v = v * 16.0 + c.to_digit(16)? as f64;
    }
    let mut scale = 1.0 / 16.0;
    for c in frac.chars() {
        v += c.to_digit(16)? as f64 * scale;
        scale /= 16.0;
    }
    Some(v * 2f64.powi(exp))
}
