# Getting started

> **Pre-alpha.** Steps marked *(planned)* do not work yet.

## Install a release

Every tagged version has installers on the
[Releases](https://github.com/AryanMahajan/routelogic/releases) page: an `.exe`/`.msi` for
Windows, a universal `.dmg` for macOS (Apple Silicon and Intel), and an AppImage/`.deb`/`.rpm` for
Linux.

The builds are **unsigned**, so the first launch is met with a warning:

| OS | What you see | What to do |
|---|---|---|
| Windows | SmartScreen: "Windows protected your PC" | **More info → Run anyway** |
| macOS | "Apple could not verify … is free of malware" | Dismiss it, then **System Settings → Privacy & Security → Open Anyway** (right-click → Open also works on macOS 14 and earlier) |
| macOS | "… is damaged and can't be opened" | The quarantine flag on an unsigned download. Run `xattr -cr /Applications/RouteLogic.app` in Terminal, then open it again |
| Linux | AppImage is not executable | `chmod +x RouteLogic_*.AppImage` |

The warning appears once per install. Signing will come when the project can afford
certificates; until then, the alternative is to build from source below.

## Run from source

```bash
git clone https://github.com/AryanMahajan/routelogic.git
cd routelogic
npm install && npm install --prefix ui
npm run tauri dev                  # development build, hot-reloading UI
npm run tauri build                # the same installer the release workflow produces
```

The first `dev` compiles the Rust side, which takes a few minutes; afterwards it is seconds.
The sample projects under `tests/fixtures/` — `fastapi`, `nextjs`, `express` — are the
quickest things to open: each contains routes that should be found, deliberate gaps that
should be *shown* rather than guessed, and an unmounted router that should be flagged.

Requires a recent stable Rust toolchain and Node 20+. Tauri also needs platform
prerequisites — WebView2 on Windows, `webkit2gtk` on Linux, Xcode command line tools on macOS.

## Open a project

Choose **Open Project** and select a repository root. RouteLogic will:

1. Detect the language and framework from manifests and imports.
2. Scan the source for route registrations.
3. Resolve router prefixes into full paths.
4. Infer candidate base URLs (from run scripts, `.env`, Dockerfile, compose files).
5. Show you the endpoint tree.

A first scan of a mid-sized project should complete in well under a second; rescans are
incremental and near-instant.

RouteLogic only reads files inside the directory you selected, and it does not execute your
project's code unless you explicitly ask for [runtime enrich](discovery/runtime-enrich.md).

## Read an endpoint

Selecting an endpoint shows everything discovery could establish:

- Method and full path, with path parameters called out
- Query parameters, headers, and request body schema where detectable
- Authentication requirement where detectable
- The **file and line** it was defined on — click to open it in your editor
- Which framework and which discovery source it came from

Where discovery could not establish something, the UI says so. An unresolvable path segment
appears as `?` with the source expression, rather than a guess.

The tree opens with its groups folded; click a group to open it, or type in the filter
box (`Ctrl+Shift+F`) and every group with a hit opens on its own. Quicker still, from
anywhere: `Ctrl+K`, type part of the path, summary or group, `Enter` — the endpoint opens
in a tab, whatever was in front.

## Save the whole API as a collection

**Save all** in the API panel writes every resolved endpoint into one collection, one
folder per group (router, blueprint, tag or app), and switches to **Collections** to show
it. Endpoints with an unresolved path are skipped and counted rather than written as
`/?/stats`. Run it again after a rescan and it updates the discovered requests in place —
matched by the endpoint they came from — while leaving your own edits, folders and
hand-added requests alone.

## Ask the application (FastAPI, Flask and Django)

Static analysis stops where the code needs running: a prefix from `settings.API_PREFIX`,
routes registered in a loop, Pydantic schemas. For FastAPI, Flask and Django projects the
endpoint list offers **Ask the app**. It shows the exact command it will run — interpreter, helper
script, `module:app` target — and where each part came from, and waits for you to press
**Run**. Nothing is executed before that.

The helper imports your application, asks it for its own route table (`app.openapi()` or
`url_map`), and exits. The result is merged onto the static scan: exact paths and schemas,
source locations kept, and every difference labelled — `RT` for a route only the
application knows about, `✓` for a gap the application closed, `∅` for something declared
in source that the application does not serve.

The target is remembered in `.routelogic/workspace.yaml`; **Forget** in the dialog withdraws
it. Rescanning returns to the static result. Details in
[runtime enrich](discovery/runtime-enrich.md).

## Send a request

1. Pick a base URL — RouteLogic suggests candidates it found; you can override it.
2. Fill in path parameters. Adjust query, headers, and body as needed.
3. Set auth, or reference an environment variable such as `{{token}}`.
4. **Send.**

The response pane shows status, timing, response headers, and the body — pretty-printed,
raw, or saved to a file for large payloads.

## Save it

**Save** (Ctrl+S) writes the request into a collection — the box next to the button names
which; type a new name to create one. A request opened from a collection saves back to it.
Collections are yours, not the project's: they live in your user data directory
(`%LOCALAPPDATA%\routelogic\collections` on Windows, `~/.local/share/routelogic/collections`
on Linux) and the same list appears in every project you open. Environments, secrets and
history stay with the project.

The **Collections** panel is where they are managed. Folders start folded; click one to
open it. Drag a request to reorder it, drop it
on a folder or another collection to move it, and use the `…` menu on a request to rename,
duplicate, delete or file it under a folder (`Users/Admin` nests). The `…` on a collection
renames or deletes it; **+ Collection** makes an empty one.

## Paste a cURL command

Choose **Import → cURL** and paste:

```bash
curl 'https://api.example.com/users?page=2' \
  -H 'Authorization: Bearer token' \
  -H 'Content-Type: application/json' \
  --data-raw '{"name":"Aryan"}'
```

RouteLogic splits this into method, URL, query parameters, headers, recognised auth, and body
automatically — you never sort the pieces by hand. See [import](import.md).

## Copy a request out

The **Copy** button beside **Send** copies the open request as a command, resolved exactly
as it would be sent — variables filled in, auth applied, so the copied command carries any
secrets. The arrow beside it lists every format: the URL alone, cURL for bash or Windows
`cmd`, PowerShell (`Invoke-WebRequest`), and `fetch` for the browser or Node.js. The format
you pick becomes what the button does next time.

The same menu copies the latest response body, and everything in the request's collection
at once — as a list of URLs, as commands one after another, or as a HAR file with
credentials blanked so it can be shared.

## Keyboard shortcuts

`Ctrl+/` (or `F1`, or the ⌨ button at the foot of the sidebar) shows the full sheet. The
ones worth learning first:

| Keys | Does |
|---|---|
| `Ctrl+K` | Find an endpoint in the API and open it |
| `Ctrl+T` · `Ctrl+W` | New request tab · close the tab |
| `Ctrl+Tab` · `Ctrl+1`…`9` | Next tab · go to a tab (`Ctrl+9`: the last) |
| `Ctrl+L` | Jump to the URL bar; `Enter` there sends |
| `Ctrl+Shift+1`…`5` | Params, Headers, Body, Auth, Settings |
| `Ctrl+Enter` · `Ctrl+S` | Send · save |
| `Ctrl+F` | Find in the response body |
| `Ctrl+Shift+C` | Copy the request in the format used last |
| `Alt+1`…`4` | Sidebar: API, Flows, Collections, History |
| `Ctrl+Shift+F` | Filter the API tree |
| `Ctrl+Shift+R` | Rescan the project |
| `Ctrl+B` | Hide or show the sidebar |

The flow canvas has its own set — see [flows](flows.md).

## Work without a project

Choose **New Workspace** to use RouteLogic as a standalone API client — collections,
environments, variables, and history, with no project attached.
