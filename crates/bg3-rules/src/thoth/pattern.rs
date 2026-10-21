//! A subset of Lua patterns, enough for `string.gsub`/`string.find` as the condition scripts
//! use them: literals, `%` escapes and classes (`%a %d %l %s %u %w %x %p %c` and complements),
//! `.`, sets `[...]`, the quantifiers `* + - ?`, and the anchors `^`/`$`. Captures are not
//! supported.

use super::interp::{LResult, rt_err};

#[derive(Debug, Clone)]
enum Class {
    Any,
    Lit(u8),
    Esc(u8),
    Set { neg: bool, items: Vec<SetItem> },
}

#[derive(Debug, Clone)]
enum SetItem {
    Lit(u8),
    Range(u8, u8),
    Esc(u8),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Quant {
    One,
    Star,
    Plus,
    Lazy,
    Opt,
}

#[derive(Debug, Clone)]
pub struct Pattern {
    anchored: bool,
    items: Vec<(Class, Quant)>,
    end_anchor: bool,
}

fn class_match(esc: u8, c: u8) -> bool {
    let r = match esc.to_ascii_lowercase() {
        b'a' => c.is_ascii_alphabetic(),
        b'd' => c.is_ascii_digit(),
        b'l' => c.is_ascii_lowercase(),
        b's' => c.is_ascii_whitespace() || c == 0x0b,
        b'u' => c.is_ascii_uppercase(),
        b'w' => c.is_ascii_alphanumeric(),
        b'x' => c.is_ascii_hexdigit(),
        b'p' => c.is_ascii_punctuation(),
        b'c' => c.is_ascii_control(),
        _ => return esc == c,
    };
    if esc.is_ascii_uppercase() { !r } else { r }
}

impl Class {
    fn matches(&self, c: u8) -> bool {
        match self {
            Class::Any => true,
            Class::Lit(l) => *l == c,
            Class::Esc(e) => class_match(*e, c),
            Class::Set { neg, items } => {
                let hit = items.iter().any(|i| match i {
                    SetItem::Lit(l) => *l == c,
                    SetItem::Range(a, b) => (*a..=*b).contains(&c),
                    SetItem::Esc(e) => class_match(*e, c),
                });
                hit != *neg
            }
        }
    }
}

impl Pattern {
    pub fn compile(p: &str) -> LResult<Pattern> {
        let b = p.as_bytes();
        let mut i = 0;
        let anchored = b.first() == Some(&b'^');
        if anchored {
            i = 1;
        }
        let mut items = Vec::new();
        let mut end_anchor = false;
        while i < b.len() {
            let class = match b[i] {
                b'%' => {
                    let Some(&e) = b.get(i + 1) else {
                        return rt_err("malformed pattern (ends with '%')");
                    };
                    i += 2;
                    if e.is_ascii_alphanumeric() {
                        Class::Esc(e)
                    } else {
                        Class::Lit(e)
                    }
                }
                b'.' => {
                    i += 1;
                    Class::Any
                }
                b'$' if i + 1 == b.len() => {
                    end_anchor = true;
                    i += 1;
                    continue;
                }
                b'(' | b')' => return rt_err("pattern captures are not supported"),
                b'[' => {
                    i += 1;
                    let neg = b.get(i) == Some(&b'^');
                    if neg {
                        i += 1;
                    }
                    let mut set = Vec::new();
                    let mut first = true;
                    loop {
                        let Some(&c) = b.get(i) else {
                            return rt_err("malformed pattern (missing ']')");
                        };
                        if c == b']' && !first {
                            i += 1;
                            break;
                        }
                        first = false;
                        if c == b'%' {
                            let Some(&e) = b.get(i + 1) else {
                                return rt_err("malformed pattern");
                            };
                            set.push(if e.is_ascii_alphanumeric() {
                                SetItem::Esc(e)
                            } else {
                                SetItem::Lit(e)
                            });
                            i += 2;
                        } else if b.get(i + 1) == Some(&b'-')
                            && b.get(i + 2).is_some_and(|&e| e != b']')
                        {
                            set.push(SetItem::Range(c, b[i + 2]));
                            i += 3;
                        } else {
                            set.push(SetItem::Lit(c));
                            i += 1;
                        }
                    }
                    Class::Set { neg, items: set }
                }
                c => {
                    i += 1;
                    Class::Lit(c)
                }
            };
            let q = match b.get(i) {
                Some(b'*') => Quant::Star,
                Some(b'+') => Quant::Plus,
                Some(b'-') => Quant::Lazy,
                Some(b'?') => Quant::Opt,
                _ => Quant::One,
            };
            if q != Quant::One {
                i += 1;
            }
            items.push((class, q));
        }
        Ok(Pattern {
            anchored,
            items,
            end_anchor,
        })
    }

