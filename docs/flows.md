# Flows

A flow is a multi-step API test built on a canvas: requests from the project's discovered
API, wired together so that what one response returns feeds the next request, with checks
along the way. It runs as one test, shows exactly where it broke, and every card can jump
to the handler that serves it.

```
POST /auth/login ──▶ GET /me ──▶ GET /users/{{user_id}}
  ↓ auth_token        ↓ user_id      ✓ status < 400
  ✓ status < 400      ✓ status < 400 ✓ body.name == Aryan
```

## Building one

Open a project, scan it, then **Flows → New flow**. Add steps three ways:

- **click** an endpoint in the API panel — it becomes a card, wired after the selected one;
- **drag** an endpoint from the API panel onto the canvas — it lands where you drop it,
  unconnected;
- **+ Add** in the toolbar (`Ctrl+Alt+K`) — the same endpoint list, grouped as in the API
  panel, with a filter: type, `↑` `↓`, `Enter`. Plus a blank request, a condition, a
  variables block and a display, which also have keys of their own: `Ctrl+Alt+A`,
  `Ctrl+Alt+3`, `Ctrl+Alt+1`, `Ctrl+Alt+2`.

Connect cards by dragging from a right-hand handle to a left-hand one. Edges are
*dependencies*: a card runs after everything wired into it, and only if those passed.

| Action | How |
|---|---|
| Pan · zoom | drag the background · scroll or pinch |
| Move | drag a card (Shift+drag for a box selection, Ctrl+click to add to it) |
| Delete | select, then `Delete` or `Backspace` |
| Duplicate | `Ctrl+D` |
| Select all · none | `Ctrl+A` · `Esc` |
| Undo · redo | `Ctrl+Z` · `Ctrl+Y` (or `Ctrl+Shift+Z`), or ↶ ↷ in the toolbar |
| Add a step | `Ctrl+Alt+K` to search the API; `Ctrl+Alt+A` blank request, `Ctrl+Alt+1` variables, `Ctrl+Alt+2` display, `Ctrl+Alt+3` condition |
| Fit in view | `Ctrl+0` |
| Right-click | the background: add a card right there, undo/redo, run, fit view. A card: run it, run what it is wired to, duplicate, disconnect, open its source, delete. A connection: delete it |
| Run | `Ctrl+Enter` or **▶ Run** — everything, or what is wired to the selected card |
| Run one card | `Ctrl+Shift+Enter` or **▶ Step** in the inspector |
| Save | `Ctrl+S` or **Save** |
| `{{` anywhere | the variables in scope — or `Ctrl+Space` to open the list without typing |

Undo covers everything done to the document: adding and deleting cards, connecting and
moving them, and edits in the inspector. A burst of typing is one step. Inside a text field
`Ctrl+Z` is the field's own undo, as usual; click the canvas first for the flow's.

## What a card is

A request card is an ordinary RouteLogic request — the same editor a request tab has — plus
two things that make it a test step:

**Extract** — pull values out of the response for later steps. Each becomes a
`{{variable}}` for everything downstream, taking precedence over an environment variable
of the same name. Sources: a body path (`access_token`, `user.id`, `items[0].name`), a
header, the status, the raw body, or the duration. Scalars are extracted as plain text;
objects and arrays as compact JSON, so a whole record can be re-sent as a body.

You do not have to remember the response's shape: the path field's **⌄** lists every
path in a real response with the value found there — this card's last run, or, before
any run, the newest history entry for the same endpoint. Picking one fills the path and
names the variable after it. The same list serves header names.

**Assert** — what must hold for the step to pass. Every new card starts with
`status < 400`. Picking a body path or header from the response fills in `equals` and
the value as it is, so "keep it this way" is one click. Operators: `==`, `!=`, `contains`, `not contains`, `exists`, `not exists`,
`>`, `<`. When both sides are numbers the comparison is numeric, so `200 == "200"` and
`9 < 10`. The right-hand side may use `{{variables}}`.

A **condition** card compares two interpolated values and sends the run down its `true` or
`false` output. Whatever hangs off the other output is *skipped*, not failed.

A **variables** block declares `name = value` pairs — the flow's own inputs. Write the user
you look up once, as `who = ann`, and every step says `{{who}}`; changing the flow to look
up Dana is one edit on the canvas, not an environment change and not a hunt through the
steps. A value may use other variables (`greeting = hello {{who}}`), and the block's values
beat the environment's, while anything a later step extracts beats them in turn. A
variables block with nothing wired into it **runs before everything else**, wherever it
sits; wire something into it and it runs in its place, which is how a value can be set
mid-flow from something extracted.

