//! # rl-core
//!
//! The facade. Owns application state, orchestrates the other crates, and exposes one
//! coherent API.
//!
//! ```text
//! rl-model  ←──  rl-discovery
//!     ↑     ←──  rl-import
//!     │     ←──  rl-http
//!     │     ←──  rl-workspace
//!     │               │
//!     └────  rl-core ─┘
//!                │
//!           src-tauri      ← command wrappers only, no logic
//!                │
//!               ui
//! ```
//!
//! ## The boundary this crate exists to hold
//!
//! `src-tauri` contains **no logic** — only command wrappers, event emission, and filesystem
//! scope handling. Everything it calls lives here.
//!
//! That keeps discovery testable with a plain `cargo test` against fixture repositories, with
//! no GUI harness in the loop, which matters because discovery is the highest-risk component.
//! It also means a second shell — a CLI, for use over SSH or in a dev container — would be a
//! small binary over an existing library rather than a rewrite.
//!
//! ## The send path
//!
//! [`RouteLogic::send`] is where the crates meet, and the order matters:
//!
//! 1. Assemble variables from the workspace's three tiers.
//! 2. Resolve `{{variables}}` — as late as possible, so a plaintext secret exists briefly.
//! 3. Execute.
//! 4. Record to history, redacted.
//!
//! Step 4 cannot be skipped by accident: [`rl_workspace::History::record`] accepts only a
//! `RedactedEntry`.
//!
//! ## Flows
//!
//! A flow run is the same path, once per node, driven by `rl-flow`. [`RouteLogic::prepare_flow`]
//! captures what a run needs — the engine, the active environment's variables, a history
//! handle — into a [`PreparedFlow`] that runs *without* the application lock, so the UI stays
//! usable while a long flow is in progress. Every request the flow sends lands in history,
//! redacted, exactly as a single send does.

#![forbid(unsafe_code)]

pub mod error;

pub mod agent;

pub use agent::{AgentPolicy, AgentResponse, AgentRun, AgentSend, Refusal};
pub use error::{CoreError, Result};

use rl_discovery::enrich::{self, AppTarget, Interpreter, Provenance};
use rl_discovery::{ProjectContext, ScanResult};
use rl_flow::{Failure, FlowEvent, FlowRun, NodeResult, Outcome, RunOptions};
use rl_http::{Exchange, HttpEngine, PreparedRequest};
use rl_import::{Imported, OpenApiImport};
use rl_model::{Confidence, EndpointSpec, Flow, Origin, RequestDraft, VariableContext};
use rl_workspace::{
    Collection, Environment, History, HistoryEntry, NewEntry, SecretStore, Workspace, WorkspaceKind,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A summary of the open workspace, for the UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceInfo {
    pub name: String,
    pub root: PathBuf,
    pub kind: WorkspaceKind,
    pub collections: Vec<String>,
    pub environments: Vec<String>,
    #[serde(default)]
    pub flows: Vec<String>,
    pub active_environment: Option<String>,
    /// Secret names the active environment expects but the store does not hold.
    pub missing_secrets: Vec<String>,
}

/// One discovered endpoint, flattened for the UI.
///
/// A deliberate DTO rather than sending [`rl_model::EndpointSpec`] straight out: the model's
/// path is a segment list, and the UI wants a rendered string plus the handful of booleans it
/// actually branches on. Keeping the wire shape explicit means changing the model does not
/// silently change what the UI receives.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EndpointView {
    pub id: String,
    pub method: String,
    /// Rendered in `{brace}` form.
    pub path: String,
    /// `GET /api/v1/users/{user_id}`
    pub display: String,
    pub group: Option<String>,
    pub summary: Option<String>,
    pub source: Option<SourceView>,
    /// The router this belongs to is never mounted.
    pub orphaned: bool,
    /// At least one path segment could not be determined statically.
    pub unresolved: bool,
    /// The source expressions that defeated resolution, for the tooltip.
    pub unresolved_exprs: Vec<String>,
    pub auth: bool,
    pub has_body: bool,
    pub query: Vec<String>,
    /// After runtime enrich: `matched`, `runtime_only`, `static_only` or `gap_filled`.
    /// Absent until enrich has run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enrich: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceView {
    /// Forward slashes, so the UI renders identically on every platform.
    pub file: String,
    pub line: u32,
}

/// What one runtime-enrich run did, for the UI to report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrichReport {
    pub framework: String,
    pub target: String,
    pub interpreter: PathBuf,
    /// The exact command that ran.
    pub command: String,
    pub matched: usize,
    pub runtime_only: usize,
    pub static_only: usize,
    pub gaps_filled: usize,
    pub duration_ms: u64,
    /// What the application printed while importing — logs, warnings. Shown, never parsed.
    pub stderr: String,
    /// What the OpenAPI importer could not honour.
    pub warnings: Vec<String>,
}

/// What a scan hands the UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectScan {
    pub frameworks: Vec<rl_discovery::DetectedFramework>,
    pub endpoints: Vec<EndpointView>,
    pub base_urls: Vec<rl_discovery::BaseUrlCandidate>,
    pub warnings: Vec<String>,
    pub stats: rl_discovery::ScanStats,
    /// Whether runtime enrich can be offered at all for this project.
    #[serde(default)]
    pub enrichable: bool,
    /// Present once runtime enrich has run on this scan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enrich: Option<EnrichReport>,
}

impl ProjectScan {
    fn from_result(result: &ScanResult, enrich: Option<EnrichReport>) -> ProjectScan {
        ProjectScan {
            frameworks: result.frameworks.clone(),
            endpoints: result.endpoints.iter().map(to_view).collect(),
            base_urls: result.base_urls.clone(),
            warnings: result.warnings.clone(),
            stats: result.stats,
            enrichable: is_enrichable(result),
            enrich,
        }
    }
}

/// Frameworks the helper can ask. Next.js and Express have no runtime spec to fetch.
const ENRICHABLE_FRAMEWORKS: &[&str] = &["fastapi", "flask", "django"];

fn is_enrichable(result: &ScanResult) -> bool {
    result
        .frameworks
        .iter()
        .any(|f| ENRICHABLE_FRAMEWORKS.contains(&f.id.as_str()))
}

/// What [`RouteLogic::save_scan_as_collection`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveAllReport {
    pub added: usize,
    pub updated: usize,
    pub skipped_unresolved: usize,
}

/// What the consent dialog shows before anything runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrichProposal {
    /// Candidate application objects, best first.
    pub targets: Vec<AppTarget>,
    /// Candidate interpreters, best first.
    pub interpreters: Vec<Interpreter>,
    /// The target recorded in `workspace.yaml` from an earlier consent, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remembered_target: Option<String>,
    /// The command line for the default choice, or none when a part is missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Where the helper script is (or will be) written, so it can be read first.
    pub helper_path: PathBuf,
}

fn to_view(spec: &rl_model::EndpointSpec) -> EndpointView {
    EndpointView {
        id: spec.id.as_str().to_string(),
        method: spec.method.to_string(),
        path: spec.path.render(rl_model::ParamStyle::Braces),
        display: spec.display(),
        group: spec.group.clone(),
        summary: spec.summary.clone(),
        source: spec.source.as_ref().map(|s| SourceView {
            file: s.file.display().to_string().replace('\\', "/"),
            line: s.line,
        }),
        orphaned: spec.orphaned,
        unresolved: !spec.path.is_resolved(),
        unresolved_exprs: spec
            .path
            .unresolved_exprs()
            .iter()
            .map(|s| s.to_string())
            .collect(),
        auth: spec.auth.is_some(),
        has_body: spec.body.is_some(),
        query: spec.query_params.iter().map(|p| p.name.clone()).collect(),
        enrich: Provenance::of(spec).map(|p| p.as_str().to_string()),
    }
}

/// The application.
#[derive(Debug)]
pub struct RouteLogic {
    /// Where per-user state — collections — lives. The user's data directory, or a
    /// temporary one under test.
    data_dir: PathBuf,
    workspace: Option<Workspace>,
    active_environment: Option<String>,
    /// Shared with in-progress flow runs, which outlive the lock on the application.
    engine: Arc<HttpEngine>,
    /// Kept so the UI can open an endpoint by id without rescanning.
    last_scan: Option<ScanResult>,
    /// The last runtime-enrich report, cleared by a rescan.
    last_enrich: Option<EnrichReport>,
}

impl Default for RouteLogic {
    fn default() -> Self {
        RouteLogic::with_data_dir(rl_workspace::default_data_dir())
    }
}

impl RouteLogic {
    pub fn new() -> Self {
        Self::default()
    }

    /// An application whose per-user state lives under `data_dir`.
    pub fn with_data_dir(data_dir: impl Into<PathBuf>) -> Self {
        RouteLogic {
            data_dir: data_dir.into(),
            workspace: None,
            active_environment: None,
            engine: Arc::new(HttpEngine::default()),
            last_scan: None,
            last_enrich: None,
        }
    }

    fn layout_for(&self, root: &Path) -> rl_workspace::Layout {
        rl_workspace::Layout::with_data_dir(root, &self.data_dir)
    }

    // --- workspace ----------------------------------------------------------------------

    pub fn open_workspace(&mut self, root: impl AsRef<Path>) -> Result<WorkspaceInfo> {
        let workspace = Workspace::open_in(self.layout_for(root.as_ref()))?;
        self.active_environment = workspace.manifest().default_environment.clone();
        self.workspace = Some(workspace);
        self.last_scan = None;
        self.ensure_an_environment()?;
        self.info()
    }

    pub fn create_workspace(
        &mut self,
        root: impl AsRef<Path>,
        name: impl Into<String>,
        kind: WorkspaceKind,
    ) -> Result<WorkspaceInfo> {
        let workspace = Workspace::create_in(self.layout_for(root.as_ref()), name, kind)?;
        self.active_environment = None;
        self.workspace = Some(workspace);
        self.ensure_an_environment()?;
        self.info()
    }

