//! What a framework adapter emits.
//!
//! Adapters recognise three things — *this creates a router*, *this registers a route*, *this
//! mounts a router at a prefix* — plus the imports needed to link them across files. They do
//! **not** compose paths. [`crate::graph`] does that, once, for every framework.
//!
//! If an adapter ever starts joining paths itself, the abstraction has sprung a leak.

use rl_model::{AuthRequirement, BodySchema, HttpMethod, ParamSpec, PathTemplate};
use std::path::{Path, PathBuf};

/// A position in a source file, 1-indexed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub line: u32,
    pub column: u32,
}

impl Span {
    pub fn new(line: u32, column: u32) -> Self {
        Span { line, column }
    }
}

/// A symbol declared in a module — normally a router variable.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SymbolId {
    /// Relative to the project root.
    pub module: PathBuf,
    pub name: String,
}

impl SymbolId {
    pub fn new(module: impl Into<PathBuf>, name: impl Into<String>) -> Self {
        SymbolId {
            module: module.into(),
            name: name.into(),
        }
    }
}

impl std::fmt::Display for SymbolId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Forward slashes regardless of platform: these strings reach the UI and snapshot
        // tests, and a warning should not read differently on Windows.
        write!(
            f,
            "{}:{}",
            self.module.display().to_string().replace('\\', "/"),
            self.name
        )
    }
}

/// A *use* of a symbol, from inside some module.
///
/// Unresolved on purpose: `include_router(router)` in `main.py` might mean a local variable
/// or something imported from another file, and only the graph knows which.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SymbolRef {
    /// The module the reference appears in.
    pub module: PathBuf,
    /// As written — possibly dotted, e.g. `users.router`.
    pub name: String,
}

impl SymbolRef {
    pub fn new(module: impl Into<PathBuf>, name: impl Into<String>) -> Self {
        SymbolRef {
            module: module.into(),
            name: name.into(),
        }
    }

    /// Split a dotted reference into its qualifier and final attribute.
    ///
    /// `users.router` → (`Some("users")`, `"router"`); `router` → (`None`, `"router"`).
    ///
    /// Only a trailing *identifier* counts as an attribute. An inline `require("./x.js")`
    /// is a single reference, and the dot in its file name must not be mistaken for one.
    pub fn split(&self) -> (Option<&str>, &str) {
        match self.name.rsplit_once('.') {
            Some((qualifier, attribute)) if is_identifier(attribute) => {
                (Some(qualifier), attribute)
            }
            _ => (None, self.name.as_str()),
        }
    }
}

fn is_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    match chars.next() {
        Some(c) if c.is_alphabetic() || c == '_' || c == '$' => {}
        _ => return false,
    }
    chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

/// A router was created.
#[derive(Debug, Clone)]
pub struct RouterFact {
    pub symbol: SymbolId,
    /// The prefix the router carries itself, e.g. `APIRouter(prefix="/users")`.
    pub prefix: PathTemplate,
    /// Tags or a name, used to group endpoints in the explorer.
    pub group: Option<String>,
    /// This symbol is an application root, so path resolution starts here.
    pub is_app_root: bool,
    /// The function an application root is created inside — `create_app` in an app
    /// factory. Runtime enrich needs to call it rather than import it.
    pub factory: Option<String>,
    /// Declared on the chance something mounts it, not because it is a router in its own
    /// right — a Django view function, say, which only serves anything once a URL pattern
    /// names it. Never reported as an orphan, and its routes are dropped when unreached.
    pub implicit: bool,
    pub span: Span,
}

/// A route was registered on a router.
#[derive(Debug, Clone)]
pub struct RouteFact {
    pub router: SymbolRef,
    /// One registration may declare several methods, as `@app.route(methods=[...])` does.
    pub methods: Vec<HttpMethod>,
    /// Relative to the router it is registered on.
    pub path: PathTemplate,
    pub query_params: Vec<ParamSpec>,
    pub headers: Vec<ParamSpec>,
    pub body: Option<BodySchema>,
    pub auth: Option<AuthRequirement>,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub group: Option<String>,
    pub deprecated: bool,
    /// The handler as the registration names it — `h.List`, normalised to `Handler.List`
    /// — when it is declared somewhere other than where it is registered. What it reads
    /// from the request is filled in afterwards from a [`HandlerFact`] by that name.
    pub handler: Option<SymbolRef>,
    pub span: Span,
}

