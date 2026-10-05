/// A citation key identifying a paper already declared in the local library
/// (e.g. `turing1936`). Distinct from [`crate::provider::CandidateId`]: this
/// addresses the closed set of papers PAX already knows about, not the open
/// set of anything a provider might return.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PaperRef(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Identity {
    pub doi: Option<String>,
    pub title: String,
    pub authors: Vec<String>,
    pub year: Option<i32>,
    pub venue: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Artifact {
    pub source_url: Option<String>,
    pub hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Local {
    pub citation_key: String,
    pub tags: Vec<String>,
    pub notes: Option<String>,
}

/// A paper declared in the local library, per the three-part shape from
/// pax-project/.github's mvp.md: identity (what the paper is), artifact (where/how Nix
/// fetches it), and local metadata (how the user organizes it).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Paper {
    pub identity: Identity,
    pub artifact: Artifact,
    pub local: Local,
}

/// Extracts a year from a provider's `publish_date`, whose format varies
/// (`"1986-01-01"`, an RFC3339 timestamp, or a possibly-empty Crossref
/// string) — takes the leading 4 characters and requires they parse as a
/// plausible year.
pub(crate) fn year_from_publish_date(publish_date: &str) -> Option<i32> {
    let digits: String = publish_date.chars().take(4).collect();
    if digits.len() != 4 {
        return None;
    }
    digits.parse().ok()
}

/// Applies incremental tag add/remove, an optional notes overwrite, and an
/// optional citation-key rename to a paper's local metadata. Adding an
/// already-present tag is a no-op (no duplicates); removing an absent tag is
/// a no-op (no error) — both safe to call again. Rename validity and
/// uniqueness are the caller's responsibility (`lib.rs::edit_paper`) — this
/// function just performs the assignment.
pub(crate) fn apply_local_edits(
    local: &mut Local,
    add_tags: &[String],
    remove_tags: &[String],
    notes: Option<&str>,
    rename: Option<&str>,
) {
    for tag in add_tags {
        if !local.tags.contains(tag) {
            local.tags.push(tag.clone());
        }
    }
    local.tags.retain(|t| !remove_tags.contains(t));
    if let Some(notes) = notes {
        local.notes = Some(notes.to_string());
    }
    if let Some(new_key) = rename {
        local.citation_key = new_key.to_string();
    }
}

/// Sets or replaces a declared paper's PDF source URL — the one `Artifact`
/// field a user can usefully correct by hand: a provider often finds no
/// open-access copy at `add` time (`source_url` stays `None`), and the user
/// may since have found one themselves (a preprint mirror, an author's
/// homepage, ...). `None` leaves it untouched, same convention as
/// `apply_identity_corrections`.
///
/// Deliberately does *not* touch `hash`: changing the source invalidates any
/// previously fetched artifact, but recomputing that hash means re-fetching
/// (network I/O), which isn't this function's job — the caller (`edit_paper`)
/// clears `hash` so the next `fetch`/`open` re-materializes against the new
/// URL instead of silently keeping stale bytes.
pub(crate) fn apply_artifact_edits(artifact: &mut Artifact, source_url: Option<&str>) {
    if let Some(source_url) = source_url {
        artifact.source_url = Some(source_url.to_string());
        artifact.hash = None;
    }
}