    /// A workspace with no environment cannot resolve `{{base_url}}`, and every discovered
    /// request uses it. So there is always one — `local` — created on first open and made
    /// the default. Nothing is overwritten: an existing default is respected, and an
    /// existing environment with no default becomes it.
    fn ensure_an_environment(&mut self) -> Result<()> {
        if self.active_environment.is_some() {
            return Ok(());
        }
        let existing = self.workspace()?.environment_names()?;
        let name = match existing.first() {
            Some(name) => name.clone(),
            None => {
                let environment = Environment::new("local");
                self.workspace()?.save_environment(&environment)?;
                "local".to_string()
            }
        };
        self.set_active_environment(Some(&name))
    }

    pub fn open_or_create_workspace(
        &mut self,
        root: impl AsRef<Path>,
        name: impl Into<String>,
        kind: WorkspaceKind,
    ) -> Result<WorkspaceInfo> {
        let root = root.as_ref();
        if Workspace::exists(root) {
            self.open_workspace(root)
        } else {
            self.create_workspace(root, name, kind)
        }
    }

    /// Report every change another process — an agent's MCP server, a `git checkout`, an
    /// editor — makes to the open workspace's flows, environments and collections, until
    /// the returned watcher is dropped. This process's own saves are not reported.
    pub fn watch(
        &self,
        on_change: impl Fn(rl_workspace::Change) + Send + 'static,
    ) -> Result<rl_workspace::Watcher> {
        Ok(rl_workspace::Watcher::start(
            self.workspace()?.layout(),
            on_change,
        )?)
    }

    pub fn close_workspace(&mut self) {
        self.workspace = None;
        self.active_environment = None;
        self.last_scan = None;
    }

    pub fn is_open(&self) -> bool {
        self.workspace.is_some()
    }

    fn workspace(&self) -> Result<&Workspace> {
        self.workspace.as_ref().ok_or(CoreError::NoWorkspace)
    }

    fn workspace_mut(&mut self) -> Result<&mut Workspace> {
        self.workspace.as_mut().ok_or(CoreError::NoWorkspace)
    }

    pub fn info(&self) -> Result<WorkspaceInfo> {
        let workspace = self.workspace()?;
        let manifest = workspace.manifest();

        let missing_secrets = match &self.active_environment {
            Some(name) => match workspace.load_environment(name) {
                Ok(env) => env.missing_secrets(&workspace.secrets().load_all()?),
                Err(_) => Vec::new(),
            },
            None => Vec::new(),
        };

        Ok(WorkspaceInfo {
            name: manifest.name.clone(),
            root: workspace.layout().root().to_path_buf(),
            kind: manifest.kind,
            collections: workspace.collection_names()?,
            environments: workspace.environment_names()?,
            flows: workspace.flow_names()?,
            active_environment: self.active_environment.clone(),
            missing_secrets,
        })
    }

    // --- environments -------------------------------------------------------------------

    pub fn active_environment(&self) -> Option<&str> {
        self.active_environment.as_deref()
    }

    /// Switch environments, refusing a name that does not exist rather than silently
    /// falling back to no variables at all. The choice is remembered as the workspace's
    /// default, so the next open lands in the same environment.
    pub fn set_active_environment(&mut self, name: Option<&str>) -> Result<()> {
        if let Some(name) = name {
            self.workspace()?.load_environment(name)?;
        }
        self.active_environment = name.map(str::to_string);

        let workspace = self.workspace.as_mut().ok_or(CoreError::NoWorkspace)?;
        if workspace.manifest().default_environment.as_deref() != name {
            workspace.manifest_mut().default_environment = name.map(str::to_string);
            workspace.save()?;
        }
        Ok(())
    }

    /// Remove an environment. If it was active, the first remaining one takes over — or
    /// none, which the UI then shows as such.
    pub fn delete_environment(&mut self, name: &str) -> Result<()> {
        self.workspace()?.delete_environment(name)?;
        if self.active_environment.as_deref() == Some(name) {
            let remaining = self.workspace()?.environment_names()?;
            let next = remaining.first().cloned();
            self.set_active_environment(next.as_deref())?;
        }
        Ok(())
    }

    /// Every name a `{{…}}` reference could use right now, for autocomplete.
    ///
    /// Secrets are listed as `secret:NAME` — names only, never values.
    pub fn variable_names(&self) -> Result<Vec<String>> {
        let ctx = self.variables()?;
        let mut names: Vec<String> = ctx
            .globals
            .keys()
            .chain(ctx.environment.keys())
            .cloned()
            .collect();
        names.extend(ctx.secrets.keys().map(|k| format!("secret:{k}")));
        names.sort();
        names.dedup();
        Ok(names)
    }

    pub fn load_environment(&self, name: &str) -> Result<Environment> {
        Ok(self.workspace()?.load_environment(name)?)
    }

    pub fn save_environment(&self, environment: &Environment) -> Result<()> {
        Ok(self.workspace()?.save_environment(environment)?)
    }

    pub fn variables(&self) -> Result<VariableContext> {
        Ok(self
            .workspace()?
            .variable_context(self.active_environment.as_deref())?)
    }

    // --- secrets ------------------------------------------------------------------------

    pub fn set_secret(&self, name: &str, value: &str) -> Result<()> {
        self.workspace()?.secrets().set(name, value)?;
        Ok(())
    }

    pub fn delete_secret(&self, name: &str) -> Result<()> {
        self.workspace()?.secrets().delete(name)?;
        Ok(())
    }

    /// Names only. Values never leave this process for the UI.
    pub fn secret_names(&self) -> Result<Vec<String>> {
        Ok(self.workspace()?.secrets().names()?)
    }

    // --- collections --------------------------------------------------------------------

    pub fn collection_names(&self) -> Result<Vec<String>> {
        Ok(self.workspace()?.collection_names()?)
    }

    pub fn load_collection(&self, name: &str) -> Result<Collection> {
        Ok(self.workspace()?.load_collection(name)?)
    }

    pub fn save_collection(&self, collection: &Collection) -> Result<()> {
        Ok(self.workspace()?.save_collection(collection)?)
    }

    /// Append a request to a collection, creating it if needed.
    pub fn save_request(&self, collection_name: &str, request: RequestDraft) -> Result<()> {
        let workspace = self.workspace()?;
        let mut collection = workspace
            .load_collection(collection_name)
            .unwrap_or_else(|_| Collection::new(collection_name));

        // Replace the same request when it is already there — by id first, so renaming
        // a saved request updates it rather than duplicating it — then by name, so saving
        // a fresh draft under an existing name replaces it.
        let existing = collection
            .requests
            .iter()
            .position(|r| r.id == request.id)
            .or_else(|| {
                request.name.as_deref().and_then(|name| {
                    collection
                        .requests
                        .iter()
                        .position(|r| r.name.as_deref() == Some(name))
                })
            });
        match existing {
            Some(index) => {
                // A save from the editor keeps the folder the request was filed under.
                let folder = collection.requests[index].folder.take();
                let mut request = request;
                request.folder = request.folder.or(folder);
                collection.requests[index] = request;
            }
            None => collection.push(request),
        }

        workspace.save_collection(&collection)?;
        Ok(())
    }

    /// Remove a collection file. The requests in it are gone; that is what was asked.
    pub fn delete_collection(&self, name: &str) -> Result<()> {
        Ok(self.workspace()?.delete_collection(name)?)
    }

    /// Rename a collection: the file moves, the requests stay in order.
    pub fn rename_collection(&self, from: &str, to: &str) -> Result<()> {
        let workspace = self.workspace()?;
        let mut collection = workspace.load_collection(from)?;
        if workspace.load_collection(to).is_ok() {
            return Err(CoreError::CollectionExists {
                name: to.to_string(),
            });
        }
        collection.name = to.to_string();
        workspace.save_collection(&collection)?;
        workspace.delete_collection(from)?;
        Ok(())
    }

    // --- flows --------------------------------------------------------------------------

    pub fn flow_names(&self) -> Result<Vec<String>> {
        Ok(self.workspace()?.flow_names()?)
    }

    /// Load a flow, placing any node written without a position — by hand, or by an
    /// agent — so the canvas never receives a card with nowhere to go.
    pub fn load_flow(&self, name: &str) -> Result<Flow> {
        let mut flow = self.workspace()?.load_flow(name)?;
        flow.lay_out();
        Ok(flow)
    }

    /// Save a flow, placing unplaced nodes first, so what is on disk is what the canvas
    /// will show.
    pub fn save_flow(&self, flow: &Flow) -> Result<()> {
        let mut flow = flow.clone();
        flow.lay_out();
        Ok(self.workspace()?.save_flow(&flow)?)
    }

    /// Where a flow is stored, whether or not it exists yet.
    pub fn flow_path(&self, name: &str) -> Result<PathBuf> {
        Ok(self.workspace()?.layout().flow_file(name)?)
    }

    /// Mistakes that would only show when `flow` runs, checked against the variables of
    /// `environment` (the active one when `None`) and the endpoints of the last scan.
    pub fn lint_flow(
        &self,
        flow: &Flow,
        environment: Option<&str>,
    ) -> Result<Vec<rl_flow::Warning>> {
        let variables = self.variables_for(environment)?;
        let endpoints: Option<std::collections::BTreeSet<String>> =
            self.last_scan.as_ref().map(|scan| {
                scan.endpoints
                    .iter()
                    .map(|e| e.id.as_str().to_string())
                    .collect()
            });
        Ok(rl_flow::lint(flow, &variables, endpoints.as_ref()))
    }

