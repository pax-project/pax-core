//! Paper identity across providers: whether two records — from different
//! providers, or the backward and forward lists of a snowballing round — are
//! the same paper.
//!
//! A record yields up to three [`IdentityKey`]s, in order of strength:
//!
//! 1. its **DOI**, normalized (URL/`doi:` prefixes stripped, lowercased) —
//!    except arXiv's own DOIs (`10.48550/arXiv.<id>`), which become an arXiv
//!    key instead;
//! 2. its **arXiv id**, without version suffix (`2301.01234v2` →
//!    `2301.01234`), taken from an arXiv provider id, an arXiv DOI, or an
//!    `arxiv.org/abs|pdf/` PDF link (OpenAlex often points at arXiv's copy);
//! 3. its **normalized title + year**: title lowercased, markup stripped,
//!    every run of non-alphanumerics collapsed to one space — exact match
//!    only, no fuzzy matching, so "Part I" and "Part II" stay distinct. Needs
//!    both a non-empty title and a year.
//!
//! Two records are the same paper when they share any key ([`group`]), with
//! one guard: a title+year match never joins records carrying *different*
//! DOIs — those are distinct reports (e.g. a conference and a journal
//! version, or an erratum) that happen to share a title. arXiv DOIs don't
//! count for the guard: they identify the preprint, not a separate
//! publication, so a preprint still joins its published version. A record
//! with no key at all is never merged with anything.

use std::collections::HashMap;
use std::fmt;

use crate::normalize_doi;
use crate::paper::year_from_publish_date;
use crate::provider::{CandidateWork, ProviderId};

/// One way of identifying a paper. See the module docs for how each is
/// normalized.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum IdentityKey {
    Doi(String),
    Arxiv(String),
    TitleYear { title: String, year: i32 },
}

impl fmt::Display for IdentityKey {
    /// A stable, prefixed text form (`doi:…`, `arxiv:…`, `title:…|year`),
    /// e.g. for storing keys in a log.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IdentityKey::Doi(doi) => write!(f, "doi:{doi}"),
            IdentityKey::Arxiv(id) => write!(f, "arxiv:{id}"),
            IdentityKey::TitleYear { title, year } => write!(f, "title:{title}|{year}"),
        }
    }
}

/// Every identity key `work` has, strongest first (DOI, arXiv id,
/// title+year). Empty when the record has none — it can't be matched.
pub fn identity_keys(work: &CandidateWork) -> Vec<IdentityKey> {
    let mut keys = Vec::new();

    let doi = work
        .doi
        .as_deref()
        .map(normalize_doi_key)
        .filter(|doi| !doi.is_empty());
    let mut arxiv = None;
    if let Some(doi) = doi {
        match arxiv_id_from_doi(&doi) {
            Some(id) => arxiv = Some(id),
            None => keys.push(IdentityKey::Doi(doi)),
        }
    }
    if arxiv.is_none() && work.id.provider == ProviderId::ArXiv {
        arxiv = Some(strip_arxiv_version(&work.id.native_id).to_string());
    }
    if arxiv.is_none() {
        arxiv = work.pdf_url.as_deref().and_then(arxiv_id_from_url);
    }
    if let Some(id) = arxiv.filter(|id| !id.is_empty()) {
        keys.push(IdentityKey::Arxiv(id));
    }

    let title = normalize_title(&work.title);
    if let (false, Some(year)) = (title.is_empty(), year_from_publish_date(&work.publish_date)) {
        keys.push(IdentityKey::TitleYear { title, year });
    }
    keys
}

/// The strongest identity key `work` has, or `None` if it has none.
pub fn identity_key(work: &CandidateWork) -> Option<IdentityKey> {
    identity_keys(work).into_iter().next()
}

