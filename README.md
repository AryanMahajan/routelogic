# RouteLogic — API client that discovers endpoints from your source code

**Open a FastAPI, Flask, Django, Express, Next.js or Go project and see every API route it
serves — then test it. A local-first, open-source API client built in Rust, with codebase-aware route
discovery instead of hand-configured collections.**

[![CI](https://github.com/AryanMahajan/routelogic/actions/workflows/ci.yml/badge.svg)](https://github.com/AryanMahajan/routelogic/actions/workflows/ci.yml)
![Rust](https://img.shields.io/badge/core-Rust-dea584?logo=rust&logoColor=white)
![Tauri v2](https://img.shields.io/badge/desktop-Tauri%20v2-24C8D8?logo=tauri&logoColor=white)
![Windows · macOS · Linux](https://img.shields.io/badge/platforms-Windows%20%C2%B7%20macOS%20%C2%B7%20Linux-555)
![Status: pre-alpha](https://img.shields.io/badge/status-pre--alpha-orange)

> **Pre-alpha.** Unsigned installers for every platform are on the
> [Releases](https://github.com/AryanMahajan/routelogic/releases) page, or run it from source.
> [Install](#install) · [Run it from source](#run-it-from-source) · [What works](#what-works-today) · [Flows](#flows-multi-step-api-tests-on-a-canvas) · [Agents](#agents-describe-a-test-get-a-flow) · [Docs](docs/)

---

Point RouteLogic at a repository. It reads the source — it does not run it — works out which
HTTP endpoints the project exposes, and gives you a request editor for each one, linked back
to the file and line that defines it.

```
fastapi-app  ·  FastAPI  ·  http://localhost:9000

USERS
  GET     /api/v1/users/                 app/api/users.py:17
  POST    /api/v1/users/                 app/api/users.py:35    🔒 body
  GET     /api/v1/users/{user_id}        app/api/users.py:26
  DELETE  /api/v1/users/{user_id}        app/api/users.py:42    🔒
ITEMS
  GET     /v1/items                      app/api/items.py:11
  GET     /v2/items                      app/api/items.py:11    ← same router, mounted twice
ADMIN
  GET     /?/stats                       app/api/admin.py:11    ⚠ prefix comes from settings
UNGROUPED
  GET     /orphan/forgotten              app/api/orphan.py:8    ⚠ router is never mounted

14 endpoints · 2 with gaps · scanned in 86 ms
```

Every endpoint carries its method, path, query parameters, headers, request body, auth
requirement and source location. Click one, fill in the blanks, send, read the response.
No endpoint setup by hand — and where static analysis genuinely cannot know something, it
**shows the gap instead of guessing**.

Then take those endpoints onto a canvas and chain them into a test — log in, take the token,
create a record, fetch it, assert on it, delete it, confirm it is gone — and run the whole
thing with one key. See [Flows](#flows-multi-step-api-tests-on-a-canvas).

## Why another API client?

Postman, Insomnia, Bruno and Hoppscotch are good at storing requests you have already
described to them. None of them read your source tree and tell you what the project actually
serves. That gap — between *"I just cloned this repo"* and *"I can call its API"* — is what
RouteLogic closes.

It is deliberately not a Postman clone. It is the shortest path from *"what APIs does this
project have?"* to *"I can see it, understand it, and test it."*

|                                    | RouteLogic | Postman | Insomnia | Bruno | Hoppscotch |
|------------------------------------|:---------:|:-------:|:--------:|:-----:|:----------:|
| Discovers routes from source code  | **✅**    | ✗       | ✗        | ✗     | ✗          |
| Click-through to the defining line | **✅**    | ✗       | ✗        | ✗     | ✗          |
| Visual multi-step flows, built from discovered routes | **✅** | partial | partial | partial | ✗ |
| Works offline, no account          | ✅        | partial | partial  | ✅    | ✅         |
| Git-friendly plain-text environments | ✅      | ✗       | ✗        | ✅    | ✗          |
| Secrets kept out of committed files| ✅        | vault   | vault    | ✅    | ✗          |
| cURL / OpenAPI / raw HTTP import   | ✅        | ✅      | ✅       | ✅    | ✅         |
| Environments and `{{variables}}`   | ✅        | ✅      | ✅       | ✅    | ✅         |
| Open source                        | ✅        | ✗       | ✅       | ✅    | ✅         |

## What works today

- **Route discovery** for **FastAPI**, **Flask** (blueprints, `MethodView`, Flask-RESTful
  and RESTX), **Django + DRF** (`urlpatterns`, `include()`, class-based views, ViewSets and
  routers), **Express** (CommonJS and ESM, nested routers, `.route()` chains),
  **Next.js** (App Router route handlers and `pages/api`) and **Go** (net/http, Gin, Echo,
  chi, Fiber, gorilla/mux — groups, `Route` closures, `Mount`, `StripPrefix`, routers
  handed to functions or returned from them), across files, following imports, re-exports
  and `include_router` / `register_blueprint` / `include()` / `app.use` / `Group` prefixes.
- **Honest gaps**: a prefix read from an environment variable shows as `/?/…`, a router
  nobody mounts is flagged as an orphan, a router built by a factory is reported rather than
  dropped.
- **Ask the app** (opt-in runtime enrich, FastAPI, Flask and Django): import the
  application and take its own route table — `app.openapi()`, `url_map`, or Django's URL
  resolver — merged onto the static scan.
  Gaps get resolved, loop-registered routes appear, dead routes are labelled, and every
  source location is kept. The exact command is shown before anything runs.
- **Keyboard first.** `Ctrl+K` finds any endpoint in the scanned API and opens it;
  `Ctrl+/` shows every shortcut — tabs, sidebar panels, the URL bar, find-in-response,
  copy, and the flow canvas all have one.
- **Request editor** with tabs, path/query/header/body/auth editing, `{{variable}}`
  autocomplete, and **paste-a-cURL-into-the-URL-bar** (bash *and* Windows `cmd` quoting).
  **Copy** any request back out as cURL (bash or `cmd`), PowerShell, `fetch` (browser or
  Node.js), or just its URL — resolved exactly as it would be sent — and a whole collection
  at once, including as a sanitized HAR.
- **HTTP engine** built for predictability: redirects off by default and shown as a chain
  when on, credentials stripped on cross-origin hops, raw bytes preserved, per-request
  timeouts.
- **Environments, variables and secrets** in three tiers — committed workspace files,
  a private local secret store, disposable caches — with history recorded redacted.
  **Collections are yours**: saved requests live in your user data directory and follow
  you into every project.
- **Import** from cURL, raw HTTP and OpenAPI 3.x / Swagger 2.0.
- **Flows**: multi-step API tests on a canvas. Drag discovered endpoints in, wire them up,
  extract `{{auth_token}}` from one response into the next request, assert on status,
  headers and body, run the whole thing and read the failure path off the graph — then jump
  from the failing card to the handler in your editor. Saved with the project.
- **Agents over MCP**: Claude Code, Cursor and others can read the discovered API, try
  requests and write and run flows for you — loopback-only by default, secrets masked.

Verified by 570+ tests, including fixture projects per framework whose snapshots record
**expected misses** as well as hits, runs against the `expressjs/express` repository,
chi's examples, Echo's cookbook and Fiber's recipes, and an end-to-end runtime-enrich pass
over a real Flask application.

## Flows: multi-step API tests on a canvas

A flow is a graph of requests where each response can feed the next. Cards come straight
from the discovered API — click an endpoint in the API panel or drag it onto the canvas —
so nothing is retyped, and every card still knows the file and line that serves it.

```
 ┌─ POST /api/v1/users/ ─┐    ┌─ GET /api/v1/users/{id} ─┐    ┌─ IF {{user_id}} > 3 ─┐
 │ ↓ user_id = body.id   │──▶ │ id = {{user_id}}          │──▶ │             true ●──┼──▶ DELETE … ──▶ GET …
 │ ✓ status == 201       │    │ ✓ body.name == Dana       │    │            false ●  │      ✓ 204        ✓ 404
 └───────────────────────┘    └───────────────────────────┘    └─────────────────────┘
        201 · 12 ms                    200 · 4 ms                     took true
```

- **Extract** a value from any response — a body path like `user.id` or `items[0].name`, a
  header, the status — and it becomes `{{user_id}}` for every later step: in a URL, a path
  parameter, a header, a body.
- **Assert** on status, headers, body fields or duration with `==`, `!=`, `contains`,
  `exists`, `>`, `<`. Every new card starts with `status < 400`; delete it when a 404 is
  the point.
- **Conditions** send the run down a `true` or `false` output; the other arm is skipped, not
  failed, and both arms can rejoin.
- **Variables blocks** declare the flow's own inputs on the canvas — `who = ann` once, `{{who}}`
  everywhere — and **Display blocks** put the result in a sentence: `{{who}} is user {{id}}`.
- **Run all, run what is wired to the selected card, or re-run one card** with the last
  run's variables.
- **Execution follows the edges**, never the layout: a step runs after everything wired into
  it, and only if those passed. When something fails, the failed card turns red with the
  reason, the edges it cut turn red, and every step it took down is dashed and says which
  step's failure stopped it.
- **Inspect** any card after a run: the request as actually sent, extracted values, each
  check with expected vs found, and the full response in the same viewer a request tab uses.
- **Jump to source** from a card with `↗`, exactly like the API panel.
- Flows are saved as readable YAML in `.routelogic/flows/` with the project, so they travel
  with the repository; every request a run sends lands in history, redacted.

`Ctrl+Enter` runs, `Ctrl+S` saves, `Ctrl+D` duplicates, `Delete` deletes, `Ctrl+Z` undoes,
`Ctrl+Alt+K` searches the API for a step to add (`Ctrl+Alt+A`, `1`, `2`, `3` add a blank
request, a variables block, a display, a condition), right-click for the rest, and `Ctrl+B`
hides the sidebar when you want the whole screen for the canvas.

Read [docs/flows.md](docs/flows.md) for the reference,
[docs/flows-walkthrough.md](docs/flows-walkthrough.md) for a step-by-step build of a real
flow against the bundled FastAPI fixture and a trace of what the runner does with it, and
[docs/examples/fastapi-user-lifecycle.yaml](docs/examples/fastapi-user-lifecycle.yaml) for
that flow as a file.

## Agents: describe a test, get a flow

Connect Claude Code, Cursor or any other MCP client — **Agent…** in the sidebar shows the
exact command for your machine — and ask in words:

> Write a flow that logs in, creates a user, fetches it, deletes it and checks it's gone.

The agent lists the endpoints RouteLogic discovered, sends each request to your running
server to see what really comes back, writes the flow with the right captures and
assertions, runs it, and fixes what fails. It appears on the canvas while you watch, as an
ordinary file you can edit and commit.

- The server is the app itself — `routelogic mcp` — so there is nothing else to install.
- Requests go to **loopback only** unless you list more hosts in `workspace.yaml`, checked
  on the resolved host and again at every redirect.
- **Secret values never reach the agent**; it sees `{{secret:NAME}}`, which is also how a
  flow should use one.
- Everything it sends is in History, marked `agent`.

See [docs/agents.md](docs/agents.md).

## Framework support

| Framework | Language | Status | Notes |
|---|---|---|---|
| [FastAPI](docs/discovery/frameworks.md#fastapi)  | Python | ✅ Implemented | Routers, prefixes, dependencies as auth, signatures |
| [Express](docs/discovery/frameworks.md#express)  | JS/TS  | ✅ Implemented | Module graph across `require`/`import`, mounting, chains |
| [Next.js](docs/discovery/frameworks.md#nextjs)   | TS/JS  | ✅ Implemented | App Router + legacy `pages/api`, dynamic and catch-all segments |
| [Flask](docs/discovery/frameworks.md#flask)      | Python | ✅ Implemented | Blueprints (nested, re-registered), `MethodView`, Flask-RESTful / RESTX, `add_url_rule` |
| [Django / DRF](docs/discovery/frameworks.md#django--drf) | Python | ✅ Implemented | `urlpatterns`, `include()`, `re_path`, class-based views, ViewSets, `DefaultRouter`, `@action` |
| [Go](docs/discovery/frameworks.md#go) | Go | ✅ Implemented | net/http (Go 1.22 patterns), Gin, Echo, chi, Fiber, gorilla/mux; package-scoped names, routers passed to or returned from functions, `Mount`, `StripPrefix`, struct `json` tags as bodies |

Discovery is static by default — RouteLogic reads your code and never executes it. For the
Python frameworks, an opt-in [runtime enrich](docs/discovery/runtime-enrich.md) step imports
your app for an exact result when you ask for it, and always shows you the exact command
first.

Adding a framework is a self-contained job against a documented contract — see
[adding a framework](docs/discovery/adding-a-framework.md).

## Install

Download the build for your platform from the
[latest release](https://github.com/AryanMahajan/routelogic/releases/latest):

| Platform | File |
|---|---|
| Windows 10/11 | `RouteLogic_x.y.z_x64-setup.exe` (or the `.msi`) |
| macOS (Apple Silicon and Intel) | `RouteLogic_x.y.z_universal.dmg` |
| Linux | `RouteLogic_x.y.z_amd64.AppImage`, `.deb` or `.rpm` |

**The builds are not code-signed** — signing certificates cost money, and this is a free
project with no income. Each OS will warn once before the first launch:

- **Windows:** SmartScreen says "Windows protected your PC". Click **More info → Run anyway**.
- **macOS:** "Apple could not verify … is free of malware". Dismiss it, then **System
  Settings → Privacy & Security → Open Anyway**. If it says the app is *damaged*, that is the
  quarantine flag on an unsigned download: `xattr -cr /Applications/RouteLogic.app`, then open
  it again.
- **Linux:** `chmod +x RouteLogic_*.AppImage`, or install the `.deb` / `.rpm` with your package
  manager.

If you would rather not run an unsigned binary, build it yourself — the release workflow is
[`.github/workflows/release.yml`](.github/workflows/release.yml), and `npm run tauri build`
produces the same installer locally.

## Run it from source

Requires a stable Rust toolchain, Node 20+, and Tauri's platform prerequisites
(WebView2 on Windows — already present on Windows 10/11; `webkit2gtk` on Linux; Xcode
command-line tools on macOS).

```bash
git clone https://github.com/AryanMahajan/routelogic.git
cd routelogic
npm install && npm install --prefix ui
npm run tauri dev
```

The first build compiles the Rust core and takes a few minutes; after that it is seconds.
Open any of **`tests/fixtures/{fastapi,flask,django,express,nextjs,go,go-chi}`** for a
project with every kind of route, gap and orphan in it. The Python ones run
(`pip install -r requirements.txt`), so you can send the requests and try **Ask the app**.

## How discovery works, in three sentences

Framework adapters recognise only three things in a syntax tree — *this creates a router*,
*this registers a route*, *this mounts a router at a prefix* — plus imports and exports.
A single registration graph, shared by every framework, links those facts across files and
composes the full paths. Anything it cannot resolve statically becomes a visible
`Unresolved` segment rather than a guess.

Longer version: [how it works](docs/discovery/how-it-works.md).

## Honest limitations

- Paths built from runtime values (`settings.API_PREFIX`, `process.env.PREFIX`) are shown
  as unresolved, not guessed.
- Routes registered dynamically — in a loop, from config, by a factory — may be missed.
  Fixtures record which ones, on purpose.
- Request and response schemas are best-effort from type hints and what handlers read.
- Auth detection from middleware and dependency *names* is a heuristic and says so.

## Design principles

1. **Fast** — noticeably lighter than a full API platform. Measured, not assumed.
2. **Local-first** — no account, no cloud, everything works offline.
3. **Zero configuration where possible** — if the project already states something, read it.
4. **Codebase-aware** — always know where an endpoint actually comes from.
5. **Git-friendly** — environments and workspace settings are readable files you can
   review in a diff; collections are plain YAML too, kept per user.
6. **Extensible** — frameworks are independent adapters, never special cases in the UI.
7. **No feature bloat** — solve discovery and testing exceptionally well first.

## Documentation

Start at **[docs/](docs/)**.

- [Getting started](docs/getting-started.md) · [Concepts](docs/concepts.md) ·
  [Import](docs/import.md)
- [Flows](docs/flows.md) · [Flows walkthrough](docs/flows-walkthrough.md) ·
  [Example flow](docs/examples/fastapi-user-lifecycle.yaml) · [Agents](docs/agents.md)
- [How discovery works](docs/discovery/how-it-works.md) ·
  [Framework support](docs/discovery/frameworks.md) ·
  [Adding a framework](docs/discovery/adding-a-framework.md)
- [Architecture](docs/architecture.md) · [Security](docs/security.md) ·
  [Workspace format](docs/workspace/format.md)

## Tech

Rust core in a Cargo workspace · Tauri v2 desktop shell · React 18 + TypeScript + Tailwind
UI · tree-sitter parsing for Python, JavaScript, TypeScript and Go · reqwest · SQLite history.

## FAQ

**Does RouteLogic run my project's code?** Not unless you ask. Discovery is static analysis
of the source. Runtime enrich is opt-in, shows you the exact command before running it, and
remembers the decision per project until you withdraw it.

**Is it a Postman alternative?** For testing the API of a codebase you have in front of you,
yes. It is not trying to replace team collaboration features, mock servers or monitoring.

**Which frameworks are supported?** FastAPI, Flask, Django (with DRF), Express, Next.js,
and Go with net/http, Gin, Echo, chi, Fiber or gorilla/mux. See
[framework support](docs/discovery/frameworks.md).

**Can an AI agent use it?** Yes — over MCP, with the app itself as the server. It can
reach loopback hosts and whatever you allow, never sees secret values, and everything it
sends is in history. See [agents](docs/agents.md).

**Where are my secrets stored?** Outside the committed workspace, in a private local store.
Environment files reference them by name only. See [security](docs/security.md).

## License

[MIT](LICENSE). Free to use, modify and redistribute — commercially or otherwise. RouteLogic
has no paid tier, no accounts, no telemetry, and no cloud; it never will.