    /// The variables a request would resolve against in `environment`, or in the active
    /// environment when `None` — without changing which one is active.
    pub fn variables_for(&self, environment: Option<&str>) -> Result<VariableContext> {
        let Some(workspace) = self.workspace.as_ref() else {
            return Ok(VariableContext::new());
        };
        let name = environment.or(self.active_environment.as_deref());
        if let Some(name) = environment {
            if !workspace.environment_names()?.iter().any(|n| n == name) {
                return Err(CoreError::NoSuchEnvironment {
                    name: name.to_string(),
                });
            }
        }
        Ok(workspace.variable_context(name)?)
    }

    pub fn delete_flow(&self, name: &str) -> Result<()> {
        Ok(self.workspace()?.delete_flow(name)?)
    }

    /// Rename a flow: the file moves, the graph is untouched.
    pub fn rename_flow(&self, from: &str, to: &str) -> Result<()> {
        let workspace = self.workspace()?;
        let mut flow = workspace.load_flow(from)?;
        if from != to && workspace.load_flow(to).is_ok() {
            return Err(CoreError::FlowExists {
                name: to.to_string(),
            });
        }
        flow.name = to.to_string();
        workspace.save_flow(&flow)?;
        if from != to {
            workspace.delete_flow(from)?;
        }
        Ok(())
    }

    /// Everything a flow run needs, captured so the run itself can proceed without holding
    /// the application: the engine, the active environment's variables, and history.
    ///
    /// The flow is checked for structural problems here — a cycle, a dangling edge — so a
    /// broken graph is refused before the first request rather than mid-run.
    pub fn prepare_flow(&self, flow: Flow, options: RunOptions) -> Result<PreparedFlow> {
        flow.validate()?;
        let (variables, history) = match self.workspace.as_ref() {
            Some(workspace) => (
                workspace.variable_context(self.active_environment.as_deref())?,
                workspace.history().ok(),
            ),
            None => (VariableContext::new(), None),
        };
        Ok(PreparedFlow {
            flow,
            options,
            variables,
            engine: Arc::clone(&self.engine),
            history,
        })
    }

    // --- discovery ----------------------------------------------------------------------

    /// Scan the open workspace's project for endpoints.
    ///
    /// Static analysis only — nothing in the project is executed. See `docs/security.md`.
    pub fn scan(&mut self) -> Result<ProjectScan> {
        let workspace = self.workspace()?;
        if workspace.manifest().kind == WorkspaceKind::Standalone {
            return Err(CoreError::NotAProject);
        }

        let root = workspace.layout().root().to_path_buf();
        let result = rl_discovery::scan(&root)?;

        // Seed the environment's base URL from the best candidate, but never overwrite one
        // the developer has already chosen.
        if let (Some(name), Some(candidate)) =
            (self.active_environment.clone(), result.base_urls.first())
        {
            if let Ok(mut environment) = workspace.load_environment(&name) {
                if !environment.variables.contains_key("base_url") {
                    environment.set("base_url", &candidate.url);
                    let _ = workspace.save_environment(&environment);
                }
            }
        }

        let view = ProjectScan::from_result(&result, None);
        self.last_scan = Some(result);
        self.last_enrich = None;
        Ok(view)
    }

    /// The most recent scan, if one has been run.
    pub fn last_scan(&self) -> Option<&ScanResult> {
        self.last_scan.as_ref()
    }

    // --- runtime enrich -----------------------------------------------------------------

    /// What running runtime enrich *would* do. Executes nothing, writes nothing.
    ///
    /// The result is the consent dialog's content: every candidate target and interpreter,
    /// with the reason each was suggested, and the exact command line for the default pair.
    pub fn enrich_proposal(&self) -> Result<EnrichProposal> {
        let workspace = self.workspace()?;
        let scan = self.last_scan.as_ref().ok_or(CoreError::NoScan)?;
        if !is_enrichable(scan) {
            return Err(CoreError::NotEnrichable {
                frameworks: describe_frameworks(scan),
            });
        }

        let root = workspace.layout().root().to_path_buf();
        let project = ProjectContext::scan(&root)?;
        let candidates = enrich::candidates(&project, &scan.app_roots);

        let remembered_target = workspace
            .manifest()
            .project
            .as_ref()
            .and_then(|p| p.app_target.clone());
        let helper_dir = workspace.layout().local_dir();

        // The remembered target leads, then the best inferred one.
        let target = remembered_target
            .as_ref()
            .map(|t| self.target_named(t, &candidates.targets))
            .or_else(|| candidates.targets.first().cloned());
        let command = match (target, candidates.interpreters.first()) {
            (Some(target), Some(interpreter)) => {
                Some(enrich::plan(&root, &helper_dir, interpreter.clone(), target).command_line())
            }
            _ => None,
        };

        Ok(EnrichProposal {
            targets: candidates.targets,
            interpreters: candidates.interpreters,
            remembered_target,
            command,
            helper_path: helper_dir.join(enrich::HELPER_FILE_NAME),
        })
    }

    /// A target by its spelling, falling back to a hand-typed one at the project root.
    fn target_named(&self, spelling: &str, known: &[AppTarget]) -> AppTarget {
        known
            .iter()
            .find(|t| t.target == spelling)
            .cloned()
            .unwrap_or_else(|| AppTarget {
                target: spelling.to_string(),
                cwd: PathBuf::from("."),
                source: "entered by hand".to_string(),
                confidence: 10,
            })
    }

    /// The exact command line a given choice would run — for the consent dialog to show
    /// as the developer changes the target or interpreter. Executes nothing.
    pub fn enrich_command(&self, target: &str, interpreter: Option<&Path>) -> Result<String> {
        let proposal = self.enrich_proposal()?;
        let target = self.target_named(target, &proposal.targets);
        let interpreter = match interpreter {
            Some(path) => Interpreter {
                path: path.to_path_buf(),
                source: "chosen".to_string(),
            },
            None => proposal
                .interpreters
                .first()
                .cloned()
                .ok_or(rl_discovery::EnrichError::NoInterpreter)?,
        };
        let workspace = self.workspace()?;
        let root = workspace.layout().root().to_path_buf();
        Ok(
            enrich::plan(&root, &workspace.layout().local_dir(), interpreter, target)
                .command_line(),
        )
    }

    /// Run runtime enrich. **This executes the project's code.**
    ///
    /// The caller must have shown [`RouteLogic::enrich_proposal`] and been told yes; the
    /// target is then recorded in `workspace.yaml` as the standing consent, which
    /// [`RouteLogic::revoke_enrich`] withdraws. The static scan is merged with the result
    /// and becomes the scan the UI works from — source locations included.
    pub fn run_enrich(&mut self, target: &str, interpreter: Option<&Path>) -> Result<ProjectScan> {
        let proposal = self.enrich_proposal()?;
        let target = self.target_named(target, &proposal.targets);
        let interpreter = match interpreter {
            Some(path) => Interpreter {
                path: path.to_path_buf(),
                source: "chosen".to_string(),
            },
            None => proposal
                .interpreters
                .first()
                .cloned()
                .ok_or(rl_discovery::EnrichError::NoInterpreter)?,
        };

        // Consent is recorded *before* running, so a run that hangs and gets killed still
        // leaves the decision on disk — and so the dialog next time shows what was agreed.
        {
            let workspace = self.workspace_mut()?;
            if let Some(project) = workspace.manifest_mut().project.as_mut() {
                if project.app_target.as_deref() != Some(&target.target) {
                    project.app_target = Some(target.target.clone());
                    workspace.save()?;
                }
            }
        }

        let workspace = self.workspace()?;
        let root = workspace.layout().root().to_path_buf();
        let plan = enrich::plan(&root, &workspace.layout().local_dir(), interpreter, target);
        let output = enrich::run(&plan, enrich::DEFAULT_TIMEOUT)?;

        let imported = rl_import::parse_openapi_value(&output.openapi)?;
        let runtime: Vec<EndpointSpec> = imported
            .value
            .endpoints
            .into_iter()
            .map(|mut spec| {
                spec.origin = Origin::Runtime {
                    framework: output.framework.clone(),
                };
                spec.confidence = Confidence::High;
                spec
            })
            .collect();

        let scan = self.last_scan.as_mut().ok_or(CoreError::NoScan)?;
        let static_specs = std::mem::take(&mut scan.endpoints);
        let (merged, report) = enrich::merge::merge(static_specs, runtime);
        scan.endpoints = merged;
        scan.stats.endpoints_found = scan.endpoints.len();
        scan.stats.unresolved = scan
            .endpoints
            .iter()
            .filter(|e| !e.path.is_resolved())
            .count();

        let report = EnrichReport {
            framework: output.framework,
            target: plan.target.target.clone(),
            interpreter: plan.interpreter.path.clone(),
            command: plan.command_line(),
            matched: report.matched,
            runtime_only: report.runtime_only,
            static_only: report.static_only,
            gaps_filled: report.gaps_filled,
            duration_ms: output.duration.as_millis() as u64,
            stderr: output.stderr,
            warnings: imported.warnings,
        };
        let view = ProjectScan::from_result(scan, Some(report.clone()));
        self.last_enrich = Some(report);
        Ok(view)
    }

    /// Withdraw the standing consent: forget the target. The next run asks again.
    pub fn revoke_enrich(&mut self) -> Result<()> {
        let workspace = self.workspace_mut()?;
        if let Some(project) = workspace.manifest_mut().project.as_mut() {
            if project.app_target.take().is_some() {
                workspace.save()?;
            }
        }
        Ok(())
    }

    /// The last enrich report, if the current scan has been enriched.
    pub fn last_enrich(&self) -> Option<&EnrichReport> {
        self.last_enrich.as_ref()
    }

