use arxiv_tools::{ArXiv, Client, QueryParams};

use super::{CandidateId, CandidateWork, Provider, ProviderError, ProviderId, SEARCH_RESULT_LIMIT};

pub struct ArxivProvider {
    client: Client,
}

impl ArxivProvider {
    pub fn new(contact: Option<&str>) -> Result<Self, ProviderError> {
        let mut builder = Client::builder();
        if let Some(contact) = contact {
            builder = builder.user_agent(format!(
                "pax-core/{} (+{contact})",
                env!("CARGO_PKG_VERSION")
            ));
        }
        let client = builder
            .build()
            .map_err(|e| ProviderError::Request(e.to_string()))?;
        Ok(ArxivProvider { client })
    }
}

impl Provider for ArxivProvider {
    fn id(&self) -> ProviderId {
        ProviderId::ArXiv
    }

    async fn search(&self, query: &str) -> Result<Vec<CandidateWork>, ProviderError> {
        let query =
            ArXiv::from_args(QueryParams::title(query)).max_results(SEARCH_RESULT_LIMIT as u64);
        let papers = self
            .client
            .fetch(&query)
            .await
            .map_err(|e| ProviderError::Request(e.to_string()))?;
        Ok(papers.into_iter().map(CandidateWork::from).collect())
    }

    async fn get(&self, native_id: &str) -> Result<CandidateWork, ProviderError> {
        let query = ArXiv::from_id_list([native_id]);
        let papers = self
            .client
            .fetch(&query)
            .await
            .map_err(|e| ProviderError::Request(e.to_string()))?;
        papers
            .into_iter()
            .next()
            .map(CandidateWork::from)
            .ok_or(ProviderError::NotFound)
    }

    async fn search_by_author(&self, author: &str) -> Result<Vec<CandidateWork>, ProviderError> {
        let query =
            ArXiv::from_args(QueryParams::author(author)).max_results(SEARCH_RESULT_LIMIT as u64);
        let papers = self
            .client
            .fetch(&query)
            .await
            .map_err(|e| ProviderError::Request(e.to_string()))?;
        Ok(papers.into_iter().map(CandidateWork::from).collect())
    }
}

impl From<arxiv_tools::Paper> for CandidateWork {
    fn from(value: arxiv_tools::Paper) -> Self {
        CandidateWork {
            id: CandidateId {
                provider: ProviderId::ArXiv,
                native_id: value.arxiv_id().to_string(),
            },
            title: value.title,
            authors: value.authors,
            publish_date: value.published,
            doi: (!value.doi.is_empty()).then_some(value.doi),
            pdf_url: (!value.pdf_url.is_empty()).then_some(value.pdf_url),
            venue: (!value.journal_ref.is_empty()).then_some(value.journal_ref),
            abstract_text: (!value.abstract_text.is_empty()).then_some(value.abstract_text),
        }
    }
}
