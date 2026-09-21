//! The scan: project → endpoints.
//!
//! ```text
//!   project root
//!        │
//!   1. ProjectContext      language + files, gitignore-aware
//!   2. detect              which frameworks, scored
//!   3. parse               tree-sitter, candidate files only
//!   4. extract             adapters emit facts
//!   5. RegistrationGraph   compose prefixes across files
//!   6. base URL inference  candidate hosts
//!        ▼
//!   EndpointSpec[]
//! ```

use crate::adapters::{self, FrameworkAdapter};
use crate::baseurl::{self, BaseUrlCandidate};
use crate::error::Result;
use crate::facts::FactSink;
use crate::graph::RegistrationGraph;
use crate::index::SourceIndex;
use crate::project::ProjectContext;
use rl_model::{Confidence, EndpointSpec, Origin, SourceLocation};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// A framework that was found, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectedFramework {
    pub id: String,
    pub score: u32,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanStats {
    pub files_seen: usize,
    pub files_parsed: usize,
    pub routers_found: usize,
    pub endpoints_found: usize,
    /// Endpoints with at least one path segment that could not be resolved.
    pub unresolved: usize,
}

/// An application object found in source: `app = FastAPI()` in `app/main.py`.
///
/// What runtime enrich imports. Recorded by the static scan so the target can be inferred
/// without a second pass over the code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppRoot {
    pub framework: String,
    /// Relative to the project root.
    pub module: PathBuf,
    pub name: String,
    /// The factory function it is created inside, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub factory: Option<String>,
}

/// Everything one scan produced.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanResult {
    pub root: PathBuf,
    pub frameworks: Vec<DetectedFramework>,
    pub endpoints: Vec<EndpointSpec>,
    #[serde(default)]
    pub app_roots: Vec<AppRoot>,
    pub base_urls: Vec<BaseUrlCandidate>,
    /// Gaps and oddities, in words fit to show a developer.
    pub warnings: Vec<String>,
    pub stats: ScanStats,
}

impl ScanResult {
    /// Endpoints whose path could not be fully worked out.
    pub fn with_gaps(&self) -> impl Iterator<Item = &EndpointSpec> {
        self.endpoints.iter().filter(|e| e.has_gaps())
    }
}

/// Scan a project directory.
pub fn scan(root: impl AsRef<Path>) -> Result<ScanResult> {
    let project = ProjectContext::scan(root)?;
    scan_project(&project)
}

