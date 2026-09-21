//! The route registration graph.
//!
//! Every framework except a file-system-routed one has the same shape: a router declared in
//! one file, mounted with a prefix in another. The real path exists in neither file alone.
//!
//! ```text
//! Module ──declares──▶ RouterSymbol ──has──▶ RouteRegistration
//!                           │
//!                           └──mounted at "/api/v1"──▶ RouterSymbol (app root)
//! ```
//!
//! Composing those prefixes is written here, once, rather than in each adapter. That is what
//! keeps adapters small enough to add cheaply.

use crate::facts::{
    ExportFact, FactSink, ImportFact, MountFact, RouteFact, RouterFact, SymbolId, SymbolRef,
};
use rl_model::PathTemplate;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Guards against a mount chain that never terminates.
const MAX_DEPTH: usize = 32;

/// Guards against an import that re-exports an import that re-exports an import.
const MAX_ALIAS_HOPS: usize = 16;

/// Something the graph could not do, worth telling the developer about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphWarning {
    /// A `include_router(x)` whose `x` could not be traced to a declaration.
    UnresolvedReference { module: PathBuf, name: String },
    /// A mounted name that is neither declared nor imported from inside the project — a
    /// router from an installed package, or the result of a call. Nothing can be listed
    /// under it, and that is worth knowing.
    UndeclaredMount { module: PathBuf, name: String },
    /// Routes were registered on a name that is not a declared router — usually a function
    /// parameter, as in `def register(app): app.add_url_rule(...)`. They are listed as
    /// orphans, because where they end up mounted cannot be known statically.
    UndeclaredRouter { symbol: String, routes: usize },
    /// A router was declared but never mounted, so its routes may be unreachable.
    ///
    /// Kept and reported rather than dropped: this is usually a bug in the project being
    /// inspected, and saying so is more useful than staying silent.
    OrphanedRouter { symbol: String },
    /// Mount edges formed a loop.
    Cycle { symbol: String },
    /// Resolution stopped at [`MAX_DEPTH`].
    TooDeep { symbol: String },
}

impl std::fmt::Display for GraphWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GraphWarning::UnresolvedReference { module, name } => write!(
                f,
                "could not trace `{name}` in {} to a router declaration",
                crate::project::display(module)
            ),
            GraphWarning::UndeclaredMount { module, name } => write!(
                f,
                "`{name}` is mounted in {} but declared nowhere in the project, so nothing is listed under it",
                crate::project::display(module)
            ),
            GraphWarning::UndeclaredRouter { symbol, routes } => write!(
                f,
                "{routes} route(s) are registered on `{symbol}`, which is not declared there — probably a parameter; their full paths are unknown"
            ),
            GraphWarning::OrphanedRouter { symbol } => {
                write!(
                    f,
                    "router `{symbol}` is never mounted, so its routes may be unreachable"
                )
            }
            GraphWarning::Cycle { symbol } => {
                write!(f, "mount cycle involving `{symbol}`")
            }
            GraphWarning::TooDeep { symbol } => {
                write!(f, "mount chain from `{symbol}` is deeper than {MAX_DEPTH}")
            }
        }
    }
}

/// One route, with its path fully composed.
#[derive(Debug, Clone)]
pub struct ResolvedRoute<'a> {
    pub method: rl_model::HttpMethod,
    /// Mount prefixes, router prefix, and route path, joined.
    pub path: PathTemplate,
    pub fact: &'a RouteFact,
    /// Nearest group: the mount's, then the router's, then the route's own.
    pub group: Option<String>,
    /// The route's own auth, or the nearest mount's.
    pub auth: Option<rl_model::AuthRequirement>,
    /// Reached from no application root.
    pub orphaned: bool,
}

/// Routes and everything the graph could not do.
#[derive(Debug)]
pub struct Resolution<'a> {
    pub routes: Vec<ResolvedRoute<'a>>,
    pub warnings: Vec<GraphWarning>,
}