impl RouteFact {
    pub fn new(router: SymbolRef, method: HttpMethod, path: PathTemplate, span: Span) -> Self {
        RouteFact {
            router,
            methods: vec![method],
            path,
            query_params: Vec::new(),
            headers: Vec::new(),
            body: None,
            auth: None,
            summary: None,
            description: None,
            group: None,
            deprecated: false,
            handler: None,
            span,
        }
    }
}

/// What a handler function reads from its request, recorded where the function is
/// declared so a route registered in another file can pick it up.
///
/// Go keeps `routes.go` and `handlers.go` apart as a matter of course, and a route knows
/// its handler only by name. This is the name's other half.
#[derive(Debug, Clone)]
pub struct HandlerFact {
    pub module: PathBuf,
    /// `List`, or `Handler.List` for a method.
    pub name: String,
    pub query_params: Vec<ParamSpec>,
    pub headers: Vec<ParamSpec>,
    pub body: Option<BodySchema>,
    pub auth: Option<AuthRequirement>,
    /// The methods the handler checks `r.Method` against, for a route registered without
    /// one.
    pub methods: Vec<HttpMethod>,
}

/// A router was mounted onto another router at a prefix.
#[derive(Debug, Clone)]
pub struct MountFact {
    pub parent: SymbolRef,
    pub child: SymbolRef,
    pub prefix: PathTemplate,
    pub group: Option<String>,
    /// Auth the mount imposes on everything under it: `app.use("/admin", requireAuth,
    /// adminRouter)`, `include_router(r, dependencies=[Depends(auth)])`. Inherited by
    /// every route reached through this mount that declares none of its own.
    pub auth: Option<AuthRequirement>,
    /// Only these methods are served through this mount; empty means all of them. Flask's
    /// `add_url_rule("/notes", view_func=NoteAPI.as_view(), methods=["GET"])` narrows a
    /// class-based view to one of its methods at one rule.
    pub methods: Vec<HttpMethod>,
    /// The mount's prefix *replaces* the child's own prefix instead of being joined in
    /// front of it. Flask's `register_blueprint(bp, url_prefix="/x")` overrides the
    /// `url_prefix` the blueprint was declared with; FastAPI's `include_router(prefix=)`
    /// composes with the router's. The graph must not have to know which framework it is
    /// walking, so the adapter says which it meant.
    pub replaces_child_prefix: bool,
    pub span: Span,
}

/// A name was bound by an import.
///
/// Needed because the router declared in `api/users.py` and the `include_router` call in
/// `main.py` are only connected by an import statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportFact {
    /// The importing module.
    pub module: PathBuf,
    /// The name bound locally.
    pub local_name: String,
    /// The module as written, in the importing language's own convention:
    ///
    /// - Python: dotted, without leading dots — `api.users`, or `` for `from . import x`.
    /// - JavaScript: the specifier — `./routes/users`, or a bare package name.
    /// - A leading `/` means *project-root-relative and already resolved*. No language
    ///   writes imports this way; it is how a file-system-routed framework attaches every
    ///   route file to one virtual root without an import statement existing anywhere.
    pub source: String,
    /// The name inside the source module, or `None` when the module itself was bound
    /// (`import api.users as users`, `import * as users from "./users"`).
    ///
    /// JavaScript's default export is the name `default`, so `const x = require("./m")`
    /// and `import x from "./m"` both bind `x` to `default` in `m`.
    pub original: Option<String>,
    /// Leading dots on a relative Python import. `0` means absolute; JavaScript ignores it.
    pub level: u32,
}

/// A module made a local name available under an exported name.
///
/// This is how `module.exports = router` in `routes/users.js` connects to the
/// `require("./routes/users")` in `app.js`: the import binds `default`, and this fact says
/// `default` is really `router`. Python modules export every top-level name, so the Python
/// adapters never emit one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportFact {
    pub module: PathBuf,
    /// The name importers see: `default`, or a named export.
    pub exported: String,
    /// The name inside the module.
    pub local: String,
}

/// Where adapters push what they found.
#[derive(Debug, Default)]
pub struct FactSink {
    pub routers: Vec<RouterFact>,
    pub routes: Vec<RouteFact>,
    pub mounts: Vec<MountFact>,
    pub imports: Vec<ImportFact>,
    pub exports: Vec<ExportFact>,
    /// Classes with annotated fields, for filling in request bodies. See [`crate::models`].
    pub models: Vec<crate::models::ModelFact>,
    /// Handler functions, for routes that only name theirs. See [`crate::handlers`].
    pub handlers: Vec<HandlerFact>,
    /// Ports the source says it listens on — `http.ListenAndServe(":8080", …)` — with
    /// where each was seen. A manifest is the usual place to learn the port; Go states it
    /// in code instead.
    pub ports: Vec<(u16, String)>,
    /// Anything the adapter noticed but could not express.
    pub warnings: Vec<String>,
}

