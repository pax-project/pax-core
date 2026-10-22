use ::crossref::query::ResultControl;
use ::crossref::{Crossref, CrossrefQuery, WorkListQuery, WorkResultControl, Works, WorksQuery};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use super::{
    CandidateId, CandidateWork, Provider, ProviderError, ProviderId, SearchOptions, SearchPage,
    collect_pages,
};

/// Crossref serves at most 1,000 rows per request.
const MAX_PAGE_SIZE: usize = 1000;

/// Crossref rejects offsets past 10,000; going deeper needs cursor paging,
/// which offset-based [`collect_pages`] doesn't drive.
const MAX_RETRIEVABLE: u64 = 10_000;

/// Talks to Crossref's REST API. The `crossref` crate builds the request URLs
/// and supplies the HTTP client, but responses are parsed into this module's
/// own lenient types: the crate's `Work` makes `title`, `publisher`, and other
/// fields required, so a single incomplete record failed a whole page.
pub struct CrossrefProvider {
    client: Crossref,
}

impl CrossrefProvider {
    pub fn new() -> Result<Self, ProviderError> {
        let client = Crossref::builder()
            .build()
            .map_err(|e| ProviderError::Request(e.to_string()))?;
        Ok(CrossrefProvider { client })
    }

    /// Fetches `query`'s route and returns the response's `message`.
    async fn fetch_message<Q: CrossrefQuery, T: DeserializeOwned>(
        &self,
        query: &Q,
    ) -> Result<T, ProviderError> {
        let url = query
            .to_url(&self.client.base_url)
            .map_err(|e| ProviderError::Request(e.to_string()))?;
        let body = self
            .client
            .http_client
            .get(&url)
            .send()
            .await
            .map_err(|e| ProviderError::Request(e.to_string()))?
            .text()
            .await
            .map_err(|e| ProviderError::Request(e.to_string()))?;
        parse_message(&body)
    }
}

impl Provider for CrossrefProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Crossref
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
        collect_pages(
            options,
            MAX_PAGE_SIZE,
            Some(MAX_RETRIEVABLE),
            |offset, rows| {
                let query = WorkListQuery::from(WorksQuery::new(query).result_control(
                    WorkResultControl::Standard(ResultControl::RowsOffset { rows, offset }),
                ));
                async move {
                    let list: WorkList = self.fetch_message(&query).await?;
                    let works = list.items.into_iter().map(CandidateWork::from).collect();
                    Ok((works, list.total_results))
                }
            },
        )
        .await
    }

    async fn get(&self, native_id: &str) -> Result<CandidateWork, ProviderError> {
        let work: Work = self
            .fetch_message(&Works::Identifier(native_id.to_string()))
            .await?;
        Ok(CandidateWork::from(work))
    }

    // No `search_by_author` override: the `crossref` crate's `FieldQuery`
    // doesn't prefix its param key with `query.` (it sends bare `author=...`
    // instead of `query.author=...`), which the real Crossref API rejects as a
    // validation failure — confirmed against the live API, and 0.2.2 is the
    // latest release, so this is a crate defect, not a usage mistake. Falls
    // back to the trait default (plain full-text search) instead, same
    // graceful degradation already accepted for Semantic Scholar.

    async fn get_by_doi(&self, doi: &str) -> Result<CandidateWork, ProviderError> {
        self.get(doi).await
    }
}

/// Parses a Crossref response body into its `message`. Crossref answers an
/// unknown DOI with a plain-text "Resource not found." body, not JSON.
fn parse_message<T: DeserializeOwned>(body: &str) -> Result<T, ProviderError> {
    if body.starts_with("Resource not found") {
        return Err(ProviderError::NotFound);
    }
    let response: Response<T> = serde_json::from_str(body).map_err(|e| {
        let excerpt: String = body.chars().take(200).collect();
        ProviderError::Request(format!("unexpected Crossref response ({e}): {excerpt}"))
    })?;
    Ok(response.message)
}