    /// Turn a discovered endpoint into an editable request.
    ///
    /// Refuses an endpoint whose path never resolved: sending `/?/stats` would hit a
    /// meaningless URL, and the gap is the thing worth showing rather than papering over.
    pub fn request_for(&self, endpoint_id: &str, base_url: Option<&str>) -> Result<RequestDraft> {
        let scan = self.last_scan.as_ref().ok_or(CoreError::NoScan)?;

        let spec = scan
            .endpoints
            .iter()
            .find(|e| e.id.as_str() == endpoint_id)
            .ok_or_else(|| CoreError::NoSuchEndpoint {
                id: endpoint_id.to_string(),
            })?;

        if !spec.path.is_resolved() {
            return Err(CoreError::UnresolvedEndpoint {
                id: endpoint_id.to_string(),
                expressions: spec
                    .path
                    .unresolved_exprs()
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            });
        }

        // `{{base_url}}` by default, so switching environments repoints every request.
        let base = base_url.unwrap_or("{{base_url}}");
        let mut draft = RequestDraft::from_spec(spec, base);
        draft.name = Some(spec.summary.clone().unwrap_or_else(|| spec.display()));
        Ok(draft)
    }

    /// Save every resolved endpoint of the last scan into one collection, filed under
    /// folders by group — the whole API as a committed, editable set of requests.
    ///
    /// Re-running it onto the same collection updates the requests that came from
    /// discovery (matched by `spec_ref`) and leaves hand-made ones alone. Endpoints whose
    /// path never resolved are skipped and counted, not written as `/?/stats`.
    pub fn save_scan_as_collection(&self, collection_name: &str) -> Result<SaveAllReport> {
        let workspace = self.workspace()?;
        let scan = self.last_scan.as_ref().ok_or(CoreError::NoScan)?;

        let mut collection = workspace
            .load_collection(collection_name)
            .unwrap_or_else(|_| Collection::new(collection_name));
        let mut report = SaveAllReport::default();

        for spec in &scan.endpoints {
            if !spec.path.is_resolved() {
                report.skipped_unresolved += 1;
                continue;
            }
            let mut draft = RequestDraft::from_spec(spec, "{{base_url}}");
            draft.name = Some(spec.summary.clone().unwrap_or_else(|| spec.display()));
            draft.folder = spec.group.clone();

            match collection
                .requests
                .iter()
                .position(|r| r.spec_ref.as_ref() == Some(&spec.id))
            {
                Some(index) => {
                    // Keep the id (open tabs point at it) and any folder the person chose.
                    draft.id = collection.requests[index].id.clone();
                    draft.folder = collection.requests[index].folder.clone().or(draft.folder);
                    collection.requests[index] = draft;
                    report.updated += 1;
                }
                None => {
                    collection.push(draft);
                    report.added += 1;
                }
            }
        }

        workspace.save_collection(&collection)?;
        Ok(report)
    }

    /// Open a discovered endpoint's definition in the developer's editor.
    ///
    /// The path is resolved against the workspace root and checked to be inside it, because
    /// it reaches here from a scan result and this method starts a process with it.
    pub fn reveal_in_editor(&self, file: &Path, line: u32) -> Result<()> {
        let root = self.workspace()?.layout().root().to_path_buf();
        let full = root.join(file);

        let canonical = full
            .canonicalize()
            .map_err(|_| CoreError::NoSuchSource { path: full.clone() })?;
        if !canonical.starts_with(&root) {
            return Err(CoreError::NoSuchSource { path: full });
        }

        open_in_editor(&canonical, line);
        Ok(())
    }

    // --- sending ------------------------------------------------------------------------

    /// Resolve, send, and record.
    ///
    /// Works without a workspace, in which case there are no variables to resolve and
    /// nothing is written to history.
    pub async fn send(&self, draft: &RequestDraft) -> Result<Exchange> {
        let (resolved, variables) = self.resolve(draft)?;
        let outcome = self.engine.execute(&resolved).await;

        // History records failures too: "it did not connect" is worth keeping.
        if let Some(workspace) = self.workspace.as_ref() {
            if let Ok(history) = workspace.history() {
                let entry = build_entry(&resolved, &outcome);
                let _ = history.record(&entry.redacted(&variables));
            }
        }

        outcome.map_err(CoreError::from)
    }

    /// The request as it would go on the wire, for copying as a cURL command, a script
    /// or a plain URL. Resolves exactly as [`RouteLogic::send`] does, so what is copied is
    /// what would be sent — secrets included, which is what a copied command is for.
    pub fn prepare(&self, draft: &RequestDraft) -> Result<PreparedRequest> {
        let (resolved, _variables) = self.resolve(draft)?;
        Ok(rl_http::prepare(&resolved)?)
    }

    /// Resolve every `{{variable}}` against the active environment.
    ///
    /// Undefined variables are reported together, before anything is sent, so the user
    /// fixes them in one pass rather than one failed request at a time.
    fn resolve(&self, draft: &RequestDraft) -> Result<(RequestDraft, VariableContext)> {
        let variables = match self.workspace.as_ref() {
            Some(workspace) => workspace.variable_context(self.active_environment.as_deref())?,
            None => VariableContext::new(),
        };

        let undefined = draft
            .variable_references()
            .into_iter()
            .filter(|name| !variables.is_defined(name))
            .collect::<Vec<_>>();
        if !undefined.is_empty() {
            return Err(CoreError::UndefinedVariables { names: undefined });
        }

        let (resolved, _secrets_used) = draft.resolve(&variables)?;
        Ok((resolved, variables))
    }

    // --- history ------------------------------------------------------------------------

    pub fn history(&self, limit: usize) -> Result<Vec<HistoryEntry>> {
        Ok(self.open_history()?.recent(limit)?)
    }

    pub fn history_entry(&self, id: i64) -> Result<Option<HistoryEntry>> {
        Ok(self.open_history()?.get(id)?)
    }

    pub fn clear_history(&self) -> Result<()> {
        Ok(self.open_history()?.clear()?)
    }

    fn open_history(&self) -> Result<History> {
        Ok(self.workspace()?.history()?)
    }

    // --- import -------------------------------------------------------------------------

    pub fn import_curl(&self, text: &str) -> Result<Imported<RequestDraft>> {
        Ok(rl_import::parse_curl(text)?)
    }

    pub fn import_raw_http(&self, text: &str) -> Result<Imported<RequestDraft>> {
        Ok(rl_import::parse_raw_http(text)?)
    }

    pub fn import_openapi(&self, text: &str) -> Result<Imported<OpenApiImport>> {
        Ok(rl_import::parse_openapi(text)?)
    }

    /// Import a specification and save it as a collection.
    ///
    /// Returns the imported document alongside any warnings, so the UI can show what could
    /// not be honoured rather than presenting a silently partial import.
    pub fn import_openapi_as_collection(
        &self,
        text: &str,
        collection_name: &str,
    ) -> Result<Imported<OpenApiImport>> {
        let imported = self.import_openapi(text)?;
        let base_url = imported
            .value
            .servers
            .first()
            .cloned()
            .unwrap_or_else(|| "{{base_url}}".to_string());

        let mut collection = Collection::new(collection_name);
        for spec in &imported.value.endpoints {
            let mut draft = RequestDraft::from_spec(spec, &base_url);
            draft.name = Some(spec.summary.clone().unwrap_or_else(|| spec.display()));
            collection.push(draft);
        }

        self.workspace()?.save_collection(&collection)?;
        Ok(imported)
    }
}

/// A flow about to run. See [`RouteLogic::prepare_flow`].
pub struct PreparedFlow {
    flow: Flow,
    options: RunOptions,
    variables: VariableContext,
    engine: Arc<HttpEngine>,
    history: Option<History>,
}

impl std::fmt::Debug for PreparedFlow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedFlow")
            .field("flow", &self.flow.name)
            .finish_non_exhaustive()
    }
}

impl PreparedFlow {
    pub fn flow(&self) -> &Flow {
        &self.flow
    }

    /// Run it. `on_event` sees every step as it happens; the return value is the whole run.
    ///
    /// Each request node that reached the network is recorded in history, redacted, whether
    /// it passed or not — "the login step returned 401" is worth keeping.
    pub async fn run(mut self, on_event: &mut (dyn FnMut(FlowEvent) + Send)) -> Result<FlowRun> {
        let PreparedFlow {
            flow,
            options,
            variables,
            engine,
            history,
        } = &mut self;

        // `history` is captured mutably on purpose: a SQLite connection is `Send` but not
        // `Sync`, and the closure has to be `Send` to run inside an async command.
        let mut forward = |event: FlowEvent| {
            if let FlowEvent::NodeFinished { result } = &event {
                if let (Some(history), Some(entry)) = (history.as_mut(), flow_entry(result)) {
                    let _ = history.record(&entry.redacted(variables));
                }
            }
            on_event(event);
        };

        Ok(rl_flow::run_with(
            flow,
            options.clone(),
            variables,
            engine.as_ref(),
            &mut forward,
        )
        .await?)
    }
}

/// The history row for one finished request node, if it got as far as being resolved.
fn flow_entry(result: &NodeResult) -> Option<NewEntry> {
    let sent = result.request.as_ref()?;
    let mut entry = NewEntry::new(sent.method.to_string(), sent.url_with_path_values());
    match (&result.exchange, &result.outcome) {
        (Some(exchange), _) => {
            entry.status = Some(exchange.response.status);
            entry.duration_ms = Some(exchange.response.timing.total_ms);
            entry.request = serde_json::to_value(&exchange.request).unwrap_or_default();
            entry.response = serde_json::to_value(&exchange.response).ok();
        }
        (
            None,
            Outcome::Failed {
                failure: Failure::Transport { message },
            },
        ) => {
            entry.error = Some(message.clone());
            entry.request = serde_json::to_value(sent).unwrap_or_default();
        }
        // Resolved but never sent for any other reason: nothing happened on the wire.
        _ => return None,
    }
    Some(entry)
}