/// Applies explicit Identity corrections — each `Some` overwrites, `None`
/// leaves the field untouched. `authors` is a full replace when given, not an
/// incremental add/remove like tags: an author-list correction means the list
/// was wrong, not that one name needs adding.
pub(crate) fn apply_identity_corrections(
    identity: &mut Identity,
    title: Option<&str>,
    authors: Option<&[String]>,
    year: Option<i32>,
    doi: Option<&str>,
) {
    if let Some(title) = title {
        identity.title = title.to_string();
    }
    if let Some(authors) = authors {
        identity.authors = authors.to_vec();
    }
    if let Some(year) = year {
        identity.year = Some(year);
    }
    if let Some(doi) = doi {
        identity.doi = Some(doi.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_artifact_edits_sets_source_url_and_clears_a_stale_hash() {
        let mut artifact = Artifact {
            source_url: Some("https://old.example/paper.pdf".to_string()),
            hash: Some("sha256-old".to_string()),
        };
        apply_artifact_edits(&mut artifact, Some("https://new.example/paper.pdf"));
        assert_eq!(
            artifact.source_url,
            Some("https://new.example/paper.pdf".to_string())
        );
        assert!(
            artifact.hash.is_none(),
            "a changed source invalidates the old hash"
        );
    }

    #[test]
    fn apply_artifact_edits_is_a_no_op_with_none() {
        let original = Artifact {
            source_url: Some("https://example/paper.pdf".to_string()),
            hash: Some("sha256-x".to_string()),
        };
        let mut artifact = original.clone();
        apply_artifact_edits(&mut artifact, None);
        assert_eq!(artifact, original);
    }

    #[test]
    fn year_from_publish_date_parses_leading_digits() {
        assert_eq!(year_from_publish_date("1986-01-01"), Some(1986));
        assert_eq!(
            year_from_publish_date("2022-03-23T14:33:15+00:00"),
            Some(2022)
        );
    }

    #[test]
    fn year_from_publish_date_rejects_short_or_non_numeric() {
        assert_eq!(year_from_publish_date(""), None);
        assert_eq!(year_from_publish_date("198"), None);
        assert_eq!(year_from_publish_date("actor model"), None);
    }

    #[test]
    fn apply_edits_add_is_idempotent() {
        let mut local = Local::default();
        apply_local_edits(&mut local, &["a".to_string()], &[], None, None);
        apply_local_edits(&mut local, &["a".to_string()], &[], None, None);
        assert_eq!(local.tags, vec!["a".to_string()]);
    }

    #[test]
    fn apply_edits_remove_absent_tag_is_a_no_op() {
        let mut local = Local {
            tags: vec!["a".to_string()],
            ..Default::default()
        };
        apply_local_edits(&mut local, &[], &["b".to_string()], None, None);
        assert_eq!(local.tags, vec!["a".to_string()]);
    }

    #[test]
    fn apply_edits_add_and_remove_combine() {
        let mut local = Local {
            tags: vec!["a".to_string()],
            ..Default::default()
        };
        apply_local_edits(
            &mut local,
            &["b".to_string()],
            &["a".to_string()],
            None,
            None,
        );
        assert_eq!(local.tags, vec!["b".to_string()]);
    }

    #[test]
    fn apply_edits_overwrites_notes() {
        let mut local = Local {
            notes: Some("old".to_string()),
            ..Default::default()
        };
        apply_local_edits(&mut local, &[], &[], Some("new"), None);
        assert_eq!(local.notes, Some("new".to_string()));
    }

    #[test]
    fn apply_edits_renames_citation_key() {
        let mut local = Local {
            citation_key: "old2020".to_string(),
            ..Default::default()
        };
        apply_local_edits(&mut local, &[], &[], None, Some("new2020"));
        assert_eq!(local.citation_key, "new2020");
    }

    #[test]
    fn apply_identity_corrections_overwrites_only_given_fields() {
        let mut identity = Identity {
            doi: Some("10.1/old".to_string()),
            title: "Old Title".to_string(),
            authors: vec!["Old Author".to_string()],
            year: Some(2000),
            venue: None,
        };
        apply_identity_corrections(&mut identity, Some("New Title"), None, None, None);
        assert_eq!(identity.title, "New Title");
        assert_eq!(identity.authors, vec!["Old Author".to_string()]);
        assert_eq!(identity.year, Some(2000));
        assert_eq!(identity.doi, Some("10.1/old".to_string()));
    }

    #[test]
    fn apply_identity_corrections_replaces_whole_author_list() {
        let mut identity = Identity {
            authors: vec!["A".to_string(), "B".to_string()],
            ..Default::default()
        };
        apply_identity_corrections(&mut identity, None, Some(&["C".to_string()]), None, None);
        assert_eq!(identity.authors, vec!["C".to_string()]);
    }

    #[test]
    fn apply_identity_corrections_is_a_no_op_with_all_none() {
        let original = Identity {
            doi: Some("10.1/x".to_string()),
            title: "T".to_string(),
            authors: vec!["A".to_string()],
            year: Some(1999),
            venue: Some("V".to_string()),
        };
        let mut identity = original.clone();
        apply_identity_corrections(&mut identity, None, None, None, None);
        assert_eq!(identity, original);
    }
}
