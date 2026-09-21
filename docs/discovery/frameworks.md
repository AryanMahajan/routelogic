# Framework support

Support matrix, honest about gaps. "Planned" means designed and scheduled, not implemented.
Every implemented adapter is pinned by a fixture project under `tests/fixtures/<framework>/`
whose snapshot records what is found **and what is expected not to be**.

| Framework | Language | Phase | Status |
|---|---|---|---|
| FastAPI | Python | P3 | Implemented |
| Next.js | TS/JS | P4 | Implemented |
| Express | JS/TS | P4 | Implemented |
| Flask | Python | P5 | Implemented |
| Django / DRF | Python | P6 | Implemented |
| Go — net/http, Gin, Echo, chi, Fiber, gorilla/mux | Go | — | Implemented |

Every adapter shares the same limits, which come from static analysis itself rather than
from any one framework:

- **Nothing is executed.** A prefix read from an environment variable or a settings object
  is shown as an unresolved gap (`/?/items`), never guessed.
- **Routers built inside functions** are found, but the mount that would place them is
  not: they appear as orphans with a warning naming the call that builds them.
- **Handlers are inspected only within the file that registers them.** A controller
  defined elsewhere contributes no parameters. (Two exceptions: Flask's class-based views,
  where a `MethodView` is a router in its own right and its methods are read where the
  class is declared; and Go, where a handler named by a route is looked up by name across
  the package.)
- **Routes registered on a function parameter** — `def register(app): app.add_url_rule(…)`
  — are kept as orphans with a warning, because where that `app` is mounted cannot be
  known.
- **Auth is a name-based heuristic.** A middleware or dependency whose name says `auth`,
  `jwt`, `token`, `protect`, `guard`… is taken as a requirement and reported as
  `Unknown` with that name, so the UI can show what was seen rather than a confident
  claim. It misfires both ways.

---

## FastAPI

**Detected by** `fastapi` in a manifest (weak), plus `from fastapi import` in source
(strong).

**Recognised**

- `app = FastAPI()` and `router = APIRouter(prefix=..., tags=[...])`
- `@app.<method>(...)` and `@router.<method>(...)` for every HTTP method
- `app.include_router(router, prefix=..., tags=..., dependencies=[...])`, nested to any
  depth, across files, through relative and absolute imports — including submodule
  imports (`from .api import admin` then `admin.router`) and re-exports
- The same router mounted at two prefixes yields both paths
- `add_api_route(...)` and `add_route(...)`
- Path parameters with converters, e.g. `{user_id:int}`, `{path:path}`
- Query parameters from defaulted handler arguments and `Query(...)`; headers from
  `Header(...)`
- Request bodies from Pydantic-looking annotations and `Body(...)`
- `Depends(...)` / `Security(...)` on security-looking names as an auth signal, on a route
  or on an `include_router`
- `summary`, `description`, docstrings, `deprecated`, `tags` as the grouping

**Gaps**

- Prefixes read from settings objects: shown unresolved
- Routers built by factory functions: found as orphans, with a warning
- Pydantic model *contents* are not reconstructed — the body is known to be JSON of that
  model, not its fields
- Custom `APIRoute` subclasses that rewrite paths

**Runtime enrich available** — `app.openapi()` supersedes every schema gap above, and the config-derived prefix becomes a resolved path with its source line kept. See [runtime enrich](runtime-enrich.md).

---

## Next.js

**Detected by** `next` in `package.json`, a `next.config.*` file, and — the strongest
signal — route files where Next's conventions put them.

The cheapest adapter, because the path *is* the file system. The syntax tree is only
consulted for which methods a file handles.

**Recognised**

- App Router: `app/**/route.{ts,js,tsx,jsx,mjs}`, also under `src/app/` and in monorepo
  sub-apps (`apps/web/app/`)
- Dynamic `[id]`, catch-all `[...slug]`, optional catch-all `[[...slug]]`
- Route groups `(group)`, parallel slots `@slot` and intercept markers `(.)`, all absent
  from the URL; private folders `_name`, which are not routes at all
- Method handlers in every export spelling: `export async function GET`, `export const
  POST = …`, `export { handler as DELETE }`, `export const { PUT } = …`, re-exports
- What a handler reads: `searchParams.get("q")` → query, `request.headers.get(…)` →
  header, `request.json()` / `formData()` / `text()` → body type
- Pages Router: `pages/api/**` with a default export, `index` files, dynamic file names,
  underscore-prefixed files skipped
