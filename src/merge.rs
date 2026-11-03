//! Merging duplicate records — the same paper returned by several providers,
//! or twice by one — into one row each, with the counts a review reports
//! ("duplicates removed"). Sameness is decided by [`crate::identity`].

use std::collections::HashMap;

use crate::identity::{self, IdentityKey};
use crate::provider::{CandidateId, CandidateWork, ProviderId, SearchPage};

/// Provider order used when merging per-provider results: the order results
/// are reported in, and the tie-break for which record represents a group.
/// Every provider is in it, so a slice: a new provider extends it without
/// changing its type.
pub const MERGE_ORDER: &[ProviderId] = &[
    ProviderId::OpenAlex,
    ProviderId::Crossref,
    ProviderId::SemanticScholar,
    ProviderId::ArXiv,
    ProviderId::Dblp,
];

/// The first four providers of [`MERGE_ORDER`], from before dblp was added;
/// its array type fixes its length, so it can't grow with new providers.
#[deprecated(
    since = "3.1.0",
    note = "lacks dblp; use `MERGE_ORDER`, which lists every provider"
)]
pub const PROVIDER_ORDER: [ProviderId; 4] = [
    ProviderId::OpenAlex,
    ProviderId::Crossref,
    ProviderId::SemanticScholar,
    ProviderId::ArXiv,
];

/// One paper after merging: a representative record plus every other record
/// found to be the same paper.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct MergedWork {
    /// The representative record: a published one if the group has any (a
    /// record with a venue, or with a DOI that isn't an arXiv
    /// `10.48550/arXiv.*` DOI), since that's the version `add` declares and
    /// users cite. Within the same class the most complete record wins (most
    /// of DOI, PDF link, abstract, venue, authors, date, title present), then
    /// the earlier record — with [`merge_pages`], the earlier provider in
    /// [`MERGE_ORDER`]. Kept exactly as its provider returned it, so its
    /// `id` still resolves with `show`/`add`.
    pub work: CandidateWork,
    /// The other records merged into this one, in input order.
    pub duplicates: Vec<CandidateWork>,
    /// The group's strongest identity key; `None` for a record that had no
    /// key and so couldn't be matched.
    pub key: Option<IdentityKey>,
}

impl MergedWork {
    /// Every record in the group, representative first.
    pub fn records(&self) -> impl Iterator<Item = &CandidateWork> {
        std::iter::once(&self.work).chain(&self.duplicates)
    }

    /// Every record's id, representative first.
    pub fn ids(&self) -> Vec<&CandidateId> {
        self.records().map(|work| &work.id).collect()
    }

    /// The providers the group's records came from, without repeats, in the
    /// order they first appear.
    pub fn providers(&self) -> Vec<ProviderId> {
        let mut providers = Vec::new();
        for work in self.records() {
            if !providers.contains(&work.id.provider) {
                providers.push(work.id.provider);
            }
        }
        providers
    }

    /// The group's abstract: the first non-empty one among its records,
    /// taking providers in [`MERGE_ORDER`] (and records of one provider
    /// representative first) — not necessarily the representative's own,
    /// which may have none.
    pub fn abstract_text(&self) -> Option<&str> {
        let rank = |work: &CandidateWork| {
            MERGE_ORDER
                .iter()
                .position(|provider| *provider == work.id.provider)
                .unwrap_or(MERGE_ORDER.len())
        };
        let mut records: Vec<&CandidateWork> = self.records().collect();
        records.sort_by_key(|work| rank(work));
        records
            .into_iter()
            .filter_map(|work| work.abstract_text.as_deref())
            .find(|text| !text.trim().is_empty())
    }
}

/// The outcome of a merge: one [`MergedWork`] per distinct paper, plus counts.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct MergeReport {
    /// One entry per distinct paper, in order of first appearance.
    pub works: Vec<MergedWork>,
    /// Records before merging.
    pub before: usize,
    /// Records after merging (`works.len()`).
    pub after: usize,
    /// Records with no identity key (no DOI, no arXiv id, and no title+year),
    /// which can't be matched and are kept as their own rows.
    pub unmatched: usize,
}

