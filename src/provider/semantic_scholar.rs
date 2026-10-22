use ::semantic_scholar::{Paper, SemanticScholar};

use super::{
    AppliedFilter, CandidateId, CandidateWork, DocType, FilterApplication, FilterKind, Filters,
    Provider, ProviderError, ProviderId, RawPage, SearchOptions, SearchPage, collect_pages,
};

/// Semantic Scholar's relevance search serves at most 100 papers per request.
const MAX_PAGE_SIZE: usize = 100;

/// Relevance search requires `offset + limit <= 1000`: nothing past the
/// first 1,000 matches is reachable, however far it's paged.
const MAX_RETRIEVABLE: u64 = 1000;

pub struct SemanticScholarProvider {
    client: SemanticScholar,
}

impl SemanticScholarProvider {
    pub fn new(api_key: Option<&str>) -> Result<Self, ProviderError> {
        let client = match api_key {
            Some(key) => SemanticScholar::with_api_key(key),
            None => SemanticScholar::new(),
        }
        .map_err(|e| ProviderError::Request(e.to_string()))?;
        Ok(SemanticScholarProvider { client })
    }

    /// The papers `paper` cites — its reference list — paged. `paper` is
    /// anything [`Provider::get`] accepts: a paper id, `DOI:<doi>`,
    /// `ARXIV:<id>`, …
    ///
    /// The reference and citation endpoints take no filters, so year and type
    /// are applied after retrieval (see [`citation_filters`]).
    pub async fn references_page(
        &self,
        paper: &str,
        options: &SearchOptions,
    ) -> Result<SearchPage, ProviderError> {
        self.citation_list_page(paper, CitationList::References, options)
            .await
    }

    /// The papers that cite `paper`, paged; as for [`Self::references_page`].
    pub async fn citations_page(
        &self,
        paper: &str,
        options: &SearchOptions,
    ) -> Result<SearchPage, ProviderError> {
        self.citation_list_page(paper, CitationList::Citations, options)
            .await
    }

    async fn citation_list_page(
        &self,
        paper: &str,
        list: CitationList,
        options: &SearchOptions,
    ) -> Result<SearchPage, ProviderError> {
        // The list endpoints report no total; the paper's own counts are it.
        let seed = self
            .client
            .get_paper(paper)
            .fields("paperId,referenceCount,citationCount")
            .send()
            .await
            .map_err(citation_error)?;
        let total = match list {
            CitationList::References => seed.reference_count,
            CitationList::Citations => seed.citation_count,
        };
        let total = u64::from(total.unwrap_or(0));

        let filters = &options.filters;
        let post_filtered = filters.years.is_some() || filters.doc_type.is_some();
        let page = collect_pages(
            options,
            MAX_CITATION_PAGE_SIZE,
            Some(MAX_CITATIONS_RETRIEVABLE),
            post_filtered,
            |offset, limit| {
                let paper_id = seed.paper_id.clone();
                async move {
                    let papers: Vec<Option<Paper>> = match list {
                        CitationList::References => self
                            .client
                            .get_references(&paper_id)
                            .offset(offset as u32)
                            .limit(limit as u32)
                            .fields(CITATION_FIELDS)
                            .send()
                            .await
                            .map_err(citation_error)?
                            .data
                            .into_iter()
                            .map(|reference| reference.cited_paper)
                            .collect(),
                        CitationList::Citations => self
                            .client
                            .get_citations(&paper_id)
                            .offset(offset as u32)
                            .limit(limit as u32)
                            .fields(CITATION_FIELDS)
                            .send()
                            .await
                            .map_err(citation_error)?
                            .data
                            .into_iter()
                            .map(|citation| citation.citing_paper)
                            .collect(),
                    };
                    let fetched = papers.len();
                    // A reference Semantic Scholar couldn't match to a paper
                    // has no id, so it can't be addressed — it's skipped.
                    let works = papers
                        .into_iter()
                        .flatten()
                        .filter(|paper| !paper.paper_id.is_empty() && keeps(filters, paper))
                        .map(CandidateWork::from)
                        .collect();
                    Ok(RawPage {
                        works,
                        total,
                        fetched,
                    })
                }
            },
        )
        .await?;
        Ok(page.with_filters(citation_filters(filters)))
    }
}

/// Which of a paper's citation lists to page through.
#[derive(Clone, Copy)]
enum CitationList {
    References,
    Citations,
}

/// The reference and citation endpoints serve at most 1,000 papers per
/// request.
const MAX_CITATION_PAGE_SIZE: usize = 1000;

/// The reference and citation endpoints require `offset + limit < 10000`.
const MAX_CITATIONS_RETRIEVABLE: u64 = 9_999;

