//! The small Nix-attrset subset PAX's library files are written in, shared
//! by `research/papers.nix` (see `library`) and `research/excluded.nix` (see
//! `exclusions`): a top-level attrset of `key = { field = value; ... };`
//! entries, whose values are strings, integers, `null`, or lists of those.
//!
//! Not a general Nix-language parser — anything outside this shape is an
//! error. Each file's own module maps the generic entries this returns onto
//! its typed records, and writes them back with the helpers here.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;

use crate::error::PaxError;

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/// Whether `s` can be written as a bare Nix attribute name per this module's
/// own tokenizer grammar — `(alpha|_)(alnum|_|-)*`, excluding `null`, which
/// the tokenizer reads as a value.
fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    s != "null"
        && matches!(chars.next(), Some(c) if c.is_alphabetic() || c == '_')
        && chars.all(|c| c.is_alphanumeric() || c == '_' || c == '-')
}

/// Renders an entry key: bare when it's a valid identifier, quoted otherwise
/// (DOIs and `provider:id` keys contain `/`, `.`, and `:`).
pub(crate) fn format_key(key: &str) -> String {
    if is_identifier(key) {
        key.to_string()
    } else {
        quote(key)
    }
}

pub(crate) fn write_field_string(out: &mut String, key: &str, value: &str) {
    let _ = writeln!(out, "    {key} = {};", quote(value));
}

pub(crate) fn write_field_string_opt(out: &mut String, key: &str, value: Option<&str>) {
    match value {
        Some(v) => write_field_string(out, key, v),
        None => {
            let _ = writeln!(out, "    {key} = null;");
        }
    }
}

pub(crate) fn write_field_int_opt(out: &mut String, key: &str, value: Option<i32>) {
    match value {
        Some(v) => {
            let _ = writeln!(out, "    {key} = {v};");
        }
        None => {
            let _ = writeln!(out, "    {key} = null;");
        }
    }
}

pub(crate) fn write_field_string_list(out: &mut String, key: &str, values: &[String]) {
    let rendered = values
        .iter()
        .map(|v| quote(v))
        .collect::<Vec<_>>()
        .join(" ");
    let _ = writeln!(out, "    {key} = [ {rendered} ];");
}

pub(crate) fn quote(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for c in value.chars() {
        match c {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            _ => escaped.push(c),
        }
    }
    escaped.push('"');
    escaped
}

/// Written to a sibling temp file and renamed into place rather than
/// `fs::write`ing `path` directly — a plain write truncates the file
/// before the new content is flushed, so a concurrent reader (another
/// `pax`/`lazypax` process against the same library, or `nix build`
/// evaluating the file mid-save) can observe an empty or partial file
/// instead of either the old or new content. `rename` onto an existing path
/// is atomic on the same filesystem, so any concurrent reader always sees
/// one complete version or the other, never a torn one.
pub(crate) fn write_atomically(path: &Path, contents: &str) -> Result<(), PaxError> {
    let tmp_path = path.with_file_name(format!(
        "{}.tmp.{}",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("library.nix"),
        std::process::id()
    ));
    std::fs::write(&tmp_path, contents)?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// Parses a whole file into its `(key, fields)` entries, in file order.
pub(crate) fn parse_entries(text: &str) -> Result<Vec<Entry>, PaxError> {
    let tokens = tokenize(text)?;
    TokenParser::new(&tokens).parse_file()
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Equals,
    Semicolon,
    Ident(String),
    Str(String),
    Int(i64),
    Null,
}

fn tokenize(input: &str) -> Result<Vec<Token>, PaxError> {
    let chars: Vec<char> = input.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            c if c.is_whitespace() => i += 1,
            '#' => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '{' => {
                tokens.push(Token::LBrace);
                i += 1;
            }
            '}' => {
                tokens.push(Token::RBrace);
                i += 1;
            }
            '[' => {
                tokens.push(Token::LBracket);
                i += 1;
            }
            ']' => {
                tokens.push(Token::RBracket);
                i += 1;
            }
            '=' => {
                tokens.push(Token::Equals);
                i += 1;
            }
            ';' => {
                tokens.push(Token::Semicolon);
                i += 1;
            }
            '"' => {
                i += 1;
                let mut s = String::new();
                loop {
                    if i >= chars.len() {
                        return Err(PaxError::LibraryParse(
                            "unterminated string literal".to_string(),
                        ));
                    }
                    match chars[i] {
                        '"' => {
                            i += 1;
                            break;
                        }
                        '\\' if i + 1 < chars.len() => {
                            s.push(chars[i + 1]);
                            i += 2;
                        }
                        other => {
                            s.push(other);
                            i += 1;
                        }
                    }
                }
                tokens.push(Token::Str(s));
            }
            c if c.is_ascii_digit()
                || (c == '-' && chars.get(i + 1).is_some_and(|c| c.is_ascii_digit())) =>
            {
                let start = i;
                if c == '-' {
                    i += 1;
                }
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
                let s: String = chars[start..i].iter().collect();
                let n: i64 = s
                    .parse()
                    .map_err(|_| PaxError::LibraryParse(format!("invalid integer: {s}")))?;
                tokens.push(Token::Int(n));
            }
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '-')
                {
                    i += 1;
                }
                let s: String = chars[start..i].iter().collect();
                if s == "null" {
                    tokens.push(Token::Null);
                } else {
                    tokens.push(Token::Ident(s));
                }
            }
            other => {
                return Err(PaxError::LibraryParse(format!(
                    "unexpected character: {other:?}"
                )));
            }
        }
    }
    Ok(tokens)
}

pub(crate) type Entry = (String, HashMap<String, Value>);

#[derive(Debug, Clone)]
pub(crate) enum Value {
    Str(String),
    Int(i64),
    Null,
    List(Vec<Value>),
}

