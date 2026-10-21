//! Line tokenizer for stats text files.
//!
//! A statement is one line. Tokens are separated by whitespace, `,` or `:`; a token is either a
//! double-quoted string or a bare word. There is no escape syntax. Strings never span lines: a
//! string without a closing quote runs to the end of the line and the statement is flagged
//! [`Statement::unterminated`] (a few shipped files have a stray line break or an extra `"`
//! inside a value; see `docs/formats/stats.md`). `//` at the start of a line starts a comment.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub text: String,
    /// Whether the token was written as a `"quoted string"`.
    pub quoted: bool,
}

/// One logical line of a stats file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    /// 1-based line number of the first token.
    pub line: u32,
    pub tokens: Vec<Token>,
    /// A quoted string on this line had no closing quote.
    pub unterminated: bool,
}

impl Statement {
    pub fn word(&self, i: usize) -> Option<&str> {
        self.tokens.get(i).map(|t| t.text.as_str())
    }

    /// Leading bare (unquoted) word `i`, if present.
    pub fn bare(&self, i: usize) -> Option<&str> {
        self.tokens
            .get(i)
            .filter(|t| !t.quoted)
            .map(|t| t.text.as_str())
    }

    pub fn texts(&self) -> impl Iterator<Item = &str> {
        self.tokens.iter().map(|t| t.text.as_str())
    }
}

pub fn tokenize(src: &str) -> Vec<Statement> {
    let src = src.strip_prefix('\u{feff}').unwrap_or(src);
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let mut cur: Vec<Token> = Vec::new();
    let mut cur_line = 1u32;
    let mut line = 1u32;
    let mut at_line_start = true;
    let mut unterminated = false;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b'\n' => {
                if !cur.is_empty() {
                    out.push(Statement {
                        line: cur_line,
                        tokens: std::mem::take(&mut cur),
                        unterminated,
                    });
                }
                unterminated = false;
                line += 1;
                at_line_start = true;
                i += 1;
            }
            b' ' | b'\t' | b'\r' | b',' | b':' => i += 1,
            b'/' if at_line_start && cur.is_empty() && bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'"' => {
                let start = i + 1;
                let eol = src[start..].find('\n').map_or(src.len(), |n| start + n);
                let (text, next) = match src[start..eol].find('"') {
                    Some(len) => (&src[start..start + len], start + len + 1),
                    None => {
                        unterminated = true;
                        (src[start..eol].trim_end_matches('\r'), eol)
                    }
                };
                if cur.is_empty() {
                    cur_line = line;
                }
                cur.push(Token {
                    text: text.to_owned(),
                    quoted: true,
                });
                at_line_start = false;
                i = next;
            }
            _ => {
                let start = i;
                while i < bytes.len()
                    && !matches!(bytes[i], b' ' | b'\t' | b'\r' | b'\n' | b',' | b':' | b'"')
                {
                    i += 1;
                }
                if cur.is_empty() {
                    cur_line = line;
                }
                cur.push(Token {
                    text: src[start..i].to_owned(),
                    quoted: false,
                });
                at_line_start = false;
            }
        }
    }
    if !cur.is_empty() {
        out.push(Statement {
            line: cur_line,
            tokens: cur,
            unterminated,
        });
    }
    out
}