A **display** block resolves a template — `{{who}} is user {{found_id}}` — and shows the
sentence on the card once the run reaches it: the result the flow was after, in plain
words on the canvas rather than buried in a response body. An unknown variable fails it.

## How a run proceeds

Steps run one at a time in dependency order — a topological sort of the edges, with ties
broken by the order cards were added, never by where they sit on the canvas. Moving a card
cannot change what the test does.

Before each step the runner looks at the edges into it:

- an edge from a step that **failed** — or was skipped because something upstream of it
  failed — is *dead*, and one dead edge is enough to skip the step. A step whose
  prerequisite did not happen must not run against half-set-up state.
- an edge out of a condition's untaken output is *inactive*. A step runs as long as **one**
  of its edges is live, so the two arms of a condition can rejoin.
- a step with no edges into it always runs.

A request step **fails** when an assertion does not hold, an extraction finds nothing, a
`{{variable}}` it needs is undefined (reported before anything is sent), or the request
never gets a response. It **passes** otherwise — a 404 with no assertion against it is
not a failure, which is what lets `DELETE … → GET … → assert status == 404` work.

Extracted values are committed only when the step passed.

### Running part of a flow

**▶ Run all** with nothing selected runs every card from the roots. Select a card and the
same button becomes **▶ Run connected (n)**: only the *n* cards wired to the selected one —
in either direction, however far — run, and the islands elsewhere on the canvas are left
alone. A card wired to nothing shows **▶ Run selected**. Both include every input block, so
the flow's variables are always in scope (they are not counted in the *n*; they are implied).

**▶ Step** in the inspector (or `Ctrl+Shift+Enter`) runs the selected card on its own. The
steps before it are taken as done, and the variables from the last run stand in for what
they would have produced — so a step whose assertion you just edited can be re-run in
isolation without logging in again. With no previous run only the environment and the
input blocks are in scope.

## Reading a failure

The canvas is the report: passed cards are green, the failed card is red with its reason,
skipped cards are dashed and say which step's failure kept them from running, and the
edges the run took are green while the ones a failure cut are red. The bar above the
canvas totals it up and lists every variable the run extracted.

Select a card and open **Result** to see what happened to it: the request as it was
actually sent, the values it extracted, each check with what was expected and what was
found, and the full response — body, headers, timing — in the same viewer a request tab
uses.

Every request a flow sends is recorded in history, redacted, exactly like a single send.

## Source

A card built from a discovered endpoint keeps the endpoint's identity (`spec_ref`), so the
`↗` on the card and in the inspector opens the handler in your editor — the same jump the
API panel offers. If the project has not been scanned in this session the button is
absent until it has.

## On disk

Flows are saved to `.routelogic/flows/<name>.yaml` beside the project's environments, so
they are committed with the project and whoever clones it gets the tests. The file is a
flat list of cards, each with its request, `extract` and `assert`, plus the edges — meant
to be readable in a pull request.

```yaml
name: login smoke
nodes:
  - id: 3f1c…
    type: request
    position: { x: 80, y: 120 }
    request: { method: POST, url: "{{base_url}}/auth/login", … }
    extract:
      - name: auth_token
        from: body
        path: access_token
    assert:
      - from: status
        op: less_than
        expected: "400"
edges:
  - from: 3f1c…
    to: 9a2e…
```

Secret values never appear: a request references `{{secret:name}}` and the value is
substituted moments before sending, as everywhere else in RouteLogic.

## A worked example

[flows-walkthrough.md](flows-walkthrough.md) builds the flow below in the UI click by
click, then follows the run through the engine.


[`docs/examples/fastapi-user-lifecycle.yaml`](examples/fastapi-user-lifecycle.yaml) is a
flow for the FastAPI fixture in `tests/fixtures/fastapi`: health → create a user → fetch it
→ *if it is a new row* → delete → confirm 404, plus an items chain across the `/v1` and
`/v2` mounts. The header of the file says how to run the fixture and where to copy the
flow; `cargo test -p rl-core --test fixture_flow` keeps it in step with the scan, and runs
it for real when `ROUTELOGIC_FIXTURE_URL` points at the app.

## Not yet

- Loops, retries, delays, parallel branches — this is a test, not a workflow engine.
- Assertions with a regular expression.
- Cancelling a run in progress.
- Persisting run history as a unit (each request is in history; the run as a whole is not).