/// Scan an already-built context, so a caller that has one need not walk twice.
pub fn scan_project(project: &ProjectContext) -> Result<ScanResult> {
    let mut warnings = Vec::new();
    if project.truncated() {
        warnings.push(format!(
            "stopped after {} files; results are incomplete",
            crate::project::MAX_FILES
        ));
    }

    let mut frameworks = Vec::new();
    let mut endpoints = Vec::new();
    let mut app_roots = Vec::new();
    let mut stats = ScanStats {
        files_seen: project.files().len(),
        ..ScanStats::default()
    };

    let known: BTreeSet<PathBuf> = project.files().iter().cloned().collect();
    let mut index = SourceIndex::new()?;
    let mut listen_ports: Vec<(u16, String)> = Vec::new();

    for adapter in adapters::all() {
        let detection = adapter.detect(project);
        if !detection.matched() {
            continue;
        }

        frameworks.push(DetectedFramework {
            id: adapter.id().to_string(),
            score: detection.score,
            evidence: detection.evidence.clone(),
        });

        let mut sink = FactSink::new();
        for path in adapter.candidate_files(project) {
            let Some(parsed) = index.parse_file(project.root(), &path) else {
                continue;
            };
            stats.files_parsed += 1;
            adapter.extract(&parsed, &mut sink);
        }

        warnings.append(&mut sink.warnings.clone());
        sink.warnings.clear();
        listen_ports.append(&mut sink.ports);
        // The graph consumes the sink; models and handlers are resolved against the
        // routes afterwards.
        let models = crate::models::ModelIndex::new(std::mem::take(&mut sink.models));
        let handlers = crate::handlers::HandlerIndex::new(std::mem::take(&mut sink.handlers));

        let graph = RegistrationGraph::build(sink, &known);
        stats.routers_found += graph.router_count();
        app_roots.extend(graph.app_roots().map(|r| AppRoot {
            framework: adapter.id().to_string(),
            module: r.symbol.module.clone(),
            name: r.symbol.name.clone(),
            factory: r.factory.clone(),
        }));

        let resolution = graph.resolve();
        for warning in &resolution.warnings {
            warnings.push(warning.to_string());
        }

        for route in resolution.routes {
            let mut spec = to_spec(adapter.as_ref(), &route);
            // A route that only names its handler gets what the handler reads — and a
            // route registered without a method keeps only the methods the handler
            // checks for.
            if !handlers.fill(&mut spec, route.fact) {
                continue;
            }
            // A body that only names its model gets the model's fields, so the editor
            // opens with a body to edit rather than a blank to guess at.
            if let Some(body) = &mut spec.body {
                models.fill(body);
            }
            endpoints.push(spec);
        }
    }

    // Deterministic order, so a snapshot test is stable and the tree does not reshuffle
    // between scans.
    endpoints.sort_by(|a, b| {
        a.path
            .normalized()
            .cmp(&b.path.normalized())
            .then_with(|| a.method.as_str().cmp(b.method.as_str()))
    });

    stats.endpoints_found = endpoints.len();
    stats.unresolved = endpoints.iter().filter(|e| !e.path.is_resolved()).count();

    // Best-detected framework first, so its default port is the one offered first.
    frameworks.sort_by_key(|f| std::cmp::Reverse(f.score));
    let framework_ids: Vec<&str> = frameworks.iter().map(|f| f.id.as_str()).collect();
    let base_urls = baseurl::infer_with_listen_ports(project, &framework_ids, &listen_ports);

    Ok(ScanResult {
        root: project.root().to_path_buf(),
        frameworks,
        endpoints,
        app_roots,
        base_urls,
        warnings,
        stats,
    })
}

fn to_spec(
    adapter: &dyn FrameworkAdapter,
    route: &crate::graph::ResolvedRoute<'_>,
) -> EndpointSpec {
    let fact = route.fact;

    let mut spec = EndpointSpec::new(
        route.method.clone(),
        route.path.clone(),
        Origin::StaticScan {
            framework: adapter.id().to_string(),
        },
    );

    // The route was declared in the module its decorator sits in, which is what makes
    // click-through-to-source work.
    spec.source = Some(SourceLocation {
        file: fact.router.module.clone(),
        line: fact.span.line,
        column: fact.span.column,
    });

    spec.query_params = fact.query_params.clone();
    spec.headers = fact.headers.clone();
    spec.body = fact.body.clone();
    spec.auth = route.auth.clone();
    spec.summary = fact.summary.clone();
    spec.description = fact.description.clone();
    spec.group = route.group.clone();
    spec.orphaned = route.orphaned;

    if fact.deprecated {
        spec.metadata
            .insert("deprecated".into(), serde_json::Value::Bool(true));
    }

    // An unreachable route is worth less confidence even when its path resolved cleanly.
    if route.orphaned && spec.confidence == Confidence::Medium {
        spec.confidence = Confidence::Low;
    }

    spec
}

#[cfg(test)]
mod tests {
    use super::*;
    use rl_model::ParamStyle;
    use std::fs;
    use tempfile::TempDir;

    fn project(files: &[(&str, &str)]) -> TempDir {
        let dir = TempDir::new().unwrap();
        for (path, contents) in files {
            let full = dir.path().join(path);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(full, contents).unwrap();
        }
        dir
    }

    fn rendered(result: &ScanResult) -> Vec<String> {
        result
            .endpoints
            .iter()
            .map(|e| format!("{} {}", e.method, e.path.render(ParamStyle::Braces)))
            .collect()
    }

