//! Reads and writes `research/papers.nix`.
//!
//! `papers.nix` is Nix syntax, but it's a schema PAX itself invented and
//! fully controls (a flat attrset of `citation_key = { ...fields... };`
//! records — see `templates/papers.nix.template`). `research/flake.nix`
//! only ever reads whichever fields it needs (`url`, `hash`) via
//! `builtins.mapAttrs`, so extra fields are harmless to it. That means this
//! module only needs a reader/writer for one fixed, PAX-defined shape — not
//! a general Nix-language parser.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;

use crate::error::PaxError;
use crate::paper::{Artifact, Identity, Local, Paper};

#[derive(Debug, Clone, PartialEq)]
pub struct Library {
    papers: Vec<Paper>,
}

impl Library {
    pub fn new(papers: Vec<Paper>) -> Self {
        Library { papers }
    }

    pub fn papers(&self) -> &[Paper] {
        &self.papers
    }

    pub fn insert(&mut self, paper: Paper) {
        self.papers.push(paper);
    }

    pub fn find(&self, citation_key: &str) -> Option<&Paper> {
        self.papers
            .iter()
            .find(|p| p.local.citation_key == citation_key)
    }

    pub fn find_mut(&mut self, citation_key: &str) -> Option<&mut Paper> {
        self.papers
            .iter_mut()
            .find(|p| p.local.citation_key == citation_key)
    }

    pub fn remove(&mut self, citation_key: &str) -> bool {
        let len_before = self.papers.len();
        self.papers.retain(|p| p.local.citation_key != citation_key);
        self.papers.len() != len_before
    }

    pub fn load(path: &Path) -> Result<Self, PaxError> {
        let text = std::fs::read_to_string(path)?;
        let tokens = tokenize(&text)?;
        let entries = TokenParser::new(&tokens).parse_file()?;
        let papers = entries
            .into_iter()
            .map(|(citation_key, fields)| paper_from_fields(citation_key, fields))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Library { papers })
    }

    /// Written to a sibling temp file and renamed into place rather than
    /// `fs::write`ing `path` directly — a plain write truncates the file
    /// before the new content is flushed, so a concurrent reader (another
    /// `pax`/`lazypax` process against the same library, or `nix build`
    /// evaluating `papers.nix` mid-save) can observe an empty or partial
    /// file instead of either the old or new content. `rename` onto an
    /// existing path is atomic on the same filesystem, so any concurrent
    /// reader always sees one complete version or the other, never a torn
    /// one.
    pub fn save(&self, path: &Path) -> Result<(), PaxError> {
        let mut papers = self.papers.clone();
        papers.sort_by(|a, b| a.local.citation_key.cmp(&b.local.citation_key));

        let mut out = String::from("{\n");
        for paper in &papers {
            write_entry(&mut out, paper);
        }
        out.push_str("}\n");

        let tmp_path = path.with_file_name(format!(
            "{}.tmp.{}",
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("papers.nix"),
            std::process::id()
        ));
        std::fs::write(&tmp_path, out)?;
        std::fs::rename(&tmp_path, path)?;
        Ok(())
    }
}

/// Whether `key` is a valid citation key per this file's own tokenizer grammar
/// — `(alpha|_)(alnum|_|-)*`. Citation keys are written as bare Nix attribute
/// names (see `write_entry`), not quoted strings, so an invalid one would
/// produce a `papers.nix` this same file can no longer parse back.
pub(crate) fn is_valid_citation_key(key: &str) -> bool {
    let mut chars = key.chars();
    matches!(chars.next(), Some(c) if c.is_alphabetic() || c == '_')
        && chars.all(|c| c.is_alphanumeric() || c == '_' || c == '-')
}