impl MergeReport {
    /// Records removed as duplicates (`before - after`).
    pub fn duplicates(&self) -> usize {
        self.before - self.after
    }
}

/// Merges records that are the same paper. Input order matters only for
/// output order and tie-breaks; grouping itself doesn't depend on it.
pub fn merge_works(works: impl IntoIterator<Item = CandidateWork>) -> MergeReport {
    merge_indexed(works).0
}

/// [`merge_works`], also returning, for each merged work, the input
/// position of each of its records in [`MergedWork::records`] order — so a
/// caller can say where each record came from.
pub(crate) fn merge_indexed(
    works: impl IntoIterator<Item = CandidateWork>,
) -> (MergeReport, Vec<Vec<usize>>) {
    let works: Vec<CandidateWork> = works.into_iter().collect();
    let key_sets: Vec<Vec<IdentityKey>> = works.iter().map(identity::identity_keys).collect();
    let groups = identity::group(&key_sets);
    let unmatched = key_sets.iter().filter(|keys| keys.is_empty()).count();
    let before = works.len();

    let group_count = groups.iter().copied().max().map_or(0, |max| max + 1);
    let mut members: Vec<Vec<Record>> = vec![Vec::new(); group_count];
    for (position, ((work, keys), group)) in works.into_iter().zip(key_sets).zip(groups).enumerate()
    {
        members[group].push((work, keys, position));
    }

    let (merged, positions): (Vec<MergedWork>, Vec<Vec<usize>>) =
        members.into_iter().map(merge_group).unzip();
    let report = MergeReport {
        after: merged.len(),
        works: merged,
        before,
        unmatched,
    };
    (report, positions)
}

/// Merges the successful pages of a paged search (e.g.
/// [`crate::search_all_paged`]) or of a citation lookup
/// ([`crate::references`], [`crate::cited_by`]), taking providers in
/// [`MERGE_ORDER`]. Failed providers contribute nothing; their errors stay
/// in `results` for the caller to report.
pub fn merge_pages<E>(results: &HashMap<ProviderId, Result<SearchPage, E>>) -> MergeReport {
    merge_works(
        MERGE_ORDER
            .iter()
            .filter_map(|provider| results.get(provider)?.as_ref().ok())
            .flat_map(|page| page.works.iter().cloned()),
    )
}

/// A record being merged: the work, its identity keys, and its input position.
type Record = (CandidateWork, Vec<IdentityKey>, usize);

/// Merges one group, returning the merged work and its records' input
/// positions, representative first.
fn merge_group(mut records: Vec<Record>) -> (MergedWork, Vec<usize>) {
    // Strongest key across the group: a DOI beats an arXiv id beats title+year.
    let key = records
        .iter()
        .flat_map(|(_, keys, _)| keys.iter())
        .min_by_key(|key| key_strength(key))
        .cloned();
    // `max_by_key` keeps the last maximum; reversing makes ties go to the
    // earliest record.
    let representative = records
        .iter()
        .enumerate()
        .rev()
        .max_by_key(|(_, (work, _, _))| (is_published(work), completeness(work)))
        .map(|(index, _)| index)
        .unwrap_or(0);
    let (work, _, first) = records.remove(representative);
    let positions = std::iter::once(first)
        .chain(records.iter().map(|(_, _, position)| *position))
        .collect();
    let merged = MergedWork {
        work,
        duplicates: records.into_iter().map(|(work, _, _)| work).collect(),
        key,
    };
    (merged, positions)
}

fn key_strength(key: &IdentityKey) -> u8 {
    match key {
        IdentityKey::Doi(_) => 0,
        IdentityKey::Arxiv(_) => 1,
        _ => 2,
    }
}

