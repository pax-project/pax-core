use std::time::Duration;

use ::semantic_scholar::{Paper, SemanticScholar};
use reqwest::StatusCode;
use reqwest::header::{HeaderMap, HeaderValue, RETRY_AFTER};
use serde::Deserialize;
use serde::de::DeserializeOwned;

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
    /// For the reference and citation lists, which the `semantic_scholar`
    /// client can't parse: it requires a `paperId` that unmatched entries
    /// lack, so one such entry failed the whole page.
    http: reqwest::Client,
}

impl SemanticScholarProvider {
    pub fn new(api_key: Option<&str>) -> Result<Self, ProviderError> {
        let client = match api_key {
            Some(key) => SemanticScholar::with_api_key(key),
            None => SemanticScholar::new(),
        }
        .map_err(|e| ProviderError::Request(e.to_string()))?;
        let mut headers = HeaderMap::new();
        if let Some(key) = api_key {
            let key = HeaderValue::from_str(key)
                .map_err(|_| ProviderError::Request("invalid Semantic Scholar API key".into()))?;
            headers.insert("x-api-key", key);
        }
        let http = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| ProviderError::Request(e.to_string()))?;
        Ok(SemanticScholarProvider { client, http })
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
            .map_err(seed_error)?;
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
                let path = format!("/paper/{}/{}", seed.paper_id, list.endpoint());
                async move {
                    let query = [
                        ("offset", offset.to_string()),
                        ("limit", limit.to_string()),
                        ("fields", PAPER_FIELDS.to_string()),
                    ];
                    let response: ListResponse = self.get_json(&path, &query).await?;
                    let entries = response.data.ok_or_else(|| {
                        ProviderError::Request(format!(
                            "Semantic Scholar withholds this paper's {} at the publisher's request",
                            list.endpoint()
                        ))
                    })?;
                    let fetched = entries.len();
                    let works = entries
                        .into_iter()
                        .filter_map(ListEntry::into_paper)
                        .filter(|paper| keeps(filters, paper))
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

    /// GETs `path` under the Graph API, retrying a rate-limited or failed
    /// request with backoff the way the `semantic_scholar` client does.
    async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T, ProviderError> {
        let url = format!("{GRAPH_API}{path}");
        let mut backoff = INITIAL_BACKOFF;
        for attempt in 0..=MAX_RETRIES {
            let response = self
                .http
                .get(&url)
                .query(query)
                .send()
                .await
                .map_err(|e| ProviderError::Request(e.to_string()))?;
            let status = response.status();
            if status.is_success() {
                let body = response
                    .text()
                    .await
                    .map_err(|e| ProviderError::Request(e.to_string()))?;
                return serde_json::from_str(&body).map_err(|e| {
                    ProviderError::Request(format!("unexpected Semantic Scholar response: {e}"))
                });
            }
            if status == StatusCode::NOT_FOUND {
                return Err(ProviderError::NotFound);
            }
            let retryable = status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error();
            if retryable && attempt < MAX_RETRIES {
                let wait = response
                    .headers()
                    .get(RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse().ok())
                    .map_or(backoff, Duration::from_secs);
                tokio::time::sleep(wait).await;
                backoff *= 2;
                continue;
            }
            let body = response.text().await.unwrap_or_default();
            let excerpt: String = body.chars().take(200).collect();
            return Err(ProviderError::Request(format!(
                "Semantic Scholar answered {status} for {path}: {excerpt}"
            )));
        }
        unreachable!("the last attempt always returns")
    }
}

const GRAPH_API: &str = "https://api.semanticscholar.org/graph/v1";
const MAX_RETRIES: u32 = 3;
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);

/// Which of a paper's citation lists to page through.
#[derive(Clone, Copy)]
enum CitationList {
    References,
    Citations,
}

impl CitationList {
    fn endpoint(self) -> &'static str {
        match self {
            CitationList::References => "references",
            CitationList::Citations => "citations",
        }
    }
}

/// One page of a reference or citation list. `data` is `null` when the
/// publisher has the list withheld.
#[derive(Deserialize)]
struct ListResponse {
    #[serde(default)]
    data: Option<Vec<ListEntry>>,
}

/// A list entry: the cited paper (references) or the citing one (citations).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListEntry {
    #[serde(default, alias = "citingPaper")]
    cited_paper: Option<serde_json::Value>,
}

impl ListEntry {
    /// The entry's paper, or `None` for one Semantic Scholar couldn't match
    /// to a paper: with no `paperId` it can't be addressed, so it's skipped.
    fn into_paper(self) -> Option<Paper> {
        let paper = self.cited_paper?;
        let has_id = paper
            .get("paperId")
            .and_then(|id| id.as_str())
            .is_some_and(|id| !id.is_empty());
        has_id.then(|| serde_json::from_value(paper).ok()).flatten()
    }
}

/// The reference and citation endpoints serve at most 1,000 papers per
/// request.
const MAX_CITATION_PAGE_SIZE: usize = 1000;