/// Every field `CandidateWork` reads, plus what post-retrieval filters need.
const CITATION_FIELDS: &str =
    "paperId,title,authors,year,publicationDate,externalIds,venue,openAccessPdf,publicationTypes";

/// Maps a citation-list error. A paper missing from Semantic Scholar is
/// [`ProviderError::NotFound`]; a list the publisher withheld comes back as
/// `"data": null`, which the client can't deserialize, so it's named here.
fn citation_error(e: ::semantic_scholar::Error) -> ProviderError {
    match e {
        ::semantic_scholar::Error::Api { status: 404, .. } => ProviderError::NotFound,
        ::semantic_scholar::Error::Deserialize { ref source, .. }
            if source.to_string().starts_with("invalid type: null") =>
        {
            ProviderError::Request(
                "Semantic Scholar withholds this list for this paper at the publisher's request"
                    .to_string(),
            )
        }
        e => ProviderError::Request(e.to_string()),
    }
}

/// How the reference and citation lists apply `filters`: year and (mapped)
/// type after retrieval, since those endpoints take no filters; language
/// not at all, since Semantic Scholar records carry none.
fn citation_filters(filters: &Filters) -> Vec<AppliedFilter> {
    filters
        .kinds()
        .into_iter()
        .map(|kind| {
            let application = match kind {
                FilterKind::Year => FilterApplication::PostRetrieval,
                FilterKind::DocType if filters.doc_type.and_then(publication_type).is_some() => {
                    FilterApplication::PostRetrieval
                }
                _ => FilterApplication::NotApplied,
            };
            AppliedFilter::new(kind, application)
        })
        .collect()
}

/// Whether `paper` passes the year and type filters applied after
/// retrieval. A paper missing the field doesn't match a filter on it.
fn keeps(filters: &Filters, paper: &Paper) -> bool {
    let year_ok = filters.years.is_none_or(|years| {
        paper
            .year
            .and_then(|year| i32::try_from(year).ok())
            .is_some_and(|year| years.contains(year))
    });
    let type_ok = filters
        .doc_type
        .and_then(publication_type)
        .is_none_or(|wanted| {
            paper
                .publication_types
                .as_ref()
                .is_some_and(|types| types.iter().any(|t| t == wanted))
        });
    year_ok && type_ok
}

impl Provider for SemanticScholarProvider {
    fn id(&self) -> ProviderId {
        ProviderId::SemanticScholar
    }

    async fn search(&self, query: &str) -> Result<Vec<CandidateWork>, ProviderError> {
        let page = self.search_page(query, &SearchOptions::default()).await?;
        Ok(page.works)
    }

    async fn search_page(
        &self,
        query: &str,
        options: &SearchOptions,
    ) -> Result<SearchPage, ProviderError> {
        let years = options.filters.years.map(|years| years.to_string());
        let publication_type = options.filters.doc_type.and_then(publication_type);
        let page = collect_pages(
            options,
            MAX_PAGE_SIZE,
            Some(MAX_RETRIEVABLE),
            false,
            |offset, limit| {
                let mut request = self
                    .client
                    .search_papers(query)
                    .offset(offset as u32)
                    .limit(limit as u32);
                if let Some(years) = &years {
                    request = request.year(years);
                }
                if let Some(publication_type) = publication_type {
                    request = request.publication_types(publication_type);
                }
                async move {
                    let result = request
                        .send()
                        .await
                        .map_err(|e| ProviderError::Request(e.to_string()))?;
                    let works = result.data.into_iter().map(CandidateWork::from).collect();
                    Ok(RawPage::new(works, result.total))
                }
            },
        )
        .await?;
        Ok(page.with_filters(applied_filters(&options.filters)))
    }

    async fn get(&self, native_id: &str) -> Result<CandidateWork, ProviderError> {
        let paper = self
            .client
            .get_paper(native_id)
            .send()
            .await
            .map_err(|e| ProviderError::Request(e.to_string()))?;
        Ok(CandidateWork::from(paper))
    }

    async fn get_by_doi(&self, doi: &str) -> Result<CandidateWork, ProviderError> {
        self.get(&format!("DOI:{doi}")).await
    }
}

/// Semantic Scholar's `publicationTypes` value for `doc_type`, if it has an
/// equivalent: it has no preprint or thesis type.
fn publication_type(doc_type: DocType) -> Option<&'static str> {
    match doc_type {
        DocType::JournalArticle => Some("JournalArticle"),
        DocType::ConferencePaper => Some("Conference"),
        DocType::Review => Some("Review"),
        DocType::Book => Some("Book"),
        DocType::BookChapter => Some("BookSection"),
        DocType::Dataset => Some("Dataset"),
        DocType::Preprint | DocType::Thesis => None,
    }
}

