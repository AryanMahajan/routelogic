//! # rl-mcp
//!
//! RouteLogic over the Model Context Protocol, for the agent a developer already has open
//! — Claude Code, Cursor, Zed. It reads the project's API, sends requests to check what an
//! endpoint really returns, and writes flows the developer then opens on the canvas.
//!
//! Every tool is a thin wrapper over [`rl_core::RouteLogic`]. What an agent may send, and
//! what it is shown, is decided in [`rl_core::agent`], not here: loopback only unless the
//! workspace allows more, and secrets masked as `{{secret:NAME}}`.
//!
//! The server speaks MCP over stdio. It ships inside the desktop app as a subcommand —
//! `routelogic mcp --workspace <project>` — and as `routelogic-mcp` for development.

#![forbid(unsafe_code)]

use rl_core::{AgentSend, CoreError, RouteLogic};
use rl_model::{Flow, NodeId, RequestDraft};
use rl_workspace::WorkspaceKind;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::{tool, tool_handler, tool_router, ServerHandler, ServiceExt};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

/// How much of a response body a tool returns unless asked for more. A flow is built from
/// a response's shape, and the shape is near the top.
const DEFAULT_BODY: usize = 4_000;
const MAX_BODY: usize = 200_000;
/// A failing step's body in a run: enough to see the error, not a page of HTML.
const RUN_BODY: usize = 2_000;
const DEFAULT_ENDPOINTS: usize = 50;
const MAX_ENDPOINTS: usize = 200;

/// Sent to the agent when it connects. It is the manual: what the tools are for, in the
/// order a flow gets built, and the rules it will run into.
pub const INSTRUCTIONS: &str = "\
RouteLogic knows this project's HTTP API from its source code, can send requests to it, \
and stores multi-step API tests (\"flows\") that the developer opens and runs on a canvas.

