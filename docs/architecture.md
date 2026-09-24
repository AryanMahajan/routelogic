# Architecture

## Shape

```
                        RouteLogic
                            │
              ┌─────────────┴─────────────┐
              │                           │
        Project Engine              API Workspace
     (detect · parse · resolve)   (collections · envs · history)
              │                           │
              └─────────────┬─────────────┘
                            ▼
                    Unified API Model
                            │
                            ▼
                       API Engine
                            │
            ┌───────────────┼───────────────┐
            ▼               ▼               ▼
         Request         Response        History
            │
            ▼
            UI
```

## Crates

A Cargo workspace, with the core split into libraries and Tauri as a shell.

```
routelogic/
├── crates/
│   ├── rl-model/         # unified model + variable resolver — no I/O
│   ├── rl-discovery/     # project detect, source index, registration graph, adapters
│   ├── rl-import/        # cURL, OpenAPI, raw HTTP importers
│   ├── rl-http/          # request execution engine
│   ├── rl-flow/          # flow runner: dependency order, extraction, assertions
│   ├── rl-workspace/     # workspace files, secrets, history, index cache
│   ├── rl-core/          # facade — the only surface the shells call
│   └── rl-mcp/           # MCP server: rl-core's API as tools for an agent
├── src-tauri/            # Tauri v2 shell; `routelogic mcp` runs rl-mcp instead
├── ui/                   # React + TypeScript + Vite
└── tests/fixtures/       # sample projects + expected-route snapshots
```

### Why split at all

Discovery is the highest-risk component and needs to be tested against dozens of fixture
projects. Testing it through a GUI harness would be slow and awkward, so it lives in a library
with no knowledge of Tauri, and its tests are ordinary `cargo test` runs.

The same boundary makes a CLI shell nearly free later, since `rl-core` already exposes
everything a shell needs. That is a side benefit, not the goal.

### Dependency direction

```
rl-model  ←──  rl-discovery
    ↑      ←──  rl-import
    │      ←──  rl-http  ←──  rl-flow
    │      ←──  rl-workspace
    │                │
    └──────  rl-core ┘
                │
           src-tauri  ──→  rl-mcp
                │
               ui
```

`rl-model` depends on nothing else in the workspace and performs no I/O. Everything else
depends on it. Nothing depends on `src-tauri`.

### Crate responsibilities

**`rl-model`** — `EndpointSpec`, `RequestDraft`, `PathTemplate`, auth and body types, and the
`{{var}}` resolver. Pure data and pure functions, which makes it exhaustively testable and
keeps the type definitions honest.

**`rl-discovery`** — the pipeline in [how discovery works](discovery/how-it-works.md).
Framework adapters live under `adapters/` and are deliberately thin; the registration graph in
`graph.rs` holds the shared logic.

**`rl-import`** — three importers, one output type. `openapi.rs` is reused by runtime enrich.

**`rl-http`** — request execution. Deliberately low-magic; see below.

**`rl-flow`** — runs a `Flow` (the model lives in `rl-model`): execution order from the
edges, skip semantics for failed and untaken branches, extraction of response values into
variables, assertions. It sends through a `Sender` trait — `rl-http` in the application, a
scripted fake in its tests — and streams `FlowEvent`s so the canvas lights up as it goes.
See [flows](flows.md).

**`rl-workspace`** — the three storage tiers, YAML serialization with stable ordering, secret
handling, SQLite for history and the source index.

**`rl-core`** — the facade. Owns application state, orchestrates the others, exposes one
coherent API. `src-tauri` contains no logic beyond command wrappers, event emission, and
filesystem scope handling. What an agent may send and see — the allow list, secret
masking — is decided here too (`agent.rs`), so the rules sit beside everything else and
are tested without a client.

**`rl-mcp`** — the second shell: [agents](agents.md) over the Model Context Protocol, on
stdio. Each tool is a thin call into `rl-core`'s `*_for_agent` methods plus a JSON answer
an agent can read. The desktop binary hands `routelogic mcp …` to it before any window
exists, so one executable is both the app and the server; `routelogic-mcp` is the same
server as a standalone binary for development. Its end-to-end test spawns that binary and
builds and runs a flow over stdio the way an agent would.

## The HTTP engine is deliberately low-magic

Built on `reqwest`, but configured against most of its conveniences. An API client's job is
to send *exactly* what the user described, including things a normal HTTP client would
helpfully correct:

- Redirects are **not** followed by default; when enabled, the chain is shown
- Raw response bytes are preserved alongside the decoded body — no silent decompression
- Header order is preserved, and duplicate or unusual headers are allowed through
- Certificate verification is toggleable per request, for local development
- Response bodies stream, with the in-UI preview capped and large payloads saved to file
- Timing is broken down into DNS, TCP, TLS, TTFB, and total via a custom connector

That last point is the fiddliest. If the custom connector proves troublesome early, TTFB and
total ship first and the breakdown is refined afterwards.

## Data flow

**Opening a project**

```
UI  →  rl-core.open_project(path)
       rl-workspace   load or create .routelogic/
       rl-discovery   detect → index → graph → resolve → extract → infer base URL
       rl-core        merge with any stored runtime results
    →  EndpointSpec[]  →  UI tree
```

Long scans stream progress over a Tauri channel, so the UI stays responsive and can show
partial results.

**Sending a request**

```
UI  →  RequestDraft
       rl-model      resolve {{variables}} (secrets last, latest possible)
       rl-http       execute
       rl-workspace  append to history, with secrets redacted
    →  Response  →  UI
```

## Frontend

React + TypeScript + Vite. Three panes: endpoint tree, request editor, response viewer.

- **CodeMirror 6** for body and response editing, not Monaco. Monaco is far too heavy for a
  tool whose first design principle is "fast", and CodeMirror handles large documents better.
- The endpoint tree is virtualized — projects with hundreds of routes must stay smooth.
- Rust types are the source of truth for the wire format; TypeScript definitions are generated
  from them rather than hand-maintained, so the two cannot drift.

## Testing strategy

| Layer | Approach |
|---|---|
| `rl-model` | Unit tests; variable resolution precedence and edge cases |
| `rl-discovery` | **Fixture snapshot tests** — the highest-value investment in the project |
| `rl-import` | Round-trip tests: parse → model → re-emit, over a real-world cURL corpus |
| `rl-http` | Tests against a local mock server |
| `rl-flow` | Runner tests through a fake `Sender`: chaining, skip paths, branches, undefined variables |
| `rl-workspace` | Round-trip YAML; a redaction test that must never regress |
| Performance | Scan budgets, cold and warm, asserted in CI |

Fixture snapshots matter most because discovery regressions are otherwise invisible — a route
that quietly stops being found produces no error, just a slightly shorter list that nobody
notices. Snapshots also record *expected misses*, documenting the boundary of each adapter.