/// The assembled graph.
#[derive(Debug, Default)]
pub struct RegistrationGraph {
    routers: BTreeMap<SymbolId, RouterFact>,
    routes: Vec<RouteFact>,
    mounts: Vec<MountFact>,
    /// (module, local name) → what it refers to.
    import_bindings: BTreeMap<(PathBuf, String), Binding>,
    /// (module, exported name) → local name.
    exports: BTreeMap<(PathBuf, String), String>,
    /// Go scopes a name to the package — the directory — not the file, so a router
    /// declared in `routes.go` is `r` in `main.go` next to it. (directory, name) → symbol.
    package_routers: BTreeMap<(PathBuf, String), SymbolId>,
    /// Go aliases, by package: `NewHandler` stands for the `Handler` it returns, so
    /// `NewHandler.Routes` is `Handler.Routes`. (directory, alias) → name.
    package_exports: BTreeMap<(PathBuf, String), String>,
}

/// What an imported name points at.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Binding {
    /// `from .users import router` — a name inside another module.
    Symbol(SymbolId),
    /// `import api.users as users` — the module itself, so `users.router` can be resolved.
    Module(PathBuf),
}

impl RegistrationGraph {
    /// Build the graph, resolving imports against the set of files actually in the project.
    pub fn build(sink: FactSink, project_files: &BTreeSet<PathBuf>) -> RegistrationGraph {
        let known = |path: &Path| project_files.contains(path);

        let mut routers = BTreeMap::new();
        for fact in sink.routers {
            // A module may re-run a declaration (a factory called twice); the first wins so
            // the recorded source location is the declaration site.
            routers.entry(fact.symbol.clone()).or_insert(fact);
        }

        let import_bindings = resolve_imports(&sink.imports, &known);
        let exports = index_exports(&sink.exports);

        let is_go = |module: &Path| {
            crate::project::Language::of(module) == Some(crate::project::Language::Go)
        };
        let package_of = |module: &Path| module.parent().map(Path::to_path_buf).unwrap_or_default();
        let package_routers = routers
            .keys()
            .filter(|symbol| is_go(&symbol.module))
            .map(|symbol| {
                (
                    (package_of(&symbol.module), symbol.name.clone()),
                    symbol.clone(),
                )
            })
            .collect();
        let package_exports = sink
            .exports
            .iter()
            .filter(|export| is_go(&export.module))
            .map(|export| {
                (
                    (package_of(&export.module), export.exported.clone()),
                    export.local.clone(),
                )
            })
            .collect();

        // Two settings files naming the same `ROOT_URLCONF`, or `app.use` called twice
        // with the same arguments, would otherwise walk the child twice and list every
        // route under it twice.
        let mut seen = BTreeSet::new();
        let mounts: Vec<MountFact> = sink
            .mounts
            .into_iter()
            .filter(|m| {
                seen.insert((
                    m.parent.clone(),
                    m.child.clone(),
                    m.prefix.render(rl_model::ParamStyle::Braces),
                    m.methods.clone(),
                    m.replaces_child_prefix,
                ))
            })
            .collect();

        RegistrationGraph {
            routers,
            routes: sink.routes,
            mounts,
            import_bindings,
            exports,
            package_routers,
            package_exports,
        }
    }

    pub fn router_count(&self) -> usize {
        self.routers.len()
    }

    /// The application objects — where resolution starts, and what runtime enrich imports.
    pub fn app_roots(&self) -> impl Iterator<Item = &RouterFact> {
        self.routers.values().filter(|r| r.is_app_root)
    }