/// A published version: has a venue, or a DOI that isn't an arXiv DOI.
fn is_published(work: &CandidateWork) -> bool {
    work.venue.is_some()
        || work
            .doi
            .as_deref()
            .is_some_and(|doi| !identity::is_arxiv_doi(doi))
}

fn completeness(work: &CandidateWork) -> usize {
    [
        work.doi.is_some(),
        work.pdf_url.is_some(),
        work.abstract_text.is_some(),
        work.venue.is_some(),
        !work.authors.is_empty(),
        !work.publish_date.is_empty(),
        !work.title.is_empty(),
    ]
    .into_iter()
    .filter(|present| *present)
    .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ProviderError;

    fn work(
        provider: ProviderId,
        native_id: &str,
        title: &str,
        doi: Option<&str>,
    ) -> CandidateWork {
        CandidateWork {
            id: CandidateId {
                provider,
                native_id: native_id.to_string(),
            },
            title: title.to_string(),
            authors: Vec::new(),
            publish_date: "2020-01-01".to_string(),
            doi: doi.map(str::to_string),
            pdf_url: None,
            venue: None,
            abstract_text: None,
        }
    }

    #[test]
    fn counts_duplicates_and_unmatched() {
        let mut keyless = work(ProviderId::OpenAlex, "W3", "", None);
        keyless.publish_date.clear();
        let report = merge_works([
            work(ProviderId::OpenAlex, "W1", "A Study", Some("10.1/a")),
            work(
                ProviderId::Crossref,
                "10.1/a",
                "A study.",
                Some("https://doi.org/10.1/a"),
            ),
            work(ProviderId::SemanticScholar, "s1", "A Study", Some("10.1/A")),
            work(ProviderId::OpenAlex, "W2", "Another Study", None),
            keyless,
        ]);
        assert_eq!(report.before, 5);
        assert_eq!(report.after, 3);
        assert_eq!(report.duplicates(), 2);
        assert_eq!(report.unmatched, 1);
        assert_eq!(report.works[0].key, Some(IdentityKey::Doi("10.1/a".into())));
        assert_eq!(
            report.works[0].providers(),
            [
                ProviderId::OpenAlex,
                ProviderId::Crossref,
                ProviderId::SemanticScholar
            ]
        );
        assert_eq!(report.works[2].key, None);
    }

    #[test]
    fn most_complete_record_represents_the_group() {
        let sparse = work(ProviderId::OpenAlex, "W1", "A Study", Some("10.1/a"));
        let mut rich = work(ProviderId::Crossref, "10.1/a", "A Study", Some("10.1/a"));
        rich.venue = Some("Journal".into());
        rich.abstract_text = Some("…".into());
        let report = merge_works([sparse, rich]);
        assert_eq!(report.works[0].work.id.provider, ProviderId::Crossref);
        assert_eq!(
            report.works[0].duplicates[0].id.provider,
            ProviderId::OpenAlex
        );
    }

    #[test]
    fn published_record_beats_a_more_complete_preprint() {
        let mut preprint = work(
            ProviderId::ArXiv,
            "2001.00001",
            "A Study",
            Some("10.48550/arXiv.2001.00001"),
        );
        preprint.pdf_url = Some("https://arxiv.org/pdf/2001.00001".into());
        preprint.abstract_text = Some("…".into());
        let published = work(ProviderId::Crossref, "10.1/a", "A Study", Some("10.1/a"));
        let report = merge_works([preprint, published]);
        assert_eq!(report.after, 1);
        assert_eq!(report.works[0].work.id.provider, ProviderId::Crossref);
    }

    #[test]
    fn more_complete_published_record_wins_among_published() {
        let sparse = work(ProviderId::OpenAlex, "W1", "A Study", Some("10.1/a"));
        let mut rich = work(ProviderId::Crossref, "10.1/a", "A Study", Some("10.1/a"));
        rich.pdf_url = Some("https://example.org/a.pdf".into());
        let report = merge_works([sparse, rich]);
        assert_eq!(report.works[0].work.id.provider, ProviderId::Crossref);
    }

    #[test]
    fn more_complete_record_wins_when_none_is_published() {
        let sparse = work(ProviderId::OpenAlex, "W1", "A Study", None);
        let mut rich = work(
            ProviderId::ArXiv,
            "2001.00001",
            "A Study",
            Some("10.48550/arXiv.2001.00001"),
        );
        rich.pdf_url = Some("https://arxiv.org/pdf/2001.00001".into());
        let report = merge_works([sparse, rich]);
        assert_eq!(report.after, 1);
        assert_eq!(report.works[0].work.id.provider, ProviderId::ArXiv);
    }

    #[test]
    fn ties_go_to_the_earlier_record() {
        let report = merge_works([
            work(ProviderId::OpenAlex, "W1", "A Study", Some("10.1/a")),
            work(ProviderId::Crossref, "10.1/a", "A Study", Some("10.1/a")),
        ]);
        assert_eq!(report.works[0].work.id.provider, ProviderId::OpenAlex);
    }

    #[test]
    fn merge_pages_takes_providers_in_order_and_skips_failures() {
        let mut results = HashMap::new();
        results.insert(
            ProviderId::ArXiv,
            Ok(SearchPage::new(
                vec![work(ProviderId::ArXiv, "2001.00001", "A Study", None)],
                Some(1),
                None,
            )),
        );
        results.insert(
            ProviderId::OpenAlex,
            Ok(SearchPage::new(
                vec![work(ProviderId::OpenAlex, "W1", "A Study", None)],
                Some(1),
                None,
            )),
        );
        results.insert(ProviderId::Crossref, Err(ProviderError::NotFound));
        let report = merge_pages(&results);
        assert_eq!(report.before, 2);
        assert_eq!(report.after, 1);
        assert_eq!(
            report.works[0].providers(),
            [ProviderId::OpenAlex, ProviderId::ArXiv]
        );
    }

    #[test]
    fn the_abstract_is_the_first_non_empty_one_in_provider_order() {
        let mut published = work(ProviderId::SemanticScholar, "s1", "A Study", Some("10.1/a"));
        published.venue = Some("Journal".into());
        published.abstract_text = Some("From Semantic Scholar.".into());
        let mut preprint = work(ProviderId::ArXiv, "2001.00001", "A Study", Some("10.1/a"));
        preprint.abstract_text = Some("From arXiv.".into());
        let mut blank = work(ProviderId::Crossref, "10.1/a", "A Study", Some("10.1/a"));
        blank.abstract_text = Some("  ".into());
        let none = work(ProviderId::OpenAlex, "W1", "A Study", Some("10.1/a"));

        let report = merge_works([preprint.clone(), published.clone(), blank, none]);
        assert_eq!(report.after, 1);
        assert_eq!(
            report.works[0].abstract_text(),
            Some("From Semantic Scholar.")
        );

        // Even when a later provider's record represents the group.
        let mut openalex = work(ProviderId::OpenAlex, "W1", "A Study", Some("10.1/a"));
        openalex.abstract_text = Some("From OpenAlex.".into());
        let report = merge_works([published, openalex, preprint]);
        assert_eq!(
            report.works[0].work.id.provider,
            ProviderId::SemanticScholar
        );
        assert_eq!(report.works[0].abstract_text(), Some("From OpenAlex."));

        let bare = merge_works([work(ProviderId::OpenAlex, "W2", "Other", None)]);
        assert_eq!(bare.works[0].abstract_text(), None);
    }

    #[test]
    fn empty_input_gives_empty_report() {
        let report = merge_works(Vec::new());
        assert_eq!((report.before, report.after, report.unmatched), (0, 0, 0));
    }
}
