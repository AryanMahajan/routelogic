//! The desktop shell.
//!
//! Command wrappers, state, and filesystem scope. **No logic** — every command here is a thin
//! call into [`rl_core::RouteLogic`].
//!
//! That rule is what keeps discovery and the request engine testable with a plain
//! `cargo test`, with no GUI harness in the loop. If a command in this file starts making
//! decisions, the decision belongs in `rl-core` instead.

use rl_core::{EnrichProposal, ProjectScan, RouteLogic, SaveAllReport, WorkspaceInfo};
use rl_flow::{FlowEvent, FlowRun, RunOptions};
use rl_http::{Exchange, PreparedRequest};
use rl_model::{Flow, RequestDraft};
use rl_workspace::{Collection, Environment, HistoryEntry, WorkspaceKind};
use serde::Serialize;
use std::path::PathBuf;
use tauri::ipc::Channel;
use tauri::State;
use tokio::sync::Mutex;

/// Application state.
///
/// A `tokio` mutex rather than a `std` one because [`RouteLogic::send`] is async and the guard
/// is held across an await point.
struct AppState {
    app: Mutex<RouteLogic>,
}

/// An error on its way to the UI.
///
/// Carries the flattened source chain, because "request failed" on its own is not actionable
/// and "request failed: connection refused" is.
#[derive(Debug, Serialize)]
struct CommandError {
    message: String,
}

impl From<rl_core::CoreError> for CommandError {
    fn from(error: rl_core::CoreError) -> Self {
        CommandError {
            message: error.message(),
        }
    }
}

type CommandResult<T> = std::result::Result<T, CommandError>;

// --- workspace ---------------------------------------------------------------------------

#[tauri::command]
async fn open_workspace(state: State<'_, AppState>, path: PathBuf) -> CommandResult<WorkspaceInfo> {
    Ok(state.app.lock().await.open_workspace(path)?)
}

#[tauri::command]
async fn create_workspace(
    state: State<'_, AppState>,
    path: PathBuf,
    name: String,
    standalone: bool,
) -> CommandResult<WorkspaceInfo> {
    let kind = if standalone {
        WorkspaceKind::Standalone
    } else {
        WorkspaceKind::Project
    };
    Ok(state.app.lock().await.create_workspace(path, name, kind)?)
}

#[tauri::command]
async fn open_or_create_workspace(
    state: State<'_, AppState>,
    path: PathBuf,
    name: String,
) -> CommandResult<WorkspaceInfo> {
    Ok(state
        .app
        .lock()
        .await
        .open_or_create_workspace(path, name, WorkspaceKind::Project)?)
}

#[tauri::command]
async fn workspace_info(state: State<'_, AppState>) -> CommandResult<WorkspaceInfo> {
    Ok(state.app.lock().await.info()?)
}

#[tauri::command]
async fn close_workspace(state: State<'_, AppState>) -> CommandResult<()> {
    state.app.lock().await.close_workspace();
    Ok(())
}

// --- environments ------------------------------------------------------------------------

#[tauri::command]
async fn set_environment(
    state: State<'_, AppState>,
    name: Option<String>,
) -> CommandResult<WorkspaceInfo> {
    let mut app = state.app.lock().await;
    app.set_active_environment(name.as_deref())?;
    Ok(app.info()?)
}

#[tauri::command]
async fn delete_environment(
    state: State<'_, AppState>,
    name: String,
) -> CommandResult<WorkspaceInfo> {
    let mut app = state.app.lock().await;
    app.delete_environment(&name)?;
    Ok(app.info()?)
}

#[tauri::command]
async fn variable_names(state: State<'_, AppState>) -> CommandResult<Vec<String>> {
    Ok(state.app.lock().await.variable_names()?)
}

#[tauri::command]
async fn load_environment(state: State<'_, AppState>, name: String) -> CommandResult<Environment> {
    Ok(state.app.lock().await.load_environment(&name)?)
}

#[tauri::command]
async fn save_environment(
    state: State<'_, AppState>,
    environment: Environment,
) -> CommandResult<()> {
    Ok(state.app.lock().await.save_environment(&environment)?)
}

// --- secrets -----------------------------------------------------------------------------

/// Names only. Values are never sent to the UI — they are resolved in the engine, moments
/// before the request goes out.
#[tauri::command]
async fn secret_names(state: State<'_, AppState>) -> CommandResult<Vec<String>> {
    Ok(state.app.lock().await.secret_names()?)
}

