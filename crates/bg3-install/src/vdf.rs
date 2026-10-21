//! A minimal parser for Valve's text KeyValues ("VDF") format, enough for Steam's
//! `steamapps/libraryfolders.vdf`.
//!
//! Grammar: a sequence of `key value` pairs, where a key is a (usually quoted) string and a value
//! is either a string or a `{ … }` block of further pairs. `//` starts a line comment. Quoted
//! strings support the escapes `\\`, `\"`, `\n` and `\t` (Steam writes Windows paths as
//! `"D:\\SteamLibrary"`).

/// A VDF value: a string or a nested block of key/value pairs (in file order; keys may repeat).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Str(String),
    Block(Vec<(String, Value)>),
}

impl Value {
    /// First child named `key` (case-insensitive, like Steam's own lookup).
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Block(items) => items
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| v),
            Value::Str(_) => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            Value::Block(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Str(String),
    Open,
    Close,
}

fn tokenize(text: &str) -> Option<Vec<Token>> {
    let mut tokens = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            c if c.is_whitespace() => {
                chars.next();
            }
            '{' => {
                chars.next();
                tokens.push(Token::Open);
            }
            '}' => {
                chars.next();
                tokens.push(Token::Close);
            }
            '/' => {
                chars.next();
                if chars.peek() == Some(&'/') {
                    for c in chars.by_ref() {
                        if c == '\n' {
                            break;
                        }
                    }
                } else {
                    // A lone `/` starts an unquoted token.
                    let mut s = String::from('/');
                    while let Some(&c) = chars.peek() {
                        if c.is_whitespace() || matches!(c, '{' | '}' | '"') {
                            break;
                        }
                        s.push(c);
                        chars.next();
                    }
                    tokens.push(Token::Str(s));
                }
            }
            '"' => {
                chars.next();
                let mut s = String::new();
                loop {
                    match chars.next()? {
                        '"' => break,
                        '\\' => match chars.next()? {
                            'n' => s.push('\n'),
                            't' => s.push('\t'),
                            other => s.push(other),
                        },
                        other => s.push(other),
                    }
                }
                tokens.push(Token::Str(s));
            }
            _ => {
                let mut s = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_whitespace() || matches!(c, '{' | '}' | '"') {
                        break;
                    }
                    s.push(c);
                    chars.next();
                }
                tokens.push(Token::Str(s));
            }
        }
    }
    Some(tokens)
}

/// Parse a VDF document into its top-level block. Returns `None` on malformed input.
pub fn parse(text: &str) -> Option<Value> {
    let tokens = tokenize(text.trim_start_matches('\u{feff}'))?;
    let mut pos = 0;
    let items = parse_items(&tokens, &mut pos)?;
    (pos == tokens.len()).then_some(Value::Block(items))
}

fn parse_items(tokens: &[Token], pos: &mut usize) -> Option<Vec<(String, Value)>> {
    let mut items = Vec::new();
    while let Some(tok) = tokens.get(*pos) {
        let key = match tok {
            Token::Str(s) => s.clone(),
            Token::Close => return Some(items),
            Token::Open => return None,
        };
        *pos += 1;
        let value = match tokens.get(*pos)? {
            Token::Str(s) => {
                *pos += 1;
                Value::Str(s.clone())
            }
            Token::Open => {
                *pos += 1;
                let inner = parse_items(tokens, pos)?;
                if tokens.get(*pos) != Some(&Token::Close) {
                    return None;
                }
                *pos += 1;
                Value::Block(inner)
            }
            Token::Close => return None,
        };
        items.push((key, value));
    }
    Some(items)
}

/// Library folder paths listed in a `libraryfolders.vdf`.
///
/// Handles the current format (`"0" { "path" "…" … }`) and the pre-2021 one (`"1" "D:\\Lib"`).
/// Non-numeric keys (`contentstatsid`, `TimeNextStatsReport`, …) are ignored.
pub fn library_folders(text: &str) -> Vec<String> {
    let Some(doc) = parse(text) else {
        return Vec::new();
    };
    let Some(Value::Block(entries)) = doc.get("libraryfolders") else {
        return Vec::new();
    };
    entries
        .iter()
        .filter(|(k, _)| !k.is_empty() && k.bytes().all(|b| b.is_ascii_digit()))
        .filter_map(|(_, v)| match v {
            Value::Str(path) => Some(path.clone()),
            Value::Block(_) => v.get("path").and_then(Value::as_str).map(str::to_owned),
        })
        .filter(|p| !p.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODERN: &str = r#"
"libraryfolders"
{
	"0"
	{
		"path"		"C:\\Program Files (x86)\\Steam"
		"label"		""
		"contentid"		"5136493501455742466"
		"totalsize"		"0"
		"apps"
		{
			"228980"		"0"
		}
	}
	"1"
	{
		"path"		"D:\\SteamLibrary"
		"label"		"Games \"fast\""
		"apps"
		{
			"1086940"		"150000000000"
		}
	}
}
"#;

    #[test]
    fn parses_modern_library_folders() {
        assert_eq!(
            library_folders(MODERN),
            vec![
                r"C:\Program Files (x86)\Steam".to_owned(),
                r"D:\SteamLibrary".to_owned()
            ]
        );
    }

    #[test]
    fn parses_escaped_quotes() {
        let doc = parse(MODERN).unwrap();
        let label = doc
            .get("libraryfolders")
            .and_then(|v| v.get("1"))
            .and_then(|v| v.get("label"))
            .and_then(Value::as_str);
        assert_eq!(label, Some("Games \"fast\""));
    }

    #[test]
    fn parses_legacy_library_folders() {
        let text = r#"
"LibraryFolders"
{
	// legacy format
	"TimeNextStatsReport"		"1600000000"
	"ContentStatsID"		"-123"
	"1"		"/mnt/games/SteamLibrary"
	"2"		"E:\\Games\\Steam"
}
"#;
        assert_eq!(
            library_folders(text),
            vec![
                "/mnt/games/SteamLibrary".to_owned(),
                r"E:\Games\Steam".to_owned()
            ]
        );
    }

    #[test]
    fn unix_paths_and_bom() {
        let text =
            "\u{feff}\"libraryfolders\" { \"0\" { \"path\" \"/home/u/.local/share/Steam\" } }";
        assert_eq!(
            library_folders(text),
            vec!["/home/u/.local/share/Steam".to_owned()]
        );
    }

    #[test]
    fn malformed_input_yields_nothing() {
        assert!(parse("\"a\" {").is_none());
        assert!(parse("\"a\" }").is_none());
        assert!(parse("\"unterminated").is_none());
        assert!(library_folders("garbage {").is_empty());
        assert!(library_folders("\"other\" { \"0\" \"x\" }").is_empty());
    }
}
