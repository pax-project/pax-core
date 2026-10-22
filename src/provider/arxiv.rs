use arxiv_tools::{ArXiv, Client, MAX_RESULTS_PER_REQUEST, QueryParams};

use super::{
    CandidateId, CandidateWork, Provider, ProviderError, ProviderId, SearchOptions, SearchPage,
    collect_pages,
};

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

    /// Pages through `query`'s results. arXiv reports no ceiling of its own;
    /// the client spaces requests per arXiv's terms of use, so a large
    /// [`Limit::All`](super::Limit::All) search is slow by design.
    async fn query_page(
        &self,
        query: ArXiv,
        options: &SearchOptions,
    ) -> Result<SearchPage, ProviderError> {
        collect_pages(
            options,
            MAX_RESULTS_PER_REQUEST as usize,
            None,
            |offset, size| {
                let query = query.clone().start(offset as u64).max_results(size as u64);
                async move {
                    let page = self
                        .client
                        .fetch_page(&query)
                        .await
                        .map_err(|e| ProviderError::Request(e.to_string()))?;
                    let works = page.papers.into_iter().map(CandidateWork::from).collect();
                    Ok((works, page.total_results))
                }
            },
        )
        .await
    }
}

impl Provider for ArxivProvider {
    fn id(&self) -> ProviderId {
        ProviderId::ArXiv
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
        self.query_page(ArXiv::from_args(QueryParams::title(query)), options)
            .await
    }

    async fn search_by_author_page(
        &self,
        author: &str,
        options: &SearchOptions,
    ) -> Result<SearchPage, ProviderError> {
        self.query_page(ArXiv::from_args(QueryParams::author(author)), options)
            .await
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
        let page = self
            .search_by_author_page(author, &SearchOptions::default())
            .await?;
        Ok(page.works)
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
