use papers_openalex::{GetParams, ListParams, OpenAlexClient};

use super::{
    AppliedFilter, CandidateId, CandidateWork, DocType, FilterApplication, Filters, Provider,
    ProviderError, ProviderId, RawPage, SearchOptions, SearchPage, collect_pages,
};

/// OpenAlex serves at most 200 works per page.
const MAX_PAGE_SIZE: usize = 200;

/// Page-number paging stops at 10,000 results; going deeper needs cursor
/// paging, which offset-based [`collect_pages`] doesn't drive.
const MAX_RETRIEVABLE: u64 = 10_000;

pub struct OpenAlexProvider {
    client: OpenAlexClient,
}

impl OpenAlexProvider {
    pub fn new() -> Self {
        OpenAlexProvider {
            client: OpenAlexClient::new(),
        }
    }

    /// Pages through `list_works` for `params` (a search or filter with no
    /// paging set yet), adding `options.filters` to its `filter` expression.
    /// OpenAlex filters on every supported criterion server-side.
    async fn list_works_page(
        &self,
        mut params: ListParams,
        options: &SearchOptions,
    ) -> Result<SearchPage, ProviderError> {
        let mut clauses: Vec<String> = params.filter.take().into_iter().collect();
        clauses.extend(filter_clauses(&options.filters));
        params.filter = (!clauses.is_empty()).then(|| clauses.join(","));

        let page = collect_pages(
            options,
            MAX_PAGE_SIZE,
            Some(MAX_RETRIEVABLE),
            false,
            |offset, size| {
                let mut params = params.clone();
                params.per_page = Some(size as u32);
                params.page = Some((offset / size + 1) as u32);
                async move {
                    let response = self
                        .client
                        .list_works(&params)
                        .await
                        .map_err(|e| ProviderError::Request(e.to_string()))?;
                    let total = u64::try_from(response.meta.count).unwrap_or(0);
                    let works = response
                        .results
                        .into_iter()
                        .map(CandidateWork::from)
                        .collect();
                    Ok(RawPage::new(works, total))
                }
            },
        )
        .await?;
        let applied = options
            .filters
            .kinds()
            .into_iter()
            .map(|kind| AppliedFilter::new(kind, FilterApplication::Server))
            .collect();
        Ok(page.with_filters(applied))
    }
}

/// OpenAlex `filter` clauses for `filters`. A journal article and a
/// conference paper are both `type:article` in OpenAlex; the venue's source
/// type tells them apart.
fn filter_clauses(filters: &Filters) -> Vec<String> {
    let mut clauses = Vec::new();
    if let Some(years) = filters.years {
        if let Some(from) = years.from {
            clauses.push(format!("from_publication_date:{from}-01-01"));
        }
        if let Some(to) = years.to {
            clauses.push(format!("to_publication_date:{to}-12-31"));
        }
    }
    if let Some(doc_type) = filters.doc_type {
        clauses.push(
            match doc_type {
                DocType::JournalArticle => "type:article,primary_location.source.type:journal",
                DocType::ConferencePaper => "type:article,primary_location.source.type:conference",
                DocType::Preprint => "type:preprint",
                DocType::Review => "type:review",
                DocType::Book => "type:book",
                DocType::BookChapter => "type:book-chapter",
                DocType::Thesis => "type:dissertation",
                DocType::Dataset => "type:dataset",
            }
            .to_string(),
        );
    }
    if let Some(language) = &filters.language {
        clauses.push(format!("language:{language}"));
    }
    clauses
}

impl Default for OpenAlexProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl Provider for OpenAlexProvider {
    fn id(&self) -> ProviderId {
        ProviderId::OpenAlex
    }

    async fn search(&self, query: &str) -> Result<Vec<CandidateWork>, ProviderError> {
        let page = self.search_page(query, &SearchOptions::default()).await?;
        Ok(page.works)
    }

    async fn get(&self, native_id: &str) -> Result<CandidateWork, ProviderError> {
        let work = self
            .client
            .get_work(native_id, &GetParams::default())
            .await
            .map_err(|e| ProviderError::Request(e.to_string()))?;
        Ok(CandidateWork::from(work))
    }

    async fn search_by_author(&self, author: &str) -> Result<Vec<CandidateWork>, ProviderError> {
        let page = self
            .search_by_author_page(author, &SearchOptions::default())
            .await?;
        Ok(page.works)
    }

    async fn search_page(
        &self,
        query: &str,
        options: &SearchOptions,
    ) -> Result<SearchPage, ProviderError> {
        let params = ListParams::builder().search(query.to_string()).build();
        self.list_works_page(params, options).await
    }

    async fn search_by_author_page(
        &self,
        author: &str,
        options: &SearchOptions,
    ) -> Result<SearchPage, ProviderError> {
        let params = ListParams::builder()
            .filter(format!("raw_author_name.search:{author}"))
            .build();
        self.list_works_page(params, options).await
    }

    async fn get_by_doi(&self, doi: &str) -> Result<CandidateWork, ProviderError> {
        // OpenAlex needs the `doi:` prefix — a bare DOI contains `/`, which
        // would otherwise be read as an extra path segment (`/works/10.1145/x`
        // looks like two segments, not one opaque id) and 404s.
        self.get(&format!("doi:{doi}")).await
    }
}

impl From<papers_openalex::Work> for CandidateWork {
    fn from(value: papers_openalex::Work) -> Self {
        // `value.id` is a full URI, e.g. "https://openalex.org/W2741809807" —
        // keep just the trailing "W2741809807" as the native id, which is
        // also what `get_work` accepts.
        let native_id = value.id.rsplit('/').next().unwrap_or(&value.id).to_string();
        CandidateWork {
            id: CandidateId {
                provider: ProviderId::OpenAlex,
                native_id,
            },
            // Deep result pages include records missing any of these fields;
            // an empty value keeps the record visible instead of failing the search.
            title: value.title.unwrap_or_default(),
            authors: value
                .authorships
                .unwrap_or_default()
                .into_iter()
                .filter_map(|author| author.raw_author_name)
                .collect(),
            publish_date: value.publication_date.unwrap_or_default(),
            doi: value.doi,
            pdf_url: value
                .best_oa_location
                .as_ref()
                .and_then(|loc| loc.pdf_url.clone())
                .or_else(|| value.open_access.as_ref().and_then(|oa| oa.oa_url.clone())),
            venue: value
                .primary_location
                .and_then(|loc| loc.source)
                .and_then(|source| source.display_name),
            abstract_text: value.abstract_text,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_missing_title_date_and_author_names_still_converts() {
        let work: papers_openalex::Work = serde_json::from_str(
            r#"{
                "id": "https://openalex.org/W1",
                "authorships": [{"author_position": "first"}]
            }"#,
        )
        .unwrap();
        let candidate = CandidateWork::from(work);
        assert_eq!(candidate.id.native_id, "W1");
        assert_eq!(candidate.title, "");
        assert_eq!(candidate.publish_date, "");
        assert!(candidate.authors.is_empty());
    }

    #[test]
    fn filters_become_openalex_filter_clauses() {
        let filters = Filters::default()
            .with_years("2015-".parse().unwrap())
            .with_doc_type(DocType::ConferencePaper)
            .with_language("EN");
        assert_eq!(
            filter_clauses(&filters),
            [
                "from_publication_date:2015-01-01",
                "type:article,primary_location.source.type:conference",
                "language:en",
            ]
        );
        assert!(filter_clauses(&Filters::default()).is_empty());
    }
}
