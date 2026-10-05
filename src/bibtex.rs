//! Renders declared papers as BibTeX. Every entry is emitted as `@article`
//! — `Paper` has no venue/type field to infer `@inproceedings`/`@book`/etc.
//! from, and `@article` is the overwhelmingly common case for what the four
//! providers return; entry-type inference is a deferred non-goal.

use crate::paper::Paper;

pub fn render(papers: &[Paper]) -> String {
    papers
        .iter()
        .map(render_entry)
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn render_entry(paper: &Paper) -> String {
    let mut fields = Vec::new();
    if !paper.identity.authors.is_empty() {
        fields.push(format!(
            "  author = {{{}}}",
            escape(&paper.identity.authors.join(" and "))
        ));
    }
    fields.push(format!("  title = {{{}}}", escape(&paper.identity.title)));
    if let Some(year) = paper.identity.year {
        fields.push(format!("  year = {{{year}}}"));
    }
    if let Some(doi) = &paper.identity.doi {
        fields.push(format!("  doi = {{{}}}", escape(doi)));
    }
    if !paper.local.tags.is_empty() {
        fields.push(format!(
            "  keywords = {{{}}}",
            escape(&paper.local.tags.join(", "))
        ));
    }
    if let Some(notes) = &paper.local.notes {
        fields.push(format!("  note = {{{}}}", escape(notes)));
    }
    format!(
        "@article{{{},\n{}\n}}",
        paper.local.citation_key,
        fields.join(",\n")
    )
}

fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('{', "\\{")
        .replace('}', "\\}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paper::{Artifact, Identity, Local};

    fn full_paper() -> Paper {
        Paper {
            identity: Identity {
                doi: Some("10.1145/37401.37406".to_string()),
                title: "Actors: A Model".to_string(),
                authors: vec!["Gul Agha".to_string(), "Carl Hewitt".to_string()],
                year: Some(1986),
                venue: None,
            },
            artifact: Artifact::default(),
            local: Local {
                citation_key: "agha1986".to_string(),
                tags: vec!["concurrency".to_string(), "distributed".to_string()],
                notes: Some("foundational paper".to_string()),
            },
        }
    }

    fn minimal_paper() -> Paper {
        Paper {
            identity: Identity {
                doi: None,
                title: "A minimal paper".to_string(),
                authors: vec![],
                year: None,
                venue: None,
            },
            artifact: Artifact::default(),
            local: Local {
                citation_key: "anon2020".to_string(),
                tags: vec![],
                notes: None,
            },
        }
    }

    #[test]
    fn renders_full_paper_with_all_fields() {
        let bibtex = render(&[full_paper()]);
        assert_eq!(
            bibtex,
            "@article{agha1986,\n  \
             author = {Gul Agha and Carl Hewitt},\n  \
             title = {Actors: A Model},\n  \
             year = {1986},\n  \
             doi = {10.1145/37401.37406},\n  \
             keywords = {concurrency, distributed},\n  \
             note = {foundational paper}\n}"
        );
    }

    #[test]
    fn renders_minimal_paper_omitting_absent_fields() {
        let bibtex = render(&[minimal_paper()]);
        assert_eq!(bibtex, "@article{anon2020,\n  title = {A minimal paper}\n}");
    }

    #[test]
    fn escapes_braces_and_backslashes_in_field_values() {
        let mut paper = minimal_paper();
        paper.identity.title = "A {special} title\\path".to_string();
        let bibtex = render(&[paper]);
        assert!(bibtex.contains("title = {A \\{special\\} title\\\\path}"));
    }

    #[test]
    fn joins_multiple_entries_with_a_blank_line() {
        let bibtex = render(&[minimal_paper(), full_paper()]);
        assert!(bibtex.contains("}\n\n@article{"));
    }

    #[test]
    fn empty_library_renders_empty_string() {
        assert_eq!(render(&[]), "");
    }
}
