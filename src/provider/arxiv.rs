use arxiv_tools::{ArXiv, Client, MAX_RESULTS_PER_REQUEST, QueryParams};

use super::{
    AppliedFilter, CandidateId, CandidateWork, DocType, FilterApplication, FilterKind, Filters,
    Provider, ProviderError, ProviderId, RawPage, SearchOptions, SearchPage, YearRange,
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

    /// Pages through the results of `args` with `options.filters` applied.
    /// arXiv reports no ceiling of its own; the client spaces requests per
    /// arXiv's terms of use, so a large [`Limit::All`](super::Limit::All)
    /// search is slow by design.
    async fn query_page(
        &self,
        args: QueryParams,
        options: &SearchOptions,
    ) -> Result<SearchPage, ProviderError> {
        let filters = &options.filters;
        let applied = applied_filters(filters);
        // Every arXiv record is a preprint: any other type matches nothing,
        // so there's nothing to ask arXiv for.
        if filters
            .doc_type
            .is_some_and(|doc_type| doc_type != DocType::Preprint)
        {
            return Ok(SearchPage::new(Vec::new(), Some(0), None).with_filters(applied));
        }
        let args = match filters.years {
            Some(years) => QueryParams::and(vec![args, submitted_in(years)]),
            None => args,
        };
        let query = ArXiv::from_args(args);

        let page = collect_pages(
            options,
            MAX_RESULTS_PER_REQUEST as usize,
            None,
            false,
            |offset, size| {
                let query = query.clone().start(offset as u64).max_results(size as u64);
                async move {
                    let page = self
                        .client
                        .fetch_page(&query)
                        .await
                        .map_err(|e| ProviderError::Request(e.to_string()))?;
                    let works = page.papers.into_iter().map(CandidateWork::from).collect();
                    Ok(RawPage::new(works, page.total_results))
                }
            },
        )
        .await?;
        Ok(page.with_filters(applied))
    }
}

/// A `submittedDate` range covering `years`. arXiv's API errors on a `*`
/// bound, so an open start is arXiv's first year and an open end year 9999.
fn submitted_in(years: YearRange) -> QueryParams {
    let from = years.from.unwrap_or(ARXIV_FIRST_YEAR);
    let to = years.to.unwrap_or(9999);
    QueryParams::submitted_date(&format!("{from}01010000"), &format!("{to}12312359"))
}

/// arXiv opened in 1991; nothing was submitted earlier.
const ARXIV_FIRST_YEAR: i32 = 1991;

/// Year (by submission date) and type are applied server-side — type
/// trivially, since arXiv holds only preprints; arXiv records carry no
/// language, so a language filter can't be applied.
fn applied_filters(filters: &Filters) -> Vec<AppliedFilter> {
    filters
        .kinds()
        .into_iter()
        .map(|kind| {
            let application = match kind {
                FilterKind::Language => FilterApplication::NotApplied,
                _ => FilterApplication::Server,
            };
            AppliedFilter::new(kind, application)
        })
        .collect()
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
        self.query_page(QueryParams::title(query), options).await
    }

    async fn search_by_author_page(
        &self,
        author: &str,
        options: &SearchOptions,
    ) -> Result<SearchPage, ProviderError> {
        self.query_page(QueryParams::author(author), options).await
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn year_range_becomes_a_submitted_date_range() {
        assert_eq!(
            submitted_in("2015-2020".parse().unwrap()).to_string(),
            "submittedDate:[201501010000 TO 202012312359]"
        );
        assert_eq!(
            submitted_in("2015-".parse().unwrap()).to_string(),
            "submittedDate:[201501010000 TO 999912312359]"
        );
        assert_eq!(
            submitted_in("-2020".parse().unwrap()).to_string(),
            "submittedDate:[199101010000 TO 202012312359]"
        );
    }

    #[test]
    fn a_non_preprint_type_matches_nothing_without_a_request() {
        // A sent request would pend, which tokio_free_block_on rejects.
        let provider = ArxivProvider::new(None).unwrap();
        let options = SearchOptions::default()
            .with_filters(Filters::default().with_doc_type(DocType::JournalArticle));
        let page = tokio_free_block_on(provider.search_page("anything", &options)).unwrap();
        assert_eq!(page.retrieved(), 0);
        assert_eq!(page.total, Some(0));
    }

    /// The short-circuit returns before any await point that could pend.
    fn tokio_free_block_on<F: std::future::Future>(future: F) -> F::Output {
        use std::task::{Context, Poll, Waker};
        let mut future = std::pin::pin!(future);
        match future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(output) => output,
            Poll::Pending => panic!("search_page sent a request"),
        }
    }
}