impl FactSink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn router(&mut self, fact: RouterFact) {
        self.routers.push(fact);
    }

    pub fn route(&mut self, fact: RouteFact) {
        self.routes.push(fact);
    }

    pub fn mount(&mut self, fact: MountFact) {
        self.mounts.push(fact);
    }

    pub fn import(&mut self, fact: ImportFact) {
        self.imports.push(fact);
    }

    pub fn export(&mut self, fact: ExportFact) {
        self.exports.push(fact);
    }

    pub fn model(&mut self, fact: crate::models::ModelFact) {
        self.models.push(fact);
    }

    pub fn handler(&mut self, fact: HandlerFact) {
        self.handlers.push(fact);
    }

    pub fn warn(&mut self, message: impl Into<String>) {
        self.warnings.push(message.into());
    }

    pub fn port(&mut self, port: u16, seen_at: impl Into<String>) {
        self.ports.push((port, seen_at.into()));
    }

    pub fn is_empty(&self) -> bool {
        self.routers.is_empty() && self.routes.is_empty() && self.mounts.is_empty()
    }

    pub fn absorb(&mut self, other: FactSink) {
        self.routers.extend(other.routers);
        self.routes.extend(other.routes);
        self.mounts.extend(other.mounts);
        self.imports.extend(other.imports);
        self.exports.extend(other.exports);
        self.models.extend(other.models);
        self.handlers.extend(other.handlers);
        self.ports.extend(other.ports);
        self.warnings.extend(other.warnings);
    }
}

/// Resolve an import's source to a file in the project, in the importing file's language.
///
/// `None` means the import names something outside the project — a package — and there is
/// nothing to link it to. That is not an error.
pub fn resolve_module(
    importing: &Path,
    source: &str,
    level: u32,
    known: &dyn Fn(&Path) -> bool,
) -> Option<PathBuf> {
    // Pre-resolved by the adapter; see `ImportFact::source`.
    if let Some(absolute) = source.strip_prefix('/') {
        return Some(PathBuf::from(absolute));
    }

    match crate::project::Language::of(importing)? {
        crate::project::Language::Python => resolve_python_module(importing, source, level, known),
        crate::project::Language::JavaScript | crate::project::Language::TypeScript => {
            resolve_js_module(importing, source, known)
        }
        // The Go adapter rewrites an import of the project's own module into the
        // root-relative form handled above (`example.com/app/internal/routes` →
        // `/internal/routes`); anything still bare names a module outside the project.
        crate::project::Language::Go => None,
    }
}

/// Extensions a JavaScript import may omit, in the order Node and TypeScript try them.
const JS_EXTENSIONS: &[&str] = &["js", "ts", "jsx", "tsx", "mjs", "cjs", "mts", "cts"];

/// Resolve a JavaScript or TypeScript import specifier to a file in the project.
///
/// Relative specifiers (`./users`, `../routes/users.js`) are resolved the way Node does:
/// the exact file, then with each extension, then `<dir>/index.<ext>`. A `.js` extension
/// is also tried as `.ts`, since that is how TypeScript ESM output is written. The `@/` and
/// `~/` aliases are tried from `src/` and the project root, which covers the common
/// `tsconfig` setup without reading it. Bare specifiers name packages and never resolve.
pub fn resolve_js_module(
    importing: &Path,
    source: &str,
    known: &dyn Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let bases: Vec<PathBuf> = if source.starts_with("./") || source.starts_with("../") {
        vec![normalize(&importing.parent()?.join(source))]
    } else {
        let rest = source
            .strip_prefix("@/")
            .or_else(|| source.strip_prefix("~/"))?;
        vec![
            normalize(&Path::new("src").join(rest)),
            normalize(Path::new(rest)),
        ]
    };

    for base in bases {
        if known(&base) {
            return Some(base);
        }
        // `./users.js` written against a `users.ts` source file.
        if let Some(extension) = base.extension().and_then(|e| e.to_str()) {
            let swapped = match extension {
                "js" => Some("ts"),
                "mjs" => Some("mts"),
                "cjs" => Some("cts"),
                "jsx" => Some("tsx"),
                _ => None,
            };
            if let Some(swapped) = swapped {
                let candidate = base.with_extension(swapped);
                if known(&candidate) {
                    return Some(candidate);
                }
            }
        }
        for extension in JS_EXTENSIONS {
            // Not `with_extension`: that would replace the `.route` in `users.route`.
            let candidate = PathBuf::from(format!("{}.{extension}", base.display()));
            if known(&candidate) {
                return Some(candidate);
            }
        }
        for extension in JS_EXTENSIONS {
            let candidate = base.join(format!("index.{extension}"));
            if known(&candidate) {
                return Some(candidate);
            }
        }
    }

    None
}