#[tauri::command]
async fn set_secret(state: State<'_, AppState>, name: String, value: String) -> CommandResult<()> {
    Ok(state.app.lock().await.set_secret(&name, &value)?)
}

#[tauri::command]
async fn delete_secret(state: State<'_, AppState>, name: String) -> CommandResult<()> {
    Ok(state.app.lock().await.delete_secret(&name)?)
}

// --- collections -------------------------------------------------------------------------

#[tauri::command]
async fn load_collection(state: State<'_, AppState>, name: String) -> CommandResult<Collection> {
    Ok(state.app.lock().await.load_collection(&name)?)
}

#[tauri::command]
async fn save_collection(state: State<'_, AppState>, collection: Collection) -> CommandResult<()> {
    Ok(state.app.lock().await.save_collection(&collection)?)
}

#[tauri::command]
async fn delete_collection(state: State<'_, AppState>, name: String) -> CommandResult<()> {
    Ok(state.app.lock().await.delete_collection(&name)?)
}

#[tauri::command]
async fn rename_collection(
    state: State<'_, AppState>,
    from: String,
    to: String,
) -> CommandResult<()> {
    Ok(state.app.lock().await.rename_collection(&from, &to)?)
}

#[tauri::command]
async fn save_request(
    state: State<'_, AppState>,
    collection: String,
    request: RequestDraft,
) -> CommandResult<()> {
    Ok(state.app.lock().await.save_request(&collection, request)?)
}

// --- flows -------------------------------------------------------------------------------

#[tauri::command]
async fn load_flow(state: State<'_, AppState>, name: String) -> CommandResult<Flow> {
    Ok(state.app.lock().await.load_flow(&name)?)
}

#[tauri::command]
async fn save_flow(state: State<'_, AppState>, flow: Flow) -> CommandResult<()> {
    Ok(state.app.lock().await.save_flow(&flow)?)
}

#[tauri::command]
async fn delete_flow(state: State<'_, AppState>, name: String) -> CommandResult<()> {
    Ok(state.app.lock().await.delete_flow(&name)?)
}

#[tauri::command]
async fn rename_flow(state: State<'_, AppState>, from: String, to: String) -> CommandResult<()> {
    Ok(state.app.lock().await.rename_flow(&from, &to)?)
}

/// Run a flow, streaming each step over `on_event` as it happens.
///
/// The application lock is held only long enough to capture the environment; the run
/// itself proceeds without it, so the rest of the UI keeps working while a flow is in
/// progress.
#[tauri::command]
async fn run_flow(
    state: State<'_, AppState>,
    flow: Flow,
    options: Option<RunOptions>,
    on_event: Channel<FlowEvent>,
) -> CommandResult<FlowRun> {
    let prepared = state
        .app
        .lock()
        .await
        .prepare_flow(flow, options.unwrap_or_default())?;
    let mut forward = move |event: FlowEvent| {
        // A closed channel means the window went away; the run still completes and returns.
        let _ = on_event.send(event);
    };
    Ok(prepared.run(&mut forward).await?)
}

// --- discovery ---------------------------------------------------------------------------

/// Scan the open project for endpoints.
///
/// Static analysis only. Nothing in the project is executed — see `docs/security.md`.
#[tauri::command]
async fn scan_project(state: State<'_, AppState>) -> CommandResult<ProjectScan> {
    Ok(state.app.lock().await.scan()?)
}

/// Every resolved endpoint of the last scan, as one collection filed by group.
#[tauri::command]
async fn save_scan_as_collection(
    state: State<'_, AppState>,
    name: String,
) -> CommandResult<SaveAllReport> {
    Ok(state.app.lock().await.save_scan_as_collection(&name)?)
}

#[tauri::command]
async fn open_endpoint(
    state: State<'_, AppState>,
    id: String,
    base_url: Option<String>,
) -> CommandResult<RequestDraft> {
    Ok(state
        .app
        .lock()
        .await
        .request_for(&id, base_url.as_deref())?)
}

// --- runtime enrich ------------------------------------------------------------------------
//
// The only commands that can execute project code. `enrich_proposal` and `enrich_command`
// run nothing; `run_enrich` runs exactly what the UI showed. See `docs/security.md`.

#[tauri::command]
async fn enrich_proposal(state: State<'_, AppState>) -> CommandResult<EnrichProposal> {
    Ok(state.app.lock().await.enrich_proposal()?)
}