fn describe_frameworks(scan: &ScanResult) -> String {
    if scan.frameworks.is_empty() {
        "not a recognised framework".to_string()
    } else {
        scan.frameworks
            .iter()
            .map(|f| f.id.as_str())
            .collect::<Vec<_>>()
            .join(" + ")
    }
}

/// Jump to a file and line in whatever editor is available.
///
/// Editors that accept a line number are tried first, because landing on the right line is
/// the whole point; the OS default is the fallback so the action never simply does nothing.
/// Failures are silent by design — a missing editor is not worth an error dialog.
fn open_in_editor(path: &Path, line: u32) {
    let target = format!("{}:{}", path.display(), line);

    for (program, args) in [
        ("code", vec!["-g", target.as_str()]),
        ("cursor", vec!["-g", target.as_str()]),
        ("subl", vec![target.as_str()]),
        ("zed", vec![target.as_str()]),
    ] {
        if std::process::Command::new(program)
            .args(&args)
            .spawn()
            .is_ok()
        {
            return;
        }
    }

    // No line number available from here, but the file still opens.
    let path = path.as_os_str();
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", ""])
        .arg(path)
        .spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(path).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
}

fn build_entry(sent: &RequestDraft, outcome: &rl_http::Result<Exchange>) -> NewEntry {
    let mut entry = NewEntry::new(sent.method.to_string(), sent.url_with_path_values());

    match outcome {
        Ok(exchange) => {
            entry.status = Some(exchange.response.status);
            entry.duration_ms = Some(exchange.response.timing.total_ms);
            entry.request = serde_json::to_value(&exchange.request).unwrap_or_default();
            entry.response = serde_json::to_value(&exchange.response).ok();
        }
        Err(error) => {
            entry.error = Some(error.to_string());
            entry.request = serde_json::to_value(sent).unwrap_or_default();
        }
    }

    entry
}

#[cfg(test)]
mod tests {
    use super::*;
    use rl_model::{AuthConfig, HttpMethod};
    use tempfile::TempDir;

    /// A throwaway per-user data directory, so tests never touch the real one.
    fn data_dir() -> PathBuf {
        let dir = TempDir::new().unwrap();
        // Kept alive by leaking: the directory must outlive the `RouteLogic` using it.
        let path = dir.path().to_path_buf();
        std::mem::forget(dir);
        path
    }

    fn app() -> (TempDir, RouteLogic) {
        let dir = TempDir::new().unwrap();
        let mut app = RouteLogic::with_data_dir(data_dir());
        app.create_workspace(dir.path(), "test", WorkspaceKind::Project)
            .unwrap();
        (dir, app)
    }

    #[test]
    fn a_new_workspace_reports_itself() {
        let (_dir, app) = app();
        let info = app.info().unwrap();
        assert_eq!(info.name, "test");
        assert!(info.collections.is_empty());
        assert!(app.is_open());
    }

    #[test]
    fn operations_without_a_workspace_are_refused_clearly() {
        let app = RouteLogic::with_data_dir(data_dir());
        assert!(matches!(app.info(), Err(CoreError::NoWorkspace)));
        assert!(matches!(
            app.collection_names(),
            Err(CoreError::NoWorkspace)
        ));
    }

    #[test]
    fn switching_to_an_unknown_environment_is_refused() {
        let (_dir, mut app) = app();
        let before = app.active_environment().map(str::to_string);
        assert!(app.set_active_environment(Some("nope")).is_err());
        assert_eq!(
            app.active_environment(),
            before.as_deref(),
            "nothing changed"
        );
    }

    /// Every discovered request uses `{{base_url}}`, so a workspace must always have an
    /// environment to hold it — and reopening must land in the one that was chosen.
    #[test]
    fn a_new_workspace_gets_a_local_environment_that_is_remembered() {
        let dir = TempDir::new().unwrap();
        let mut app = RouteLogic::with_data_dir(data_dir());
        let info = app
            .create_workspace(dir.path(), "t", WorkspaceKind::Standalone)
            .unwrap();
        assert_eq!(info.environments, vec!["local"]);
        assert_eq!(info.active_environment.as_deref(), Some("local"));

        let mut staging = Environment::new("staging");
        staging.set("base_url", "https://staging.example.com");
        app.save_environment(&staging).unwrap();
        app.set_active_environment(Some("staging")).unwrap();

        let mut reopened = RouteLogic::with_data_dir(data_dir());
        let info = reopened.open_workspace(dir.path()).unwrap();
        assert_eq!(info.active_environment.as_deref(), Some("staging"));

        reopened.delete_environment("staging").unwrap();
        assert_eq!(reopened.active_environment(), Some("local"));
        assert_eq!(
            reopened.variable_names().unwrap(),
            Vec::<String>::new(),
            "local has no variables until a scan seeds base_url"
        );
    }

    #[test]
    fn switching_environments_changes_the_variables() {
        let (_dir, mut app) = app();

        let mut local = Environment::new("local");
        local.set("base_url", "http://localhost:8000");
        app.save_environment(&local).unwrap();

        let mut staging = Environment::new("staging");
        staging.set("base_url", "https://staging.example.com");
        app.save_environment(&staging).unwrap();

        app.set_active_environment(Some("local")).unwrap();
        assert_eq!(
            app.variables()
                .unwrap()
                .resolve("{{base_url}}")
                .unwrap()
                .value,
            "http://localhost:8000"
        );

        app.set_active_environment(Some("staging")).unwrap();
        assert_eq!(
            app.variables()
                .unwrap()
                .resolve("{{base_url}}")
                .unwrap()
                .value,
            "https://staging.example.com"
        );
    }

    #[test]
    fn saving_the_same_request_twice_replaces_rather_than_duplicates() {
        let (_dir, app) = app();

        let mut draft = RequestDraft::new(HttpMethod::Get, "https://x.test/a");
        draft.name = Some("Get a".into());
        app.save_request("Users", draft.clone()).unwrap();

        draft.url = "https://x.test/b".into();
        app.save_request("Users", draft).unwrap();

        let collection = app.load_collection("Users").unwrap();
        assert_eq!(collection.len(), 1);
        assert_eq!(collection.requests[0].url, "https://x.test/b");
    }

    #[test]
    fn renaming_a_saved_request_updates_it_by_id_and_keeps_its_folder() {
        let (_dir, app) = app();

        let mut draft = RequestDraft::new(HttpMethod::Get, "https://x.test/a");
        draft.name = Some("Get a".into());
        draft.folder = Some("Admin".into());
        app.save_request("Users", draft.clone()).unwrap();

        // The editor renames it and saves again, without knowing about folders.
        draft.name = Some("Fetch a".into());
        draft.folder = None;
        app.save_request("Users", draft).unwrap();

        let collection = app.load_collection("Users").unwrap();
        assert_eq!(collection.len(), 1, "a rename must not duplicate");
        assert_eq!(collection.requests[0].name.as_deref(), Some("Fetch a"));
        assert_eq!(collection.requests[0].folder.as_deref(), Some("Admin"));
    }

    #[test]
    fn the_whole_scan_saves_as_a_collection_filed_by_group() {
        let dir = fixture_copy("flask");
        let mut app = RouteLogic::with_data_dir(data_dir());
        app.create_workspace(dir.path(), "flask", WorkspaceKind::Project)
            .unwrap();
        assert!(matches!(
            app.save_scan_as_collection("API"),
            Err(CoreError::NoScan)
        ));
        app.scan().unwrap();

        let report = app.save_scan_as_collection("API").unwrap();
        assert_eq!(
            report.skipped_unresolved, 4,
            "gaps are not written as /?/stats"
        );
        assert_eq!(report.updated, 0);
        assert!(report.added >= 15);

        let collection = app.load_collection("API").unwrap();
        let users = collection
            .requests
            .iter()
            .find(|r| r.url == "{{base_url}}/api/v1/users/" && r.method == HttpMethod::Post)
            .expect("the create-user endpoint");
        assert_eq!(users.folder.as_deref(), Some("users"));
        assert!(users.spec_ref.is_some());
        assert_eq!(users.name.as_deref(), Some("Create a user."));

        // Saving again updates in place: same count, same ids, a moved folder kept.
        let first_id = users.id.clone();
        let mut edited = collection.clone();
        for r in &mut edited.requests {
            if r.id == first_id {
                r.folder = Some("Mine".into());
            }
        }
        app.save_collection(&edited).unwrap();
        let again = app.save_scan_as_collection("API").unwrap();
        assert_eq!(again.added, 0);
        assert_eq!(again.updated, report.added);
        let after = app.load_collection("API").unwrap();
        assert_eq!(after.len(), collection.len());
        let same = after.requests.iter().find(|r| r.id == first_id).unwrap();
        assert_eq!(same.folder.as_deref(), Some("Mine"));
    }

    #[test]
    fn collections_can_be_renamed_and_deleted() {
        let (_dir, app) = app();
        let mut draft = RequestDraft::new(HttpMethod::Get, "https://x.test/a");
        draft.name = Some("Get a".into());
        app.save_request("Users", draft.clone()).unwrap();
        app.save_request("Other", draft).unwrap();

        app.rename_collection("Users", "People").unwrap();
        assert_eq!(app.collection_names().unwrap(), vec!["Other", "People"]);
        assert_eq!(app.load_collection("People").unwrap().len(), 1);

        assert!(matches!(
            app.rename_collection("People", "Other"),
            Err(CoreError::CollectionExists { .. })
        ));

        app.delete_collection("Other").unwrap();
        assert_eq!(app.collection_names().unwrap(), vec!["People"]);
    }

