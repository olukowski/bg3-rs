//! Text-key groups: `A;B;Cast2[C;D];CastOffhand[E]`.
//!
//! Functor lists and roll conditions can be split into groups keyed by an animation text event
//! (`Cast2`, `Cast3`, `CastOffhand`, …); everything outside brackets belongs to the default key.
// bg3: (anonymous)::SplitStringPerTextKey (0x102015fe8, 4.1.1.7631656), used through
// TextKeyParsingHelpers::SplitStringPerTextKeyWithMerge (functors) and …NoMerge (roll
// conditions).

/// The key of text outside any `Key[...]` group (`eoc::strDefault`).
pub const DEFAULT_KEY: &str = "Default";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TextKeyError {
    #[error("nested '[' in a text-key group")]
    Nested,
    #[error("']' without a group")]
    StrayClose,
    #[error("unclosed text-key group '{0}'")]
    Unclosed(String),
}

fn trim(s: &str) -> &str {
    s.trim_matches([' ', '\t', '\n', '\r'])
}

/// Split into `(key, content)` groups, in first-appearance order. Groups with the same key are
/// merged with `;` (both the default bucket and repeated keys).
pub fn split(s: &str) -> Result<Vec<(String, String)>, TextKeyError> {
    let mut groups: Vec<(String, String)> = Vec::new();
    fn add(groups: &mut Vec<(String, String)>, key: &str, content: &str) {
        if content.is_empty() {
            if !groups.iter().any(|(k, _)| k == key) && key != DEFAULT_KEY {
                groups.push((key.to_owned(), String::new()));
            }
            return;
        }
        match groups.iter_mut().find(|(k, _)| k == key) {
            Some((_, c)) if c.is_empty() => *c = content.to_owned(),
            Some((_, c)) => {
                c.push(';');
                c.push_str(content);
            }
            None => groups.push((key.to_owned(), content.to_owned())),
        }
    }
    if !s.contains('[') {
        if s.contains(']') {
            return Err(TextKeyError::StrayClose);
        }
        let t = trim(s);
        if !t.is_empty() {
            groups.push((DEFAULT_KEY.to_owned(), t.to_owned()));
        }
        return Ok(groups);
    }
    let mut seg_start = 0;
    let mut open: Option<(String, usize)> = None;
    for (i, c) in s.char_indices() {
        match c {
            '[' => {
                if open.is_some() {
                    return Err(TextKeyError::Nested);
                }
                open = Some((trim(&s[seg_start..i]).to_owned(), i + 1));
            }
            ']' => {
                let Some((key, start)) = open.take() else {
                    return Err(TextKeyError::StrayClose);
                };
                add(&mut groups, &key, trim(&s[start..i]));
                seg_start = i + 1;
            }
            ';' if open.is_none() => {
                add(&mut groups, DEFAULT_KEY, trim(&s[seg_start..i]));
                seg_start = i + 1;
            }
            _ => {}
        }
    }
    if let Some((key, _)) = open {
        return Err(TextKeyError::Unclosed(key));
    }
    add(&mut groups, DEFAULT_KEY, trim(&s[seg_start..]));
    Ok(groups)
}