To build a flow:
1. workspace_info: environments, variable names to use as {{name}}, and where you may send \
requests.
2. list_endpoints (use `filter` on a large API), then describe_endpoint for parameters, a body \
example, auth, the handler's file and line, and a request node ready to put in a flow.
3. send_request to try a step before writing it. Read the real status and body, so captures \
use the field names the API actually returns.
4. save_flow. Nodes are steps; an edge makes its target run after its source, and only if \
the source passed. Capture a value with `extract` ({\"name\": \"token\", \"from\": \"body\", \
\"path\": \"access_token\"}) and use it in later steps as {{token}}. Assert on every request, \
at least its status. Node ids can be words (login, create_user). Leave positions out: \
RouteLogic lays the cards out.
5. run_flow, read the first failing step, fix, and save_flow again with overwrite: true.

Start URLs with {{base_url}}, so a flow runs against whichever environment is chosen. \
Secret values are never shown: they appear as {{secret:NAME}}, which is also how to use one. \
Requests may go to loopback hosts; any other host must be listed under agent.allow in \
.routelogic/workspace.yaml. A refusal says what to add. Tell the user rather than working \
around it.";

/// The server: one workspace, opened for the life of the connection.
#[derive(Clone)]
pub struct RouteLogicServer {
    app: Arc<Mutex<RouteLogic>>,
    #[allow(dead_code, reason = "read by the code #[tool_handler] generates")]
    tool_router: ToolRouter<Self>,
}

impl RouteLogicServer {
    /// Open the workspace at `root`, creating `.routelogic/` if the project has none yet —
    /// what the app does when a folder is opened.
    pub fn open(root: &Path) -> Result<Self, CoreError> {
        let mut app = RouteLogic::new();
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "workspace".into());
        app.open_or_create_workspace(root, name, WorkspaceKind::Project)?;
        Ok(Self::with_app(app))
    }

    /// A server over an application already set up — for tests.
    pub fn with_app(app: RouteLogic) -> Self {
        RouteLogicServer {
            app: Arc::new(Mutex::new(app)),
            tool_router: Self::tool_router(),
        }
    }

    /// Serve over stdin and stdout until the client disconnects.
    pub async fn serve_stdio(self) -> Result<(), String> {
        let running = self
            .serve(rmcp::transport::stdio())
            .await
            .map_err(|e| e.to_string())?;
        running.waiting().await.map_err(|e| e.to_string())?;
        Ok(())
    }
}

// --- parameters --------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ListEndpoints {
    /// Case-insensitive text matched against each endpoint's method, path, group and
    /// summary, e.g. `users` or `POST /auth`.
    #[serde(default)]
    pub filter: Option<String>,
    /// Skip this many matches, to page through a large API.
    #[serde(default)]
    pub offset: Option<usize>,
    /// At most this many (default 50, at most 200).
    #[serde(default)]
    pub limit: Option<usize>,
    /// Scan the source again first, after the code has changed.
    #[serde(default)]
    pub rescan: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct Endpoint {
    /// As `list_endpoints` shows it: `GET /api/v1/users/{user_id}`.
    pub id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendRequest {
    /// The request. Only `method` and `url` are required; `url` should start with
    /// `{{base_url}}`, and `{param}` segments are filled from `path_values`.
    pub request: RequestDraft,
    /// Resolve variables in this environment rather than the active one.
    #[serde(default)]
    pub environment: Option<String>,
    /// How much of the response body to return (default 4000).
    #[serde(default)]
    pub max_body_bytes: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PrepareRequest {
    pub request: RequestDraft,
    #[serde(default)]
    pub environment: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FlowName {
    pub name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SaveFlow {
    /// The whole flow. Its `name` is the file it is saved as.
    pub flow: Flow,
    /// Replace a flow that already exists by that name. Read it with `get_flow` first;
    /// cards keep their places on the canvas when their ids are kept.
    #[serde(default)]
    pub overwrite: bool,
    /// Check the flow's variables against this environment rather than the active one.
    #[serde(default)]
    pub environment: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RunFlow {
    /// A saved flow to run.
    #[serde(default)]
    pub name: Option<String>,
    /// Or a flow to run without saving it, to try an idea.
    #[serde(default)]
    pub flow: Option<Flow>,
    #[serde(default)]
    pub environment: Option<String>,
    /// Run only these node ids; steps outside them count as passed.
    #[serde(default)]
    pub only: Option<Vec<String>>,
    /// How much of a failing step's response body to return (default 2000).
    #[serde(default)]
    pub max_body_bytes: Option<usize>,
}

// --- tools -------------------------------------------------------------------------------

#[tool_router]
impl RouteLogicServer {
    #[tool(
        description = "The workspace: its environments and the active one, the variable and \
                       secret names a request can use as {{name}}, saved flows, and where an \
                       agent may send requests. Call this first.",
        annotations(read_only_hint = true)
    )]
    async fn workspace_info(&self) -> CallToolResult {
        let app = self.app.lock().await;
        respond((|| {
            let info = app.info()?;
            Ok(json!({
                "name": info.name,
                "root": info.root,
                "active_environment": info.active_environment,
                "environments": info.environments,
                "variables": app.variable_names()?,
                "secrets": app
                    .secret_names()?
                    .iter()
                    .map(|s| format!("{{{{secret:{s}}}}}"))
                    .collect::<Vec<_>>(),
                "missing_secrets": info.missing_secrets,
                "flows": info.flows,
                "endpoints": match app.last_scan() {
                    Some(scan) => json!(scan.endpoints.len()),
                    None => json!("not scanned yet; list_endpoints scans"),
                },
                "agent_permissions": app.agent_policy()?.view(),
            }))
        })())
    }

    #[tool(
        description = "The HTTP endpoints this project serves, found by reading its source: \
                       one line each with the method, path, group, whether it needs auth or \
                       takes a body, and the handler's file and line. Scans on first use.",
        annotations(read_only_hint = true)
    )]
    async fn list_endpoints(&self, Parameters(p): Parameters<ListEndpoints>) -> CallToolResult {
        let mut app = self.app.lock().await;
        if p.rescan.unwrap_or(false) || app.last_scan().is_none() {
            if let Err(e) = app.scan() {
                return fail(e);
            }
        }
        let Some(scan) = app.last_scan() else {
            return fail("no scan result");
        };

        let needle = p.filter.as_deref().unwrap_or("").to_ascii_lowercase();
        let matched: Vec<&rl_model::EndpointSpec> = scan
            .endpoints
            .iter()
            .filter(|e| {
                needle.is_empty()
                    || [
                        e.display(),
                        e.group.clone().unwrap_or_default(),
                        e.summary.clone().unwrap_or_default(),
                    ]
                    .iter()
                    .any(|field| field.to_ascii_lowercase().contains(&needle))
            })
            .collect();

        let offset = p.offset.unwrap_or(0);
        let limit = p.limit.unwrap_or(DEFAULT_ENDPOINTS).clamp(1, MAX_ENDPOINTS);
        let page: Vec<String> = matched
            .iter()
            .skip(offset)
            .take(limit)
            .map(|e| line(e))
            .collect();

        let frameworks: Vec<&str> = scan.frameworks.iter().map(|f| f.id.as_str()).collect();
        let mut out = format!(
            "{} of {} endpoints{} ({}). describe_endpoint takes the METHOD /path shown.\n\n",
            page.len(),
            matched.len(),
            if needle.is_empty() {
                String::new()
            } else {
                format!(
                    " matching {:?}, of {} in all",
                    p.filter.unwrap_or_default(),
                    scan.endpoints.len()
                )
            },
            if frameworks.is_empty() {
                "no framework recognised".to_string()
            } else {
                frameworks.join(", ")
            },
        );
        out.push_str(&page.join("\n"));
        if offset + page.len() < matched.len() {
            out.push_str(&format!(
                "\n\nMore: call again with offset {}.",
                offset + page.len()
            ));
        }
        if !scan.warnings.is_empty() {
            out.push_str(&format!(
                "\n\n{} scan warning(s), e.g.: {}",
                scan.warnings.len(),
                scan.warnings
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" | ")
            ));
        }
        CallToolResult::success(vec![ContentBlock::text(out)])
    }

    #[tool(
        description = "Everything known about one endpoint: path, query and header \
                       parameters, the request body's schema and an example, auth, the \
                       handler's file and line, and `flow_node`, a request node ready to put \
                       in a flow.",
        annotations(read_only_hint = true)
    )]
    async fn describe_endpoint(&self, Parameters(p): Parameters<Endpoint>) -> CallToolResult {
        let app = self.app.lock().await;
        let Some(scan) = app.last_scan() else {
            return fail("nothing scanned yet: call list_endpoints first");
        };
        let wanted = p.id.trim();
        let Some(spec) = scan.endpoints.iter().find(|e| {
            e.id.as_str().eq_ignore_ascii_case(wanted) || e.display().eq_ignore_ascii_case(wanted)
        }) else {
            return fail(format!(
                "no endpoint {wanted:?}; list_endpoints shows them as `METHOD /path`"
            ));
        };

        let mut described = serde_json::to_value(spec).unwrap_or(Value::Null);
        described["path"] = json!(spec.path.render(rl_model::ParamStyle::Braces));
        described["id"] = json!(spec.display());
        if let Some(object) = described.as_object_mut() {
            object.remove("metadata");
        }

        match app.request_for(spec.id.as_str(), None) {
            Ok(request) => {
                let mut request = serde_json::to_value(&request).unwrap_or(Value::Null);
                if let Some(object) = request.as_object_mut() {
                    object.remove("id"); // generated on save; nothing to copy
                }
                described["flow_node"] = json!({
                    "id": node_id_for(spec),
                    "type": "request",
                    "request": request,
                    "assert": [{"from": "status", "op": "less_than", "expected": "400"}],
                });
            }
            Err(e) => described["flow_node"] = json!(format!("not available: {}", e.message())),
        }
        respond(Ok(described))
    }

    #[tool(
        description = "The request exactly as it would go on the wire, variables resolved and \
                       secrets masked. Sends nothing.",
        annotations(read_only_hint = true)
    )]
    async fn prepare_request(&self, Parameters(p): Parameters<PrepareRequest>) -> CallToolResult {
        let app = self.app.lock().await;
        respond(
            app.prepare_for_agent(&p.request, p.environment.as_deref())
                .map(|prepared| serde_json::to_value(prepared).unwrap_or(Value::Null)),
        )
    }

    #[tool(
        description = "Send one request and return the real response: status, headers, and \
                       the body (JSON pretty-printed, cut at max_body_bytes). Use it to see \
                       what an endpoint returns before capturing from it in a flow. Loopback \
                       hosts only, unless the workspace allows more.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            open_world_hint = true
        )
    )]
    async fn send_request(&self, Parameters(p): Parameters<SendRequest>) -> CallToolResult {
        let app = self.app.lock().await;
        let limit = p.max_body_bytes.unwrap_or(DEFAULT_BODY).min(MAX_BODY);
        match app
            .send_for_agent(&p.request, p.environment.as_deref(), limit)
            .await
        {
            Ok(sent @ AgentSend::Sent { .. }) => respond(Ok(json!(sent))),
            // Not sent: the agent should notice, so these are tool errors — with the
            // structured reason, which says what would allow it.
            Ok(other) => CallToolResult::error(vec![ContentBlock::text(pretty(&json!(other)))]),
            Err(e) => fail(e),
        }
    }

    #[tool(
        description = "The flows saved in this workspace, with how many steps each has.",
        annotations(read_only_hint = true)
    )]
    async fn list_flows(&self) -> CallToolResult {
        let app = self.app.lock().await;
        respond((|| {
            let mut flows = Vec::new();
            for name in app.flow_names()? {
                let flow = app.load_flow(&name)?;
                flows.push(json!({
                    "name": flow.name,
                    "description": flow.description,
                    "steps": flow.nodes.len(),
                }));
            }
            Ok(json!(flows))
        })())
    }

    #[tool(
        description = "A saved flow, whole: its nodes (steps, with what each extracts and \
                       asserts) and edges. Edit it and save_flow with overwrite: true.",
        annotations(read_only_hint = true)
    )]
    async fn get_flow(&self, Parameters(p): Parameters<FlowName>) -> CallToolResult {
        let app = self.app.lock().await;
        respond(
            app.load_flow(&p.name)
                .map(|flow| serde_json::to_value(flow).unwrap_or(Value::Null)),
        )
    }

    #[tool(
        description = "Save a flow to the workspace, where the developer sees it on the \
                       canvas. Refused if the graph cannot run (a cycle, an edge to a missing \
                       node). Returns warnings for what would fail when it runs: a variable \
                       nothing defines, a capture used before the step that makes it, a \
                       request with no assertions.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn save_flow(&self, Parameters(p): Parameters<SaveFlow>) -> CallToolResult {
        let app = self.app.lock().await;
        let mut flow = p.flow;
        flow.name = flow.name.trim().to_string();
        if flow.name.is_empty() {
            return fail("the flow needs a name; it becomes the file name");
        }
        if let Err(e) = flow.validate() {
            return fail(format!("the flow cannot run as written: {e}"));
        }
        let existing = match app.flow_names() {
            Ok(names) => names.contains(&flow.name),
            Err(e) => return fail(e),
        };
        if existing && !p.overwrite {
            return fail(format!(
                "a flow named {:?} already exists; read it with get_flow, then save_flow with \
                 overwrite: true, or choose another name",
                flow.name
            ));
        }
        if existing {
            // Cards the agent kept keep their places, even if it left positions out.
            if let Ok(before) = app.load_flow(&flow.name) {
                for node in flow.nodes.iter_mut().filter(|n| n.position.is_none()) {
                    node.position = before.node(&node.id).and_then(|n| n.position);
                }
            }
        }

        respond((|| {
            let warnings = app.lint_flow(&flow, p.environment.as_deref())?;
            app.save_flow(&flow)?;
            Ok(json!({
                "saved": flow.name,
                "path": app.flow_path(&flow.name)?,
                "steps": flow.nodes.len(),
                "edges": flow.edges.len(),
                "warnings": warnings,
                "next": if warnings.is_empty() {
                    "run_flow to try it; it is in RouteLogic's Flows panel"
                } else {
                    "fix the warnings, or run_flow to see them fail"
                },
            }))
        })())
    }

    #[tool(
        description = "Run a flow: a saved one by name, or one passed in without saving. \
                       Returns each step's outcome, the assertions with what was actually \
                       found, captured variables, and the response of any step that failed. \
                       Requests go to loopback hosts only, unless the workspace allows more.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            open_world_hint = true
        )
    )]
    async fn run_flow(&self, Parameters(p): Parameters<RunFlow>) -> CallToolResult {
        let app = self.app.lock().await;
        let flow = match (p.name, p.flow) {
            (Some(name), None) => match app.load_flow(&name) {
                Ok(flow) => flow,
                Err(e) => return fail(e),
            },
            (None, Some(flow)) => flow,
            (Some(_), Some(_)) => return fail("give either `name` or `flow`, not both"),
            (None, None) => return fail("give `name` (a saved flow) or `flow` (one to try)"),
        };
        let options = rl_flow::RunOptions {
            only: p
                .only
                .map(|ids| ids.into_iter().map(NodeId::from_raw).collect()),
            seed: Default::default(),
        };
        let limit = p.max_body_bytes.unwrap_or(RUN_BODY).min(MAX_BODY);
        let warnings = app
            .lint_flow(&flow, p.environment.as_deref())
            .unwrap_or_default();
        match app
            .run_flow_for_agent(&flow, p.environment.as_deref(), options, limit)
            .await
        {
            Ok(run) => {
                let mut value = json!(run);
                if !warnings.is_empty() {
                    value["warnings"] = json!(warnings);
                }
                respond(Ok(value))
            }
            Err(e) => fail(e),
        }
    }
}