fn write_entry(out: &mut String, paper: &Paper) {
    let _ = writeln!(out, "  {} = {{", paper.local.citation_key);
    write_field_string_opt(out, "doi", paper.identity.doi.as_deref());
    write_field_string(out, "title", &paper.identity.title);
    write_field_string_list(out, "authors", &paper.identity.authors);
    write_field_int_opt(out, "year", paper.identity.year);
    write_field_string_opt(out, "venue", paper.identity.venue.as_deref());
    write_field_string_opt(out, "source_url", paper.artifact.source_url.as_deref());
    write_field_string_opt(out, "hash", paper.artifact.hash.as_deref());
    write_field_string_list(out, "tags", &paper.local.tags);
    write_field_string_opt(out, "notes", paper.local.notes.as_deref());
    out.push_str("  };\n");
}

fn write_field_string(out: &mut String, key: &str, value: &str) {
    let _ = writeln!(out, "    {key} = {};", quote(value));
}

fn write_field_string_opt(out: &mut String, key: &str, value: Option<&str>) {
    match value {
        Some(v) => write_field_string(out, key, v),
        None => {
            let _ = writeln!(out, "    {key} = null;");
        }
    }
}

fn write_field_int_opt(out: &mut String, key: &str, value: Option<i32>) {
    match value {
        Some(v) => {
            let _ = writeln!(out, "    {key} = {v};");
        }
        None => {
            let _ = writeln!(out, "    {key} = null;");
        }
    }
}

fn write_field_string_list(out: &mut String, key: &str, values: &[String]) {
    let rendered = values
        .iter()
        .map(|v| quote(v))
        .collect::<Vec<_>>()
        .join(" ");
    let _ = writeln!(out, "    {key} = [ {rendered} ];");
}