    /// Trace a reference back to the symbol it names.
    fn resolve_symbol(&self, reference: &SymbolRef) -> Option<SymbolId> {
        if crate::project::Language::of(&reference.module) == Some(crate::project::Language::Go) {
            return Some(self.resolve_go_symbol(reference));
        }

        let (qualifier, attribute) = reference.split();

        match qualifier {
            // `users.router`: the qualifier names a module, so the attribute lives there.
            Some(qualifier) => {
                let key = (reference.module.clone(), qualifier.to_string());
                match self.import_bindings.get(&key) {
                    Some(Binding::Module(module)) => {
                        Some(self.canonical(SymbolId::new(module.clone(), attribute)))
                    }
                    // `const routes = require("./routes")` then `routes.users`: the
                    // qualifier is a module's default export, and in CommonJS that *is*
                    // the module, so the attribute is one of its named exports.
                    Some(Binding::Symbol(symbol)) if symbol.name == "default" => {
                        let key = (symbol.module.clone(), attribute.to_string());
                        self.exports.contains_key(&key).then(|| {
                            self.canonical(SymbolId::new(symbol.module.clone(), attribute))
                        })
                    }
                    // A qualifier bound to some other *value* — `from .models import user`
                    // then `user.router`. Reading an attribute off a value needs
                    // evaluation, so this is reported rather than guessed at.
                    Some(Binding::Symbol(_)) => None,
                    None => None,
                }
            }

            None => {
                // Declared right here?
                let local = SymbolId::new(reference.module.clone(), attribute);
                if self.routers.contains_key(&local) {
                    return Some(local);
                }
                // Imported by name?
                let key = (reference.module.clone(), attribute.to_string());
                match self.import_bindings.get(&key) {
                    Some(Binding::Symbol(symbol)) => Some(self.canonical(symbol.clone())),
                    Some(Binding::Module(_)) => None,
                    // Not declared and not imported. Still return the local id: an app root
                    // such as `app = FastAPI()` is a symbol we know about even when no
                    // RouterFact was recorded for it.
                    None => Some(local),
                }
            }
        }
    }

    /// A Go reference: the file, then the package, then an imported package.
    ///
    /// `r` is whatever file of the package declares it. `routes.Register` is `Register` in
    /// the package `routes` was imported as. `NewHandler.Routes` follows the alias the
    /// adapter recorded for the constructor, `Server` the one for a type that serves its
    /// router. What resolves nowhere keeps its local id, so routes on it are reported as
    /// registered on an undeclared router rather than lost.
    fn resolve_go_symbol(&self, reference: &SymbolRef) -> SymbolId {
        let file = &reference.module;
        let local = SymbolId::new(file.clone(), reference.name.clone());
        if self.routers.contains_key(&local) {
            return local;
        }

        let mut package = file.parent().map(Path::to_path_buf).unwrap_or_default();
        let mut parts: Vec<&str> = reference.name.split('.').collect();
        if parts.len() > 1 {
            let key = (file.clone(), parts[0].to_string());
            if let Some(Binding::Module(module)) = self.import_bindings.get(&key) {
                package = module.clone();
                parts.remove(0);
            }
        }

        for _ in 0..MAX_ALIAS_HOPS {
            let name = parts.join(".");
            if let Some(symbol) = self.package_routers.get(&(package.clone(), name)) {
                return symbol.clone();
            }
            match self
                .package_exports
                .get(&(package.clone(), parts[0].to_string()))
            {
                Some(alias) if alias != parts[0] => {
                    let mut next: Vec<&str> = alias.split('.').collect();
                    next.extend_from_slice(&parts[1..]);
                    parts = next;
                }
                _ => break,
            }
        }

        local
    }

    /// Follow exports and re-exports until a declared router — or a dead end.
    ///
    /// `require("./routes/users")` binds `default`; `module.exports = router` says `default`
    /// is `router`; and `router` may itself be an import from a third file. Each hop is one
    /// lookup, and the chain is bounded so a circular re-export cannot spin.
    fn canonical(&self, mut symbol: SymbolId) -> SymbolId {
        for _ in 0..MAX_ALIAS_HOPS {
            if self.routers.contains_key(&symbol) {
                return symbol;
            }

            let key = (symbol.module.clone(), symbol.name.clone());
            if let Some(local) = self.exports.get(&key) {
                if *local != symbol.name {
                    symbol = SymbolId::new(symbol.module, local.clone());
                    continue;
                }
            }

            match self.import_bindings.get(&key) {
                Some(Binding::Symbol(target)) => symbol = target.clone(),
                // `import * as routes` re-exported: treat as the module's default.
                Some(Binding::Module(module)) => symbol = SymbolId::new(module.clone(), "default"),
                None => return symbol,
            }
        }
        symbol
    }