#[tool_handler]
impl ServerHandler for RouteLogicServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("routelogic", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }
}

// --- helpers -----------------------------------------------------------------------------

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn respond(result: Result<Value, CoreError>) -> CallToolResult {
    match result {
        Ok(value) => CallToolResult::success(vec![ContentBlock::text(pretty(&value))]),
        Err(e) => fail(e),
    }
}

/// A failure the agent should read — a tool error, which the client shows it, rather than
/// a protocol error, which it would not.
fn fail(error: impl Into<Failure>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(error.into().0)])
}

struct Failure(String);

impl From<CoreError> for Failure {
    fn from(error: CoreError) -> Self {
        Failure(error.message())
    }
}

impl From<&str> for Failure {
    fn from(text: &str) -> Self {
        Failure(text.to_string())
    }
}

impl From<String> for Failure {
    fn from(text: String) -> Self {
        Failure(text)
    }
}

/// `GET     /users/{id} · users · auth — app/users.py:14`
fn line(spec: &rl_model::EndpointSpec) -> String {
    let mut out = format!(
        "{:<7} {}",
        spec.method.as_str(),
        spec.path.render(rl_model::ParamStyle::Braces)
    );
    if let Some(group) = &spec.group {
        out.push_str(&format!(" · {group}"));
    }
    if spec.auth.is_some() {
        out.push_str(" · auth");
    }
    if spec.body.is_some() {
        out.push_str(" · body");
    }
    if spec.orphaned {
        out.push_str(" · never mounted, may be unreachable");
    }
    if !spec.path.is_resolved() {
        out.push_str(&format!(
            " · path not resolvable statically ({})",
            spec.path.unresolved_exprs().join(", ")
        ));
    }
    if let Some(source) = &spec.source {
        out.push_str(&format!(
            " — {}:{}",
            source.file.display().to_string().replace('\\', "/"),
            source.line
        ));
    }
    out
}

