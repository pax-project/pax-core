use ::crossref::query::ResultControl;
use ::crossref::{Crossref, WorkResultControl, WorksQuery};

use super::{CandidateId, CandidateWork, Provider, ProviderError, ProviderId, SEARCH_RESULT_LIMIT};

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
}

impl Provider for CrossrefProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Crossref
    }

    async fn search(&self, query: &str) -> Result<Vec<CandidateWork>, ProviderError> {
        let query = WorksQuery::new(query).result_control(WorkResultControl::Standard(
            ResultControl::Rows(SEARCH_RESULT_LIMIT),
        ));
        let works = self
            .client
            .works(query)
            .await
            .map_err(|e| ProviderError::Request(e.to_string()))?;
        Ok(works.items.into_iter().map(CandidateWork::from).collect())
    }

    async fn get(&self, native_id: &str) -> Result<CandidateWork, ProviderError> {
        let work = self
            .client
            .work(native_id)
            .await
            .map_err(|e| ProviderError::Request(e.to_string()))?;
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

impl From<::crossref::Work> for CandidateWork {
    fn from(value: ::crossref::Work) -> Self {
        CandidateWork {
            id: CandidateId {
                provider: ProviderId::Crossref,
                native_id: value.doi.clone(),
            },
            title: value.title.into_iter().next().unwrap_or_default(),
            authors: value
                .author
                .unwrap_or_default()
                .into_iter()
                .map(|author| match author.given {
                    Some(given) => format!("{} {}", given, author.family),
                    None => author.family,
                })
                .collect(),
            publish_date: value
                .issued
                .date_parts
                .0
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
            doi: Some(value.doi),
            pdf_url: value.link.as_ref().and_then(|links| {
                links
                    .iter()
                    .find(|link| link.content_type.as_deref() == Some("application/pdf"))
                    .map(|link| link.url.clone())
            }),
            venue: value
                .container_title
                .and_then(|titles| titles.into_iter().next()),
            abstract_text: value.abstract_,
        }
    }
}