/// Groups records by identity: given each record's keys (from
/// [`identity_keys`]), returns one group index per record, in the same
/// order. Records with the same index are the same paper; group indices are
/// numbered by first appearance. A record with no keys always gets its own
/// group.
///
/// Works on keys alone, so it can group records that are no longer in memory
/// (e.g. keys read back from a search log).
pub fn group(key_sets: &[Vec<IdentityKey>]) -> Vec<usize> {
    let mut sets = DisjointSets::new(key_sets.len());

    // Shared DOI or arXiv id: always the same paper.
    let mut first_with: HashMap<&IdentityKey, usize> = HashMap::new();
    for (record, keys) in key_sets.iter().enumerate() {
        for key in keys {
            if matches!(key, IdentityKey::TitleYear { .. }) {
                continue;
            }
            match first_with.get(key) {
                Some(&other) => sets.union(other, record),
                None => {
                    first_with.insert(key, record);
                }
            }
        }
    }

    // Shared title+year: the same paper, unless that would join two
    // different DOIs (arXiv DOIs ignored: they name the preprint).
    let mut by_title: HashMap<&IdentityKey, Vec<usize>> = HashMap::new();
    for (record, keys) in key_sets.iter().enumerate() {
        for key in keys {
            if matches!(key, IdentityKey::TitleYear { .. }) {
                by_title.entry(key).or_default().push(record);
            }
        }
    }
    let mut buckets: Vec<Vec<usize>> = by_title.into_values().collect();
    buckets.sort(); // deterministic merge order
    for bucket in buckets {
        for &record in &bucket[1..] {
            let candidate = bucket[0];
            if sets.find(candidate) == sets.find(record) {
                continue;
            }
            let dois_a = sets.dois(candidate, key_sets);
            let dois_b = sets.dois(record, key_sets);
            if dois_a.is_empty() || dois_b.is_empty() || dois_a == dois_b {
                sets.union(candidate, record);
            }
        }
    }

    // Number groups by first appearance.
    let mut numbering: HashMap<usize, usize> = HashMap::new();
    (0..key_sets.len())
        .map(|record| {
            let root = sets.find(record);
            let next = numbering.len();
            *numbering.entry(root).or_insert(next)
        })
        .collect()
}

/// Lowercases a DOI and strips the URL and `doi:` prefixes providers put in
/// front of it (on top of [`normalize_doi`]'s `https://doi.org/`).
fn normalize_doi_key(doi: &str) -> String {
    let doi = normalize_doi(doi.trim());
    let lower = doi.to_lowercase();
    let stripped = [
        "http://doi.org/",
        "https://dx.doi.org/",
        "http://dx.doi.org/",
        "doi:",
    ]
    .iter()
    .find_map(|prefix| lower.strip_prefix(prefix))
    .unwrap_or(&lower);
    stripped.trim().to_string()
}

/// Whether a DOI, in any of the forms providers return, is an arXiv DOI
/// (`10.48550/arXiv.*`) — i.e. names a preprint, not a published version.
pub(crate) fn is_arxiv_doi(doi: &str) -> bool {
    arxiv_id_from_doi(&normalize_doi_key(doi)).is_some()
}

/// `10.48550/arxiv.2301.01234` → `2301.01234` (expects a normalized DOI).
fn arxiv_id_from_doi(doi: &str) -> Option<String> {
    doi.strip_prefix("10.48550/arxiv.")
        .map(|id| strip_arxiv_version(id).to_string())
}

/// `https://arxiv.org/pdf/2301.01234v2` (or `/abs/`, optionally `.pdf`) →
/// `2301.01234`.
fn arxiv_id_from_url(url: &str) -> Option<String> {
    let (_, rest) = url.split_once("arxiv.org/")?;
    let id = rest
        .strip_prefix("abs/")
        .or_else(|| rest.strip_prefix("pdf/"))?;
    let id = id.split(['?', '#']).next().unwrap_or(id);
    let id = id.strip_suffix(".pdf").unwrap_or(id);
    Some(strip_arxiv_version(id).to_lowercase()).filter(|id| !id.is_empty())
}

/// `2301.01234v2` → `2301.01234`; `hep-th/9901001v1` → `hep-th/9901001`.
fn strip_arxiv_version(id: &str) -> &str {
    match id.rfind('v') {
        Some(pos)
            if pos > 0
                && pos + 1 < id.len()
                && id[pos + 1..].bytes().all(|b| b.is_ascii_digit()) =>
        {
            &id[..pos]
        }
        _ => id,
    }
}

/// Lowercases, drops `<…>` markup (Crossref titles carry `<i>` etc.), and
/// collapses every run of non-alphanumeric characters to a single space.
fn normalize_title(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut in_tag = false;
    let mut pending_space = false;
    for c in title.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if in_tag => {}
            c if c.is_alphanumeric() => {
                if pending_space && !out.is_empty() {
                    out.push(' ');
                }
                pending_space = false;
                out.extend(c.to_lowercase());
            }
            _ => pending_space = true,
        }
    }
    out
}

