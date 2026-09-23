/**
 * The only place this UI talks to the Rust core.
 *
 * Every call goes through Tauri's `invoke`. Keeping them in one module means the command
 * names — which are strings, and so invisible to the type checker — are written once.
 */

import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { PreparedRequest } from "./codegen";
import {
  normalizeFlow,
  normalizeFlowRun,
  normalizeNodeResult,
  type Flow,
  type FlowEvent,
  type FlowRun,
  type WireFlow,
} from "./flowTypes";
import {
  normalizeRequest,
  type Collection,
  type EnrichProposal,
  type SaveAllReport,
  type ScanResult,
  type Environment,
  type Exchange,
  type HistoryEntry,
  type ImportResult,
  type OpenApiSummary,
  type RequestDraft,
  type WireCollection,
  type WireRequestDraft,
  type WorkspaceChange,
  type WorkspaceInfo,
} from "./types";

/** An error raised by the core, already carrying its flattened source chain. */
export class CoreError extends Error {}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    // Commands reject with `{ message }`; anything else is a genuine surprise.
    if (typeof error === "object" && error !== null && "message" in error) {
      throw new CoreError(String((error as { message: unknown }).message));
    }
    throw new CoreError(String(error));
  }
}

/** `enrichable` and `enrich` are skipped on the wire when false/absent. */
function normalizeScan(wire: ScanResult): ScanResult {
  return { ...wire, enrichable: wire.enrichable ?? false, enrich: wire.enrich ?? null };
}

/**
 * Call `handler` for every document another process changes in the open workspace. Returns
 * the unsubscribe function. Outside the desktop shell — the browser harness — there are no
 * events, and this quietly does nothing.
 */
export function onWorkspaceChanged(handler: (change: WorkspaceChange) => void): () => void {
  let unlisten: (() => void) | null = null;
  let cancelled = false;
  try {
    void listen<WorkspaceChange>("workspace-changed", (event) => handler(event.payload))
      .then((stop) => {
        if (cancelled) stop();
        else unlisten = stop;
      })
      .catch(() => {});
  } catch {
    // No event bridge: not running inside Tauri.
  }
  return () => {
    cancelled = true;
    unlisten?.();
  };
}

