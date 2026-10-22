use ::semantic_scholar::SemanticScholar;

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