/// Union-find over record indices.
struct DisjointSets {
    parent: Vec<usize>,
}

impl DisjointSets {
    fn new(len: usize) -> Self {
        DisjointSets {
            parent: (0..len).collect(),
        }
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    /// Joins the two groups; the root with the lower index stays the root, so
    /// grouping is independent of union order.
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            let (keep, merge) = if ra < rb { (ra, rb) } else { (rb, ra) };
            self.parent[merge] = keep;
        }
    }

    /// Every DOI key held by any record in `record`'s group, sorted —
    /// except arXiv DOIs, which name the preprint rather than a separate
    /// publication.
    fn dois<'a>(&mut self, record: usize, key_sets: &'a [Vec<IdentityKey>]) -> Vec<&'a str> {
        let root = self.find(record);
        let mut dois: Vec<&str> = (0..key_sets.len())
            .filter(|&other| self.find(other) == root)
            .flat_map(|other| key_sets[other].iter())
            .filter_map(|key| match key {
                IdentityKey::Doi(doi) if arxiv_id_from_doi(&doi.to_lowercase()).is_none() => {
                    Some(doi.as_str())
                }
                _ => None,
            })
            .collect();
        dois.sort_unstable();
        dois.dedup();
        dois
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::CandidateId;

    fn work(
        provider: ProviderId,
        native_id: &str,
        title: &str,
        date: &str,
        doi: Option<&str>,
    ) -> CandidateWork {
        CandidateWork {
            id: CandidateId {
                provider,
                native_id: native_id.to_string(),
            },
            title: title.to_string(),
            authors: Vec::new(),
            publish_date: date.to_string(),
            doi: doi.map(str::to_string),
            pdf_url: None,
            venue: None,
            abstract_text: None,
        }
    }

    fn groups(works: &[CandidateWork]) -> Vec<usize> {
        let keys: Vec<_> = works.iter().map(identity_keys).collect();
        group(&keys)
    }

    #[test]
    fn doi_variants_normalize_to_one_key() {
        let a = work(
            ProviderId::OpenAlex,
            "W1",
            "X",
            "2020",
            Some("https://doi.org/10.1145/ABC.1"),
        );
        let b = work(
            ProviderId::Crossref,
            "10.1145/abc.1",
            "X",
            "2020",
            Some("10.1145/abc.1"),
        );
        let c = work(
            ProviderId::SemanticScholar,
            "s2",
            "X",
            "2020",
            Some("doi:10.1145/Abc.1"),
        );
        assert_eq!(
            identity_key(&a),
            Some(IdentityKey::Doi("10.1145/abc.1".into()))
        );
        assert_eq!(identity_key(&a), identity_key(&b));
        assert_eq!(identity_key(&a), identity_key(&c));
    }

    #[test]
    fn arxiv_ids_come_from_provider_ids_dois_and_pdf_links_without_versions() {
        let from_provider = work(ProviderId::ArXiv, "2301.01234v3", "X", "2023", None);
        let from_doi = work(
            ProviderId::OpenAlex,
            "W1",
            "X",
            "2023",
            Some("https://doi.org/10.48550/arXiv.2301.01234"),
        );
        let mut from_pdf = work(ProviderId::OpenAlex, "W2", "X", "2023", None);
        from_pdf.pdf_url = Some("https://arxiv.org/pdf/2301.01234v1.pdf".into());
        let old_style = work(ProviderId::ArXiv, "hep-th/9901001v1", "X", "1999", None);

        let expected = Some(IdentityKey::Arxiv("2301.01234".into()));
        assert_eq!(identity_key(&from_provider), expected);
        assert_eq!(identity_key(&from_doi), expected);
        assert_eq!(identity_key(&from_pdf), expected);
        assert_eq!(
            identity_key(&old_style),
            Some(IdentityKey::Arxiv("hep-th/9901001".into()))
        );
    }

    #[test]
    fn titles_normalize_case_punctuation_and_markup() {
        let a = work(
            ProviderId::Crossref,
            "c",
            "<i>E. coli</i>: A  Study!",
            "2020-05-01",
            None,
        );
        assert_eq!(
            identity_key(&a),
            Some(IdentityKey::TitleYear {
                title: "e coli a study".into(),
                year: 2020
            })
        );
    }

    #[test]
    fn records_without_title_or_year_and_without_ids_have_no_key() {
        assert!(identity_keys(&work(ProviderId::OpenAlex, "W1", "", "2020", None)).is_empty());
        assert!(identity_keys(&work(ProviderId::OpenAlex, "W1", "Title", "", None)).is_empty());
    }

    #[test]
    fn preprint_and_published_version_merge_via_arxiv_id() {
        let preprint = work(
            ProviderId::ArXiv,
            "2301.01234v2",
            "Attention Is Fine",
            "2023-01-04",
            None,
        );
        let mut published = work(
            ProviderId::OpenAlex,
            "W9",
            "Attention is fine",
            "2024-03-01",
            Some("10.1000/journal.5"),
        );
        published.pdf_url = Some("https://arxiv.org/abs/2301.01234".into());
        assert_eq!(groups(&[preprint, published]), vec![0, 0]);
    }

    #[test]
    fn preprint_and_published_version_merge_via_title_and_year() {
        let preprint = work(
            ProviderId::ArXiv,
            "2301.01234",
            "Attention Is Fine",
            "2023-01-04",
            None,
        );
        let published = work(
            ProviderId::Crossref,
            "10.1000/j.5",
            "Attention is fine.",
            "2023-09-01",
            Some("10.1000/j.5"),
        );
        assert_eq!(groups(&[preprint, published]), vec![0, 0]);
    }

    #[test]
    fn similar_titles_do_not_merge() {
        let part1 = work(
            ProviderId::OpenAlex,
            "W1",
            "Actor Semantics, Part I",
            "1977",
            None,
        );
        let part2 = work(
            ProviderId::OpenAlex,
            "W2",
            "Actor Semantics, Part II",
            "1977",
            None,
        );
        assert_eq!(groups(&[part1, part2]), vec![0, 1]);
    }

    #[test]
    fn same_title_and_year_with_different_dois_do_not_merge() {
        let conference = work(
            ProviderId::Crossref,
            "a",
            "A Study",
            "2020",
            Some("10.1/conf"),
        );
        let journal = work(
            ProviderId::Crossref,
            "b",
            "A Study",
            "2020",
            Some("10.1/journal"),
        );
        assert_eq!(groups(&[conference, journal]), vec![0, 1]);
    }

    #[test]
    fn title_match_does_not_bridge_two_different_dois_through_a_doi_less_record() {
        let a = work(ProviderId::Crossref, "a", "A Study", "2020", Some("10.1/a"));
        let untitled = work(ProviderId::ArXiv, "2001.00001", "A Study", "2020", None);
        let b = work(ProviderId::Crossref, "b", "A Study", "2020", Some("10.1/b"));
        let grouped = groups(&[a, untitled, b]);
        assert_ne!(grouped[0], grouped[2]);
    }

    #[test]
    fn arxiv_doi_does_not_block_title_match_with_published_doi() {
        let title = IdentityKey::TitleYear {
            title: "a study".into(),
            year: 2020,
        };
        let preprint = vec![
            IdentityKey::Doi("10.48550/arXiv.2001.00001".into()),
            title.clone(),
        ];
        let published = vec![IdentityKey::Doi("10.1/journal".into()), title];
        assert_eq!(group(&[preprint, published]), vec![0, 0]);
    }

    #[test]
    fn different_non_arxiv_dois_still_block_title_match() {
        let title = IdentityKey::TitleYear {
            title: "a study".into(),
            year: 2020,
        };
        let conference = vec![
            IdentityKey::Doi("10.48550/arxiv.2001.00001".into()),
            IdentityKey::Doi("10.1/conf".into()),
            title.clone(),
        ];
        let journal = vec![IdentityKey::Doi("10.1/journal".into()), title];
        assert_eq!(group(&[conference, journal]), vec![0, 1]);
    }

    #[test]
    fn keyless_records_are_never_grouped() {
        let a = work(ProviderId::OpenAlex, "W1", "", "", None);
        let b = work(ProviderId::OpenAlex, "W2", "", "", None);
        assert_eq!(groups(&[a, b]), vec![0, 1]);
    }

    #[test]
    fn keys_display_with_stable_prefixes() {
        assert_eq!(IdentityKey::Doi("10.1/x".into()).to_string(), "doi:10.1/x");
        assert_eq!(
            IdentityKey::Arxiv("2301.01234".into()).to_string(),
            "arxiv:2301.01234"
        );
        assert_eq!(
            IdentityKey::TitleYear {
                title: "a study".into(),
                year: 2020
            }
            .to_string(),
            "title:a study|2020"
        );
    }
}
