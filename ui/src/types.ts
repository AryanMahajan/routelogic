/**
 * The wire format between the Rust core and this UI.
 *
 * These mirror the serde representation of `rl-model` and `rl-http` types. Where a Rust enum
 * is tagged, the tag name here matches the `#[serde(tag = "...")]` attribute exactly — get
 * that wrong and the value silently fails to deserialize on the way back.
 */

export type HttpMethod =
  | "GET"
  | "HEAD"
  | "POST"
  | "PUT"
  | "PATCH"
  | "DELETE"
  | "OPTIONS"
  | "TRACE"
  | string; // Unknown methods round-trip as themselves.

export interface KeyValue {
  key: string;
  value: string;
  enabled: boolean;
  description?: string | null;
}

export type ApiKeyLocation = "header" | "query" | "cookie";

export type AuthConfig =
  | { type: "none" }
  | { type: "inherit" }
  | { type: "bearer"; token: string }
  | { type: "basic"; username: string; password: string }
  | { type: "api_key"; key: string; value: string; location: ApiKeyLocation };

export type BodyValue =
  | { type: "none" }
  | { type: "json"; content: string }
  | { type: "text"; content: string; content_type: string }
  | { type: "form"; fields: KeyValue[] }
  | { type: "multipart"; parts: unknown[] }
  | { type: "binary"; path: string; content_type?: string | null };

export interface RequestSettings {
  follow_redirects: boolean;
  max_redirects: number;
  /** Session-only. Never serialized by the core, so it is never persisted. */
  accept_invalid_certs: boolean;
  timeout_ms: number;
}

export interface RequestDraft {
  id: string;
  name?: string | null;
  /** Folder path inside its collection, `Users/Admin`. Null at the top level. */
  folder?: string | null;
  spec_ref?: string | null;
  method: HttpMethod;
  url: string;
  path_values: Record<string, string>;
  query: KeyValue[];
  headers: KeyValue[];
  cookies: KeyValue[];
  auth: AuthConfig;
  body: BodyValue;
  settings: RequestSettings;
}

/**
 * A request as it actually crosses the wire.
 *
 * The core serializes with `skip_serializing_if` so that saved YAML stays tidy — which
 * means every empty collection and every default is simply *absent*. This type says so,
 * and `normalizeRequest` in `api.ts` turns it into a full `RequestDraft` before any
 * component sees it. Components never receive a wire request directly.
 */
export interface WireRequestDraft {
  id: string;
  name?: string | null;
  folder?: string | null;
  spec_ref?: string | null;
  method: HttpMethod;
  url: string;
  path_values?: Record<string, string>;
  query?: WireKeyValue[];
  headers?: WireKeyValue[];
  cookies?: WireKeyValue[];
  auth?: AuthConfig;
  body?: BodyValue;
  settings?: Partial<RequestSettings>;
}

export interface WireKeyValue {
  key: string;
  value?: string;
  enabled?: boolean;
  description?: string | null;
}

export interface WireCollection {
  version: number;
  name: string;
  description?: string | null;
  requests?: WireRequestDraft[];
}

/** Fill in everything the core left out. Mirrors the serde defaults on the Rust side. */
export function normalizeRequest(wire: WireRequestDraft): RequestDraft {
  const kv = (rows: WireKeyValue[] | undefined): KeyValue[] =>
    (rows ?? []).map((row) => ({
      key: row.key,
      value: row.value ?? "",
      enabled: row.enabled ?? true,
      description: row.description ?? null,
    }));

  return {
    id: wire.id,
    name: wire.name ?? null,
    folder: wire.folder ?? null,
    spec_ref: wire.spec_ref ?? null,
    method: wire.method,
    url: wire.url,
    path_values: wire.path_values ?? {},
    query: kv(wire.query),
    headers: kv(wire.headers),
    cookies: kv(wire.cookies),
    auth: wire.auth ?? { type: "none" },
    body: tidyBody(wire.body ?? { type: "none" }),
    settings: {
      follow_redirects: wire.settings?.follow_redirects ?? false,
      max_redirects: wire.settings?.max_redirects ?? 10,
      accept_invalid_certs: false,
      timeout_ms: wire.settings?.timeout_ms ?? 30000,
    },
  };
}

/**
 * A JSON body arrives formatted, so a saved or imported one-liner reads as a document
 * the moment it opens. Done here, before the tab takes its "saved" snapshot, so it does
 * not count as an edit. Anything that does not parse is left as it is.
 */
function tidyBody(body: BodyValue): BodyValue {
  if (body.type !== "json" || !body.content.trim()) return body;
  try {
    const pretty = JSON.stringify(JSON.parse(body.content), null, 2);
    return pretty === body.content ? body : { ...body, content: pretty };
  } catch {
    return body;
  }
}

export interface Timing {
  ttfb_ms: number;
  total_ms: number;
}

export interface Hop {
  status: number;
  from: string;
  to: string;
  credentials_stripped: boolean;
}

export interface ResponseBody {
  bytes: string;
  truncated: boolean;
  reported_length?: number | null;
  content_type?: string | null;
  content_encoding?: string | null;
}

export interface HttpResponse {
  status: number;
  status_text: string;
  headers: [string, string][];
  body: ResponseBody;
  timing: Timing;
  redirects: Hop[];
  insecure: boolean;
}

export interface SentRequest {
  method: string;
  url: string;
  headers: [string, string][];
  body_size: number;
  body_preview?: string | null;
}

export interface Exchange {
  request: SentRequest;
  response: HttpResponse;
}

