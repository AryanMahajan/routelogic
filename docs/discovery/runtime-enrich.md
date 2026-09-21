# Runtime enrich

Static analysis reads your code. Runtime enrich *asks your application*. It is more accurate
and more invasive, so it is always opt-in.

## Why it exists

FastAPI already knows its own API perfectly. `app.openapi()` returns a complete OpenAPI
document with every path, parameter, schema, and security scheme, generated from the same
objects that serve the requests. No static analysis will ever match that.

Flask exposes `app.url_map`, which gives a complete and exact route list, though no schemas.
Django's URL resolver, once `django.setup()` has run, is the same thing for Django — with
ViewSet action maps and class-based view methods attached.

When fidelity matters, going to the source of truth beats inferring it. And the things
static analysis is honest about not knowing — a prefix read from `settings.API_PREFIX`,
routes registered in a loop, whether an orphaned router is really unreachable — are exactly
the things the application can answer.

## What it costs

Runtime enrich **imports your project's code and executes it**. Module-level code runs. That
can mean database connections, config validation, network calls, or any other import-time
side effect your application has.

This is a real trust boundary, so:

- It is **never automatic**. Nothing runs until you press **Run** in the dialog.
- The dialog shows the **exact command** — interpreter, helper script, target — and where
  each part was inferred from. Change any part and the command line updates before you
  approve it.
- The decision is stored per project (the target, in `.routelogic/workspace.yaml` as
  `app_target`) and withdrawn with **Forget**. The interpreter is not stored: it is
  machine-specific, and the workspace file is meant to be committed.
- If the helper fails, the failure is reported with the tail of its stderr — the traceback
  — and the static results are left exactly as they were.

See [security](../security.md) for the full trust model.

## How it works

1. **Locate the application object.** RouteLogic needs a `module:attribute` target — the
   same thing `uvicorn` takes. Candidates come from, best first:

   | Source | Example | Becomes |
   |---|---|---|
   | A run command in `Procfile`, `Makefile`, `Dockerfile`, compose, `package.json` | `uvicorn app.main:app --port 9000` | `app.main:app` |
   | `--factory` on that command | `uvicorn --factory app:create_app` | `app:create_app()` |
   | `flask --app …` or `FLASK_APP=…` in an env file or Dockerfile | `flask --app app run` | `app` (Flask's own lookup applies) |
   | `DJANGO_SETTINGS_MODULE` in `manage.py`, or where the scan found `ROOT_URLCONF` | `config/settings.py` | `config.settings` |
   | Where the static scan found `app = FastAPI()` / `Flask()` | `app/main.py` | `app.main:app` |
   | … inside a factory function | `def create_app():` in `app/__init__.py` | `app:create_app()` |

   The module path follows `__init__.py` files upward, so a `src/` layout runs from `src/`.
   Anything can be overridden by typing a target by hand.

2. **Choose the interpreter.** The project's own environment first — `.venv`, `venv`, `env`
   — then an activated `VIRTUAL_ENV` or `CONDA_PREFIX`, then `python3` / `python` on `PATH`
   (the Windows Store stub excluded). The helper has to run where the project's
   dependencies are installed, so a project without a virtual environment usually needs one
   created first.

3. **Write, then run, the helper.** A single self-contained script,
   `routelogic_enrich.py`, is written to `.routelogic/local/` (gitignored) before every run,
   so what you can read is what runs. It is embedded in the RouteLogic binary; edits do not
   survive. The command is

   ```
   cd <project> && <python> .routelogic/local/routelogic_enrich.py <module:attribute>
   ```

   with `PYTHONDONTWRITEBYTECODE=1` (no `__pycache__` litter) and `ROUTELOGIC_ENRICH=1` in
   the environment, so an application that wants to can notice. The helper imports the
   target, calls it if it is a factory, then:

   - **FastAPI** — calls `app.openapi()`
   - **Flask** — walks `app.url_map`, converting `<int:id>` rules into an OpenAPI
     document with path parameters typed from the converters, blueprint names as tags, and
     view docstrings as summaries. Flask's built-in `static` route is dropped.
   - **Django** — sets `DJANGO_SETTINGS_MODULE`, calls `django.setup()`, and walks the
     resolver: `<int:pk>` converters and `(?P<pk>…)` regex groups become typed parameters,
     namespaces become tags, ViewSet action maps and class-based view handlers give the
     methods, and `require_http_methods` is read through the decorator's closure. A plain
     function view is listed as GET with `x-methods-unknown`, so a `POST` the static scan
     read from an `if request.method == "POST"` survives the merge. DRF's format-suffix
     twins, API root and the admin are dropped.

   It introspects only: no server is started, no port is bound, nothing is written. Its
   stdout is the JSON result; anything the application prints while importing is redirected
   to stderr and shown to you afterwards, never parsed. A run that has not finished in
   60 seconds is killed and reported.

4. **Import the result.** The JSON goes through the same [OpenAPI importer](../import.md)
   used for ordinary spec imports — the reason that importer was built before this step.

5. **Merge with the static results.**

## The merge

Runtime results do not replace static results. The two are unioned, keyed on
`(method, normalized path)` — parameter names and trailing slashes do not matter, and a
catch-all read from source (`<path:name>`) matches the plain parameter an OpenAPI document
spells it as.

| Field | Winner | Why |
|---|---|---|
| Request and response schema | **Runtime** | Generated from the real models |
| Parameters | **Runtime** | Exact, including ones inference missed |
| Auth requirement | **Runtime** | Real security schemes |
| Source file and line | **Static** | Runtime does not know where code lives |
| Grouping, summary, description | Runtime, falling back to static | |
| Orphan flag | Cleared | The application serves it, so it is reachable |

Every endpoint then carries one of four labels, shown in the endpoint tree:

| Label | Meaning | Shown as |
|---|---|---|
| `matched` | Found by both | nothing extra |
| `gap_filled` | A static path with an unresolved segment (`/?/stats`) that exactly one runtime route fits (`/admin/stats`). The path is now resolved; the source line is kept; `resolved_from` records the expression. Two candidates leave the gap open. | ✓ |
| `runtime_only` | Only the application knows it — usually the dynamic registrations static analysis is blind to. No source location. | `RT` |
| `static_only` | Declared in source but not served — a dead router, an unmounted blueprint. Confidence drops to low. | ∅ |

The differences between the two lists are the informative part, which is why they are
labelled rather than smoothed over. The result is the best of both: exact routes *and*
click-through to source.

Rescanning returns to the plain static result; enrich is re-run on request.

## When you do not need it

- The project is Next.js, Express or Go, where there is no runtime spec to fetch and static
  analysis is the whole story. The button is not offered.
- The project is Django with `drf-spectacular`: its generated schema is richer than the
  resolver walk, and importing that document is the better path today.
- You only need paths and methods, which static analysis gets right in the common case.
- You cannot or would rather not install the project's dependencies.

RouteLogic is fully usable without ever enabling it.

## Trying it

`tests/fixtures/flask`, `tests/fixtures/fastapi` and `tests/fixtures/django` are runnable. Create an environment in
one, install its requirements, open it in RouteLogic, scan, then **Ask the app**:

```bash
cd tests/fixtures/flask
python -m venv .venv && .venv/Scripts/pip install -r requirements.txt   # or .venv/bin/pip
```

The Flask fixture is built to show every label at once: three `/admin` routes whose prefix
is a config value (`gap_filled`), two routes registered in a loop (`runtime_only`), and an
unregistered blueprint (`static_only`).