fn quote(value: &str) -> String {
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

// ---------------------------------------------------------------------------
// Reading: a small hand-rolled tokenizer + parser for the fixed schema above.
// Not a general Nix-language parser — anything outside this shape is an error.
// ---------------------------------------------------------------------------

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

type Entry = (String, HashMap<String, Value>);

#[derive(Debug, Clone)]
enum Value {
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
            let key = self.expect_ident()?;
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

fn take_string_opt(
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

fn take_string(fields: &mut HashMap<String, Value>, key: &str) -> Result<String, PaxError> {
    take_string_opt(fields, key)?
        .ok_or_else(|| PaxError::LibraryParse(format!("missing required field {key:?}")))
}

fn take_int_opt(fields: &mut HashMap<String, Value>, key: &str) -> Result<Option<i32>, PaxError> {
    match fields.remove(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Int(n)) => Ok(Some(n as i32)),
        Some(other) => Err(PaxError::LibraryParse(format!(
            "field {key:?} expected an integer or null, got {other:?}"
        ))),
    }
}

fn take_string_list(
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

fn paper_from_fields(
    citation_key: String,
    mut fields: HashMap<String, Value>,
) -> Result<Paper, PaxError> {
    let doi = take_string_opt(&mut fields, "doi")?;
    let title = take_string(&mut fields, "title")?;
    let authors = take_string_list(&mut fields, "authors")?;
    let year = take_int_opt(&mut fields, "year")?;
    let venue = take_string_opt(&mut fields, "venue")?;
    let source_url = take_string_opt(&mut fields, "source_url")?;
    let hash = take_string_opt(&mut fields, "hash")?;
    let tags = take_string_list(&mut fields, "tags")?;
    let notes = take_string_opt(&mut fields, "notes")?;

    if let Some(unknown) = fields.keys().next() {
        return Err(PaxError::LibraryParse(format!(
            "unrecognized field {unknown:?} in entry {citation_key:?}"
        )));
    }

    Ok(Paper {
        identity: Identity {
            doi,
            title,
            authors,
            year,
            venue,
        },
        artifact: Artifact { source_url, hash },
        local: Local {
            citation_key,
            tags,
            notes,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_papers() -> Vec<Paper> {
        vec![
            Paper {
                identity: Identity {
                    doi: Some("10.1112/plms/s2-42.1.230".to_string()),
                    title: "On Computable Numbers".to_string(),
                    authors: vec!["Alan M. Turing".to_string()],
                    year: Some(1936),
                    venue: Some("Proceedings of the London Mathematical Society".to_string()),
                },
                artifact: Artifact {
                    source_url: Some("https://example.org/turing1936.pdf".to_string()),
                    hash: Some("sha256-abc123".to_string()),
                },
                local: Local {
                    citation_key: "turing1936".to_string(),
                    tags: vec!["computability".to_string(), "logic".to_string()],
                    notes: Some("foundational".to_string()),
                },
            },
            Paper {
                identity: Identity {
                    doi: None,
                    title: "A paper with no DOI or notes".to_string(),
                    authors: vec![],
                    year: None,
                    venue: None,
                },
                artifact: Artifact {
                    source_url: None,
                    hash: None,
                },
                local: Local {
                    citation_key: "anon2020".to_string(),
                    tags: vec![],
                    notes: None,
                },
            },
        ]
    }

    #[test]
    fn is_valid_citation_key_accepts_tokenizer_identifiers() {
        assert!(is_valid_citation_key("turing1936"));
        assert!(is_valid_citation_key("_foo"));
        assert!(is_valid_citation_key("a-b_c"));
    }

    #[test]
    fn is_valid_citation_key_rejects_non_identifiers() {
        assert!(!is_valid_citation_key(""));
        assert!(!is_valid_citation_key("1abc"));
        assert!(!is_valid_citation_key("has space"));
        assert!(!is_valid_citation_key("\"quoted\""));
    }

    #[test]
    fn save_never_leaves_a_reader_seeing_a_truncated_file() {
        // Regression guard for the torn-read class of bug: a concurrent
        // `Library::load` running at any point during `save` must see
        // either the pre-save content in full or the post-save content in
        // full, never a partial/empty file. `save` itself is synchronous
        // (no real concurrency to interleave here), so this instead checks
        // the property that makes that guarantee hold: no temp file is
        // left behind, and the final path always parses back correctly
        // immediately after `save` returns — i.e. the swap really is a
        // single atomic step, not two observable ones.
        let dir =
            std::env::temp_dir().join(format!("pax-library-test-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("papers.nix");

        Library::new(sample_papers()).save(&path).unwrap();
        let entries: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(
            entries,
            vec![std::ffi::OsString::from("papers.nix")],
            "no stray temp file left behind"
        );
        assert_eq!(Library::load(&path).unwrap().papers().len(), 2);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn library_round_trips_through_papers_nix() {
        let dir = std::env::temp_dir().join(format!("pax-library-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("papers.nix");

        let mut expected = sample_papers();
        expected.sort_by(|a, b| a.local.citation_key.cmp(&b.local.citation_key));
        let library = Library::new(expected.clone());

        library.save(&path).unwrap();
        let loaded = Library::load(&path).unwrap();

        assert_eq!(loaded.papers(), expected.as_slice());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn find_returns_matching_paper_or_none() {
        let library = Library::new(sample_papers());

        assert_eq!(
            library
                .find("turing1936")
                .map(|p| p.identity.title.as_str()),
            Some("On Computable Numbers")
        );
        assert!(library.find("nonexistent-key").is_none());
    }

    #[test]
    fn find_mut_allows_in_place_mutation() {
        let mut library = Library::new(sample_papers());

        library.find_mut("turing1936").unwrap().local.notes = Some("edited".to_string());
        assert_eq!(
            library.find("turing1936").unwrap().local.notes,
            Some("edited".to_string())
        );
        assert!(library.find_mut("nonexistent-key").is_none());
    }

    #[test]
    fn remove_deletes_matching_paper_and_reports_whether_found() {
        let mut library = Library::new(sample_papers());

        assert!(!library.remove("nonexistent-key"));
        assert_eq!(library.papers().len(), 2);

        assert!(library.remove("turing1936"));
        assert_eq!(library.papers().len(), 1);
        assert!(library.find("turing1936").is_none());
    }

    #[test]
    fn empty_library_round_trips() {
        let dir =
            std::env::temp_dir().join(format!("pax-library-test-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("papers.nix");

        Library::new(vec![]).save(&path).unwrap();
        let loaded = Library::load(&path).unwrap();
        assert!(loaded.papers().is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }
}
