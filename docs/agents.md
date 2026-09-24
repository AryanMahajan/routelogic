# Agents: let Claude Code, Cursor and others write flows

RouteLogic can serve an AI agent over the [Model Context Protocol](https://modelcontextprotocol.io).
Connect one and describe a test in words, for example "log in, create a user, fetch it,
delete it, check it's gone". The agent reads the API RouteLogic discovered, tries each
request against your running server to see the real response, writes the flow, runs it and
fixes whatever fails. The flow appears on the canvas while you watch, as a normal file in
`.routelogic/flows/`.

Nothing extra is installed. The server is the RouteLogic app itself, started as
`routelogic mcp`. It uses the same workspace, discovery, variables and secrets as the window
you already have open.

## Connecting

Open the project in RouteLogic and click **Agent…** at the bottom of the sidebar. The dialog
shows the exact setup for this machine, with the app's path and this project's path already
filled in, and a Copy button.

### Claude Code

Run this in a terminal in the project's folder. The dialog gives you the line with your
paths in it:

```bash
claude mcp add routelogic -- "<path to RouteLogic>" mcp --workspace "<project folder>"
```

Then start `claude` there and ask for a flow. `/mcp` shows whether the server is connected.

### Cursor, Claude Desktop, Windsurf and others

Most clients read an `mcpServers` block from a JSON file. The dialog's second tab has the
block for this project:

```json
{
  "mcpServers": {
    "routelogic": {
      "command": "<path to RouteLogic>",
      "args": ["mcp", "--workspace", "<project folder>"]
    }
  }
}
```

| Client | Where it goes |
|---|---|
| Cursor | `.cursor/mcp.json` in the project, or `~/.cursor/mcp.json` for every project |
| Claude Desktop | `claude_desktop_config.json`: `%APPDATA%\Claude\` on Windows, `~/Library/Application Support/Claude/` on macOS |
| Anything else | The client's MCP settings. The command and arguments are all it needs |

### Where the path points

- **Windows:** the installed `routelogic.exe`.
- **macOS:** the binary inside the app bundle, under `RouteLogic.app/Contents/MacOS/`.
- **Linux:** the `.AppImage` file itself, or `/usr/bin/routelogic` for the `.deb` and `.rpm`
  packages.

If you move or reinstall the app, copy the line again.

**From source:** `cargo build -p routelogic` and point at `target/debug/routelogic`. Or
skip the desktop build and use the standalone server binary, which does the same thing:

```bash
cargo run -p rl-mcp --bin routelogic-mcp -- --workspace path/to/project
```

`--workspace` defaults to the current directory. If the folder has no `.routelogic/` yet,
one is created, just as opening the folder in the app would.

## What the agent can do

Nine tools. The server also gives the agent a short set of instructions on how to build a
flow, so you don't have to explain the steps.

| Tool | Does |
|---|---|
| `workspace_info` | Environments and the active one, the variable and secret names it can use as `{{name}}`, saved flows, and where it may send requests |
| `list_endpoints` | The discovered API, one line per endpoint with the handler's file and line. Takes `filter`, `offset`, `limit`, and `rescan` for after the code changes |
| `describe_endpoint` | One endpoint in full: parameters, body schema and example, auth, source, and a request node ready to put into a flow |
| `prepare_request` | The request as it would go on the wire, with variables filled in and secrets masked. Sends nothing |
| `send_request` | Sends one request and returns the real status, headers and body |
| `list_flows` / `get_flow` | The saved flows, and one in full |
| `save_flow` | Writes a flow. Refused if the graph can't run. Returns warnings for what would fail: a variable nothing defines, a value captured in one step and used in another with no edge between them, a request with no assertion. Replacing an existing flow needs `overwrite: true` |
| `run_flow` | Runs a saved flow, or one passed in without saving. Returns each step's outcome, the assertions with what was actually found, the captured values, and the response of any step that failed |

A flow the agent writes has the same format as one you build on the canvas. Its node ids
can be words (`login`, `create_user`), and it can leave out positions, because RouteLogic
lays the cards out. The full format is in [`flow.schema.json`](flow.schema.json), which is
generated from the code, and [flows](flows.md) describes how a run proceeds.

## While it works

- A flow the agent saves shows up in the Flows panel straight away. If you have it open
  with no unsaved edits, it reloads in place, and `Ctrl+Z` takes the reload back.
- If you have unsaved edits, the canvas keeps them and shows a banner: **Reload** takes the
  agent's version, **Keep mine** leaves yours for your next save.
- Every request the agent sends, including ones it was refused, is in **History** with an
  `agent` badge.

## Where it may send requests

An agent that can check its work writes better flows, so it sends real requests. What it
may reach:

- **Loopback hosts, always, with every method:** `localhost`, `*.localhost`,
  `127.0.0.0/8`, `::1` and `0.0.0.0`. That's the server you're running, and a flow that
  creates a user needs `POST` against it.
- **Anything else, only if you list it** under `agent.allow` in
  `.routelogic/workspace.yaml`:

```yaml
agent:
  allow:
    - host: api.staging.example.com   # this host, any port
      methods: [GET, HEAD, OPTIONS]    # leave out for every method
    - host: "*.internal.example.com"   # any subdomain
    - host: "10.0.0.5:8080"            # one port only
```

The check is on the host the request actually goes to, after `{{base_url}}` is filled in.
The environment's name doesn't count: an environment called `local` can point anywhere.
The check runs again before every redirect, so an allowed host can't pass the request on to
one that isn't allowed.

The file is read fresh for every request, so an edit applies to the agent's next call
without reconnecting. A refused request comes back to the agent as an error that names the
line that would allow it. The server's instructions tell the agent to ask you rather than
look for a way around the refusal.

By default `.routelogic/` is in the project's `.gitignore`, so the allow list only applies
on your machine. A teammate who clones the project starts with loopback only.

## What it sees

**Secret values never reach the agent.** Wherever one would appear — in a request echo, a
response body, a captured variable, an error — the agent gets `{{secret:NAME}}` instead.
That's also exactly how a flow should use a secret, so what the agent copies into a flow
is correct.

Ordinary variables (`base_url`, `user_id`) are shown as they are. Everything else the agent
sees is what you would see: the source-derived API, the responses from your server, and the
flows in the workspace.

## An example

With the FastAPI fixture in `tests/fixtures/fastapi` running and Claude Code connected
(the header of [`examples/fastapi-user-lifecycle.yaml`](examples/fastapi-user-lifecycle.yaml)
says how to run it):

> Write a flow called "user lifecycle": create a user, fetch it by id and check the name,
> delete it, then check fetching it again gives 404.

A typical run: `workspace_info` finds `base_url`. `list_endpoints users` and
`describe_endpoint` for the create route give it the body. A trial `send_request` shows the
new id comes back as `id`. It then saves four request nodes chained by edges, with `user_id`
captured from `body.id`, and `run_flow` passes. If a step fails, for example the delete
returns 204 where the agent asserted 200, it reads the failing step, fixes the assertion and
saves again with `overwrite: true`.

[`examples/fastapi-user-lifecycle.yaml`](examples/fastapi-user-lifecycle.yaml) is a flow of
the same shape, written by hand.

## Troubleshooting

- **The client says the server failed to start.** Run the command from the dialog yourself.
  `routelogic mcp --help` prints usage, and a wrong `--workspace` path says so on stderr.
- **"not in the agent allow list".** The request resolved to a host that isn't loopback. Check
  which environment is active; the agent uses it unless it asks for another. Add the host
  under `agent.allow` if you mean it to go there.
- **Flows appear but the canvas doesn't update.** The app watches the workspace it has open.
  Check that the `--workspace` path is the same folder.