/// A readable node id for an endpoint: `get_users_user_id`.
fn node_id_for(spec: &rl_model::EndpointSpec) -> String {
    let mut id = spec.method.as_str().to_ascii_lowercase();
    for segment in spec.path.render(rl_model::ParamStyle::Braces).split('/') {
        let cleaned: String = segment
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .collect();
        let cleaned = cleaned.trim_matches('_');
        if !cleaned.is_empty() {
            id.push('_');
            id.push_str(cleaned);
        }
    }
    id
}

// --- command line ------------------------------------------------------------------------

const USAGE: &str = "\
usage: routelogic mcp [--workspace <project directory>]

Serves RouteLogic to an agent over MCP on stdin and stdout. The workspace defaults to the
current directory. Add it to Claude Code with:

  claude mcp add routelogic -- <path to routelogic> mcp --workspace <project>";

/// The `mcp` command line: parse, open the workspace, serve until the client leaves.
/// Returns the process exit code. Nothing but protocol ever goes to stdout.
pub fn run_cli(args: impl IntoIterator<Item = String>) -> i32 {
    let mut args = args.into_iter();
    let mut workspace: Option<PathBuf> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                eprintln!("{USAGE}");
                return 0;
            }
            "--workspace" => match args.next() {
                Some(value) => workspace = Some(PathBuf::from(value)),
                None => {
                    eprintln!("routelogic mcp: --workspace needs a directory\n\n{USAGE}");
                    return 2;
                }
            },
            other => match other.strip_prefix("--workspace=") {
                Some(value) => workspace = Some(PathBuf::from(value)),
                None => {
                    eprintln!("routelogic mcp: unexpected argument {other:?}\n\n{USAGE}");
                    return 2;
                }
            },
        }
    }

    let root = match workspace {
        Some(path) => path,
        None => match std::env::current_dir() {
            Ok(dir) => dir,
            Err(e) => {
                eprintln!("routelogic mcp: no current directory: {e}");
                return 1;
            }
        },
    };
    if !root.is_dir() {
        eprintln!("routelogic mcp: {} is not a directory", root.display());
        return 1;
    }

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("routelogic mcp: {e}");
            return 1;
        }
    };
    let result = runtime.block_on(async {
        let server = RouteLogicServer::open(&root).map_err(|e| e.message())?;
        server.serve_stdio().await
    });
    match result {
        Ok(()) => 0,
        Err(message) => {
            eprintln!("routelogic mcp: {message}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rl_model::{HttpMethod, PathTemplate};

    #[test]
    fn node_ids_read_as_words() {
        let spec = rl_model::EndpointSpec::new(
            HttpMethod::Get,
            PathTemplate::parse("/api/v1/users/{user_id}", rl_model::ParamStyle::Braces),
            rl_model::Origin::Manual,
        );
        assert_eq!(node_id_for(&spec), "get_api_v1_users_user_id");
    }

    #[test]
    fn the_command_line_refuses_what_it_does_not_understand() {
        assert_eq!(run_cli(["--help".to_string()]), 0);
        assert_eq!(run_cli(["--workspace".to_string()]), 2);
        assert_eq!(run_cli(["--frobnicate".to_string()]), 2);
        assert_eq!(
            run_cli(["--workspace=/definitely/not/a/dir/anywhere".to_string()]),
            1
        );
    }
}