- The Pages handler's accepted methods from `req.method === "POST"`, `!==` guards,
  `switch (req.method)` and `["GET","POST"].includes(req.method)`; a handler that never
  checks is listed under GET/POST/PUT/PATCH/DELETE with a summary saying why
- `req.query` / `req.body` / `req.headers` usage in Pages handlers, with dynamic-segment
  names correctly excluded from the query list
- `export default withAuth(handler)`: the wrapped handler is followed, and the wrapper's
  name is an auth hint
- Port from `next dev -p N` in `package.json`, else 3000

**Gaps**

- `basePath` and `rewrites` in `next.config.*` are not applied
- Body *shapes* are not reconstructed, even from zod
- Edge `middleware.ts` is not modelled — it is not an endpoint, and its `matcher` is not
  used to annotate routes

---

## Express

**Detected by** `express` in `package.json` (weak), plus an `express` require or import
(strong).

Structurally the hardest adapter — the module graph is load-bearing, and it is the same
graph FastAPI uses. Express adds CommonJS/ESM export resolution on top.

**Recognised**

- `const app = express()`; `express.Router()`, `Router()`, `new Router()`, with or without
  options
- `app.<method>(path, ...handlers)` and `router.<method>(...)`; `.all` listed under
  GET/POST/PUT/PATCH/DELETE with a summary saying why
- `app.route(path).get(a).post(b)` chains, and `app.get(...).post(...)` chains
- Arrays of paths: `app.get(["/a", "/b"], h)`
- `app.use(prefix, router)`, `app.use(router)`, `app.use(prefix, require("./routes"))`,
  arrays of routers, the same router mounted twice, nesting to any depth
- Module linking in both systems: `require`, destructured `require`,
  `require("./m").name`, `import x from`, `import { a as b }`, `import * as`; exports via
  `module.exports = x`, `module.exports = { a, b: c }`, `exports.a = …`, `export default`,
  `export const`, `export { a as b }`, and `export … from` re-exports; `@/` and `~/`
  aliases tried from `src/` and the root
- Path constants folded from literals, template strings and `+`
- Path parameters `:id`, optional `:id?`, wildcard `*`
- What a handler reads off `req`: `req.query.x` and `const { x } = req.query` → query,
  `req.body.x` → body fields (offered as an example body), `req.headers["x"]` and
  `req.get("X")` → headers, with `Authorization` promoted to an auth requirement
- Auth middleware in a handler chain, ahead of a mounted router (`app.use("/admin",
  requireAuth, adminRouter)` guards everything under it), or applied router-wide with
  `router.use(requireAuth)`
- Package middleware (`cors()`, `helmet`, `express.json()`) recognised as not-a-router
  and ignored quietly
- Port from `PORT=` in `.env*`, `-p`/`--port`/`PORT=` in run scripts, else 3000

**Gaps**

- A router built by a call (`app.use("/api", createRouter())`) cannot be followed:
  the mount is reported with a warning, and the routes inside the factory appear as
  orphans
- `this.router` in class-based controllers: reported, not guessed
- Regex paths and `process.env` prefixes: shown unresolved
- Handlers in other files (`users.list` from a controllers module) contribute no
  parameters
- Request schemas from zod, joi or TypeScript types are not read

---

## Flask

**Detected by** `flask` in the manifest (+1), plus a `from flask import` in source (+3).

Reuses the Python resolver built for FastAPI; a `Blueprint` maps onto the same router
concept. Pinned by `tests/fixtures/flask/` — a runnable application with an app factory,
nested blueprints, a blueprint registered twice, a config-derived prefix, a `MethodView`
in another file, an unregistered blueprint, and routes registered in a loop.

**Recognised**

- `app = Flask(__name__)`, at module level or inside a factory (`def create_app()`)
- `Blueprint(name, __name__, url_prefix=...)`; the name becomes the group
- `@app.route(path, methods=[...])` and `@bp.route(...)`, defaulting to `GET`
- `@app.get/post/put/patch/delete(...)` shortcuts (Flask ≥ 2.0)
- `app.register_blueprint(bp, url_prefix=...)`, including nested `bp.register_blueprint(child)`
- `app.add_url_rule(rule, endpoint, view_func, methods=[...])`
- Class-based views: `MethodView` subclasses registered with `view_func=X.as_view(...)`
  (through a local alias too), one route per `get`/`post`/… method; `methods = [...]`
  narrows them; `decorators = [...]` guards them; `methods=` on the rule narrows that rule
- Flask-RESTful: `Api(app, prefix=...)`, `api.init_app(app)`, `api.add_resource(Cls, *rules)`
- Flask-RESTX: `Namespace(name, path=...)`, `api.add_namespace(ns, path=...)`,
  `@ns.route(...)` on a `Resource` class