    /// The V1 success criterion, in miniature: open a FastAPI project, see its API.
    #[test]
    fn scans_a_realistic_fastapi_layout() {
        let dir = project(&[
            ("requirements.txt", "fastapi==0.110\nuvicorn\n"),
            ("Procfile", "web: uvicorn app.main:app --port 9000\n"),
            (
                "app/api/users.py",
                "from fastapi import APIRouter, Depends\n\
                 router = APIRouter(prefix=\"/users\", tags=[\"users\"])\n\
                 \n\
                 @router.get(\"/\")\n\
                 async def list_users(limit: int = 20):\n    \"\"\"List users.\"\"\"\n    return []\n\
                 \n\
                 @router.get(\"/{user_id}\")\n\
                 async def get_user(user_id: int): ...\n\
                 \n\
                 @router.post(\"/\")\n\
                 async def create_user(payload: UserCreate, user = Depends(oauth2_scheme)): ...\n",
            ),
            (
                "app/main.py",
                "from fastapi import FastAPI\n\
                 from .api.users import router as users_router\n\
                 app = FastAPI()\n\
                 app.include_router(users_router, prefix=\"/api/v1\")\n\
                 \n\
                 @app.get(\"/health\")\n\
                 async def health(): ...\n",
            ),
        ]);

        let result = scan(dir.path()).unwrap();

        assert_eq!(
            rendered(&result),
            vec![
                "GET /api/v1/users/",
                "POST /api/v1/users/",
                "GET /api/v1/users/{user_id}",
                "GET /health",
            ]
        );

        assert_eq!(result.frameworks[0].id, "fastapi");
        assert_eq!(result.stats.unresolved, 0);
        assert_eq!(result.base_urls[0].url, "http://localhost:9000");
    }

    /// The body a handler takes is declared in another file; the scan reads it and hands
    /// the request editor something to start from.
    #[test]
    fn a_request_body_is_filled_in_from_the_model_declared_elsewhere() {
        let dir = project(&[
            ("requirements.txt", "fastapi\n"),
            (
                "app/schemas.py",
                concat!(
                    "from pydantic import BaseModel\n",
                    "class Address(BaseModel):\n",
                    "    city: str\n",
                    "class UserCreate(BaseModel):\n",
                    "    name: str\n",
                    "    email: EmailStr\n",
                    "    age: int = 18\n",
                    "    address: Address | None = None\n",
                ),
            ),
            (
                "app/main.py",
                concat!(
                    "from fastapi import FastAPI\n",
                    "from .schemas import UserCreate\n",
                    "app = FastAPI()\n",
                    "@app.post(\"/users\")\n",
                    "def create(payload: UserCreate): ...\n",
                ),
            ),
        ]);

        let result = scan(dir.path()).unwrap();
        let body = result.endpoints[0].body.as_ref().expect("a body");
        assert_eq!(
            body.example,
            Some(serde_json::json!({
                "name": "string",
                "email": "user@example.com",
                "age": 18,
                "address": { "city": "string" }
            }))
        );
        let schema = body.schema.as_ref().unwrap();
        assert_eq!(schema["required"], serde_json::json!(["name", "email"]));
    }

    #[test]
    fn endpoints_carry_their_source_location() {
        let dir = project(&[
            ("requirements.txt", "fastapi\n"),
            (
                "main.py",
                "from fastapi import FastAPI\n\
                 app = FastAPI()\n\
                 \n\
                 @app.get(\"/health\")\n\
                 def health(): ...\n",
            ),
        ]);

        let result = scan(dir.path()).unwrap();
        let source = result.endpoints[0].source.as_ref().unwrap();

        assert_eq!(source.file, PathBuf::from("main.py"));
        assert_eq!(source.line, 4, "the decorator's line, not the function's");
    }