    #[tokio::test]
    async fn a_request_with_an_undefined_variable_is_refused_before_sending() {
        let (_dir, app) = app();
        let draft = RequestDraft::new(HttpMethod::Get, "{{base_url}}/users");

        let error = app.send(&draft).await.unwrap_err();
        match error {
            CoreError::UndefinedVariables { names } => assert_eq!(names, vec!["base_url"]),
            other => panic!("expected undefined variables, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn every_undefined_variable_is_reported_at_once() {
        let (_dir, app) = app();
        let mut draft = RequestDraft::new(HttpMethod::Get, "{{host}}/users");
        draft
            .headers
            .push(rl_model::KeyValue::new("X-A", "{{token}}"));

        let error = app.send(&draft).await.unwrap_err();
        match error {
            CoreError::UndefinedVariables { names } => {
                assert!(names.contains(&"host".to_string()));
                assert!(names.contains(&"token".to_string()));
            }
            other => panic!("expected undefined variables, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_failed_send_is_still_recorded_in_history() {
        let (_dir, app) = app();
        // Nothing listens on port 1.
        let draft = RequestDraft::new(HttpMethod::Get, "http://127.0.0.1:1/");

        assert!(app.send(&draft).await.is_err());

        let history = app.history(10).unwrap();
        assert_eq!(history.len(), 1);
        assert!(history[0].error.is_some());
        assert_eq!(history[0].status, None);
    }

    // --- flows --------------------------------------------------------------------------

    #[test]
    fn flows_are_listed_saved_renamed_and_deleted() {
        let (_dir, app) = app();
        assert!(app.info().unwrap().flows.is_empty());

        let mut flow = Flow::new("smoke");
        flow.add(rl_model::Node::request(RequestDraft::new(
            HttpMethod::Get,
            "{{base_url}}/health",
        )));
        app.save_flow(&flow).unwrap();
        assert_eq!(app.info().unwrap().flows, vec!["smoke"]);
        // Saved with the node placed, since it was added without a position.
        let mut placed = flow.clone();
        placed.lay_out();
        assert_eq!(app.load_flow("smoke").unwrap(), placed);
        flow = placed;

        app.rename_flow("smoke", "health check").unwrap();
        assert_eq!(app.flow_names().unwrap(), vec!["health check"]);
        assert_eq!(app.load_flow("health check").unwrap().nodes, flow.nodes);

        app.save_flow(&Flow::new("other")).unwrap();
        assert!(matches!(
            app.rename_flow("health check", "other"),
            Err(CoreError::FlowExists { .. })
        ));

        app.delete_flow("health check").unwrap();
        assert_eq!(app.flow_names().unwrap(), vec!["other"]);
    }

    /// A flow written by hand — or by an agent — without positions opens with every card
    /// placed, and saving it writes the placement down, leaving no stray temporary file.
    #[test]
    fn a_flow_written_without_positions_is_placed_on_load_and_on_save() {
        let (_dir, app) = app();
        let path = app.flow_path("by hand").unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            concat!(
                "name: by hand
",
                "nodes:
",
                "- id: health
",
                "  type: request
",
                "  request: {method: GET, url: '{{base_url}}/health'}
",
                "- id: users
",
                "  type: request
",
                "  request: {method: GET, url: '{{base_url}}/users'}
",
                "edges:
",
                "- {from: health, to: users}
",
            ),
        )
        .unwrap();

        let loaded = app.load_flow("by hand").unwrap();
        assert!(loaded.nodes.iter().all(|n| n.position.is_some()));

        app.save_flow(&loaded).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("position:"), "{text}");
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "the atomic write cleaned up after itself"
        );
    }

    #[test]
    fn lint_checks_a_flow_against_the_chosen_environment() {
        let (_dir, app) = app();
        let mut env = rl_workspace::Environment::new("local");
        env.set("base_url", "http://localhost:8000");
        app.save_environment(&env).unwrap();
        app.save_environment(&rl_workspace::Environment::new("staging"))
            .unwrap();
        let active = app.active_environment().map(str::to_string);

        let mut flow = Flow::new("lint");
        flow.add(rl_model::Node::request(RequestDraft::new(
            HttpMethod::Get,
            "{{base_url}}/users/{{user_id}}",
        )));
        let warnings = app.lint_flow(&flow, Some("local")).unwrap();
        let text: Vec<&str> = warnings.iter().map(|w| w.message.as_str()).collect();
        assert!(text.iter().any(|m| m.contains("{{user_id}}")), "{text:?}");
        assert!(!text.iter().any(|m| m.contains("{{base_url}}")), "{text:?}");
        assert!(text.iter().any(|m| m.contains("no assertions")));

        assert!(matches!(
            app.lint_flow(&flow, Some("nowhere")),
            Err(CoreError::NoSuchEnvironment { .. })
        ));
        // Checking against an environment does not switch to it.
        assert_eq!(app.active_environment().map(str::to_string), active);
    }

    #[test]
    fn a_flow_with_a_cycle_is_refused_before_anything_runs() {
        let (_dir, app) = app();
        let mut flow = Flow::new("loop");
        let a = flow.add(rl_model::Node::request(RequestDraft::new(
            HttpMethod::Get,
            "http://x/a",
        )));
        let b = flow.add(rl_model::Node::request(RequestDraft::new(
            HttpMethod::Get,
            "http://x/b",
        )));
        flow.connect(&a, &b);
        flow.connect(&b, &a);
        assert!(matches!(
            app.prepare_flow(flow, RunOptions::default()),
            Err(CoreError::Flow(_))
        ));
    }

    // --- agents -------------------------------------------------------------------------

    /// A workspace whose `local` environment points at [`tiny_api`], with the token it hands
    /// out stored as a secret, and a `prod` environment pointing somewhere an agent may
    /// not go.
    async fn agent_app() -> (TempDir, RouteLogic) {
        let (dir, mut app) = app();
        let base = tiny_api().await;
        let mut local = Environment::new("local");
        local.set("base_url", &base);
        app.save_environment(&local).unwrap();
        let mut prod = Environment::new("prod");
        prod.set("base_url", "https://api.example.invalid");
        app.save_environment(&prod).unwrap();
        app.set_active_environment(Some("local")).unwrap();
        app.set_secret("api_token", "tok-secret-1").unwrap();
        (dir, app)
    }

    #[tokio::test]
    async fn an_agent_send_to_loopback_goes_out_masked_and_is_recorded_as_the_agents() {
        let (_dir, app) = agent_app().await;
        let login = RequestDraft::new(HttpMethod::Post, "{{base_url}}/login");

        let sent = app.send_for_agent(&login, None, 10_000).await.unwrap();
        let AgentSend::Sent { response } = sent else {
            panic!("expected a response, got {sent:?}")
        };
        assert_eq!(response.status, 200);
        assert!(
            response.body.contains("{{secret:api_token}}"),
            "the token is named, not shown: {}",
            response.body
        );
        assert!(!response.body.contains("tok-secret-1"));

        let recorded = app.history(1).unwrap();
        assert_eq!(recorded[0].source.as_deref(), Some(agent::AGENT_SOURCE));
    }

    #[tokio::test]
    async fn an_agent_send_elsewhere_is_refused_before_anything_leaves() {
        let (_dir, app) = agent_app().await;
        let draft = RequestDraft::new(HttpMethod::Delete, "{{base_url}}/users/7");

        let refused = app
            .send_for_agent(&draft, Some("prod"), 10_000)
            .await
            .unwrap();
        let AgentSend::Refused { refusal } = refused else {
            panic!("expected a refusal, got {refused:?}")
        };
        assert_eq!(refusal.host, "api.example.invalid");
        assert!(refusal.fix.contains("agent.allow"));
        // Recorded as the agent's attempt, with no response, since nothing went out.
        let recorded = app.history(10).unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].status, None);
        assert!(recorded[0].error.as_deref().unwrap().contains("refused"));
        assert_eq!(recorded[0].source.as_deref(), Some(agent::AGENT_SOURCE));
        // Sending in `prod` did not make it the active environment.
        assert_eq!(app.active_environment(), Some("local"));
    }

    /// Edited by hand while the agent is connected, and applied to its very next request.
    #[tokio::test]
    async fn the_allow_list_is_read_fresh_for_every_request() {
        let (_dir, app) = agent_app().await;
        let target = url::Url::parse("https://api.example.invalid/users").unwrap();
        assert!(app.agent_policy().unwrap().check("GET", &target).is_err());

        let path = app.workspace().unwrap().layout().manifest();
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(
            &path,
            text + "agent:\n  allow:\n    - host: api.example.invalid\n      methods: [GET]\n",
        )
        .unwrap();

        let policy = app.agent_policy().unwrap();
        assert!(policy.check("GET", &target).is_ok());
        assert!(policy.check("DELETE", &target).is_err());
    }

    /// A flow an agent runs: the loopback steps chain for real, the step aimed elsewhere
    /// fails and says why, and the token captured along the way comes back masked.
    #[tokio::test]
    async fn an_agent_flow_run_refuses_the_step_it_may_not_send_and_masks_the_rest() {
        use rl_model::{Assertion, Extraction, KeyValue, Node, NodeKind, ValueSource};
        let (_dir, app) = agent_app().await;

        let mut flow = Flow::new("agent");
        let mut login = Node::request(RequestDraft::new(HttpMethod::Post, "{{base_url}}/login"));
        if let NodeKind::Request {
            extract, assert, ..
        } = &mut login.kind
        {
            extract.push(Extraction {
                name: "auth_token".into(),
                source: ValueSource::Body {
                    path: "access_token".into(),
                },
            });
            assert.push(Assertion::status_ok());
        }
        let login = flow.add(login);
        let mut me = RequestDraft::new(HttpMethod::Get, "{{base_url}}/me");
        me.headers
            .push(KeyValue::new("Authorization", "Bearer {{auth_token}}"));
        let me = flow.add(Node::request(me));
        let outside = flow.add(Node::request(RequestDraft::new(
            HttpMethod::Get,
            "https://api.example.invalid/x",
        )));
        flow.connect(&login, &me);
        flow.connect(&me, &outside);

        let run = app
            .run_flow_for_agent(&flow, None, RunOptions::default(), 10_000)
            .await
            .unwrap();

        let outcomes: Vec<&str> = run.steps.iter().map(|s| s.outcome.as_str()).collect();
        assert_eq!(outcomes, vec!["passed", "passed", "failed"]);
        assert_eq!(
            run.steps[0].extracted,
            vec![("auth_token".to_string(), "{{secret:api_token}}".to_string())]
        );
        let reason = run.steps[2].reason.as_deref().unwrap();
        assert!(reason.contains("not in the agent allow list"), "{reason}");
        assert!(!serde_json::to_string(&run)
            .unwrap()
            .contains("tok-secret-1"));

        let recorded = app.history(10).unwrap();
        // All three are the agent's; the refused one has no response, since it never went.
        assert_eq!(recorded.len(), 3);
        assert_eq!(recorded[0].status, None);
        assert!(recorded[0].error.as_deref().unwrap().contains("refused"));
        assert!(recorded
            .iter()
            .all(|e| e.source.as_deref() == Some("agent")));
    }

    #[tokio::test]
    async fn prepare_for_an_agent_masks_what_would_be_sent() {
        use rl_model::KeyValue;
        let (_dir, app) = agent_app().await;
        let mut draft = RequestDraft::new(HttpMethod::Get, "{{base_url}}/me");
        draft.headers.push(KeyValue::new(
            "Authorization",
            "Bearer {{secret:api_token}}",
        ));
        let prepared = app.prepare_for_agent(&draft, None).unwrap();
        let text = serde_json::to_string(&prepared).unwrap();
        assert!(text.contains("Bearer {{secret:api_token}}"), "{text}");
        assert!(!text.contains("tok-secret-1"));
    }

    /// A one-connection-at-a-time HTTP/1.1 server that plays a tiny API: `POST /login`
    /// hands out a token, `GET /me` demands it. Enough to prove a flow chains for real.
    async fn tiny_api() -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&buf[..n]).to_string();
                    let line = request.lines().next().unwrap_or_default().to_string();
                    let authed = request
                        .lines()
                        .any(|l| l.eq_ignore_ascii_case("authorization: Bearer tok-secret-1"));
                    let (status, body) = if line.starts_with("POST /login") {
                        (200, r#"{"access_token":"tok-secret-1"}"#)
                    } else if line.starts_with("GET /me") && authed {
                        (200, r#"{"user":{"id":7}}"#)
                    } else if line.starts_with("GET /me") {
                        (401, r#"{"detail":"missing token"}"#)
                    } else if line.starts_with("GET /users/7") {
                        (200, r#"{"id":7,"name":"Aryan"}"#)
                    } else {
                        (404, r#"{"detail":"no"}"#)
                    };
                    let response = format!(
                        "HTTP/1.1 {status} X
content-type: application/json
content-length: {}
connection: close

{body}",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                });
            }
        });
        base
    }

    #[tokio::test]
    async fn a_flow_runs_end_to_end_records_history_and_redacts_extracted_secrets() {
        use rl_model::{Assertion, Extraction, KeyValue, Node, NodeKind, ValueSource};

        let (_dir, mut app) = app();
        let base = tiny_api().await;
        let mut env = Environment::new("local");
        env.set("base_url", &base);
        app.save_environment(&env).unwrap();
        app.set_active_environment(Some("local")).unwrap();
        // The token the API hands out is also a known secret, so history must not show it.
        app.set_secret("api_token", "tok-secret-1").unwrap();

        let mut flow = Flow::new("login");
        let mut login = Node::request(RequestDraft::new(HttpMethod::Post, "{{base_url}}/login"));
        if let NodeKind::Request {
            extract, assert, ..
        } = &mut login.kind
        {
            extract.push(Extraction {
                name: "auth_token".into(),
                source: ValueSource::Body {
                    path: "access_token".into(),
                },
            });
            assert.push(Assertion::status_ok());
        }
        let login = flow.add(login);

        let mut me_draft = RequestDraft::new(HttpMethod::Get, "{{base_url}}/me");
        me_draft
            .headers
            .push(KeyValue::new("Authorization", "Bearer {{auth_token}}"));
        let mut me = Node::request(me_draft);
        if let NodeKind::Request { extract, .. } = &mut me.kind {
            extract.push(Extraction {
                name: "user_id".into(),
                source: ValueSource::Body {
                    path: "user.id".into(),
                },
            });
        }
        let me = flow.add(me);

        let mut user = Node::request(RequestDraft::new(
            HttpMethod::Get,
            "{{base_url}}/users/{{user_id}}",
        ));
        if let NodeKind::Request { assert, .. } = &mut user.kind {
            assert.push(Assertion {
                source: ValueSource::Body {
                    path: "name".into(),
                },
                op: rl_model::Operator::Equals,
                expected: "Aryan".into(),
            });
        }
        let user = flow.add(user);
        flow.connect(&login, &me);
        flow.connect(&me, &user);

        let mut events = 0;
        let run = app
            .prepare_flow(flow, RunOptions::default())
            .unwrap()
            .run(&mut |_| events += 1)
            .await
            .unwrap();
        assert!(run.passed(), "{:#?}", run.results);
        assert_eq!(events, 1 + 3 * 2 + 1);
        assert_eq!(run.variables["user_id"], "7");
        assert_eq!(
            run.result(&user)
                .unwrap()
                .request
                .as_ref()
                .unwrap()
                .url_with_path_values(),
            format!("{base}/users/7")
        );

        // Three rows, newest first, with the token nowhere in them.
        let history = app.history(10).unwrap();
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].url, format!("{base}/users/7"));
        assert_eq!(history[2].method, "POST");
        let dumped = serde_json::to_string(&history).unwrap();
        assert!(!dumped.contains("tok-secret-1"), "{dumped}");
        assert!(dumped.contains(rl_model::REDACTION));
    }

    #[test]
    fn importing_curl_produces_a_sendable_draft() {
        let (_dir, app) = app();
        let imported = app
            .import_curl(
                r#"curl 'https://api.example.com/users?page=2' -H 'Authorization: Bearer t'"#,
            )
            .unwrap();

        assert_eq!(imported.value.url, "https://api.example.com/users");
        assert_eq!(
            imported.value.auth,
            AuthConfig::Bearer { token: "t".into() }
        );
    }

    #[test]
    fn importing_openapi_writes_a_collection_of_requests() {
        let (_dir, app) = app();
        let spec = r#"{
            "openapi": "3.0.0",
            "info": {"title": "Petstore", "version": "1.0"},
            "servers": [{"url": "https://api.example.com"}],
            "paths": {
                "/pets": {"get": {"summary": "List pets"}, "post": {"summary": "Create pet"}},
                "/pets/{id}": {"get": {"summary": "Get pet"}}
            }
        }"#;

        let imported = app.import_openapi_as_collection(spec, "Petstore").unwrap();
        assert_eq!(imported.value.endpoints.len(), 3);

        let collection = app.load_collection("Petstore").unwrap();
        assert_eq!(collection.len(), 3);

        let list = collection.find("List pets").unwrap();
        assert_eq!(list.url, "https://api.example.com/pets");
        // The discovered spec is linked, so the request knows where it came from.
        assert!(list.spec_ref.is_some());
    }

    #[test]
    fn an_openapi_import_without_a_server_falls_back_to_a_variable() {
        let (_dir, app) = app();
        let spec = r#"{
            "openapi": "3.0.0",
            "info": {"title": "X", "version": "1"},
            "paths": {"/x": {"get": {}}}
        }"#;

        let imported = app.import_openapi_as_collection(spec, "X").unwrap();
        assert!(
            !imported.is_clean(),
            "the missing server should be reported"
        );

        let collection = app.load_collection("X").unwrap();
        assert!(collection.requests[0].url.starts_with("{{base_url}}"));
    }

    /// A workspace with a small FastAPI project inside it.
    fn project_workspace() -> (TempDir, RouteLogic) {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("requirements.txt"), "fastapi\n").unwrap();
        std::fs::create_dir_all(dir.path().join("app")).unwrap();
        std::fs::write(
            dir.path().join("app/api.py"),
            "from fastapi import APIRouter\n\
             router = APIRouter(prefix=\"/users\", tags=[\"users\"])\n\
             @router.get(\"/{user_id}\")\n\
             def get_user(user_id: int): ...\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("app/main.py"),
            "from fastapi import FastAPI\n\
             from .api import router\n\
             app = FastAPI()\n\
             app.include_router(router, prefix=\"/api/v1\")\n",
        )
        .unwrap();

        let mut app = RouteLogic::with_data_dir(data_dir());
        app.create_workspace(dir.path(), "fixture", WorkspaceKind::Project)
            .unwrap();
        (dir, app)
    }

    #[test]
    fn scanning_finds_the_projects_endpoints() {
        let (_dir, mut app) = project_workspace();
        let result = app.scan().unwrap();

        assert_eq!(result.frameworks[0].id, "fastapi");
        assert_eq!(result.endpoints.len(), 1);
        assert_eq!(result.endpoints[0].display, "GET /api/v1/users/{user_id}");
        assert!(result.endpoints[0].source.is_some());
    }

    #[test]
    fn a_discovered_endpoint_becomes_an_editable_request() {
        let (_dir, mut app) = project_workspace();
        let scan = app.scan().unwrap();
        let id = scan.endpoints[0].id.clone();

        let draft = app.request_for(&id, None).unwrap();

        // Defaults to the variable, so switching environments repoints it.
        assert_eq!(draft.url, "{{base_url}}/api/v1/users/{user_id}");
        assert_eq!(draft.spec_ref.as_ref().unwrap().as_str(), id);
        assert!(draft.path_values.contains_key("user_id"));
    }

    #[test]
    fn an_endpoint_with_an_unresolved_path_is_refused_rather_than_sent_somewhere_meaningless() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("requirements.txt"), "fastapi\n").unwrap();
        std::fs::write(
            dir.path().join("main.py"),
            "from fastapi import FastAPI, APIRouter\n\
             from .config import settings\n\
             app = FastAPI()\n\
             router = APIRouter()\n\
             @router.get(\"/ping\")\n\
             def ping(): ...\n\
             app.include_router(router, prefix=settings.API_PREFIX)\n",
        )
        .unwrap();

        let mut app = RouteLogic::with_data_dir(data_dir());
        app.create_workspace(dir.path(), "x", WorkspaceKind::Project)
            .unwrap();
        let scan = app.scan().unwrap();
        let id = scan.endpoints[0].id.clone();

        match app.request_for(&id, None) {
            Err(CoreError::UnresolvedEndpoint { expressions, .. }) => {
                assert_eq!(expressions, vec!["settings.API_PREFIX"]);
            }
            other => panic!("expected a refusal naming the gap, got {other:?}"),
        }
    }

    #[test]
    fn scanning_seeds_an_empty_base_url_but_never_overwrites_a_chosen_one() {
        let (_dir, mut app) = project_workspace();

        let mut env = Environment::new("local");
        app.save_environment(&env).unwrap();
        app.set_active_environment(Some("local")).unwrap();

        app.scan().unwrap();
        let seeded = app.load_environment("local").unwrap();
        assert_eq!(
            seeded.variables.get("base_url").map(String::as_str),
            Some("http://localhost:8000")
        );

        // A deliberate choice survives a rescan.
        env = seeded;
        env.set("base_url", "https://staging.example.com");
        app.save_environment(&env).unwrap();
        app.scan().unwrap();

        assert_eq!(
            app.load_environment("local")
                .unwrap()
                .variables
                .get("base_url")
                .map(String::as_str),
            Some("https://staging.example.com")
        );
    }

    #[test]
    fn a_standalone_workspace_has_nothing_to_scan() {
        let dir = TempDir::new().unwrap();
        let mut app = RouteLogic::with_data_dir(data_dir());
        app.create_workspace(dir.path(), "client", WorkspaceKind::Standalone)
            .unwrap();

        assert!(matches!(app.scan(), Err(CoreError::NotAProject)));
    }

    #[test]
    fn opening_an_endpoint_before_scanning_is_refused_clearly() {
        let (_dir, app) = project_workspace();
        assert!(matches!(
            app.request_for("GET /x", None),
            Err(CoreError::NoScan)
        ));
    }

    #[test]
    fn secret_names_are_listable_without_exposing_values() {
        let (_dir, app) = app();
        app.set_secret("api_token", "s3cr3t").unwrap();

        assert_eq!(app.secret_names().unwrap(), vec!["api_token"]);
        // And the value is reachable only through resolution.
        let ctx = app.variables().unwrap();
        assert_eq!(ctx.resolve("{{secret:api_token}}").unwrap().value, "s3cr3t");
    }

    #[test]
    fn missing_secrets_are_reported_by_the_workspace_summary() {
        let (_dir, mut app) = app();
        let mut env = Environment::new("local");
        env.set("base_url", "http://x.test")
            .expect_secret("api_token");
        app.save_environment(&env).unwrap();
        app.set_active_environment(Some("local")).unwrap();

        assert_eq!(app.info().unwrap().missing_secrets, vec!["api_token"]);

        app.set_secret("api_token", "s3cr3t").unwrap();
        assert!(app.info().unwrap().missing_secrets.is_empty());
    }

    /// A working copy of a fixture project, so the workspace files land in a temp dir.
    fn fixture_copy(name: &str) -> TempDir {
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures")
            .join(name);
        let dir = TempDir::new().unwrap();
        copy_tree(&source, dir.path());
        dir
    }

    fn copy_tree(from: &Path, to: &Path) {
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                // A workspace left by opening the fixture in the app — under either name.
                let name = entry.file_name();
                if name == "__pycache__" || name == ".routelogic" || name == ".routelens" {
                    continue;
                }
                std::fs::create_dir_all(&target).unwrap();
                copy_tree(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    #[test]
    fn an_enrich_proposal_runs_nothing_and_names_everything() {
        let dir = fixture_copy("flask");
        let mut app = RouteLogic::with_data_dir(data_dir());
        app.create_workspace(dir.path(), "flask", WorkspaceKind::Project)
            .unwrap();

        assert!(matches!(app.enrich_proposal(), Err(CoreError::NoScan)));
        let scan = app.scan().unwrap();
        assert!(scan.enrichable);
        assert!(scan.enrich.is_none());

        let proposal = app.enrich_proposal().unwrap();
        let targets: Vec<&str> = proposal.targets.iter().map(|t| t.target.as_str()).collect();
        assert_eq!(
            targets,
            vec!["app", "app:create_app()"],
            "the Procfile's `flask --app app`, then the factory the scan saw"
        );
        assert!(proposal.remembered_target.is_none());
        assert!(
            !proposal.helper_path.exists(),
            "proposing must not write the helper"
        );
        assert!(
            app.info().unwrap().name == "flask",
            "and must not touch the workspace"
        );
    }

    #[test]
    fn enrich_is_refused_for_a_framework_with_no_runtime_spec() {
        let dir = fixture_copy("express");
        let mut app = RouteLogic::with_data_dir(data_dir());
        app.create_workspace(dir.path(), "express", WorkspaceKind::Project)
            .unwrap();
        let scan = app.scan().unwrap();
        assert!(!scan.enrichable);
        assert!(matches!(
            app.enrich_proposal(),
            Err(CoreError::NotEnrichable { .. })
        ));
    }

    /// The whole loop against the Flask fixture, when a Python with Flask is available
    /// (`ROUTELOGIC_TEST_PYTHON`, or a detected interpreter that can import it).
    #[test]
    fn enrich_merges_runtime_truth_onto_static_locations_and_records_consent() {
        let Some(python) = python_with("flask") else {
            eprintln!("skipping: no Python with flask importable");
            return;
        };
        let dir = fixture_copy("flask");
        let mut app = RouteLogic::with_data_dir(data_dir());
        app.create_workspace(dir.path(), "flask", WorkspaceKind::Project)
            .unwrap();
        let before = app.scan().unwrap();
        let static_unresolved = before.endpoints.iter().filter(|e| e.unresolved).count();
        assert_eq!(
            static_unresolved, 4,
            "3 admin routes + the loop registration"
        );

        let after = app.run_enrich("app:create_app()", Some(&python)).unwrap();
        let report = after.enrich.as_ref().expect("a report");
        assert_eq!(report.framework, "flask");
        assert_eq!(
            report.gaps_filled, 3,
            "the three /admin routes learn their prefix"
        );
        let runtime_only: Vec<&str> = after
            .endpoints
            .iter()
            .filter(|e| e.enrich.as_deref() == Some("runtime_only"))
            .map(|e| e.display.as_str())
            .collect();
        assert_eq!(
            runtime_only,
            vec!["GET /dyn/gadgets", "GET /dyn/widgets"],
            "{report:?}"
        );
        assert!(report.matched >= 15, "{report:?}");
        assert!(report.command.contains("routelogic_enrich.py"));

        // The gap closed, and the source location survived the merge.
        let stats = after
            .endpoints
            .iter()
            .find(|e| e.path == "/admin/stats")
            .expect("resolved by runtime");
        assert_eq!(stats.enrich.as_deref(), Some("gap_filled"));
        assert_eq!(stats.source.as_ref().unwrap().file, "app/api/admin.py");
        assert!(!stats.unresolved);

        // Runtime-only routes have no source, and say so.
        let widgets = after
            .endpoints
            .iter()
            .find(|e| e.path == "/dyn/widgets")
            .unwrap();
        assert!(widgets.source.is_none());
        assert_eq!(widgets.enrich.as_deref(), Some("runtime_only"));

        // The orphan was not served, and is still listed.
        let orphan = after
            .endpoints
            .iter()
            .find(|e| e.path == "/orphan/forgotten")
            .unwrap();
        assert_eq!(orphan.enrich.as_deref(), Some("static_only"));

        // The enriched endpoint opens as a request like any other.
        let draft = app.request_for(&stats.id, None).unwrap();
        assert_eq!(draft.url, "{{base_url}}/admin/stats");

        // Consent was recorded, and can be withdrawn.
        assert_eq!(
            app.enrich_proposal().unwrap().remembered_target.as_deref(),
            Some("app:create_app()")
        );
        let manifest =
            std::fs::read_to_string(dir.path().join(".routelogic/workspace.yaml")).unwrap();
        assert!(
            manifest.contains("app_target: app:create_app()"),
            "{manifest}"
        );
        app.revoke_enrich().unwrap();
        assert!(app.enrich_proposal().unwrap().remembered_target.is_none());

        // A rescan drops the enrichment; static is the default again.
        let again = app.scan().unwrap();
        assert!(again.enrich.is_none());
        assert!(again.endpoints.iter().all(|e| e.enrich.is_none()));
    }

    fn python_with(module: &str) -> Option<PathBuf> {
        let candidates: Vec<PathBuf> = std::env::var_os("ROUTELOGIC_TEST_PYTHON")
            .map(|p| vec![PathBuf::from(p)])
            .unwrap_or_else(|| {
                rl_discovery::enrich::interpreter::detect(Path::new("."))
                    .into_iter()
                    .map(|i| i.path)
                    .collect()
            });
        candidates.into_iter().find(|python| {
            std::process::Command::new(python)
                .args(["-c", &format!("import {module}")])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        })
    }
}