- Werkzeug converters — `<int:user_id>`, `<path:subpath>` (catch-all), `<uuid:id>`
- Query parameters from `request.args.get("q")` / `request.args["q"]` (the latter
  required); headers from `request.headers.get(...)`; a JSON body from
  `request.get_json()` / `request.json` with the keys the handler reads as the example;
  a form body from `request.form`; multipart from `request.files`
- Auth from decorators: `@login_required` (session cookie), `@jwt_required()` (bearer),
  `@basic_auth.login_required` (basic), `@token_required` and anything else whose name
  says auth (reported as `Unknown` with the name)
- The handler docstring as the summary

**Prefix semantics, which differ from FastAPI:** `register_blueprint(bp, url_prefix="/x")`
*replaces* the blueprint's own `url_prefix`; only when none is given does the blueprint's
apply. Nested blueprints compose. This is what Werkzeug does, and it was found by comparing
the scan with the fixture's real `url_map` — the adapter says so to the graph through
`MountFact.replaces_child_prefix`.

**Gaps**

- Schemas: Flask has no schema layer, and none is inferred from Marshmallow or Pydantic.
  Runtime enrich does not add any either — `url_map` knows paths and methods only.
- Routes registered in a loop or from data are listed as an unresolved orphan (`GET /?`)
  with a warning, not expanded. Runtime enrich lists them exactly.
- `@bp.before_request` guards are not read as auth.
- Flask-RESTX `@ns.expect(model)` / `@ns.doc(...)` are not read.
- A `view_func` imported from a package outside the project is reported as an
  undeclared mount and yields nothing.

**Runtime enrich available** — walks `app.url_map` for a complete and exact route list,
including the loop-registered and config-prefixed ones above. See
[runtime enrich](runtime-enrich.md).

---

## Django / DRF

**Detected by** `manage.py` (+2), `django` in the manifest (+1), a `from django` import
(+3), and `rest_framework` anywhere (+1, as evidence).

Pinned by `tests/fixtures/django/` — a runnable Django + DRF project with no database:
function views, generic class-based views, a `ViewSet` with `@action`s, a mixin-built
`GenericViewSet`, `include()` of a module, an inline list and a `DefaultRouter`, a
`re_path`, a settings-derived prefix, a URL conf nothing includes, and patterns built by a
list comprehension.

**How it maps onto the graph.** The settings file's `ROOT_URLCONF` is the application
root; every `urlpatterns` list (or any list of `path()` calls) is a router; `include()` is
a mount. Views are routers too — a function or class in `views.py` is a router with one
empty-path route per method it serves, and `path("users/", views.list_users)` is a mount of
it. That is what lets the methods be read where the view is *declared* rather than where
it is routed, across files, through the ordinary import. Views are declared *implicitly*:
one nobody routes to is not reported as an orphan, because it is just a function.

**Recognised**

- `ROOT_URLCONF = "config.urls"` in any settings module
- `urlpatterns = [...]`, `+= [...]`, `= [...] + [...]`, including under `if settings.DEBUG:`;
  any other module-level list of patterns, so `include(auth_patterns)` works
- `path()`, `re_path()` and the old `url()`; converters `<int:pk>`, `<slug:s>`, `<uuid:u>`,
  `<path:p>`; regex named groups `(?P<pk>\d+)` typed from their pattern, unnamed groups
  left unresolved rather than guessed
- `include("app.urls")`, `include(("app.urls", "app"), namespace=)`, `include(patterns)`,
  `include([...])` inline, `include(router.urls)`
- `app_name = "..."` as the group, else the app directory
- Function views: `@require_http_methods([...])`, `@require_GET/POST/safe`,
  `@api_view([...])`, else `request.method == "POST"` checks in the body, else GET
- Class-based views: implemented `get`/`post`/… methods, else the generic base's methods
  (`ListView`, `CreateView`, `ListCreateAPIView`, `RetrieveUpdateDestroyAPIView`, …),
  narrowed by `http_method_names`
- ViewSets: `ModelViewSet`, `ReadOnlyModelViewSet`, mixins, and hand-written
  `list`/`create`/`retrieve`/`update`/`partial_update`/`destroy`; `@action(detail=,
  methods=, url_path=, permission_classes=)`; `lookup_field` / `lookup_url_kwarg`;
  `router.register(prefix, ViewSet)` on a `DefaultRouter`/`SimpleRouter`
