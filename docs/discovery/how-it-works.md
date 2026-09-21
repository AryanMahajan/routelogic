# How discovery works

Discovery reads your source and produces `EndpointSpec`s. It does not execute your code.

## The problem

The easy case is trivial:

```python
@app.get("/users/{user_id}")
async def get_user(user_id: int): ...
```

Real projects almost never look like that. They split routes across files and mount them
with prefixes somewhere else entirely:

```python
# api/users.py
router = APIRouter(prefix="/users", tags=["users"])

@router.get("/{user_id}", response_model=UserOut)
async def get_user(user_id: int): ...

# main.py
app.include_router(router, prefix="/api/v1")
```

```js
// routes/users.js
const router = express.Router();
router.get('/:id', handler);
module.exports = router;

// app.js
app.use('/api/users', require('./routes/users'));
```

```go
// internal/users/routes.go
func Register(rg *gin.RouterGroup) {
	rg.GET("/:id", get)
}

// cmd/server/main.go
users.Register(r.Group("/api/v1").Group("/users"))
```

In every case the real path — `/api/v1/users/{user_id}`, `/api/users/:id`,
`/api/v1/users/{id}` — exists in no single file. Producing it requires linking a router *declaration* in one module to its
*mount* in another.

That linking is the actual engineering problem in discovery. Everything else is comparatively
mechanical.

## The registration graph

RouteLogic solves it once, not once per framework. Framework adapters do not resolve paths;
they only recognise three kinds of fact:

- *this expression creates a router*
- *this call registers a route on a router*
- *this call mounts a router onto another router, with a prefix*

Those facts become a graph:

```
Module ──declares──▶ RouterSymbol ──has──▶ RouteRegistration
                          │
                          └──mounted at "/api/v1"──▶ RouterSymbol (app root)
```

A resolver then walks mount edges from each application root, composing prefixes as it
descends, and emits one `EndpointSpec` per reachable registration.

This is what keeps adapters small and makes design principle #6 real: adding a framework
means teaching RouteLogic to recognise that framework's spelling of those three facts, not
reimplementing prefix resolution.

### Cases the resolver must handle

| Case | Behaviour |
|---|---|
| One router mounted at two prefixes | Emits endpoints for **both** paths |
| Router declared but never mounted | Emitted as an **orphan**, flagged, not dropped |
| Mount cycle | Cycle broken, warning recorded |
| Router mounted onto another router, several deep | Prefixes compose in order |
| Framework with no routers (Next.js) | Adapter emits registrations directly; graph is flat |
| Routes registered on a function's parameter, or on a router it returns (Go) | The function is the router; the call that hands it one is the mount |
| Names scoped to a package rather than a file (Go) | References resolve through the directory, then through an imported package |

Orphans matter: a router that is not mounted is very often a bug in the project, and telling
the developer about it is useful information rather than noise.

## Pipeline

```
  project root
       │
  1. ProjectDetector        language + project roots (monorepo aware)
       │
  2. FrameworkDetector      which frameworks, scored
       │
  3. SourceIndex            tree-sitter parse, cached by content hash
       │
  4. RegistrationGraph      adapters emit facts; resolver composes paths
       │
  5. ConstantResolver       fold string constants, or mark Unresolved
       │
  6. SchemaExtractor        params, body schema, auth requirement
       │
  7. BaseUrlInference       candidate hosts and ports
       │
  ▼
  EndpointSpec[]
```

### 1. Project detection

Walks the selected root honouring `.gitignore`, looking for markers: `pyproject.toml`,
`requirements.txt`, `setup.py`, `package.json`, `next.config.*`, `manage.py`. A monorepo may
produce several project roots, each analysed independently.

### 2. Framework detection

Scores each candidate framework using declared dependencies plus actual import statements —
a dependency in a lockfile is weak evidence, an `from fastapi import FastAPI` is strong. More
than one framework can match a single project, and that is not an error.

### 3. Source index

Candidate files are parsed with tree-sitter and the results cached in SQLite, keyed by path,
mtime, and content hash. A rescan only reparses what changed, which is what makes the
watch-and-refresh experience viable.

**Why tree-sitter** rather than `swc`/`oxc` for JS and `ruff`/`rustpython` for Python: one
uniform API across Python, JavaScript, TypeScript, TSX and Go; error-tolerant parsing, so a file
that does not currently compile still yields routes; and a query language that lets route
patterns be written declaratively instead of as hand-rolled visitors. The trade-off is that
tree-sitter provides no name resolution — but that would have been hand-written under any of
these options, since none of them are type checkers.

### 5. Constant resolution

Best-effort folding of string constants:

```python
API_PREFIX = "/api/v1"
app.include_router(router, prefix=API_PREFIX)     # ✓ resolves
```

```python
app.include_router(router, prefix=settings.API_PREFIX)   # ✗ Unresolved
```

Handled: string literals, simple concatenation, f-strings with resolvable parts, module-level
constants within a file, and simple direct-string constants imported across files.

Not handled: anything requiring evaluation — attribute access on config objects, function
calls, environment lookups, computed values. These become
`PathSegment::Unresolved { expr }` and are shown as gaps in the UI.

### 6. Schema extraction

Per-framework and explicitly best-effort:

- **Python** — type hints on handler parameters, Pydantic models to JSON Schema,
  `response_model`, dependency injection as an auth signal.
- **TypeScript** — interfaces and type aliases, zod schemas where recognisable, generic
  parameters on request/response types.
- **JavaScript** — JSDoc annotations where present; otherwise minimal.

Where extraction fails, the endpoint is still emitted — with the schema absent rather than
invented. [Runtime enrich](runtime-enrich.md) is the fix for projects where this matters.

### 7. Base URL inference

Discovered routes have no host, and "zero configuration" has to survive that. RouteLogic
collects candidates from:

- `uvicorn` / `gunicorn` / `flask run` arguments in scripts, Procfiles, and Makefiles
- `scripts` entries in `package.json`
- `PORT` and similar keys in `.env` files
- `EXPOSE` in a Dockerfile
- published `ports` in `docker-compose.yml`

Candidates are offered ranked; you pick one or type your own, and the choice is stored per
environment.

## What discovery cannot do

Stated plainly, because pretending otherwise makes the tool untrustworthy:

- **Dynamically registered routes.** Routes built in a loop, from a config file, or by a
  factory function are usually invisible to static analysis.
- **Computed paths.** Marked unresolved rather than guessed.
- **Precise schemas** in dynamically typed code with no annotations.
- **Runtime middleware effects** — a global prefix applied by middleware rather than by
  mounting may be missed.

[Runtime enrich](runtime-enrich.md) closes most of these by asking the running application
directly, at the cost of executing project code. It is always opt-in.
