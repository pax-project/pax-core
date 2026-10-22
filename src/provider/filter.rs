//! Search filters and how each provider applied them.
//!
//! A filter is an inclusion criterion applied in the query, so it's applied
//! the same way every time and can be reported. Providers apply each filter
//! server-side where their API supports it, after retrieval where the record
//! carries the field but the API can't filter on it, and not at all where the
//! provider has no such data — and say which in [`AppliedFilter`], because
//! that changes what the provider's total means.

use std::fmt;
use std::str::FromStr;

use thiserror::Error;

/// An inclusive range of publication years; either end may be open.
/// Construct with [`YearRange::new`] or by parsing (see [`FromStr`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct YearRange {
    pub from: Option<i32>,
    pub to: Option<i32>,
}

impl YearRange {
    /// `None` when both ends are open or `from` is after `to`.
    pub fn new(from: Option<i32>, to: Option<i32>) -> Option<Self> {
        match (from, to) {
            (None, None) => None,
            (Some(from), Some(to)) if from > to => None,
            _ => Some(YearRange { from, to }),
        }
    }

    /// A single year.
    pub fn exactly(year: i32) -> Self {
        YearRange {
            from: Some(year),
            to: Some(year),
        }
    }

    pub fn contains(&self, year: i32) -> bool {
        self.from.is_none_or(|from| year >= from) && self.to.is_none_or(|to| year <= to)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
#[error("year must be YYYY, YYYY-YYYY, YYYY- or -YYYY, got {0:?}")]
pub struct YearRangeParseError(String);

/// Parses `2020`, `2015-2020`, `2015-` (from 2015 on) or `-2020` (up to 2020).
impl FromStr for YearRange {
    type Err = YearRangeParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || YearRangeParseError(s.to_string());
        let year = |part: &str| -> Result<Option<i32>, YearRangeParseError> {
            if part.is_empty() {
                return Ok(None);
            }
            if part.len() != 4 || !part.chars().all(|c| c.is_ascii_digit()) {
                return Err(err());
            }
            part.parse().map(Some).map_err(|_| err())
        };
        match s.split_once('-') {
            None => Ok(YearRange::exactly(year(s)?.ok_or_else(err)?)),
            Some((from, to)) => YearRange::new(year(from)?, year(to)?).ok_or_else(err),
        }
    }
}

impl fmt::Display for YearRange {
    /// Renders as the same syntax [`FromStr`] accepts (and Semantic
    /// Scholar's `year` parameter uses).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.from, self.to) {
            (Some(from), Some(to)) if from == to => write!(f, "{from}"),
            (from, to) => {
                if let Some(from) = from {
                    write!(f, "{from}")?;
                }
                f.write_str("-")?;
                if let Some(to) = to {
                    write!(f, "{to}")?;
                }
                Ok(())
            }
        }
    }
}

/// A document type, in pax's own vocabulary; each provider maps it to its
/// own type names, where it has an equivalent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DocType {
    JournalArticle,
    ConferencePaper,
    Preprint,
    Review,
    Book,
    BookChapter,
    Thesis,
    Dataset,
}

impl DocType {
    pub const ALL: [DocType; 8] = [
        DocType::JournalArticle,
        DocType::ConferencePaper,
        DocType::Preprint,
        DocType::Review,
        DocType::Book,
        DocType::BookChapter,
        DocType::Thesis,
        DocType::Dataset,
    ];

    fn tag(self) -> &'static str {
        match self {
            DocType::JournalArticle => "journal-article",
            DocType::ConferencePaper => "conference-paper",
            DocType::Preprint => "preprint",
            DocType::Review => "review",
            DocType::Book => "book",
            DocType::BookChapter => "book-chapter",
            DocType::Thesis => "thesis",
            DocType::Dataset => "dataset",
        }
    }
}

impl fmt::Display for DocType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.tag())
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
#[error(
    "unknown document type {0:?} (expected one of: journal-article, conference-paper, preprint, review, book, book-chapter, thesis, dataset)"
)]
pub struct UnknownDocType(String);

impl FromStr for DocType {
    type Err = UnknownDocType;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        DocType::ALL
            .into_iter()
            .find(|t| t.tag() == s)
            .ok_or_else(|| UnknownDocType(s.to_string()))
    }
}

/// The filters a search applies; every field is optional. Construct with
/// `default()` and the `with_*` methods.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Filters {
    pub years: Option<YearRange>,
    pub doc_type: Option<DocType>,
    /// ISO 639-1 code, e.g. `en`.
    pub language: Option<String>,
}