- `serializer_class` as the body of POST/PUT/PATCH (named, not described — like a
  Pydantic model in FastAPI)
- Query from `request.GET` / `request.query_params`, headers from `request.headers` and
  `request.META["HTTP_…"]`, body keys from `request.data` / `request.POST`
- Auth from `permission_classes` / `authentication_classes` (`TokenAuthentication`,
  `JWTAuthentication`, `SessionAuthentication`, `BasicAuthentication`, `IsAuthenticated`…),
  `@login_required`, `@permission_classes([...])`, `@method_decorator(login_required)`

**Gaps**

- Patterns built by a comprehension or a loop are not expanded. Runtime enrich lists them.
- `admin.site.urls` and `include("django.contrib…")` are skipped silently: Django's own.
- DRF's format-suffix twins (`users.json`) and the router's API root are not listed —
  neither statically nor at runtime, on purpose.
- Serializer *fields* are not read; a serializer names the body. Runtime enrich does not
  add them either: Django has no schema unless `drf-spectacular` is installed, which is
  not consulted.
- Two settings modules naming different `ROOT_URLCONF`s both become roots.

**Runtime enrich available** — sets `DJANGO_SETTINGS_MODULE`, calls `django.setup()`, and
walks the URL resolver: exact paths, ViewSet action maps, class-based view methods,
`require_http_methods` read through its closure. A plain function view is listed as GET
with `x-methods-unknown`, so the static scan's `POST` on it survives the merge rather than
being marked "not served".

---

## Go

**Detected by** `github.com/gin-gonic/gin`, `github.com/labstack/echo`,
`github.com/go-chi/chi`, `github.com/gofiber/fiber` or `github.com/gorilla/mux` in any
`go.mod` (+1 each) or imported in source (+3 each), and a `http.HandleFunc` /
`http.Handle` / `http.NewServeMux` call anywhere (+3), since every Go program imports
`net/http`. Reported as one framework, `go`, with the evidence naming which of the six.

Pinned by two fixtures. `tests/fixtures/go/` is a Gin shop in the usual layout — `main`
builds the engine and the groups, each `internal/<area>` package registers on the group
it is handed, handlers in a file of their own — with a package mounted under two
prefixes, a `Register` nobody calls, a prefix read from configuration, and a route
registered with a method held in a variable. `tests/fixtures/go-chi/` is a chi notes API:
a `Server` type serving its router from `ServeHTTP`, resources returning routers from
`Routes()`, `Route` closures, `Mount`s that reach another package through a constructor,
a Go 1.22 `ServeMux` behind `http.StripPrefix`, a `ServeMux` nothing serves, a prefix
from the environment, and a table-driven registration.