#[tauri::command]
async fn enrich_command(
    state: State<'_, AppState>,
    target: String,
    interpreter: Option<PathBuf>,
) -> CommandResult<String> {
    Ok(state
        .app
        .lock()
        .await
        .enrich_command(&target, interpreter.as_deref())?)
}

/// Executes the project's code. The UI shows the exact command and asks first.
#[tauri::command]
async fn run_enrich(
    state: State<'_, AppState>,
    target: String,
    interpreter: Option<PathBuf>,
) -> CommandResult<ProjectScan> {
    Ok(state
        .app
        .lock()
        .await
        .run_enrich(&target, interpreter.as_deref())?)
}

#[tauri::command]
async fn revoke_enrich(state: State<'_, AppState>) -> CommandResult<()> {
    Ok(state.app.lock().await.revoke_enrich()?)
}

/// Jump to where an endpoint is defined.
#[tauri::command]
async fn reveal_in_editor(
    state: State<'_, AppState>,
    file: PathBuf,
    line: u32,
) -> CommandResult<()> {
    Ok(state.app.lock().await.reveal_in_editor(&file, line)?)
}

// --- sending -----------------------------------------------------------------------------

#[tauri::command]
async fn send_request(
    state: State<'_, AppState>,
    request: RequestDraft,
) -> CommandResult<Exchange> {
    Ok(state.app.lock().await.send(&request).await?)
}

#[tauri::command]
async fn prepare_request(
    state: State<'_, AppState>,
    request: RequestDraft,
) -> CommandResult<PreparedRequest> {
    Ok(state.app.lock().await.prepare(&request)?)
}

// --- history -----------------------------------------------------------------------------

#[tauri::command]
async fn history(state: State<'_, AppState>, limit: usize) -> CommandResult<Vec<HistoryEntry>> {
    Ok(state.app.lock().await.history(limit)?)
}

#[tauri::command]
async fn clear_history(state: State<'_, AppState>) -> CommandResult<()> {
    Ok(state.app.lock().await.clear_history()?)
}

// --- import ------------------------------------------------------------------------------

/// An import, with anything that could not be honoured.
#[derive(Debug, Serialize)]
struct ImportResult<T> {
    value: T,
    warnings: Vec<String>,
}

#[tauri::command]
async fn import_curl(
    state: State<'_, AppState>,
    text: String,
) -> CommandResult<ImportResult<RequestDraft>> {
    let imported = state.app.lock().await.import_curl(&text)?;
    Ok(ImportResult {
        value: imported.value,
        warnings: imported.warnings,
    })
}

#[tauri::command]
async fn import_raw_http(
    state: State<'_, AppState>,
    text: String,
) -> CommandResult<ImportResult<RequestDraft>> {
    let imported = state.app.lock().await.import_raw_http(&text)?;
    Ok(ImportResult {
        value: imported.value,
        warnings: imported.warnings,
    })
}

/// Summary of an imported specification. The endpoints themselves land in a collection.
#[derive(Debug, Serialize)]
struct OpenApiSummary {
    title: String,
    version: String,
    servers: Vec<String>,
    endpoint_count: usize,
}

#[tauri::command]
async fn import_openapi(
    state: State<'_, AppState>,
    text: String,
    collection: String,
) -> CommandResult<ImportResult<OpenApiSummary>> {
    let imported = state
        .app
        .lock()
        .await
        .import_openapi_as_collection(&text, &collection)?;

    Ok(ImportResult {
        value: OpenApiSummary {
            title: imported.value.title,
            version: imported.value.version,
            servers: imported.value.servers,
            endpoint_count: imported.value.endpoints.len(),
        },
        warnings: imported.warnings,
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            app: Mutex::new(RouteLogic::new()),
        })
        .invoke_handler(tauri::generate_handler![
            open_workspace,
            create_workspace,
            open_or_create_workspace,
            workspace_info,
            close_workspace,
            set_environment,
            load_environment,
            save_environment,
            secret_names,
            set_secret,
            delete_secret,
            delete_environment,
            variable_names,
            load_collection,
            save_collection,
            delete_collection,
            rename_collection,
            save_request,
            load_flow,
            save_flow,
            delete_flow,
            rename_flow,
            run_flow,
            scan_project,
            save_scan_as_collection,
            open_endpoint,
            enrich_proposal,
            enrich_command,
            run_enrich,
            revoke_enrich,
            reveal_in_editor,
            send_request,
            prepare_request,
            history,
            clear_history,
            import_curl,
            import_raw_http,
            import_openapi,
        ])
        .run(tauri::generate_context!())
        .expect("error while running RouteLogic");
}