    /// Compose every route's full path.
    ///
    /// Walks from each application root, then sweeps up whatever was never reached.
    pub fn resolve(&self) -> Resolution<'_> {
        let mut warnings = Vec::new();
        // Index routes and mounts by the symbol they attach to, so the walk is linear.
        let mut routes_by_router: BTreeMap<SymbolId, Vec<usize>> = BTreeMap::new();
        for (index, route) in self.routes.iter().enumerate() {
            match self.resolve_symbol(&route.router) {
                Some(symbol) => routes_by_router.entry(symbol).or_default().push(index),
                None => warnings.push(GraphWarning::UnresolvedReference {
                    module: route.router.module.clone(),
                    name: route.router.name.clone(),
                }),
            }
        }

        let mut mounts_by_parent: BTreeMap<SymbolId, Vec<(SymbolId, &MountFact)>> = BTreeMap::new();
        for mount in &self.mounts {
            let (Some(parent), Some(child)) = (
                self.resolve_symbol(&mount.parent),
                self.resolve_symbol(&mount.child),
            ) else {
                warnings.push(GraphWarning::UnresolvedReference {
                    module: mount.child.module.clone(),
                    name: mount.child.name.clone(),
                });
                continue;
            };
            if !self.routers.contains_key(&child) && !routes_by_router.contains_key(&child) {
                warnings.push(GraphWarning::UndeclaredMount {
                    module: mount.child.module.clone(),
                    name: mount.child.name.clone(),
                });
                continue;
            }
            mounts_by_parent
                .entry(parent)
                .or_default()
                .push((child, mount));
        }

        let roots: Vec<SymbolId> = self
            .routers
            .values()
            .filter(|r| r.is_app_root)
            .map(|r| r.symbol.clone())
            .collect();

        let mut resolved = Vec::new();
        let mut reached: BTreeSet<SymbolId> = BTreeSet::new();

        for root in &roots {
            let mut stack = Vec::new();
            self.walk(
                root,
                &PathTemplate::empty(),
                None,
                None,
                &[],
                false,
                &routes_by_router,
                &mounts_by_parent,
                &mut stack,
                &mut reached,
                &mut resolved,
                &mut warnings,
                false,
            );
        }

        // Anything never reached from a root. Reported, not dropped.
        let unreached: Vec<SymbolId> = self
            .routers
            .keys()
            .chain(routes_by_router.keys())
            .filter(|symbol| !reached.contains(*symbol))
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();

        for symbol in unreached {
            // An orphan mounted under an earlier orphan was listed by that walk already.
            if reached.contains(&symbol) {
                continue;
            }
            if let Some(router) = self.routers.get(&symbol) {
                if router.implicit {
                    // A view nobody routed to is not an API; it is just a function.
                    continue;
                }
                warnings.push(GraphWarning::OrphanedRouter {
                    symbol: symbol.to_string(),
                });
            } else {
                warnings.push(GraphWarning::UndeclaredRouter {
                    symbol: symbol.to_string(),
                    routes: routes_by_router.get(&symbol).map_or(0, Vec::len),
                });
            }
            let mut stack = Vec::new();
            self.walk(
                &symbol,
                &PathTemplate::empty(),
                None,
                None,
                &[],
                false,
                &routes_by_router,
                &mounts_by_parent,
                &mut stack,
                &mut reached,
                &mut resolved,
                &mut warnings,
                true,
            );
        }