**How it maps onto the graph.** One adapter for all six, because a project mixes them
and they differ only in method names. A Go name is scoped to the *package*, so the graph
resolves a reference through the directory: `r` in `routes.go` is the `r` declared in
`main.go` next to it, and `users.Register` is `Register` wherever `users` was imported
from (through `go.mod`'s module path — one per service in a monorepo). A function whose
parameter is a router, or which returns one it built, *is* a router; `Register(v1)` and
`r.Mount("/x", Routes())` are its mounts. A constructor stands for the type it returns
and a type for the router its `ServeHTTP` delegates to, so
`http.ListenAndServe(":3000", api.NewServer(db))` is followed to `Server.router`. A
router built with `chi.NewRouter()` or `http.NewServeMux()` is a root only once
something serves it; one built with `gin.Default()`, `echo.New()` or `fiber.New()` is a
root by construction, unless it is mounted into another. Handlers are recorded by name
where they are declared and joined to routes afterwards, so `routes.go` + `handler.go`
works.

**Recognised**

- Constructors: `gin.Default()`, `gin.New()`, `echo.New()`, `chi.NewRouter()`,
  `chi.NewMux()`, `fiber.New()`, `mux.NewRouter()`, `http.NewServeMux()`; the default mux
  through `http.HandleFunc` / `http.Handle`; `var r = …` at package level or `:=` in a
  function; `s.router = …` and `&Server{router: …}` on a type
- Routes: `GET`/`POST`/… (Gin, Echo, Fiber), `Get`/`Post`/… (chi, Fiber), `Any`, `All`,
  `Handle("GET", path, h)` (Gin), `Handle(path, h)` and `HandleFunc(path, h)` (net/http,
  chi, gorilla), `Method`/`MethodFunc` (chi), `Add` (Echo, Fiber), `Match([]string{…},
  path, h)` (Echo); gorilla chains `HandleFunc(p, h).Methods(…)`,
  `Path(p).Queries(…).HandlerFunc(h)`, `Methods(…).Path(p).Handler(h)`
- Groups and mounts: `Group(prefix, mw…)` assigned, inline (`r.Group("/v1").GET(…)`) or
  as an argument (`Register(r.Group("/users"))`); chi's `Route(prefix, func(r chi.Router))`
  and `Group(func(r chi.Router))`, Fiber's `Route`; `With(mw)`; `Mount(prefix, x)`;
  Fiber's `Use(prefix, sub)` and `Use(sub)`; `PathPrefix(p).Subrouter()` and
  `Methods(…).Subrouter()`; `Handle("/api/", http.StripPrefix("/api", mux))`; the same
  router mounted at two prefixes
- Roots: `r.Run()`, `e.Start()`, `app.Listen()`, `app.Listener()`,
  `http.ListenAndServe(addr, r)`, `http.Serve(l, r)`, `&http.Server{Handler: r}`,
  `srv.Handler = r`, and a serve call into outside code (`endless.ListenAndServe(":8080",
  r)`, Echo v5's `StartConfig.Start(ctx, e)`)
- Paths in every syntax: `{id}`, `{id:[0-9]+}` (regex dropped), `{path...}`, `{$}`,
  `:id`, `:id?`, `:id<int>`, `*`, `*name`, `+`; net/http's `"GET /items/{id}"` and
  host-rooted patterns; constants folded through `const`, `var` and `+`; `""` and `"/"`
  under a Gin group kept as two routes
- Bodies: `ShouldBindJSON(&x)`, `BindJSON`, `ShouldBind`, `Bind`, `BodyParser`,
  `json.NewDecoder(r.Body).Decode(&x)`, `render.DecodeJSON`, `json.Unmarshal` — the
  struct named by `x`, with its `json:"…"` tags as fields (`json:"-"` and unexported
  fields left out, `binding:"required"` / `validate:"required"` as required), embedded
  structs flattened, `time.Time`, `uuid.UUID`, slices and maps typed; `PostForm` /
  `FormValue` as a form body, `FormFile` as multipart
- Query from `c.Query`, `DefaultQuery`, `QueryParam`, `r.URL.Query().Get`, a `q :=
  r.URL.Query()` local, `ShouldBindQuery(&x)` through the struct's `form` tags;
  headers from `GetHeader`, `r.Header.Get`, Fiber's `c.Get("X-…")`; `Authorization`
  promoted to an auth requirement
- A route registered without a method (`HandleFunc`, `Handle`, chi's `HandleFunc`,
  gorilla without `Methods`) keeps the methods its handler checks `r.Method` against
  (`==`, `!=`, `switch`), else is listed under GET/POST/PUT/PATCH/DELETE with a summary
- Auth from middleware names in the chain, on a group, in `With(…)`, in `Use(…)` for the
  routes after it, or on a mount: `AuthRequired()`, `middleware.JWT(…)`,
  `jwtauth.Verifier(…)`, `middleware.BasicAuth(…)`, `gin.BasicAuth(…)`
- Groups for the tree from the file (`users.go`, or `routes/users/router.go`'s
  directory), the group's prefix (`/api/v1/users` → `users`), or the function
  (`RegisterUserRoutes`, `usersResource.Routes`)
- The port from the listen address in source — `":8080"`, `"0.0.0.0:8080"`, a constant,
  `http.Server{Addr:}`, `echo.StartConfig{Address:}` — else Gin's 8080 on a bare `Run()`,
  else 8080

**Gaps**

- A path or prefix from `os.Getenv`, `viper`, a config struct or `fmt.Sprintf` is shown
  unresolved. A method held in a variable — `Handle(method, path, h)` — is skipped.
  Registrations in a loop over a table are missed; over a slice of literals they are
  listed with the path unresolved.
- A router reached only through an interface, dependency injection (`fx`, `wire`,
  `parsley`) or a cloud-function adaptor is reported as never mounted: the routes are
  listed and flagged, the wiring is not followed.
- Handlers are found by name in the route's own package, then anywhere the name is
  unique; a handler behind an interface or a method value on an unknown type contributes
  nothing.
- `Static`, `StaticFS`, `http.FileServer` are not listed.
- The default `ServeMux` is taken as served; `http.HandleFunc` in a file that never
  calls `http.ListenAndServe(…, nil)` still lists its routes.
- Struct fields of types from other packages are named, not described; an `any` field
  is `null`.
- Echo's `c.Bind` reads query and path as well as the body; it is taken as a body.