/// The reference and citation endpoints require `offset + limit < 10000`.
const MAX_CITATIONS_RETRIEVABLE: u64 = 9_999;

/// Every field `CandidateWork` reads (but the abstract, which only `get`
/// asks for), plus what post-retrieval filters need. Without it the client
/// asks for its own defaults, which leave out the DOI, date, venue and PDF.
const PAPER_FIELDS: &str =
    "paperId,title,authors,year,publicationDate,externalIds,venue,openAccessPdf,publicationTypes";

/// Maps an error looking up the seed paper: one Semantic Scholar doesn't
/// have is [`ProviderError::NotFound`].
fn seed_error(e: ::semantic_scholar::Error) -> ProviderError {
    match e {
        ::semantic_scholar::Error::Api { status: 404, .. } => ProviderError::NotFound,
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
                    .limit(limit as u32)
                    .fields(PAPER_FIELDS);
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
            .fields(&format!("{PAPER_FIELDS},abstract"))
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
            // An arXiv id always has a PDF behind it; `openAccessPdf` is
            // sometimes a repository landing page instead.
            pdf_url: value
                .external_ids
                .as_ref()
                .and_then(|ids| ids.arxiv.as_deref())
                .map(|arxiv| format!("https://arxiv.org/pdf/{arxiv}"))
                .or_else(|| {
                    value
                        .open_access_pdf
                        .as_ref()
                        .and_then(|pdf| pdf.url.clone())
                }),
            venue: value.venue,
            abstract_text: value.abstract_text,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::semantic_scholar::{ExternalIds, OpenAccessPdf, Paper};

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
    fn list_pages_parse_leniently() {
        // An unmatched reference has a null paperId; a withheld list, null data.
        let body = r#"{"offset": 0, "data": [
            {"citedPaper": {"paperId": "abc", "title": "Matched", "year": 2017,
                            "publicationTypes": null, "externalIds": null}},
            {"citedPaper": {"paperId": null, "title": "Unmatched"}},
            {"citedPaper": null}
        ]}"#;
        let response: ListResponse = serde_json::from_str(body).unwrap();
        let papers: Vec<Paper> = response
            .data
            .unwrap()
            .into_iter()
            .filter_map(ListEntry::into_paper)
            .collect();
        assert_eq!(papers.len(), 1);
        assert_eq!(papers[0].paper_id, "abc");

        let citations = r#"{"data": [{"citingPaper": {"paperId": "def"}}]}"#;
        let response: ListResponse = serde_json::from_str(citations).unwrap();
        let entry = response.data.unwrap().into_iter().next().unwrap();
        assert_eq!(entry.into_paper().unwrap().paper_id, "def");

        let withheld: ListResponse = serde_json::from_str(r#"{"data": null}"#).unwrap();
        assert!(withheld.data.is_none());
    }

    #[test]
    fn a_missing_seed_is_not_found() {
        let missing = seed_error(::semantic_scholar::Error::Api {
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
    fn an_arxiv_id_gives_an_arxiv_pdf_over_open_access_pdf() {
        // ResNet's openAccessPdf is a repository landing page.
        let resnet: Paper = serde_json::from_str(
            r#"{
                "paperId": "2c03df8b48bf3fa39054345bafabfeff15bfd11d",
                "title": "Deep Residual Learning for Image Recognition",
                "externalIds": {"DOI": "10.1109/CVPR.2016.90", "ArXiv": "1512.03385"},
                "openAccessPdf": {
                    "url": "https://repositorio.unal.edu.co/handle/unal/81443",
                    "status": "GREEN"
                }
            }"#,
        )
        .unwrap();
        assert_eq!(
            CandidateWork::from(resnet).pdf_url.as_deref(),
            Some("https://arxiv.org/pdf/1512.03385")
        );
    }

    #[test]
    fn without_an_arxiv_id_pdf_url_stays_open_access_pdf() {
        let mut p = paper("Communicating Sequential Processes");
        p.external_ids = Some(ExternalIds {
            doi: Some("10.1145/359576.359585".to_string()),
            ..Default::default()
        });
        p.open_access_pdf = Some(OpenAccessPdf {
            url: Some("https://example.org/csp.pdf".to_string()),
            status: None,
        });
        assert_eq!(
            CandidateWork::from(p).pdf_url.as_deref(),
            Some("https://example.org/csp.pdf")
        );
    }

    #[test]
    fn every_request_asks_for_external_ids() {
        // Search, lookup and the citation lists all request PAPER_FIELDS;
        // without externalIds there's no DOI and no arXiv PDF.
        assert!(PAPER_FIELDS.split(',').any(|field| field == "externalIds"));
    }

    #[test]
    fn pdf_url_is_none_when_not_open_access() {
        let work = CandidateWork::from(paper("Paywalled Paper"));
        assert_eq!(work.pdf_url, None);
    }
}