    #[test]
    fn detail_from_the_signature_reaches_the_spec() {
        let dir = project(&[
            ("requirements.txt", "fastapi\n"),
            (
                "main.py",
                "from fastapi import FastAPI, Depends\n\
                 app = FastAPI()\n\
                 \n\
                 @app.post(\"/users\", tags=[\"users\"], summary=\"Create a user\")\n\
                 def create(payload: UserCreate, notify: bool = False, u = Depends(get_current_user)): ...\n",
            ),
        ]);

        let spec = &scan(dir.path()).unwrap().endpoints[0];

        assert_eq!(spec.summary.as_deref(), Some("Create a user"));
        assert_eq!(spec.group.as_deref(), Some("users"));
        assert_eq!(spec.query_params[0].name, "notify");
        assert!(spec.body.is_some());
        assert!(spec.auth.is_some());
    }

    #[test]
    fn an_unresolvable_prefix_is_counted_and_flagged() {
        let dir = project(&[
            ("requirements.txt", "fastapi\n"),
            (
                "main.py",
                "from fastapi import FastAPI, APIRouter\n\
                 from .config import settings\n\
                 app = FastAPI()\n\
                 router = APIRouter()\n\
                 @router.get(\"/ping\")\n\
                 def ping(): ...\n\
                 app.include_router(router, prefix=settings.API_PREFIX)\n",
            ),
        ]);

        let result = scan(dir.path()).unwrap();

        assert_eq!(result.stats.unresolved, 1);
        assert_eq!(result.with_gaps().count(), 1);
        assert_eq!(result.endpoints[0].confidence, Confidence::Low);
    }

    #[test]
    fn an_orphaned_route_is_marked_and_reported() {
        let dir = project(&[
            ("requirements.txt", "fastapi\n"),
            (
                "main.py",
                "from fastapi import FastAPI, APIRouter\n\
                 app = FastAPI()\n\
                 stray = APIRouter(prefix=\"/stray\")\n\
                 @stray.get(\"/x\")\n\
                 def x(): ...\n",
            ),
        ]);

        let result = scan(dir.path()).unwrap();

        assert!(result.endpoints[0].orphaned);
        assert!(result.warnings.iter().any(|w| w.contains("never mounted")));
    }

    #[test]
    fn a_project_with_no_recognised_framework_scans_to_nothing() {
        let dir = project(&[("README.md", "just docs"), ("script.py", "print('hi')\n")]);
        let result = scan(dir.path()).unwrap();

        assert!(result.frameworks.is_empty());
        assert!(result.endpoints.is_empty());
        // A base URL is still offered, so the workspace is usable by hand.
        assert!(!result.base_urls.is_empty());
    }

    #[test]
    fn results_are_ordered_deterministically() {
        let dir = project(&[
            ("requirements.txt", "fastapi\n"),
            (
                "main.py",
                "from fastapi import FastAPI\n\
                 app = FastAPI()\n\
                 @app.get(\"/zebra\")\n\
                 def z(): ...\n\
                 @app.post(\"/alpha\")\n\
                 def a(): ...\n\
                 @app.get(\"/alpha\")\n\
                 def b(): ...\n",
            ),
        ]);

        let first = rendered(&scan(dir.path()).unwrap());
        let second = rendered(&scan(dir.path()).unwrap());

        assert_eq!(first, second);
        assert_eq!(first, vec!["GET /alpha", "POST /alpha", "GET /zebra"]);
    }

    #[test]
    fn stats_describe_what_happened() {
        let dir = project(&[
            ("requirements.txt", "fastapi\n"),
            (
                "main.py",
                "from fastapi import FastAPI, APIRouter\n\
                 app = FastAPI()\n\
                 r = APIRouter()\n\
                 @r.get(\"/x\")\n\
                 def x(): ...\n\
                 app.include_router(r)\n",
            ),
        ]);

        let stats = scan(dir.path()).unwrap().stats;

        assert!(stats.files_seen >= 2);
        assert_eq!(stats.files_parsed, 1);
        assert_eq!(stats.routers_found, 2, "the app and the router");
        assert_eq!(stats.endpoints_found, 1);
    }
}