        Resolution {
            routes: resolved,
            warnings,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn walk<'a>(
        &'a self,
        symbol: &SymbolId,
        prefix: &PathTemplate,
        group: Option<&str>,
        auth: Option<&rl_model::AuthRequirement>,
        allowed_methods: &[rl_model::HttpMethod],
        skip_own_prefix: bool,
        routes_by_router: &BTreeMap<SymbolId, Vec<usize>>,
        mounts_by_parent: &BTreeMap<SymbolId, Vec<(SymbolId, &MountFact)>>,
        stack: &mut Vec<SymbolId>,
        reached: &mut BTreeSet<SymbolId>,
        out: &mut Vec<ResolvedRoute<'a>>,
        warnings: &mut Vec<GraphWarning>,
        orphaned: bool,
    ) {
        // The stack, not a global visited set: a router legitimately mounted at two prefixes
        // must be walked twice and produce both paths.
        if stack.contains(symbol) {
            warnings.push(GraphWarning::Cycle {
                symbol: symbol.to_string(),
            });
            return;
        }
        if stack.len() >= MAX_DEPTH {
            warnings.push(GraphWarning::TooDeep {
                symbol: symbol.to_string(),
            });
            return;
        }

        reached.insert(symbol.clone());
        stack.push(symbol.clone());

        let declared = self.routers.get(symbol);
        let own_prefix = declared.map(|r| r.prefix.clone()).unwrap_or_default();
        let base = if skip_own_prefix {
            prefix.clone()
        } else {
            prefix.join(&own_prefix)
        };
        let group = declared.and_then(|r| r.group.as_deref()).or(group);

        for index in routes_by_router.get(symbol).into_iter().flatten() {
            let fact = &self.routes[*index];
            let full = base.join(&fact.path);
            for method in &fact.methods {
                if !allowed_methods.is_empty() && !allowed_methods.contains(method) {
                    continue;
                }
                out.push(ResolvedRoute {
                    method: method.clone(),
                    path: full.clone(),
                    fact,
                    group: fact.group.clone().or_else(|| group.map(str::to_string)),
                    auth: fact.auth.clone().or_else(|| auth.cloned()),
                    orphaned,
                });
            }
        }

        for (child, mount) in mounts_by_parent.get(symbol).into_iter().flatten() {
            let mounted_at = base.join(&mount.prefix);
            let child_group = mount.group.as_deref().or(group);
            // The nearest mount's auth wins, so a public sub-router under a guarded one
            // still shows the guard it actually sits behind.
            let child_auth = mount.auth.as_ref().or(auth);
            let child_methods = if mount.methods.is_empty() {
                allowed_methods
            } else {
                &mount.methods
            };
            self.walk(
                child,
                &mounted_at,
                child_group,
                child_auth,
                child_methods,
                mount.replaces_child_prefix,
                routes_by_router,
                mounts_by_parent,
                stack,
                reached,
                out,
                warnings,
                orphaned,
            );
        }

        stack.pop();
    }
}

/// Turn import statements into name bindings.
fn resolve_imports(
    imports: &[ImportFact],
    known: &dyn Fn(&Path) -> bool,
) -> BTreeMap<(PathBuf, String), Binding> {
    let mut bindings = BTreeMap::new();

    for import in imports {
        let is_python =
            crate::project::Language::of(&import.module) == Some(crate::project::Language::Python);

        // `from .api import admin` looks like a symbol import but usually names a
        // *submodule*, so `admin.router` means `api/admin.py`'s `router`. Try that first —
        // before the package itself, which a namespace package without `__init__.py`
        // never resolves to — and fall back to a symbol when no such file exists.
        let submodule = match &import.original {
            Some(name) if is_python => {
                let nested = if import.source.is_empty() {
                    name.clone()
                } else {
                    format!("{}.{}", import.source, name)
                };
                crate::facts::resolve_python_module(&import.module, &nested, import.level, known)
            }
            _ => None,
        };
        if let Some(submodule) = submodule {
            bindings.insert(
                (import.module.clone(), import.local_name.clone()),
                Binding::Module(submodule),
            );
            continue;
        }

        let Some(module) =
            crate::facts::resolve_module(&import.module, &import.source, import.level, known)
        else {
            // An import of something outside the project — `fastapi` itself, say. Not an
            // error; there is simply nothing in the project to link it to.
            continue;
        };

        let binding = match &import.original {
            Some(name) => Binding::Symbol(SymbolId::new(module, name)),
            None => Binding::Module(module),
        };

        bindings.insert((import.module.clone(), import.local_name.clone()), binding);
    }

    bindings
}

