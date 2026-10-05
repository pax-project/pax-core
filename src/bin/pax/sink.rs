//! Owns how the CLI renders output. Every command handler goes through a
//! `Sink` instead of calling `println!` directly, so a future alternate
//! output mode (e.g. JSON, for an external adapter UI) is a new `Sink` impl
//! rather than a rewrite of every command handler.

use std::collections::HashSet;

use pax_core::{
    CandidateWork, CheckReport, CheckStatus, FetchOutcome, Paper, ProviderId, normalize_doi,
};

pub trait Sink {
    fn message(&mut self, message: &str);
    fn error(&mut self, message: &str);
    fn provider_header(&mut self, provider: ProviderId);
    fn candidates(&mut self, candidates: &[CandidateWork], known_dois: &HashSet<String>);
    fn candidate(&mut self, candidate: &CandidateWork);
    fn papers(&mut self, papers: &[Paper]);
    fn paper(&mut self, paper: &Paper);
    fn export(&mut self, bibtex: &str);
    fn fetched(&mut self, citation_key: &str, outcome: &FetchOutcome);
    fn checked(&mut self, reports: &[CheckReport]);
}

pub struct TextSink;

impl Sink for TextSink {
    fn message(&mut self, message: &str) {
        println!("{message}");
    }

    fn error(&mut self, message: &str) {
        println!("Error: {message}");
    }

    fn provider_header(&mut self, provider: ProviderId) {
        println!("{provider}:");
    }

    fn candidates(&mut self, candidates: &[CandidateWork], known_dois: &HashSet<String>) {
        for work in candidates {
            let in_library = work
                .doi
                .as_deref()
                .is_some_and(|doi| known_dois.contains(normalize_doi(doi)));
            println!(
                "\t{}{}",
                work.title,
                if in_library { " [in library]" } else { "" }
            );
            println!("\t\t{}", work.id);
            if !work.authors.is_empty() {
                println!("\t\t{}", work.authors.join(", "));
            }
            println!("\t\t{}", work.publish_date);
            println!("\t\t{}", work.doi.as_deref().unwrap_or("(no doi)"));
            println!("\t\t{}", work.venue.as_deref().unwrap_or("(no venue)"));
            println!(
                "\t\tPDF: {}",
                if work.pdf_url.is_some() { "✓" } else { "✗" }
            );
        }
    }

    fn candidate(&mut self, candidate: &CandidateWork) {
        println!("Title:      {}", candidate.title);
        println!("Authors:    {}", candidate.authors.join(", "));
        println!("Published:  {}", candidate.publish_date);
        println!(
            "DOI:        {}",
            candidate.doi.as_deref().unwrap_or("(no doi)")
        );
        println!(
            "Venue:      {}",
            candidate.venue.as_deref().unwrap_or("(no venue)")
        );
        println!(
            "PDF source: {}",
            candidate.pdf_url.as_deref().unwrap_or("(none found)")
        );
        if let Some(abstract_text) = &candidate.abstract_text {
            println!("Abstract:   {abstract_text}");
        }
        println!("Reference:  {}", candidate.id);
    }

    fn papers(&mut self, papers: &[Paper]) {
        for paper in papers {
            println!("{}", paper.local.citation_key);
            println!("\t{}", paper.identity.title);
            if !paper.identity.authors.is_empty() {
                println!("\t{}", paper.identity.authors.join(", "));
            }
            if let Some(year) = paper.identity.year {
                println!("\t{}", year);
            }
        }
    }

    fn paper(&mut self, paper: &Paper) {
        println!("Title:        {}", paper.identity.title);
        println!("Authors:      {}", paper.identity.authors.join(", "));
        if let Some(year) = paper.identity.year {
            println!("Year:         {}", year);
        }
        println!(
            "DOI:          {}",
            paper.identity.doi.as_deref().unwrap_or("(no doi)")
        );
        println!(
            "Venue:        {}",
            paper.identity.venue.as_deref().unwrap_or("(no venue)")
        );
        println!("Citation key: {}", paper.local.citation_key);
        println!(
            "PDF source:   {}",
            paper
                .artifact
                .source_url
                .as_deref()
                .unwrap_or("(not resolved)")
        );
        println!(
            "Artifact:     {}",
            if paper.artifact.hash.is_some() {
                "Fetched"
            } else {
                "Not fetched"
            }
        );
        if !paper.local.tags.is_empty() {
            println!("Tags:         {}", paper.local.tags.join(", "));
        }
        if let Some(notes) = &paper.local.notes {
            println!("Notes:        {}", notes);
        }
    }

    fn export(&mut self, bibtex: &str) {
        if !bibtex.is_empty() {
            println!("{bibtex}");
        }
    }

    fn fetched(&mut self, citation_key: &str, outcome: &FetchOutcome) {
        match outcome {
            FetchOutcome::AlreadyFetched { hash } => {
                println!("Already fetched {citation_key} ({hash})");
            }
            FetchOutcome::Fetched { hash } => {
                println!("Fetched {citation_key} ({hash})");
            }
        }
    }

    fn checked(&mut self, reports: &[CheckReport]) {
        let (mut reproducible, mut not_fetched, mut mismatched, mut errored) = (0, 0, 0, 0);
        for report in reports {
            match &report.status {
                CheckStatus::NotFetched => {
                    not_fetched += 1;
                    println!("{}: not fetched", report.citation_key);
                }
                CheckStatus::Reproducible => {
                    reproducible += 1;
                    println!("{}: reproducible", report.citation_key);
                }
                CheckStatus::Mismatch { expected, actual } => {
                    mismatched += 1;
                    println!(
                        "{}: MISMATCH (expected {expected}, got {actual})",
                        report.citation_key
                    );
                }
                CheckStatus::Error(message) => {
                    errored += 1;
                    println!("{}: ERROR — {message}", report.citation_key);
                }
            }
        }
        println!(
            "{} checked: {reproducible} reproducible, {not_fetched} not fetched, {mismatched} mismatched, {errored} errored",
            reports.len()
        );
    }
}
