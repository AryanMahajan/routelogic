//! # rl-discovery
//!
//! Reads a project's source and produces [`rl_model::EndpointSpec`]s. The scan never
//! executes the project's code. The one thing in this crate that does is [`enrich::run`],
//! which runs only a plan the developer has been shown — see [runtime enrich][enrich].
//!
//! ## Pipeline
//!
//! ```text
//! ProjectDetector → FrameworkDetector → SourceIndex → RegistrationGraph
//!                 → ConstantResolver → SchemaExtractor → BaseUrlInference
//! ```
//!
//! ## The one idea worth protecting
//!
//! Framework adapters do **not** resolve paths. They recognise three facts — *this creates a
//! router*, *this registers a route*, *this mounts a router at a prefix* — and the shared
//! registration graph composes full paths from them.
//!
//! That is what keeps adapters small enough to add cheaply, and why prefix resolution is
//! written once rather than once per framework. If an adapter starts joining paths itself,
//! the abstraction has sprung a leak.
//!
//! [enrich]: https://github.com/AryanMahajan/routelogic/blob/main/docs/discovery/runtime-enrich.md
//!
//! Status: FastAPI, Flask, Django, Next.js, Express and Go (net/http, Gin, Echo, chi,
//! Fiber, gorilla/mux) adapters over one graph, with Python, JavaScript/TypeScript and Go
//! module resolution, plus opt-in runtime enrich for the Python frameworks.

#![forbid(unsafe_code)]

pub mod adapters;
pub mod baseurl;
pub mod enrich;
pub mod error;
pub mod facts;
pub mod graph;
pub mod handlers;
pub mod index;
pub mod models;
pub mod project;
pub mod scan;

pub use adapters::{Detection, FrameworkAdapter};
pub use baseurl::BaseUrlCandidate;
pub use enrich::{
    AppTarget, EnrichError, EnrichOutput, EnrichPlan, Interpreter, MergeReport, Provenance,
};
pub use error::{DiscoveryError, Result};
pub use facts::{
    ExportFact, FactSink, HandlerFact, ImportFact, MountFact, RouteFact, RouterFact, Span,
    SymbolId, SymbolRef,
};
pub use graph::{GraphWarning, RegistrationGraph, ResolvedRoute};
pub use index::{ParsedFile, SourceIndex};
pub use project::{Language, ProjectContext};
pub use scan::{scan, scan_project, AppRoot, DetectedFramework, ScanResult, ScanStats};