fn index_exports(exports: &[ExportFact]) -> BTreeMap<(PathBuf, String), String> {
    exports
        .iter()
        .map(|e| ((e.module.clone(), e.exported.clone()), e.local.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{MountFact, RouteFact, RouterFact, Span};
    use rl_model::{HttpMethod, ParamStyle};

    fn files(paths: &[&str]) -> BTreeSet<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    fn path(raw: &str) -> PathTemplate {
        PathTemplate::parse(raw, ParamStyle::Braces)
    }

    fn app_root(module: &str, name: &str) -> RouterFact {
        RouterFact {
            symbol: SymbolId::new(module, name),
            prefix: PathTemplate::empty(),
            group: None,
            is_app_root: true,
            factory: None,
            implicit: false,
            span: Span::new(1, 1),
        }
    }

    fn router(module: &str, name: &str, prefix: &str, group: Option<&str>) -> RouterFact {
        RouterFact {
            symbol: SymbolId::new(module, name),
            prefix: path(prefix),
            group: group.map(str::to_string),
            is_app_root: false,
            factory: None,
            implicit: false,
            span: Span::new(1, 1),
        }
    }

    fn route(module: &str, router: &str, method: HttpMethod, p: &str) -> RouteFact {
        RouteFact::new(
            SymbolRef::new(module, router),
            method,
            path(p),
            Span::new(1, 1),
        )
    }

    fn mount(module: &str, parent: &str, child: &str, prefix: &str) -> MountFact {
        MountFact {
            parent: SymbolRef::new(module, parent),
            child: SymbolRef::new(module, child),
            prefix: path(prefix),
            group: None,
            auth: None,
            methods: Vec::new(),
            replaces_child_prefix: false,
            span: Span::new(1, 1),
        }
    }

    /// A JavaScript-style import: the source is a relative specifier, resolved against
    /// the importing module's directory.
    fn import(module: &str, local: &str, source: &str, original: Option<&str>) -> ImportFact {
        ImportFact {
            module: PathBuf::from(module),
            local_name: local.into(),
            source: format!("./{source}"),
            original: original.map(str::to_string),
            level: 0,
        }
    }

    #[test]
    fn a_mount_with_auth_guards_everything_under_it() {
        let mut sink = FactSink::new();
        sink.router(app_root("app.js", "app"));
        sink.router(router("admin.js", "router", "", None));
        sink.router(router("reports.js", "router", "", None));
        sink.route(route("admin.js", "router", HttpMethod::Get, "/stats"));
        sink.route(route("reports.js", "router", HttpMethod::Get, "/daily"));
        let mut own = route("admin.js", "router", HttpMethod::Post, "/rotate");
        own.auth = Some(rl_model::AuthRequirement::Basic);
        sink.route(own);

        let mut guarded = mount("app.js", "app", "admin_router", "/admin");
        guarded.auth = Some(rl_model::AuthRequirement::Unknown {
            hint: "requireAuth".into(),
        });
        sink.mount(guarded);
        sink.mount(mount("admin.js", "router", "reports_router", "/reports"));
        // `const admin_router = require("./admin")` with `module.exports = router`.
        sink.import(import("app.js", "admin_router", "admin", Some("default")));
        sink.import(import(
            "admin.js",
            "reports_router",
            "reports",
            Some("default"),
        ));
        sink.export(ExportFact {
            module: PathBuf::from("admin.js"),
            exported: "default".into(),
            local: "router".into(),
        });
        sink.export(ExportFact {
            module: PathBuf::from("reports.js"),
            exported: "default".into(),
            local: "router".into(),
        });

        let graph = RegistrationGraph::build(sink, &files(&["app.js", "admin.js", "reports.js"]));
        let resolution = graph.resolve();

        let auth_of = |p: &str| {
            resolution
                .routes
                .iter()
                .find(|r| r.path.render(ParamStyle::Braces) == p)
                .unwrap()
                .auth
                .clone()
        };
        assert!(matches!(
            auth_of("/admin/stats"),
            Some(rl_model::AuthRequirement::Unknown { hint }) if hint == "requireAuth"
        ));
        assert!(
            matches!(
                auth_of("/admin/reports/daily"),
                Some(rl_model::AuthRequirement::Unknown { .. })
            ),
            "a nested mount inherits the guard"
        );
        assert_eq!(
            auth_of("/admin/rotate"),
            Some(rl_model::AuthRequirement::Basic),
            "a route's own auth is not overridden by the mount's"
        );
    }

    fn rendered(routes: &[ResolvedRoute<'_>]) -> Vec<String> {
        let mut out: Vec<String> = routes
            .iter()
            .map(|r| format!("{} {}", r.method, r.path.render(ParamStyle::Braces)))
            .collect();
        out.sort();
        out
    }

    /// The shape from `docs/discovery/how-it-works.md`: a router with its own prefix,
    /// declared in one file, mounted at another prefix in a second file.
    #[test]
    fn composes_a_prefix_across_two_files() {
        let mut sink = FactSink::new();
        sink.router(app_root("main.py", "app"));
        sink.router(router("api/users.py", "router", "/users", Some("users")));
        sink.route(route(
            "api/users.py",
            "router",
            HttpMethod::Get,
            "/{user_id}",
        ));
        sink.import(ImportFact {
            module: PathBuf::from("main.py"),
            local_name: "router".into(),
            source: "api.users".into(),
            original: Some("router".into()),
            level: 0,
        });
        sink.mount(mount("main.py", "app", "router", "/api/v1"));

        let graph = RegistrationGraph::build(sink, &files(&["main.py", "api/users.py"]));
        let resolution = graph.resolve();
        let routes = &resolution.routes;

        assert_eq!(rendered(routes), vec!["GET /api/v1/users/{user_id}"]);
        assert_eq!(routes[0].group.as_deref(), Some("users"));
        assert!(!routes[0].orphaned);
    }

    #[test]
    fn a_dotted_reference_resolves_through_a_module_import() {
        let mut sink = FactSink::new();
        sink.router(app_root("main.py", "app"));
        sink.router(router("api/users.py", "router", "/users", None));
        sink.route(route("api/users.py", "router", HttpMethod::Get, "/"));
        // `from api import users` then `users.router`
        sink.import(ImportFact {
            module: PathBuf::from("main.py"),
            local_name: "users".into(),
            source: "api.users".into(),
            original: None,
            level: 0,
        });
        sink.mount(mount("main.py", "app", "users.router", "/api"));

        let graph = RegistrationGraph::build(sink, &files(&["main.py", "api/users.py"]));
        assert_eq!(rendered(&graph.resolve().routes), vec!["GET /api/users"]);
    }

    #[test]
    fn a_router_mounted_twice_produces_both_paths() {
        let mut sink = FactSink::new();
        sink.router(app_root("main.py", "app"));
        sink.router(router("api/items.py", "router", "", None));
        sink.route(route("api/items.py", "router", HttpMethod::Get, "/items"));
        sink.import(ImportFact {
            module: PathBuf::from("main.py"),
            local_name: "router".into(),
            source: "api.items".into(),
            original: Some("router".into()),
            level: 0,
        });
        sink.mount(mount("main.py", "app", "router", "/v1"));
        sink.mount(mount("main.py", "app", "router", "/v2"));

        let graph = RegistrationGraph::build(sink, &files(&["main.py", "api/items.py"]));
        assert_eq!(
            rendered(&graph.resolve().routes),
            vec!["GET /v1/items", "GET /v2/items"]
        );
    }

    #[test]
    fn nested_mounts_compose_in_order() {
        let mut sink = FactSink::new();
        sink.router(app_root("main.py", "app"));
        sink.router(router("api/v1.py", "v1", "/v1", None));
        sink.router(router("api/users.py", "users", "/users", None));
        sink.route(route("api/users.py", "users", HttpMethod::Get, "/{id}"));

        sink.import(ImportFact {
            module: PathBuf::from("main.py"),
            local_name: "v1".into(),
            source: "api.v1".into(),
            original: Some("v1".into()),
            level: 0,
        });
        sink.import(ImportFact {
            module: PathBuf::from("api/v1.py"),
            local_name: "users".into(),
            source: "users".into(),
            original: Some("users".into()),
            level: 1,
        });
        sink.mount(mount("main.py", "app", "v1", "/api"));
        sink.mount(mount("api/v1.py", "v1", "users", ""));

        let graph =
            RegistrationGraph::build(sink, &files(&["main.py", "api/v1.py", "api/users.py"]));
        assert_eq!(
            rendered(&graph.resolve().routes),
            vec!["GET /api/v1/users/{id}"]
        );
    }

    #[test]
    fn an_unmounted_router_is_reported_and_its_routes_kept() {
        let mut sink = FactSink::new();
        sink.router(app_root("main.py", "app"));
        sink.router(router("api/orphan.py", "router", "/orphan", None));
        sink.route(route("api/orphan.py", "router", HttpMethod::Get, "/x"));

        let graph = RegistrationGraph::build(sink, &files(&["main.py", "api/orphan.py"]));
        let resolution = graph.resolve();
        let routes = &resolution.routes;

        assert_eq!(rendered(routes), vec!["GET /orphan/x"]);
        assert!(routes[0].orphaned, "an unmounted route must be flagged");
        assert!(resolution
            .warnings
            .iter()
            .any(|w| matches!(w, GraphWarning::OrphanedRouter { .. })));
    }

    #[test]
    fn a_mount_cycle_is_broken_and_reported() {
        let mut sink = FactSink::new();
        sink.router(app_root("main.py", "app"));
        sink.router(router("a.py", "a", "/a", None));
        sink.router(router("b.py", "b", "/b", None));
        sink.route(route("a.py", "a", HttpMethod::Get, "/x"));

        for (module, local, source) in [
            ("main.py", "a", "a"),
            ("a.py", "b", "b"),
            ("b.py", "a", "a"),
        ] {
            sink.import(ImportFact {
                module: PathBuf::from(module),
                local_name: local.into(),
                source: source.into(),
                original: Some(local.into()),
                level: 0,
            });
        }
        sink.mount(mount("main.py", "app", "a", ""));
        sink.mount(mount("a.py", "a", "b", ""));
        sink.mount(mount("b.py", "b", "a", "")); // back to a

        let graph = RegistrationGraph::build(sink, &files(&["main.py", "a.py", "b.py"]));
        let resolution = graph.resolve();
        let routes = &resolution.routes;

        // Terminated, and the route was still found on the way round.
        assert!(rendered(routes).contains(&"GET /a/x".to_string()));
        assert!(resolution
            .warnings
            .iter()
            .any(|w| matches!(w, GraphWarning::Cycle { .. })));
    }

    #[test]
    fn a_route_registered_directly_on_the_app_needs_no_router() {
        let mut sink = FactSink::new();
        sink.router(app_root("main.py", "app"));
        sink.route(route("main.py", "app", HttpMethod::Get, "/health"));

        let graph = RegistrationGraph::build(sink, &files(&["main.py"]));
        assert_eq!(rendered(&graph.resolve().routes), vec!["GET /health"]);
    }

    #[test]
    fn one_registration_with_several_methods_becomes_several_routes() {
        let mut sink = FactSink::new();
        sink.router(app_root("main.py", "app"));
        let mut multi = route("main.py", "app", HttpMethod::Get, "/items");
        multi.methods = vec![HttpMethod::Get, HttpMethod::Post];
        sink.route(multi);

        let graph = RegistrationGraph::build(sink, &files(&["main.py"]));
        assert_eq!(
            rendered(&graph.resolve().routes),
            vec!["GET /items", "POST /items"]
        );
    }

    #[test]
    fn an_unresolvable_mount_target_is_reported() {
        let mut sink = FactSink::new();
        sink.router(app_root("main.py", "app"));
        // `ghost` is neither declared here nor imported from anywhere in the project.
        sink.mount(mount("main.py", "app", "ghost.router", "/x"));

        let graph = RegistrationGraph::build(sink, &files(&["main.py"]));
        let resolution = graph.resolve();

        assert!(resolution
            .warnings
            .iter()
            .any(|w| matches!(w, GraphWarning::UnresolvedReference { .. })));
    }

    #[test]
    fn a_relative_import_links_modules() {
        let mut sink = FactSink::new();
        sink.router(app_root("app/main.py", "app"));
        sink.router(router("app/api/users.py", "router", "/users", None));
        sink.route(route("app/api/users.py", "router", HttpMethod::Get, "/"));
        // from .api.users import router
        sink.import(ImportFact {
            module: PathBuf::from("app/main.py"),
            local_name: "router".into(),
            source: "api.users".into(),
            original: Some("router".into()),
            level: 1,
        });
        sink.mount(mount("app/main.py", "app", "router", ""));

        let graph = RegistrationGraph::build(sink, &files(&["app/main.py", "app/api/users.py"]));
        assert_eq!(rendered(&graph.resolve().routes), vec!["GET /users"]);
    }
}
