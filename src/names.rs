//! Library names: what a citation key or an alias names, looked up in one
//! place for every command that takes one.
//!
//! A declared paper has a citation key and may have an alias; an excluded
//! paper may have an alias. All of them share one namespace, so a name
//! resolves to at most one paper. The lookup tries an exact citation key
//! first, then an exact alias: case-sensitive, like keys. Because names are
//! unique, the order never changes the answer.

use std::path::Path;

use crate::error::PaxError;
use crate::exclusions::{Exclusion, Exclusions};
use crate::library::Library;
use crate::nix;
use crate::paper::Paper;

/// The paper a library name names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Named<'l, 'e> {
    Declared(&'l Paper),
    Excluded(&'e Exclusion),
}

/// The paper `name` names: the declared paper with that citation key or
/// alias, else the excluded paper with that alias.
pub(crate) fn lookup<'l, 'e>(
    name: &str,
    library: &'l Library,
    exclusions: &'e Exclusions,
) -> Option<Named<'l, 'e>> {
    library
        .find(name)
        .or_else(|| library.find_alias(name))
        .map(Named::Declared)
        .or_else(|| exclusions.find_alias(name).map(Named::Excluded))
}

/// The citation key of the declared paper `name` names in `library` (the
/// library at `root`). An alias of an excluded paper is refused with
/// [`PaxError::ExcludedNotDeclared`], anything else unknown with
/// [`PaxError::NoSuchPaper`].
pub(crate) fn declared_key(name: &str, library: &Library, root: &Path) -> Result<String, PaxError> {
    let exclusions = Exclusions::load(&nix::excluded_path(root))?;
    match lookup(name, library, &exclusions) {
        Some(Named::Declared(paper)) => Ok(paper.local.citation_key.clone()),
        Some(Named::Excluded(exclusion)) => Err(PaxError::ExcludedNotDeclared {
            name: name.to_string(),
            key: exclusion.key.clone(),
        }),
        None => Err(PaxError::NoSuchPaper(name.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exclusions::ExclusionStage;
    use crate::paper::Local;

    fn declared(key: &str, alias: Option<&str>) -> Paper {
        Paper {
            local: Local {
                citation_key: key.to_string(),
                alias: alias.map(str::to_string),
                ..Local::default()
            },
            ..Paper::default()
        }
    }

    fn excluded(key: &str, alias: &str) -> Exclusion {
        Exclusion::new(
            key.to_string(),
            "T".to_string(),
            "R".to_string(),
            ExclusionStage::Screening,
        )
        .with_alias(Some(alias.to_string()))
    }

    fn fixture() -> (Library, Exclusions) {
        let library = Library::new(vec![
            declared("turing1936", Some("P17")),
            declared("hewitt1973", None),
        ]);
        let mut exclusions = Exclusions::default();
        exclusions.upsert(excluded("10.1/abc", "P18"));
        (library, exclusions)
    }

    fn key_of<'a>(named: Option<Named<'a, 'a>>) -> Option<&'a str> {
        match named? {
            Named::Declared(paper) => Some(&paper.local.citation_key),
            Named::Excluded(exclusion) => Some(&exclusion.key),
        }
    }

    #[test]
    fn a_citation_key_and_an_alias_name_the_same_declared_paper() {
        let (library, exclusions) = fixture();
        assert_eq!(
            key_of(lookup("turing1936", &library, &exclusions)),
            Some("turing1936")
        );
        assert_eq!(
            key_of(lookup("P17", &library, &exclusions)),
            Some("turing1936")
        );
    }

    #[test]
    fn an_excluded_alias_names_the_exclusion() {
        let (library, exclusions) = fixture();
        assert!(matches!(
            lookup("P18", &library, &exclusions),
            Some(Named::Excluded(e)) if e.key == "10.1/abc"
        ));
    }

    #[test]
    fn names_are_case_sensitive_and_unknown_ones_name_nothing() {
        let (library, exclusions) = fixture();
        assert_eq!(lookup("p17", &library, &exclusions), None);
        assert_eq!(lookup("Turing1936", &library, &exclusions), None);
        assert_eq!(lookup("P19", &library, &exclusions), None);
    }
}