export const api = {
  // --- workspace ---
  openWorkspace: (path: string) => call<WorkspaceInfo>("open_workspace", { path }),
  openOrCreateWorkspace: (path: string, name: string) =>
    call<WorkspaceInfo>("open_or_create_workspace", { path, name }),
  createWorkspace: (path: string, name: string, standalone = false) =>
    call<WorkspaceInfo>("create_workspace", { path, name, standalone }),
  workspaceInfo: () => call<WorkspaceInfo>("workspace_info"),
  closeWorkspace: () => call<void>("close_workspace"),

  // --- environments ---
  setEnvironment: (name: string | null) => call<WorkspaceInfo>("set_environment", { name }),
  loadEnvironment: async (name: string): Promise<Environment> => {
    const wire = await call<Environment>("load_environment", { name });
    return { ...wire, secrets: wire.secrets ?? [] };
  },
  saveEnvironment: (environment: Environment) =>
    call<void>("save_environment", { environment }),
  deleteEnvironment: (name: string) => call<WorkspaceInfo>("delete_environment", { name }),
  /** Every `{{name}}` that would resolve right now; secrets appear as `secret:NAME`. */
  variableNames: () => call<string[]>("variable_names"),

  // --- secrets (names only ever cross this boundary) ---
  secretNames: () => call<string[]>("secret_names"),
  setSecret: (name: string, value: string) => call<void>("set_secret", { name, value }),
  deleteSecret: (name: string) => call<void>("delete_secret", { name }),

  // --- collections ---
  loadCollection: async (name: string): Promise<Collection> => {
    const wire = await call<WireCollection>("load_collection", { name });
    return { ...wire, requests: (wire.requests ?? []).map(normalizeRequest) };
  },
  saveRequest: (collection: string, request: RequestDraft) =>
    call<void>("save_request", { collection, request }),
  saveCollection: (collection: Collection) => call<void>("save_collection", { collection }),
  deleteCollection: (name: string) => call<void>("delete_collection", { name }),
  renameCollection: (from: string, to: string) =>
    call<void>("rename_collection", { from, to }),

  // --- flows ---
  loadFlow: async (name: string): Promise<Flow> =>
    normalizeFlow(await call<WireFlow>("load_flow", { name })),
  saveFlow: (flow: Flow) => call<void>("save_flow", { flow }),
  deleteFlow: (name: string) => call<void>("delete_flow", { name }),
  renameFlow: (from: string, to: string) => call<void>("rename_flow", { from, to }),
  /**
   * Run a flow. `onEvent` fires for every step as it happens; the promise resolves with the
   * whole run once the last node has finished. Rejects only for a flow that cannot run at
   * all — a cycle, a dangling edge — never for a node that failed.
   */
  runFlow: async (
    flow: Flow,
    onEvent: (event: FlowEvent) => void,
    options: { only?: string[] | null; seed?: Record<string, string> } = {},
  ): Promise<FlowRun> => {
    const channel = new Channel<FlowEvent>();
    channel.onmessage = (event) => {
      if (event.event === "node_finished") {
        onEvent({ event: "node_finished", result: normalizeNodeResult(event.result) });
      } else if (event.event === "finished") {
        onEvent({ event: "finished", run: normalizeFlowRun(event.run) });
      } else {
        onEvent(event);
      }
    };
    return normalizeFlowRun(
      await call<FlowRun>("run_flow", {
        flow,
        options: { only: options.only ?? null, seed: options.seed ?? {} },
        onEvent: channel,
      }),
    );
  },

  // --- discovery (reads source; never executes it) ---
  scanProject: async () => normalizeScan(await call<ScanResult>("scan_project")),
  saveScanAsCollection: (name: string) =>
    call<SaveAllReport>("save_scan_as_collection", { name }),

  // --- runtime enrich (the one path that executes project code — after consent) ---
  enrichProposal: () => call<EnrichProposal>("enrich_proposal"),
  enrichCommand: (target: string, interpreter: string | null) =>
    call<string>("enrich_command", { target, interpreter }),
  runEnrich: async (target: string, interpreter: string | null) =>
    normalizeScan(await call<ScanResult>("run_enrich", { target, interpreter })),
  revokeEnrich: () => call<void>("revoke_enrich"),
  openEndpoint: async (id: string, baseUrl?: string) =>
    normalizeRequest(
      await call<WireRequestDraft>("open_endpoint", { id, baseUrl: baseUrl ?? null }),
    ),
  revealInEditor: (file: string, line: number) =>
    call<void>("reveal_in_editor", { file, line }),

  // --- sending ---
  send: async (request: RequestDraft): Promise<Exchange> => {
    const wire = await call<Exchange>("send_request", { request });
    // `redirects` is omitted on the wire when empty — which is every direct response.
    return { ...wire, response: { ...wire.response, redirects: wire.response.redirects ?? [] } };
  },

  /** The request as it would be sent — resolved, auth applied — without sending it. */
  prepare: (request: RequestDraft) => call<PreparedRequest>("prepare_request", { request }),

  // --- history ---
  history: (limit = 50) => call<HistoryEntry[]>("history", { limit }),
  clearHistory: () => call<void>("clear_history"),

  // --- import ---
  importCurl: async (text: string): Promise<ImportResult<RequestDraft>> => {
    const result = await call<ImportResult<WireRequestDraft>>("import_curl", { text });
    return { ...result, value: normalizeRequest(result.value) };
  },
  importRawHttp: async (text: string): Promise<ImportResult<RequestDraft>> => {
    const result = await call<ImportResult<WireRequestDraft>>("import_raw_http", { text });
    return { ...result, value: normalizeRequest(result.value) };
  },
  importOpenApi: (text: string, collection: string) =>
    call<ImportResult<OpenApiSummary>>("import_openapi", { text, collection }),
};