/// Year and (mapped) type are filtered server-side; Semantic Scholar records
/// carry no language, so a language filter can't be applied at all.
fn applied_filters(filters: &Filters) -> Vec<AppliedFilter> {
    filters
        .kinds()
        .into_iter()
        .map(|kind| {
            let application = match kind {
                FilterKind::Year => FilterApplication::Server,
                FilterKind::DocType if filters.doc_type.and_then(publication_type).is_some() => {
                    FilterApplication::Server
                }
                _ => FilterApplication::NotApplied,
            };
            AppliedFilter::new(kind, application)
        })
        .collect()
}

impl From<::semantic_scholar::Paper> for CandidateWork {
    fn from(value: ::semantic_scholar::Paper) -> Self {
        CandidateWork {
            id: CandidateId {
                provider: ProviderId::SemanticScholar,
                native_id: value.paper_id.clone(),
            },
            title: value.title.unwrap_or_default(),
            authors: value
                .authors
                .unwrap_or_default()
                .into_iter()
                .filter_map(|author| author.name)
                .collect(),
            publish_date: value.publication_date.unwrap_or_default(),
            doi: value.external_ids.as_ref().and_then(|ids| ids.doi.clone()),
            pdf_url: value
                .open_access_pdf
                .as_ref()
                .and_then(|pdf| pdf.url.clone()),
            venue: value.venue,
            abstract_text: value.abstract_text,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::semantic_scholar::{OpenAccessPdf, Paper};

    #[test]
    fn language_and_unmapped_types_are_not_applied() {
        let filters = Filters::default()
            .with_years("2020".parse().unwrap())
            .with_doc_type(DocType::Preprint)
            .with_language("en");
        let applications: Vec<_> = applied_filters(&filters)
            .into_iter()
            .map(|f| f.application)
            .collect();
        assert_eq!(
            applications,
            [
                FilterApplication::Server,
                FilterApplication::NotApplied,
                FilterApplication::NotApplied,
            ]
        );
        let review = Filters::default().with_doc_type(DocType::Review);
        assert_eq!(
            applied_filters(&review)[0].application,
            FilterApplication::Server
        );
    }

    #[test]
    fn citation_lists_filter_year_and_type_after_retrieval() {
        let filters = Filters::default()
            .with_years("2015-2020".parse().unwrap())
            .with_doc_type(DocType::ConferencePaper)
            .with_language("en");
        let applications: Vec<_> = citation_filters(&filters)
            .into_iter()
            .map(|f| f.application)
            .collect();
        assert_eq!(
            applications,
            [
                FilterApplication::PostRetrieval,
                FilterApplication::PostRetrieval,
                FilterApplication::NotApplied,
            ]
        );

        let mut matching = paper("Hit");
        matching.year = Some(2018);
        matching.publication_types = Some(vec!["Conference".to_string()]);
        assert!(keeps(&filters, &matching));

        let mut too_old = matching.clone();
        too_old.year = Some(2010);
        assert!(!keeps(&filters, &too_old));

        let mut journal = matching.clone();
        journal.publication_types = Some(vec!["JournalArticle".to_string()]);
        assert!(!keeps(&filters, &journal));

        assert!(!keeps(&filters, &paper("No year or type")));
        assert!(keeps(&Filters::default(), &paper("Unfiltered")));
    }

    #[test]
    fn a_withheld_list_is_named_and_a_missing_paper_is_not_found() {
        let null_list = serde_json::from_str::<Vec<u8>>("null").unwrap_err();
        let withheld = citation_error(::semantic_scholar::Error::Deserialize {
            endpoint: "/graph/v1/paper/x/references".to_string(),
            source: null_list,
        });
        assert!(withheld.to_string().contains("withholds"));

        let missing = citation_error(::semantic_scholar::Error::Api {
            status: 404,
            endpoint: "/graph/v1/paper/x".to_string(),
            message: "Paper not found".to_string(),
        });
        assert!(matches!(missing, ProviderError::NotFound));
    }

    fn paper(title: &str) -> Paper {
        Paper {
            title: Some(title.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn pdf_url_comes_from_open_access_pdf() {
        let mut p = paper("On Computable Numbers");
        p.open_access_pdf = Some(OpenAccessPdf {
            url: Some("https://example.org/turing.pdf".to_string()),
            status: None,
        });
        let work = CandidateWork::from(p);
        assert_eq!(
            work.pdf_url.as_deref(),
            Some("https://example.org/turing.pdf")
        );
    }

    #[test]
    fn pdf_url_is_none_when_not_open_access() {
        let work = CandidateWork::from(paper("Paywalled Paper"));
        assert_eq!(work.pdf_url, None);
    }
}