    fn match_here(&self, s: &[u8], si: usize, pi: usize) -> Option<usize> {
        if pi == self.items.len() {
            return (!self.end_anchor || si == s.len()).then_some(si);
        }
        let (class, q) = &self.items[pi];
        match q {
            Quant::One => {
                if si < s.len() && class.matches(s[si]) {
                    self.match_here(s, si + 1, pi + 1)
                } else {
                    None
                }
            }
            Quant::Opt => {
                if si < s.len()
                    && class.matches(s[si])
                    && let Some(e) = self.match_here(s, si + 1, pi + 1)
                {
                    return Some(e);
                }
                self.match_here(s, si, pi + 1)
            }
            Quant::Star | Quant::Plus => {
                let mut n = 0;
                while si + n < s.len() && class.matches(s[si + n]) {
                    n += 1;
                }
                let min = if *q == Quant::Plus { 1 } else { 0 };
                loop {
                    if n < min {
                        return None;
                    }
                    if let Some(e) = self.match_here(s, si + n, pi + 1) {
                        return Some(e);
                    }
                    if n == 0 {
                        return None;
                    }
                    n -= 1;
                }
            }
            Quant::Lazy => {
                let mut n = 0;
                loop {
                    if let Some(e) = self.match_here(s, si + n, pi + 1) {
                        return Some(e);
                    }
                    if si + n < s.len() && class.matches(s[si + n]) {
                        n += 1;
                    } else {
                        return None;
                    }
                }
            }
        }
    }

    /// First match at or after `init`: (start, end) byte offsets.
    pub fn find(&self, s: &[u8], init: usize) -> Option<(usize, usize)> {
        let mut start = init;
        loop {
            if start > s.len() {
                return None;
            }
            if let Some(e) = self.match_here(s, start, 0) {
                return Some((start, e));
            }
            if self.anchored {
                return None;
            }
            start += 1;
        }
    }

    /// `string.gsub` with a string replacement (`%0` = whole match, `%%` = `%`).
    pub fn gsub(&self, s: &[u8], repl: &[u8]) -> (Vec<u8>, usize) {
        let mut out = Vec::with_capacity(s.len());
        let mut i = 0;
        let mut n = 0;
        while i <= s.len() {
            match self.match_here(s, i, 0) {
                Some(e) => {
                    n += 1;
                    let mut k = 0;
                    while k < repl.len() {
                        if repl[k] == b'%' && k + 1 < repl.len() {
                            match repl[k + 1] {
                                b'0' | b'1' => out.extend_from_slice(&s[i..e]),
                                c => out.push(c),
                            }
                            k += 2;
                        } else {
                            out.push(repl[k]);
                            k += 1;
                        }
                    }
                    if e > i {
                        i = e;
                    } else {
                        if i < s.len() {
                            out.push(s[i]);
                        }
                        i += 1;
                    }
                }
                None => {
                    if i < s.len() {
                        out.push(s[i]);
                    }
                    i += 1;
                }
            }
            if self.anchored {
                if i <= s.len() {
                    out.extend_from_slice(&s[i.min(s.len())..]);
                }
                break;
            }
        }
        (out, n)
    }
}
