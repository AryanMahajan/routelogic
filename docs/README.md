# RouteLogic documentation

RouteLogic is a local-first API client that discovers HTTP endpoints from a project's source
code — FastAPI, Flask, Django, Express, Next.js and Go today — and lets you test them
immediately. These pages
cover how to run it, how discovery works, what each framework adapter handles and misses,
and how the workspace is stored.

> **Pre-alpha.** Where a document describes something not yet implemented, it says so.
> Anything in `docs/` is a promise the code is expected to keep — if code and docs disagree,
> that is a bug in one of them.

## Project status

Implementation is phased so each phase leaves a usable application.

| Phase | Contents | Status |
|---|---|---|
| P0 | Crate skeleton, unified model, storage tiers, variable resolver, docs | Done |
| P1 | HTTP engine, request editor, response viewer, history | Done |
| P2 | cURL / raw HTTP / OpenAPI importers | Done |
| P3 | Discovery core + FastAPI adapter + explorer | Done |
| P4 | Next.js adapter, then Express adapter | Done |
| P5 | Runtime enrich, Flask adapter | Done |
| P6 | Collections & environments UI, Django + DRF | Done (capture rules deferred) |
| — | Go: net/http, Gin, Echo, chi, Fiber, gorilla/mux | Done |
| — | Agents over MCP: an agent reads the API, tries requests, writes and runs flows | Done |

Live phase tracking, including what slipped and why, lives in `docs/internal/roadmap.md`
(not committed).

## Reading order

**If you want to use RouteLogic**

1. [Getting started](getting-started.md) — install, open a project, send a request
2. [Concepts](concepts.md) — the two core types and why there are two
3. [Import](import.md) — cURL, OpenAPI, raw HTTP
4. [Flows](flows.md) — multi-step API tests on a canvas; the
   [walkthrough](flows-walkthrough.md) builds one against the FastAPI fixture and traces the
   run, and [examples/](examples/) has it as a file
5. [Agents](agents.md) — connect Claude Code, Cursor or another MCP client and have it
   write flows from a description
6. [Workspace format](workspace/format.md) — what lands on disk
7. [Environments](workspace/environments.md) and [secrets](workspace/secrets.md)

**If you want to understand or contribute to it**

1. [Architecture](architecture.md) — crates, boundaries, data flow
2. [How discovery works](discovery/how-it-works.md) — the registration graph
3. [Framework support](discovery/frameworks.md) — what each adapter handles and misses
4. [Adding a framework](discovery/adding-a-framework.md) — the adapter contract
5. [Runtime enrich](discovery/runtime-enrich.md) — the opt-in high-fidelity path
6. [Security](security.md) — the trust boundaries

## Documentation tiers

- **`README.md`** (repo root) — what RouteLogic is, for someone who has never seen it.
- **`docs/`** — committed, public, kept in sync with the code.
- **`docs/internal/`** — gitignored working notes: decision records, parsing spikes, live
  roadmap. Messy on purpose; nothing there is a promise.