struct TokenParser<'a> {
    tokens: &'a [Token],
    pos: usize,
}

impl<'a> TokenParser<'a> {
    fn new(tokens: &'a [Token]) -> Self {
        TokenParser { tokens, pos: 0 }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn advance(&mut self) -> Option<&Token> {
        let t = self.tokens.get(self.pos);
        self.pos += 1;
        t
    }

    fn expect(&mut self, expected: &Token) -> Result<(), PaxError> {
        match self.advance() {
            Some(t) if t == expected => Ok(()),
            other => Err(PaxError::LibraryParse(format!(
                "expected {expected:?}, got {other:?}"
            ))),
        }
    }

    fn expect_ident(&mut self) -> Result<String, PaxError> {
        match self.advance().cloned() {
            Some(Token::Ident(s)) => Ok(s),
            other => Err(PaxError::LibraryParse(format!(
                "expected identifier, got {other:?}"
            ))),
        }
    }

    /// An entry key: a bare identifier, or a quoted string for keys that
    /// aren't valid identifiers (DOIs, `provider:id`).
    fn expect_key(&mut self) -> Result<String, PaxError> {
        match self.advance().cloned() {
            Some(Token::Ident(s) | Token::Str(s)) => Ok(s),
            other => Err(PaxError::LibraryParse(format!(
                "expected an entry key, got {other:?}"
            ))),
        }
    }

    fn parse_value(&mut self) -> Result<Value, PaxError> {
        match self.advance().cloned() {
            Some(Token::Str(s)) => Ok(Value::Str(s)),
            Some(Token::Int(n)) => Ok(Value::Int(n)),
            Some(Token::Null) => Ok(Value::Null),
            Some(Token::LBracket) => {
                let mut items = Vec::new();
                while self.peek() != Some(&Token::RBracket) {
                    items.push(self.parse_value()?);
                }
                self.expect(&Token::RBracket)?;
                Ok(Value::List(items))
            }
            other => Err(PaxError::LibraryParse(format!(
                "expected a value, got {other:?}"
            ))),
        }
    }

    fn parse_fields(&mut self) -> Result<HashMap<String, Value>, PaxError> {
        self.expect(&Token::LBrace)?;
        let mut fields = HashMap::new();
        while self.peek() != Some(&Token::RBrace) {
            let key = self.expect_ident()?;
            self.expect(&Token::Equals)?;
            let value = self.parse_value()?;
            self.expect(&Token::Semicolon)?;
            fields.insert(key, value);
        }
        self.expect(&Token::RBrace)?;
        Ok(fields)
    }

    fn parse_file(&mut self) -> Result<Vec<Entry>, PaxError> {
        self.expect(&Token::LBrace)?;
        let mut entries = Vec::new();
        while self.peek() != Some(&Token::RBrace) {
            let key = self.expect_key()?;
            self.expect(&Token::Equals)?;
            let fields = self.parse_fields()?;
            self.expect(&Token::Semicolon)?;
            entries.push((key, fields));
        }
        self.expect(&Token::RBrace)?;
        if self.pos != self.tokens.len() {
            return Err(PaxError::LibraryParse(
                "trailing content after closing brace".to_string(),
            ));
        }
        Ok(entries)
    }
}

pub(crate) fn take_string_opt(
    fields: &mut HashMap<String, Value>,
    key: &str,
) -> Result<Option<String>, PaxError> {
    match fields.remove(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Str(s)) => Ok(Some(s)),
        Some(other) => Err(PaxError::LibraryParse(format!(
            "field {key:?} expected a string or null, got {other:?}"
        ))),
    }
}

pub(crate) fn take_string(
    fields: &mut HashMap<String, Value>,
    key: &str,
) -> Result<String, PaxError> {
    take_string_opt(fields, key)?
        .ok_or_else(|| PaxError::LibraryParse(format!("missing required field {key:?}")))
}

pub(crate) fn take_int_opt(
    fields: &mut HashMap<String, Value>,
    key: &str,
) -> Result<Option<i32>, PaxError> {
    match fields.remove(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Int(n)) => Ok(Some(n as i32)),
        Some(other) => Err(PaxError::LibraryParse(format!(
            "field {key:?} expected an integer or null, got {other:?}"
        ))),
    }
}

pub(crate) fn take_string_list(
    fields: &mut HashMap<String, Value>,
    key: &str,
) -> Result<Vec<String>, PaxError> {
    match fields.remove(key) {
        None => Ok(Vec::new()),
        Some(Value::List(items)) => items
            .into_iter()
            .map(|item| match item {
                Value::Str(s) => Ok(s),
                other => Err(PaxError::LibraryParse(format!(
                    "field {key:?} expected a list of strings, got an item {other:?}"
                ))),
            })
            .collect(),
        Some(other) => Err(PaxError::LibraryParse(format!(
            "field {key:?} expected a list, got {other:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_and_quoted_entry_keys() {
        let entries = parse_entries(
            r#"{
  turing1936 = { title = "On Computable Numbers"; };
  "10.1145/359545.359563" = { title = "Time, Clocks"; };
  "openalex:W2072794470" = { title = "Actors"; };
}"#,
        )
        .unwrap();
        let keys: Vec<&str> = entries.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            keys,
            [
                "turing1936",
                "10.1145/359545.359563",
                "openalex:W2072794470"
            ]
        );
    }

    #[test]
    fn quoted_keys_unescape() {
        let entries = parse_entries(r#"{ "a\"b" = { }; }"#).unwrap();
        assert_eq!(entries[0].0, "a\"b");
    }

    #[test]
    fn field_names_must_stay_bare_identifiers() {
        assert!(parse_entries(r#"{ k = { "title" = "x"; }; }"#).is_err());
    }
}
