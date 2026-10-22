use ::semantic_scholar::SemanticScholar;

use super::{CandidateId, CandidateWork, Provider, ProviderError, ProviderId, SEARCH_RESULT_LIMIT};

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
        let result = self
            .client
            .search_papers(query)
            .limit(SEARCH_RESULT_LIMIT as u32)
            .send()
            .await
            .map_err(|e| ProviderError::Request(e.to_string()))?;
        Ok(result.data.into_iter().map(CandidateWork::from).collect())
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