#[derive(Deserialize)]
struct Response<T> {
    message: T,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
struct WorkList {
    #[serde(default)]
    total_results: u64,
    #[serde(default)]
    items: Vec<Work>,
}

/// Only the fields `CandidateWork` needs, each defaulted when absent.
#[derive(Deserialize, Default)]
#[serde(default, rename_all = "kebab-case")]
struct Work {
    #[serde(rename = "DOI")]
    doi: String,
    title: Vec<String>,
    author: Vec<Contributor>,
    issued: PartialDate,
    link: Vec<ResourceLink>,
    container_title: Vec<String>,
    #[serde(rename = "abstract")]
    abstract_text: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Contributor {
    given: Option<String>,
    family: Option<String>,
    /// Set instead of given/family for organizational authors.
    name: Option<String>,
}

/// e.g. `{"date-parts": [[2020, 3, 1]]}`; parts can be `null` when unknown.
#[derive(Deserialize, Default)]
#[serde(default, rename_all = "kebab-case")]
struct PartialDate {
    date_parts: Vec<Vec<Option<i64>>>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "kebab-case")]
struct ResourceLink {
    #[serde(rename = "URL")]
    url: String,
    content_type: Option<String>,
}

impl From<Work> for CandidateWork {
    fn from(value: Work) -> Self {
        CandidateWork {
            id: CandidateId {
                provider: ProviderId::Crossref,
                native_id: value.doi.clone(),
            },
            title: value.title.into_iter().next().unwrap_or_default(),
            authors: value
                .author
                .into_iter()
                .filter_map(|author| match (author.given, author.family, author.name) {
                    (Some(given), Some(family), _) => Some(format!("{given} {family}")),
                    (None, Some(family), _) => Some(family),
                    (_, None, name) => name,
                })
                .collect(),
            publish_date: value
                .issued
                .date_parts
                .first()
                .map(|parts| {
                    parts
                        .iter()
                        .flatten()
                        .map(|n| n.to_string())
                        .collect::<Vec<_>>()
                        .join("-")
                })
                .unwrap_or_default(),
            doi: (!value.doi.is_empty()).then_some(value.doi),
            pdf_url: value
                .link
                .into_iter()
                .find(|link| link.content_type.as_deref() == Some("application/pdf"))
                .map(|link| link.url),
            venue: value.container_title.into_iter().next(),
            abstract_text: value.abstract_text,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_with_an_untitled_record_still_parses() {
        let body = r#"{
            "status": "ok",
            "message-type": "work-list",
            "message": {
                "total-results": 2,
                "items": [
                    {
                        "DOI": "10.1145/359545.359563",
                        "title": ["Communicating sequential processes"],
                        "author": [{"given": "C. A. R.", "family": "Hoare"}],
                        "issued": {"date-parts": [[1978, 8]]},
                        "container-title": ["Communications of the ACM"],
                        "link": [{"URL": "https://example.org/csp.pdf", "content-type": "application/pdf"}]
                    },
                    {
                        "DOI": "10.5555/untitled",
                        "author": [{"name": "Some Consortium"}],
                        "issued": {"date-parts": [[null]]}
                    }
                ]
            }
        }"#;
        let list: WorkList = parse_message(body).unwrap();
        assert_eq!(list.total_results, 2);
        let works: Vec<CandidateWork> = list.items.into_iter().map(CandidateWork::from).collect();

        assert_eq!(works[0].title, "Communicating sequential processes");
        assert_eq!(works[0].authors, ["C. A. R. Hoare"]);
        assert_eq!(works[0].publish_date, "1978-8");
        assert_eq!(works[0].venue.as_deref(), Some("Communications of the ACM"));
        assert_eq!(
            works[0].pdf_url.as_deref(),
            Some("https://example.org/csp.pdf")
        );

        assert_eq!(works[1].title, "");
        assert_eq!(works[1].authors, ["Some Consortium"]);
        assert_eq!(works[1].publish_date, "");
        assert_eq!(works[1].doi.as_deref(), Some("10.5555/untitled"));
    }

    #[test]
    fn a_single_work_message_parses() {
        let body = r#"{"status": "ok", "message-type": "work", "message": {"DOI": "10.1/x"}}"#;
        let work: Work = parse_message(body).unwrap();
        assert_eq!(CandidateWork::from(work).id.native_id, "10.1/x");
    }

    #[test]
    fn resource_not_found_is_not_found() {
        assert!(matches!(
            parse_message::<Work>("Resource not found."),
            Err(ProviderError::NotFound)
        ));
    }
}