impl Filters {
    pub fn with_years(mut self, years: YearRange) -> Self {
        self.years = Some(years);
        self
    }

    pub fn with_doc_type(mut self, doc_type: DocType) -> Self {
        self.doc_type = Some(doc_type);
        self
    }

    pub fn with_language(mut self, language: impl Into<String>) -> Self {
        self.language = Some(language.into().to_lowercase());
        self
    }

    pub fn is_empty(&self) -> bool {
        self.years.is_none() && self.doc_type.is_none() && self.language.is_none()
    }

    /// The kinds of filter that are set, in a stable order.
    pub fn kinds(&self) -> Vec<FilterKind> {
        let mut kinds = Vec::new();
        if self.years.is_some() {
            kinds.push(FilterKind::Year);
        }
        if self.doc_type.is_some() {
            kinds.push(FilterKind::DocType);
        }
        if self.language.is_some() {
            kinds.push(FilterKind::Language);
        }
        kinds
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FilterKind {
    Year,
    DocType,
    Language,
}

impl fmt::Display for FilterKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            FilterKind::Year => "year",
            FilterKind::DocType => "type",
            FilterKind::Language => "language",
        })
    }
}

/// Where a provider applied a filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FilterApplication {
    /// In the provider's query; its reported total already reflects it.
    Server,
    /// By pax on the retrieved records; the provider's total does not
    /// reflect it. Records lacking the field are excluded, except under a
    /// language filter: those are kept and listed in
    /// [`SearchPage::language_unknown`].
    ///
    /// [`SearchPage::language_unknown`]: super::SearchPage::language_unknown
    PostRetrieval,
    /// The provider has no such data (or no equivalent type), so its
    /// results are unfiltered on this criterion.
    NotApplied,
}

impl fmt::Display for FilterApplication {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            FilterApplication::Server => "server-side",
            FilterApplication::PostRetrieval => "post-retrieval",
            FilterApplication::NotApplied => "not applied",
        })
    }
}

/// How one requested filter was applied by one provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct AppliedFilter {
    pub kind: FilterKind,
    pub application: FilterApplication,
}

impl AppliedFilter {
    pub fn new(kind: FilterKind, application: FilterApplication) -> Self {
        AppliedFilter { kind, application }
    }
}

/// Every requested filter marked [`FilterApplication::NotApplied`] — what a
/// provider without filter support reports.
pub(crate) fn none_applied(filters: &Filters) -> Vec<AppliedFilter> {
    filters
        .kinds()
        .into_iter()
        .map(|kind| AppliedFilter::new(kind, FilterApplication::NotApplied))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn year_range_parses_every_form() {
        let r = |s: &str| s.parse::<YearRange>().unwrap();
        assert_eq!(
            r("2020"),
            YearRange {
                from: Some(2020),
                to: Some(2020)
            }
        );
        assert_eq!(
            r("2015-2020"),
            YearRange {
                from: Some(2015),
                to: Some(2020)
            }
        );
        assert_eq!(
            r("2015-"),
            YearRange {
                from: Some(2015),
                to: None
            }
        );
        assert_eq!(
            r("-2020"),
            YearRange {
                from: None,
                to: Some(2020)
            }
        );
    }

    #[test]
    fn year_range_rejects_malformed_input() {
        for bad in [
            "",
            "-",
            "20",
            "2020-19",
            "2021-2020",
            "abcd",
            "2020-2021-2022",
        ] {
            assert!(bad.parse::<YearRange>().is_err(), "{bad:?} should fail");
        }
    }

    #[test]
    fn year_range_displays_as_it_parses() {
        for s in ["2020", "2015-2020", "2015-", "-2020"] {
            assert_eq!(s.parse::<YearRange>().unwrap().to_string(), s);
        }
    }

    #[test]
    fn year_range_contains_is_inclusive() {
        let r: YearRange = "2015-2020".parse().unwrap();
        assert!(r.contains(2015) && r.contains(2020));
        assert!(!r.contains(2014) && !r.contains(2021));
        assert!("2015-".parse::<YearRange>().unwrap().contains(3000));
    }

    #[test]
    fn doc_type_round_trips_through_its_tag() {
        for t in DocType::ALL {
            assert_eq!(t.to_string().parse::<DocType>().unwrap(), t);
        }
        assert!("article".parse::<DocType>().is_err());
    }
}
