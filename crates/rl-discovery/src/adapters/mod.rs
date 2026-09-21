//! Framework adapters.
//!
//! An adapter recognises framework-specific syntax and emits framework-agnostic facts. That
//! is all. In particular it does **not** resolve prefixes, read files, or fold constants —
//! see `docs/discovery/adding-a-framework.md`.

pub mod django;
pub mod express;
pub mod fastapi;
pub mod flask;
pub mod go;
pub mod js;
pub mod nextjs;
pub mod python;

use crate::facts::FactSink;
use crate::index::ParsedFile;
use crate::project::{Language, ProjectContext};
use rl_model::HttpMethod;
use std::path::PathBuf;

/// What a route registered without a method expands to.
///
/// Express's `.all` and a `pages/api` handler that never checks `req.method` accept any
/// method. The model has no "any" — a request has to be sent with one — so these are
/// listed, leaving out `HEAD`, `OPTIONS` and `TRACE` to keep the tree readable. The route's
/// summary says why it appears five times.
pub const UNSPECIFIED_METHODS: [HttpMethod; 5] = [
    HttpMethod::Get,
    HttpMethod::Post,
    HttpMethod::Put,
    HttpMethod::Patch,
    HttpMethod::Delete,
];

/// How strongly a project looks like a given framework.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    /// Zero means "not this framework".
    pub score: u32,
    /// Why, in words the UI can show.
    pub evidence: Vec<String>,
}

impl Detection {
    pub fn none() -> Detection {
        Detection {
            score: 0,
            evidence: Vec::new(),
        }
    }

    pub fn matched(&self) -> bool {
        self.score > 0
    }

    pub fn add(&mut self, points: u32, why: impl Into<String>) {
        self.score += points;
        self.evidence.push(why.into());
    }
}

/// What every framework adapter implements.
pub trait FrameworkAdapter {
    /// Stable identifier, e.g. `"fastapi"`.
    fn id(&self) -> &'static str;

    /// Languages whose files this adapter wants to see.
    fn languages(&self) -> &[Language];

    /// How strongly this project looks like this framework.
    ///
    /// A dependency in a manifest is weak evidence; an actual import in source is strong.
    /// More than one adapter may match a project, and that is not an error.
    fn detect(&self, project: &ProjectContext) -> Detection;

    /// Which files are worth parsing at all.
    ///
    /// A cheap filter, not an analysis. Anything excluded here is never parsed, so this is
    /// the main lever on scan speed.
    fn candidate_files(&self, project: &ProjectContext) -> Vec<PathBuf>;

    /// Emit facts from one parsed file. The graph does the rest.
    fn extract(&self, file: &ParsedFile, sink: &mut FactSink);
}

/// Every adapter this build knows about.
pub fn all() -> Vec<Box<dyn FrameworkAdapter>> {
    vec![
        Box::new(fastapi::FastApiAdapter),
        Box::new(flask::FlaskAdapter),
        Box::new(django::DjangoAdapter),
        Box::new(nextjs::NextJsAdapter),
        Box::new(express::ExpressAdapter),
        Box::new(go::GoAdapter::default()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_accumulates_score_and_evidence() {
        let mut detection = Detection::none();
        assert!(!detection.matched());

        detection.add(1, "fastapi in pyproject.toml");
        detection.add(3, "from fastapi import FastAPI in app/main.py");

        assert_eq!(detection.score, 4);
        assert_eq!(detection.evidence.len(), 2);
        assert!(detection.matched());
    }

    #[test]
    fn every_registered_adapter_has_a_distinct_id() {
        let adapters = all();
        let mut ids: Vec<&str> = adapters.iter().map(|a| a.id()).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count);
    }
}