/** A document another process changed on disk — an agent, git, an editor. */
/** How an agent's client starts RouteLogic's MCP server for the open workspace. */
export interface AgentConnection {
  server_name: string;
  command: string;
  args: string[];
  /** One line for Claude Code. */
  claude_code: string;
  /** The `mcpServers` entry most other clients read from a JSON file. */
  json: string;
  /** `workspace.yaml`, where the allow list lives. */
  manifest: string;
  policy: {
    /** Always allowed, with every method. */
    loopback: string[];
    /** `methods` empty means every method. */
    allow: { host: string; methods: string[] }[];
    how_to_change: string;
  };
}

export interface WorkspaceChange {
  kind: "flow" | "collection" | "environment" | "workspace";
  /** The document's name; empty for the workspace manifest. */
  name: string;
  removed: boolean;
}

export interface WorkspaceInfo {
  name: string;
  root: string;
  kind: "project" | "standalone";
  collections: string[];
  environments: string[];
  flows: string[];
  active_environment?: string | null;
  /** Secret names the active environment expects but the store does not hold. */
  missing_secrets: string[];
}

export interface Collection {
  version: number;
  name: string;
  description?: string | null;
  requests: RequestDraft[];
}

export interface Environment {
  version: number;
  name: string;
  variables: Record<string, string>;
  secrets: string[];
}

export interface HistoryEntry {
  id: number;
  at: number;
  method: string;
  url: string;
  status?: number | null;
  duration_ms?: number | null;
  error?: string | null;
  request: unknown;
  response: unknown;
  /** `agent` for a request an agent sent over MCP; absent for the user's own. */
  source?: string | null;
}

/*
 * Rule for everything above: any `Vec` the core marks `skip_serializing_if = "is_empty"`
 * arrives as *absent*, not `[]`. `api.ts` fills those in (`normalizeRequest`, `send`,
 * `loadEnvironment`, `loadCollection`) so components can rely on the plain types. When
 * adding a wire type, check the Rust struct's serde attributes before reading `.length`.
 */

/** Every import carries what it could not honour, so nothing is silently dropped. */
export interface ImportResult<T> {
  value: T;
  warnings: string[];
}

export interface OpenApiSummary {
  title: string;
  version: string;
  servers: string[];
  endpoint_count: number;
}

/** Create a blank request. Mirrors `RequestDraft::new` on the Rust side. */
export function emptyRequest(): RequestDraft {
  return {
    id: crypto.randomUUID(),
    name: null,
    spec_ref: null,
    method: "GET",
    url: "",
    path_values: {},
    query: [],
    headers: [],
    cookies: [],
    auth: { type: "none" },
    body: { type: "none" },
    settings: {
      follow_redirects: false,
      max_redirects: 10,
      accept_invalid_certs: false,
      timeout_ms: 30000,
    },
  };
}

// --- discovery ---------------------------------------------------------------------------

export interface SourceView {
  file: string;
  line: number;
}

/** One discovered endpoint, as the core flattens it for this UI. */
export interface EndpointSpec {
  id: string;
  method: HttpMethod;
  /** Rendered in {brace} form. */
  path: string;
  display: string;
  group?: string | null;
  summary?: string | null;
  source?: SourceView | null;
  /** The router this belongs to is never mounted. */
  orphaned: boolean;
  /** At least one path segment could not be determined statically. */
  unresolved: boolean;
  /** The source expressions that defeated resolution. */
  unresolved_exprs: string[];
  auth: boolean;
  has_body: boolean;
  query: string[];
  /** After runtime enrich: how this endpoint fared in the merge. Absent until then. */
  enrich?: EnrichProvenance | null;
}

export type EnrichProvenance = "matched" | "runtime_only" | "static_only" | "gap_filled";

export interface DetectedFramework {
  id: string;
  score: number;
  evidence: string[];
}

export interface BaseUrlCandidate {
  url: string;
  source: string;
  confidence: number;
}

export interface ScanStats {
  files_seen: number;
  files_parsed: number;
  routers_found: number;
  endpoints_found: number;
  unresolved: number;
}

export interface ScanResult {
  frameworks: DetectedFramework[];
  endpoints: EndpointSpec[];
  base_urls: BaseUrlCandidate[];
  warnings: string[];
  stats: ScanStats;
  /** Whether runtime enrich can be offered — only for frameworks with a runtime spec. */
  enrichable: boolean;
  /** Present once runtime enrich has run on this scan. */
  enrich: EnrichReport | null;
}

/** What "Save all as collection" did. */
export interface SaveAllReport {
  added: number;
  updated: number;
  skipped_unresolved: number;
}

/** What one runtime-enrich run did. */
export interface EnrichReport {
  framework: string;
  target: string;
  interpreter: string;
  /** The exact command that ran. */
  command: string;
  matched: number;
  runtime_only: number;
  static_only: number;
  gaps_filled: number;
  duration_ms: number;
  /** What the application printed while importing. Shown, never parsed. */
  stderr: string;
  warnings: string[];
}

export interface AppTarget {
  /** `app.main:app`, or `app:create_app()` for a factory. */
  target: string;
  cwd: string;
  source: string;
  confidence: number;
}

export interface Interpreter {
  path: string;
  source: string;
}

/** The consent dialog's content. Nothing has run when this arrives. */
export interface EnrichProposal {
  targets: AppTarget[];
  interpreters: Interpreter[];
  remembered_target?: string | null;
  command?: string | null;
  helper_path: string;
}