/// Collapse `.` and `..` components without touching the filesystem.
fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Resolve a Python-style module specification to a file in the project.
///
/// Handles the relative form (`from .users import router`, `from ..core import settings`) and
/// the absolute form (`from app.api.users import router`), trying both `<path>.py` and
/// `<path>/__init__.py`.
pub fn resolve_python_module(
    importing: &Path,
    source: &str,
    level: u32,
    known: &dyn Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let mut base = if level == 0 {
        // Absolute: from the project root. A `src/` or package-dir layout means the same
        // module may live one level down, so both are tried below.
        PathBuf::new()
    } else {
        // Relative: one dot means the importing module's own package, each extra dot climbs.
        let mut dir = importing.parent()?.to_path_buf();
        for _ in 1..level {
            dir = dir.parent()?.to_path_buf();
        }
        dir
    };

    for segment in source.split('.').filter(|s| !s.is_empty()) {
        base = base.join(segment);
    }

    let candidates = [base.with_extension("py"), base.join("__init__.py")];
    if let Some(hit) = candidates.iter().find(|c| known(c)) {
        return Some(hit.clone());
    }

    // An absolute import may still be rooted inside a package directory, so try the
    // importing module's own top-level directory as a base before giving up.
    if level == 0 {
        let top = importing.components().next()?;
        let mut nested = PathBuf::from(top.as_os_str());
        for segment in source.split('.').filter(|s| !s.is_empty()) {
            nested = nested.join(segment);
        }
        let candidates = [nested.with_extension("py"), nested.join("__init__.py")];
        if let Some(hit) = candidates.iter().find(|c| known(c)) {
            return Some(hit.clone());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn known_files(paths: &[&str]) -> impl Fn(&Path) -> bool {
        // Owns the set, so the closure borrows nothing and needs no lifetime.
        let set: BTreeSet<PathBuf> = paths.iter().map(PathBuf::from).collect();
        move |p: &Path| set.contains(p)
    }

    #[test]
    fn a_dotted_reference_splits_into_qualifier_and_attribute() {
        let dotted = SymbolRef::new("main.py", "users.router");
        assert_eq!(dotted.split(), (Some("users"), "router"));

        let plain = SymbolRef::new("main.py", "router");
        assert_eq!(plain.split(), (None, "router"));
    }

    #[test]
    fn an_inline_require_is_one_reference_despite_its_dots() {
        let inline = SymbolRef::new("app.js", "require(\"./routes/users.js\")");
        assert_eq!(inline.split(), (None, "require(\"./routes/users.js\")"));

        let member = SymbolRef::new("app.js", "require(\"./routes\").users");
        assert_eq!(member.split(), (Some("require(\"./routes\")"), "users"));
    }

    #[test]
    fn a_deeply_dotted_reference_keeps_the_whole_qualifier() {
        let deep = SymbolRef::new("main.py", "api.v1.users.router");
        assert_eq!(deep.split(), (Some("api.v1.users"), "router"));
    }

    #[test]
    fn a_relative_import_resolves_against_the_importing_package() {
        let known = known_files(&["app/api/users.py"]);
        assert_eq!(
            resolve_python_module(Path::new("app/api/__init__.py"), "users", 1, &known),
            Some(PathBuf::from("app/api/users.py"))
        );
    }

    #[test]
    fn extra_dots_climb_out_of_the_package() {
        let known = known_files(&["app/core/config.py"]);
        assert_eq!(
            resolve_python_module(Path::new("app/api/users.py"), "core.config", 2, &known),
            Some(PathBuf::from("app/core/config.py"))
        );
    }

    #[test]
    fn an_absolute_import_resolves_from_the_project_root() {
        let known = known_files(&["app/api/users.py"]);
        assert_eq!(
            resolve_python_module(Path::new("main.py"), "app.api.users", 0, &known),
            Some(PathBuf::from("app/api/users.py"))
        );
    }

    #[test]
    fn a_package_is_found_through_its_init_file() {
        let known = known_files(&["app/api/__init__.py"]);
        assert_eq!(
            resolve_python_module(Path::new("main.py"), "app.api", 0, &known),
            Some(PathBuf::from("app/api/__init__.py"))
        );
    }

    /// A project laid out as `src/app/...` imports itself as `app.api.users`, so the
    /// top-level directory has to be tried as a base too.
    #[test]
    fn an_absolute_import_inside_a_package_directory_resolves() {
        let known = known_files(&["src/app/api/users.py"]);
        assert_eq!(
            resolve_python_module(Path::new("src/app/main.py"), "app.api.users", 0, &known),
            Some(PathBuf::from("src/app/api/users.py"))
        );
    }

    #[test]
    fn an_import_of_something_outside_the_project_does_not_resolve() {
        let known = known_files(&["app/main.py"]);
        assert_eq!(
            resolve_python_module(Path::new("app/main.py"), "fastapi", 0, &known),
            None
        );
    }

    #[test]
    fn a_relative_js_import_tries_extensions_and_index_files() {
        let known = known_files(&["src/routes/users.ts", "src/routes/orders/index.js"]);
        let from = Path::new("src/app.ts");

        assert_eq!(
            resolve_js_module(from, "./routes/users", &known),
            Some(PathBuf::from("src/routes/users.ts"))
        );
        assert_eq!(
            resolve_js_module(from, "./routes/orders", &known),
            Some(PathBuf::from("src/routes/orders/index.js"))
        );
    }

    #[test]
    fn a_js_extension_finds_the_typescript_source_it_compiles_from() {
        let known = known_files(&["src/routes/users.ts"]);
        assert_eq!(
            resolve_js_module(Path::new("src/app.ts"), "./routes/users.js", &known),
            Some(PathBuf::from("src/routes/users.ts"))
        );
    }

    #[test]
    fn parent_directory_imports_climb() {
        let known = known_files(&["src/lib/router.js"]);
        assert_eq!(
            resolve_js_module(Path::new("src/routes/users.js"), "../lib/router", &known),
            Some(PathBuf::from("src/lib/router.js"))
        );
    }

    #[test]
    fn the_at_alias_is_tried_from_src_and_the_root() {
        let known = known_files(&["src/routes/users.ts"]);
        assert_eq!(
            resolve_js_module(Path::new("src/app.ts"), "@/routes/users", &known),
            Some(PathBuf::from("src/routes/users.ts"))
        );
    }

    #[test]
    fn a_bare_specifier_is_a_package_and_does_not_resolve() {
        let known = known_files(&["node_modules/express/index.js", "express.js"]);
        assert_eq!(
            resolve_js_module(Path::new("app.js"), "express", &known),
            None
        );
    }

    #[test]
    fn a_dotted_file_name_is_not_treated_as_an_extension() {
        let known = known_files(&["src/users.route.js"]);
        assert_eq!(
            resolve_js_module(Path::new("src/app.js"), "./users.route", &known),
            Some(PathBuf::from("src/users.route.js"))
        );
    }

    #[test]
    fn resolution_dispatches_on_the_importing_language() {
        let known = known_files(&["app/api/users.py", "src/routes/users.js"]);
        assert_eq!(
            resolve_module(Path::new("app/main.py"), "api.users", 1, &known),
            Some(PathBuf::from("app/api/users.py"))
        );
        assert_eq!(
            resolve_module(Path::new("src/app.js"), "./routes/users", 0, &known),
            Some(PathBuf::from("src/routes/users.js"))
        );
    }

    #[test]
    fn a_pre_resolved_source_is_taken_as_given() {
        let known = known_files(&[]);
        assert_eq!(
            resolve_module(Path::new("app/api/x/route.ts"), "/app", 0, &known),
            Some(PathBuf::from("app"))
        );
    }

    #[test]
    fn the_sink_absorbs_another() {
        let mut a = FactSink::new();
        a.warn("one");
        let mut b = FactSink::new();
        b.warn("two");

        a.absorb(b);
        assert_eq!(a.warnings, vec!["one", "two"]);
        assert!(a.is_empty(), "warnings alone are not facts");
    }
}
