use papers_openalex::{GetParams, ListParams, OpenAlexClient};

use super::{CandidateId, CandidateWork, Provider, ProviderError, ProviderId, SEARCH_RESULT_LIMIT};

pub struct OpenAlexProvider {
    client: OpenAlexClient,
}

impl OpenAlexProvider {
    pub fn new() -> Self {
        OpenAlexProvider {
            client: OpenAlexClient::new(),
        }
    }
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
        let params = ListParams::builder()
            .search(query.to_string())
            .per_page(SEARCH_RESULT_LIMIT as u32)
            .build();
        let response = self
            .client
            .list_works(&params)
            .await
            .map_err(|e| ProviderError::Request(e.to_string()))?;
        Ok(response
            .results
            .into_iter()
            .map(CandidateWork::from)
            .collect())
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
        let params = ListParams::builder()
            .filter(format!("raw_author_name.search:{author}"))
            .per_page(SEARCH_RESULT_LIMIT as u32)
            .build();
        let response = self
            .client
            .list_works(&params)
            .await
            .map_err(|e| ProviderError::Request(e.to_string()))?;
        Ok(response
            .results
            .into_iter()
            .map(CandidateWork::from)
            .collect())
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
}
