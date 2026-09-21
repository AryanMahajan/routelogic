//! Go — net/http, Gin, Echo, chi, Fiber and gorilla/mux.
//!
//! One adapter for all six, because Go projects mix them: a chi router served by
//! `http.ListenAndServe`, a gorilla subrouter inside a `ServeMux`, Gin next to a metrics
//! handler on the default mux. Each framework is only a few method names — `GET`, `Get`,
//! `HandleFunc`; `Group`, `Route`, `Mount`, `Subrouter` — over the same three facts the
//! graph already composes.
//!
//! What is Go's own, rather than any framework's, is scope. A name belongs to the package
//! (the directory), not the file. And routes are as often registered on a function's
//! parameter — `func Register(r *gin.RouterGroup)` — or on a router the function returns —
//! `func Routes() chi.Router` — as on a variable. Such a function *is* a router here, and
//! the call that hands it a router, `Register(v1)` or `r.Mount("/users", Routes())`, is the
//! mount. The graph resolves `users.Register` across files by package; nothing here joins
//! a path.
//!
//! Go states its listen address in code rather than in a manifest, so
//! `http.ListenAndServe(":8080", r)` is also where the base URL comes from.

use super::js::{auth_from_middleware, promote_authorization_header};
use super::{Detection, FrameworkAdapter, UNSPECIFIED_METHODS};
use crate::facts::{
    ExportFact, FactSink, HandlerFact, ImportFact, MountFact, RouteFact, RouterFact, Span,
    SymbolId, SymbolRef,
};
use crate::index::{walk, ParsedFile};
use crate::models::{ModelFact, ModelField};
use crate::project::{Language, ProjectContext};
use rl_model::{
    AuthRequirement, BodySchema, HttpMethod, ParamSpec, PathSegment, PathTemplate, TypeHint,
};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use tree_sitter::Node;

/// The frameworks recognised, by import path. `net/http` is not listed: every Go program
/// imports it, so its evidence is a registration call rather than the import.
const FRAMEWORKS: &[(&str, &str)] = &[
    ("gin", "github.com/gin-gonic/gin"),
    ("echo", "github.com/labstack/echo"),
    ("chi", "github.com/go-chi/chi"),
    ("fiber", "github.com/gofiber/fiber"),
    ("gorilla/mux", "github.com/gorilla/mux"),
];

/// Calls that only the standard library's mux answers to.
const HTTP_REGISTRATIONS: &[&str] = &["http.HandleFunc(", "http.Handle(", "http.NewServeMux("];

/// The methods a chain piece may spell a route with, in both spellings.
const UPPER_METHODS: &[(&str, HttpMethod)] = &[
    ("GET", HttpMethod::Get),
    ("POST", HttpMethod::Post),
    ("PUT", HttpMethod::Put),
    ("PATCH", HttpMethod::Patch),
    ("DELETE", HttpMethod::Delete),
    ("HEAD", HttpMethod::Head),
    ("OPTIONS", HttpMethod::Options),
    ("TRACE", HttpMethod::Trace),
];

const TITLE_METHODS: &[(&str, HttpMethod)] = &[
    ("Get", HttpMethod::Get),
    ("Post", HttpMethod::Post),
    ("Put", HttpMethod::Put),
    ("Patch", HttpMethod::Patch),
    ("Delete", HttpMethod::Delete),
    ("Head", HttpMethod::Head),
    ("Options", HttpMethod::Options),
    ("Trace", HttpMethod::Trace),
];

/// gorilla/mux builds a route across a chain, so seeing any of these means the chain is
/// read as a whole rather than piece by piece.
const GORILLA_PIECES: &[&str] = &[
    "Methods",
    "Path",
    "PathPrefix",
    "HandlerFunc",
    "Handler",
    "Queries",
    "Headers",
    "Schemes",
    "Host",
    "Name",
    "Subrouter",
];

/// Calls that start serving: the router they are called on, or handed, is a root.
const SERVE_METHODS: &[&str] = &[
    "Run",
    "RunTLS",
    "Start",
    "StartTLS",
    "StartServer",
    "Listen",
    "ListenTLS",
    "ListenAndServe",
    "ListenAndServeTLS",
    "Serve",
    "ServeTLS",
];

/// File and directory names that say nothing about what a router serves.
const MEANINGLESS_NAMES: &[&str] = &[
    "main",
    "router",
    "routers",
    "routes",
    "route",
    "routing",
    "server",
    "app",
    "handler",
    "handlers",
    "api",
    "http",
    "mux",
    "cmd",
    "internal",
    "pkg",
    "web",
    "rest",
    "transport",
    "controllers",
    "controller",
    "service",
    "services",
    "src",
];

/// Which framework a router came from. Mostly irrelevant — one parser reads every path
/// syntax — but it decides what `Handle` and `Use` mean, and Gin's default port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flavour {
    Http,
    Gin,
    Echo,
    Chi,
    Fiber,
    Mux,
    Unknown,
}

impl Flavour {
    fn of_import(path: &str) -> Option<Flavour> {
        if path == "net/http" {
            return Some(Flavour::Http);
        }
        FRAMEWORKS
            .iter()
            .find(|(_, prefix)| path == *prefix || path.starts_with(&format!("{prefix}/")))
            .map(|(id, _)| match *id {
                "gin" => Flavour::Gin,
                "echo" => Flavour::Echo,
                "chi" => Flavour::Chi,
                "fiber" => Flavour::Fiber,
                _ => Flavour::Mux,
            })
    }

    /// `gin.Default()` — and whether what it builds is an application root on its own.
    /// A `chi.NewRouter()` is not: it only serves once something serves it.
    fn constructor(self, name: &str) -> Option<bool> {
        match (self, name) {
            (Flavour::Http, "NewServeMux") => Some(false),
            (Flavour::Gin, "Default" | "New") => Some(true),
            (Flavour::Echo, "New") => Some(true),
            (Flavour::Chi, "NewRouter" | "NewMux") => Some(false),
            (Flavour::Fiber, "New") => Some(true),
            (Flavour::Mux, "NewRouter") => Some(false),
            _ => None,
        }
    }

    /// `*gin.RouterGroup` — a parameter of this type is a router the caller hands over.
    fn is_router_type(self, name: &str) -> bool {
        matches!(
            (self, name),
            (Flavour::Http, "ServeMux")
                | (
                    Flavour::Gin,
                    "Engine" | "RouterGroup" | "IRouter" | "IRoutes"
                )
                | (Flavour::Echo, "Echo" | "Group")
                | (Flavour::Chi, "Router" | "Mux")
                | (Flavour::Fiber, "App" | "Router" | "Group")
                | (Flavour::Mux, "Router")
        )
    }
}

#[derive(Default)]
pub struct GoAdapter {
    /// The `module` line of `go.mod`, read by `detect`, so an import of the project's own
    /// packages can be told from everything else.
    module_path: RefCell<Option<String>>,
}

impl FrameworkAdapter for GoAdapter {
    fn id(&self) -> &'static str {
        "go"
    }

    fn languages(&self) -> &[Language] {
        &[Language::Go]
    }

    fn detect(&self, project: &ProjectContext) -> Detection {
        let mut detection = Detection::none();

        if let Some(text) = project.manifest("go.mod") {
            *self.module_path.borrow_mut() = module_path(text);
            for (id, path) in FRAMEWORKS {
                if text.lines().any(|line| line.trim().contains(path)) {
                    detection.add(1, format!("`{path}` is required in go.mod ({id})"));
                }
            }
        }

        // An actual import is much stronger evidence than a manifest entry, and a
        // registration call is the only evidence net/http can give.
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for path in project.files_of(Language::Go) {
            if seen.len() == FRAMEWORKS.len() + 1 {
                break;
            }
            let Ok(source) = project.read(path) else {
                continue;
            };
            for (id, import) in FRAMEWORKS {
                if !seen.contains(id) && imports(&source, import) {
                    seen.insert(id);
                    detection.add(
                        3,
                        format!("{id} is imported in {}", crate::project::display(path)),
                    );
                }
            }
            if !seen.contains("net/http")
                && HTTP_REGISTRATIONS
                    .iter()
                    .any(|needle| source.contains(needle))
            {
                seen.insert("net/http");
                detection.add(
                    3,
                    format!(
                        "net/http registers routes in {}",
                        crate::project::display(path)
                    ),
                );
            }
        }

        detection
    }

    fn candidate_files(&self, project: &ProjectContext) -> Vec<PathBuf> {
        project
            .files_of(Language::Go)
            .filter(|path| {
                let text = path.to_string_lossy().replace('\\', "/");
                let name = text.rsplit('/').next().unwrap_or("");
                !name.ends_with("_test.go")
                    && !name.ends_with(".pb.go")
                    && !name.ends_with(".gen.go")
                    && !text.contains("/testdata/")
                    && !text.starts_with("testdata/")
            })
            .cloned()
            .collect()
    }

    fn extract(&self, file: &ParsedFile, sink: &mut FactSink) {
        let module_path = self.module_path.borrow().clone();
        Extractor::new(file, module_path, sink).file();
    }
}

fn imports(source: &str, path: &str) -> bool {
    source.contains(&format!("\"{path}\"")) || source.contains(&format!("\"{path}/"))
}

fn module_path(go_mod: &str) -> Option<String> {
    go_mod.lines().find_map(|line| {
        line.trim()
            .strip_prefix("module ")
            .map(|rest| rest.trim().trim_matches('"').to_string())
    })
}

/// A router as an expression names it: a declared variable, a parameter, `pkg.Router`,
/// `s.router` on a receiver, or a group carved out of one of those.
#[derive(Clone, Debug)]
struct RouterExpr {
    /// The symbol name the reference resolves against.
    name: String,
    flavour: Flavour,
    /// Guards accumulated on the way — `r.With(requireAuth)`.
    auth: Option<AuthRequirement>,
    /// Known to be a router: declared here, a typed parameter, a field of the receiver, a
    /// name from a project package, or a value a known constructor produced. A bare
    /// identifier declared nowhere in the file is not — it may well be a router from the
    /// file next door, so routes on it are still reported, but it is not served or
    /// mounted on the strength of a name alone.
    trusted: bool,
}

/// A router declared in this file, emitted once the whole file has been read — a serve
/// call further down may still make it a root.
struct Declared {
    symbol: String,
    /// A root by construction — `gin.Default()`, `fiber.New()`.
    is_app_root: bool,
    /// Something serves it: `r.Run()`, `http.ListenAndServe(":8080", r)`.
    served: bool,
    /// Something mounts it — a Fiber sub-app, a chi router under `Mount` — so it is not
    /// a root of its own unless it is also served.
    mounted: bool,
    group: Option<String>,
    span: Span,
}

/// A function that is a router: routes are registered on its parameter, or on a router it
/// returns.
struct FunctionRouter {
    symbol: String,
    /// Routes, mounts or a serve call happened on it. Without any, the function is
    /// declared implicitly so the calls that hand it a router resolve quietly.
    activity: bool,
    is_app_root: bool,
    span: Span,
}

/// What one function body knows.
#[derive(Clone, Default)]
struct Scope {
    /// Local name → router. Parameters, declared routers and groups.
    routers: BTreeMap<String, RouterExpr>,
    /// Local name → normalised expression it stands for: `h := NewHandler(db)` makes `h`
    /// stand for `NewHandler`, so `h.Routes(r)` can be traced to `Handler.Routes`; and
    /// whether that expression was a call, whose result may well be a router, rather
    /// than a struct literal, which is only one when something serves it.
    aliases: BTreeMap<String, (String, bool)>,
    /// Local name → its type: `s := &Server{}`, `var req CreateUser`, every parameter.
    types: BTreeMap<String, String>,
    /// The method receiver, name and type.
    receiver: Option<(String, String)>,
    /// The function this scope belongs to, as a symbol.
    function: Option<String>,
    /// Identifiers a `return` statement returns.
    returned: BTreeSet<String>,
    /// Locals holding string constants.
    constants: BTreeMap<String, String>,
}

struct Extractor<'s, 'f> {
    file: &'f ParsedFile,
    module_path: Option<String>,
    sink: &'s mut FactSink,
    /// Import local name → flavour, for `gin.Default()` under whatever alias.
    packages: BTreeMap<String, Flavour>,
    /// Import local names that are not this project's packages.
    externals: BTreeSet<String>,
    /// Import local names that are.
    project_packages: BTreeSet<String>,
    /// Package-level string constants, folded.
    constants: BTreeMap<String, String>,
    /// Package-level routers, by variable name.
    package_routers: BTreeMap<String, RouterExpr>,
    /// Types declared in this file, with their fields' wire names, for query structs.
    structs: BTreeMap<String, Vec<StructField>>,
    /// Top-level function names in this file.
    function_names: BTreeSet<String>,
    declared: Vec<Declared>,
    functions: Vec<FunctionRouter>,
    /// `r.Use(requireAuth)` applies to every route registered on `r` afterwards.
    router_auth: BTreeMap<String, AuthRequirement>,
    /// The virtual roots this file needed: the default mux, and a server handed a router.
    default_mux: Option<Span>,
    server_root: Option<Span>,
    /// Synthetic routers already mounted, so the same group is not declared twice.
    synthetic: BTreeSet<String>,
    /// Nodes a declaration already read — a constructor, a group, a routed literal — so
    /// the walk does not read them a second time as a call in their own right.
    consumed: BTreeSet<usize>,
}

struct StructField {
    /// The name on the wire: the `json` tag, or the field.
    json: Option<String>,
    form: Option<String>,
    field: String,
}

impl<'s, 'f> Extractor<'s, 'f> {
    fn new(file: &'f ParsedFile, module_path: Option<String>, sink: &'s mut FactSink) -> Self {
        Extractor {
            file,
            module_path,
            sink,
            packages: BTreeMap::new(),
            externals: BTreeSet::new(),
            project_packages: BTreeSet::new(),
            constants: BTreeMap::new(),
            package_routers: BTreeMap::new(),
            structs: BTreeMap::new(),
            function_names: BTreeSet::new(),
            declared: Vec::new(),
            functions: Vec::new(),
            router_auth: BTreeMap::new(),
            default_mux: None,
            server_root: None,
            synthetic: BTreeSet::new(),
            consumed: BTreeSet::new(),
        }
    }

    fn span(&self, node: Node<'_>) -> Span {
        self.file.span(node)
    }

    fn text(&self, node: Node<'_>) -> &'f str {
        self.file.text(node)
    }

    fn module(&self) -> PathBuf {
        self.file.path.clone()
    }

    fn file(mut self) {
        let root = self.file.root();
        self.imports(root);
        self.package_level(root);

        let mut cursor = root.walk();
        let top: Vec<Node<'f>> = root.children(&mut cursor).collect();
        for node in top {
            match node.kind() {
                "function_declaration" => self.function(node),
                "method_declaration" => self.method(node),
                _ => {}
            }
        }

        let module = self.module();
        let group = group_from_file(&self.file.path);
        for declared in std::mem::take(&mut self.declared) {
            self.sink.router(RouterFact {
                symbol: SymbolId::new(module.clone(), declared.symbol),
                prefix: PathTemplate::empty(),
                group: declared.group.or_else(|| group.clone()),
                is_app_root: declared.served || (declared.is_app_root && !declared.mounted),
                factory: None,
                implicit: false,
                span: declared.span,
            });
        }
        for function in std::mem::take(&mut self.functions) {
            let group = group
                .clone()
                .or_else(|| group_from_function(&function.symbol));
            self.sink.router(RouterFact {
                symbol: SymbolId::new(module.clone(), function.symbol),
                prefix: PathTemplate::empty(),
                group,
                is_app_root: function.is_app_root,
                factory: None,
                implicit: !function.activity,
                span: function.span,
            });
        }
        if let Some(span) = self.default_mux {
            self.sink.router(RouterFact {
                symbol: SymbolId::new(module.clone(), "http.DefaultServeMux"),
                prefix: PathTemplate::empty(),
                group: group.clone(),
                is_app_root: true,
                factory: None,
                implicit: false,
                span,
            });
        }
        if let Some(span) = self.server_root {
            self.sink.router(RouterFact {
                symbol: SymbolId::new(module, "http.Server"),
                prefix: PathTemplate::empty(),
                group: None,
                is_app_root: true,
                factory: None,
                implicit: false,
                span,
            });
        }

        if self.file.has_errors() {
            self.sink.warn(format!(
                "{} has syntax errors; routes around them may be missing",
                self.file.display_path()
            ));
        }
    }

    /// `import users "example.com/app/internal/handlers"`: bound as `users`, and — when the
    /// path is inside this module — recorded for the graph as the package directory.
    fn imports(&mut self, root: Node<'f>) {
        let mut specs = Vec::new();
        walk(root, &mut |node| {
            if node.kind() == "import_spec" {
                specs.push(node);
            }
        });

        for spec in specs {
            let Some(path) = spec
                .child_by_field_name("path")
                .and_then(|p| string_literal(self.file, p))
            else {
                continue;
            };
            let alias = spec
                .child_by_field_name("name")
                .map(|n| self.text(n).to_string())
                .filter(|n| n != "_" && n != ".");
            let inside = self.module_path.as_deref().and_then(|module| {
                if path == module {
                    Some(String::new())
                } else {
                    path.strip_prefix(&format!("{module}/")).map(str::to_string)
                }
            });
            // A `/v2` on a module path is its major version, not its name; a `v2`
            // directory inside this project is just a directory.
            let local = alias.unwrap_or_else(|| match &inside {
                Some(directory) => directory
                    .rsplit('/')
                    .next()
                    .filter(|s| !s.is_empty())
                    .unwrap_or(&path)
                    .to_string(),
                None => package_name(&path),
            });

            if let Some(flavour) = Flavour::of_import(&path) {
                self.packages.insert(local.clone(), flavour);
            }

            match inside {
                Some(directory) => {
                    self.project_packages.insert(local.clone());
                    self.sink.import(ImportFact {
                        module: self.module(),
                        local_name: local,
                        source: format!("/{directory}"),
                        original: None,
                        level: 0,
                    });
                }
                None => {
                    self.externals.insert(local);
                }
            }
        }
    }

    /// Constants, types and package-level routers, before any function is read: a
    /// `const prefix` may be declared below the `main` that uses it.
    fn package_level(&mut self, root: Node<'f>) {
        let mut cursor = root.walk();
        let children: Vec<Node<'f>> = root.children(&mut cursor).collect();

        for node in &children {
            if node.kind() == "function_declaration" {
                if let Some(name) = node.child_by_field_name("name") {
                    self.function_names.insert(self.text(name).to_string());
                }
            }
        }

        // Two passes over constants so `v1 = prefix + "/v1"` folds whatever order they
        // are written in.
        for _ in 0..2 {
            for node in &children {
                if matches!(node.kind(), "const_declaration" | "var_declaration") {
                    self.constant_specs(*node);
                }
            }
        }

        for node in &children {
            if node.kind() == "type_declaration" {
                self.types(*node);
            }
        }

        for node in &children {
            if node.kind() != "var_declaration" {
                continue;
            }
            let mut specs = Vec::new();
            walk(*node, &mut |n| {
                if n.kind() == "var_spec" {
                    specs.push(n);
                }
            });
            let mut scope = Scope::default();
            for spec in specs {
                self.var_spec(spec, &mut scope);
            }
            self.package_routers.extend(scope.routers);
        }
    }

    fn var_spec(&mut self, spec: Node<'f>, scope: &mut Scope) {
        let names = children_by_field(spec, "name");
        let values = spec
            .child_by_field_name("value")
            .map(named_children)
            .unwrap_or_default();
        let ty = spec.child_by_field_name("type");
        for (index, name) in names.iter().enumerate() {
            match values.get(index) {
                Some(value) => self.bind(scope, *name, *value, spec),
                None => {
                    if let Some(ty) = ty {
                        self.bind_type(scope, *name, ty);
                    }
                }
            }
        }
    }

    fn constant_specs(&mut self, declaration: Node<'f>) {
        let mut specs = Vec::new();
        walk(declaration, &mut |n| {
            if matches!(n.kind(), "const_spec" | "var_spec") {
                specs.push(n);
            }
        });
        let scope = Scope::default();
        for spec in specs {
            let names = children_by_field(spec, "name");
            let values = spec
                .child_by_field_name("value")
                .map(named_children)
                .unwrap_or_default();
            for (index, name) in names.iter().enumerate() {
                if let Some(value) = values.get(index) {
                    if let Some(text) = self.string_value(&scope, *value) {
                        self.constants.insert(self.text(*name).to_string(), text);
                    }
                }
            }
        }
    }

    /// Struct types become models — `json:"name"` tags are the field names on the wire —
    /// and a type that embeds a router is recorded as standing for that router.
    fn types(&mut self, declaration: Node<'f>) {
        let mut specs = Vec::new();
        walk(declaration, &mut |n| {
            if n.kind() == "type_spec" {
                specs.push(n);
            }
        });
        for spec in specs {
            let (Some(name), Some(ty)) = (
                spec.child_by_field_name("name"),
                spec.child_by_field_name("type"),
            ) else {
                continue;
            };
            if ty.kind() != "struct_type" {
                continue;
            }
            let name = self.text(name).to_string();
            let mut fields = Vec::new();
            let mut model_fields = Vec::new();
            let mut bases = Vec::new();

            let declarations: Vec<Node<'f>> = child_of_kind(ty, "field_declaration_list")
                .map(named_children)
                .unwrap_or_default()
                .into_iter()
                .filter(|n| n.kind() == "field_declaration")
                .collect();
            for declaration in declarations {
                let Some(field_type) = declaration.child_by_field_name("type") else {
                    continue;
                };
                let type_text = self.text(field_type).trim_start_matches('*').to_string();
                let names = children_by_field(declaration, "name");
                let tag = declaration
                    .child_by_field_name("tag")
                    .and_then(|t| string_literal(self.file, t))
                    .unwrap_or_default();

                if names.is_empty() {
                    // Embedded: `*chi.Mux` makes the type a router; anything else
                    // contributes its fields.
                    let (package, base) = split_qualified(&type_text);
                    let embeds_router = package
                        .and_then(|p| self.packages.get(p))
                        .is_some_and(|flavour| flavour.is_router_type(base));
                    if embeds_router {
                        self.sink.export(ExportFact {
                            module: self.module(),
                            exported: name.clone(),
                            local: format!("{name}.{base}"),
                        });
                    } else {
                        bases.push(base.to_string());
                    }
                    continue;
                }

                for field in names {
                    let field = self.text(field).to_string();
                    let json = tag_value(&tag, "json");
                    let form = tag_value(&tag, "form");
                    if json.as_deref() == Some("-") {
                        continue;
                    }
                    let exported = field.starts_with(|c: char| c.is_ascii_uppercase());
                    if !exported && json.is_none() {
                        continue; // encoding/json never sees it
                    }
                    let wire = json.clone().unwrap_or_else(|| field.clone());
                    let required = ["binding", "validate"]
                        .iter()
                        .any(|key| tag_value(&tag, key).is_some_and(|v| v.contains("required")));
                    model_fields.push(ModelField {
                        name: wire,
                        annotation: annotation(&type_text),
                        default: None,
                        required,
                    });
                    fields.push(StructField { json, form, field });
                }
            }

            self.sink.model(ModelFact {
                name: name.clone(),
                module: self.module(),
                bases,
                fields: model_fields,
            });
            self.structs.insert(name, fields);
        }
    }

    fn function(&mut self, node: Node<'f>) {
        let Some(name) = node.child_by_field_name("name") else {
            return;
        };
        let symbol = self.text(name).to_string();
        self.constructor_alias(node, &symbol);
        self.handler_fact(node, &symbol);
        self.body(node, symbol, None);
    }

    /// A function shaped like a handler — it takes a `*gin.Context`, an `echo.Context`, a
    /// `*fiber.Ctx` or an `http.ResponseWriter` — has what it reads recorded by name, for
    /// the route in another file that names it.
    fn handler_fact(&mut self, node: Node<'f>, symbol: &str) {
        let Some(list) = node.child_by_field_name("parameters") else {
            return;
        };
        let shaped = self.parameters(list).iter().any(|(_, ty)| {
            let (package, name) = split_qualified(self.text(*ty));
            match package.and_then(|p| self.packages.get(p)) {
                Some(Flavour::Gin | Flavour::Echo) => name == "Context",
                Some(Flavour::Fiber) => name == "Ctx",
                Some(Flavour::Http) => name == "ResponseWriter",
                _ => false,
            }
        });
        if !shaped {
            return;
        }
        let usage = self.usage_of(node);
        let mut headers: Vec<ParamSpec> = usage.headers.iter().map(ParamSpec::new).collect();
        let mut auth = None;
        promote_authorization_header(&mut headers, &mut auth);
        self.sink.handler(HandlerFact {
            module: self.module(),
            name: symbol.to_string(),
            query_params: usage.query.iter().map(ParamSpec::new).collect(),
            headers,
            body: usage.body(),
            auth,
            methods: usage.checked.clone(),
        });
    }

    fn method(&mut self, node: Node<'f>) {
        let Some(name) = node.child_by_field_name("name") else {
            return;
        };
        let receiver = node
            .child_by_field_name("receiver")
            .and_then(|list| named_children(list).into_iter().next())
            .and_then(|param| {
                let ty = param.child_by_field_name("type")?;
                let ty = self.text(ty).trim_start_matches('*').to_string();
                let name = param
                    .child_by_field_name("name")
                    .map(|n| self.text(n).to_string())
                    .unwrap_or_default();
                Some((name, ty))
            });
        let Some((receiver_name, receiver_type)) = receiver else {
            return;
        };
        let method = self.text(name).to_string();
        let symbol = format!("{receiver_type}.{method}");
        self.handler_fact(node, &symbol);

        // `func (s *Server) ServeHTTP(w, r) { s.router.ServeHTTP(w, r) }`: serving the
        // type serves its router.
        if method == "ServeHTTP" {
            if let Some(field) = self.serve_http_delegate(node, &receiver_name) {
                self.sink.export(ExportFact {
                    module: self.module(),
                    exported: receiver_type.clone(),
                    local: format!("{receiver_type}.{field}"),
                });
            }
        }

        self.body(node, symbol, Some((receiver_name, receiver_type)));
    }

    fn serve_http_delegate(&self, node: Node<'f>, receiver: &str) -> Option<String> {
        let body = node.child_by_field_name("body")?;
        let mut delegate = None;
        walk(body, &mut |n| {
            if delegate.is_some() || n.kind() != "call_expression" {
                return;
            }
            let Some(function) = n.child_by_field_name("function") else {
                return;
            };
            let (Some(operand), Some(field)) = (
                function.child_by_field_name("operand"),
                function.child_by_field_name("field"),
            ) else {
                return;
            };
            if self.text(field) != "ServeHTTP" || operand.kind() != "selector_expression" {
                return;
            }
            let (Some(base), Some(member)) = (
                operand.child_by_field_name("operand"),
                operand.child_by_field_name("field"),
            ) else {
                return;
            };
            if self.text(base) == receiver {
                delegate = Some(self.text(member).to_string());
            }
        });
        delegate
    }

    /// `func NewHandler(db *DB) *Handler`: the constructor stands for the type, so
    /// `NewHandler(db).Routes` is `Handler.Routes`.
    fn constructor_alias(&mut self, node: Node<'f>, symbol: &str) {
        let Some(result) = node.child_by_field_name("result") else {
            return;
        };
        let first = if result.kind() == "parameter_list" {
            named_children(result)
                .into_iter()
                .next()
                .and_then(|p| p.child_by_field_name("type"))
        } else {
            Some(result)
        };
        let Some(first) = first else {
            return;
        };
        let (package, base) = split_qualified(self.text(first));
        if package.is_some() || !self.structs.contains_key(base) {
            return; // a framework type, a builtin, an error
        }
        self.sink.export(ExportFact {
            module: self.module(),
            exported: symbol.to_string(),
            local: base.to_string(),
        });
    }

    /// Read one function: its parameters, what it returns, then its body.
    fn body(&mut self, node: Node<'f>, symbol: String, receiver: Option<(String, String)>) {
        let Some(body) = node.child_by_field_name("body") else {
            return;
        };
        let mut scope = Scope {
            receiver,
            function: Some(symbol.clone()),
            ..Scope::default()
        };

        let mut router_params = false;
        if let Some(list) = node.child_by_field_name("parameters") {
            for (name, ty) in self.parameters(list) {
                match self.router_type(ty) {
                    Some(flavour) => {
                        router_params = true;
                        scope.routers.insert(
                            name,
                            RouterExpr {
                                name: symbol.clone(),
                                flavour,
                                auth: None,
                                trusted: true,
                            },
                        );
                    }
                    None => {
                        scope
                            .types
                            .insert(name, self.text(ty).trim_start_matches('*').to_string());
                    }
                }
            }
        }

        walk(body, &mut |n| {
            if n.kind() != "return_statement" {
                return;
            }
            if let Some(list) = child_of_kind(n, "expression_list") {
                for value in named_children(list) {
                    let value = unwrap(value);
                    if value.kind() == "identifier" {
                        scope.returned.insert(self.text(value).to_string());
                    }
                }
            }
        });

        let index = self.functions.len();
        self.functions.push(FunctionRouter {
            symbol,
            activity: false,
            is_app_root: false,
            span: self.span(node),
        });

        self.visit(body, &mut scope);

        // A function nobody registered anything through is not a router — unless it takes
        // one, in which case it is declared implicitly so `setup(r)` resolves quietly.
        if !(self.functions[index].activity || router_params) {
            self.functions.remove(index);
        }
    }

    /// `(a, b string, c ...int)` → each name with its type node.
    fn parameters(&self, list: Node<'f>) -> Vec<(String, Node<'f>)> {
        let mut out = Vec::new();
        for param in named_children(list) {
            let Some(ty) = param.child_by_field_name("type") else {
                continue;
            };
            for name in children_by_field(param, "name") {
                out.push((self.text(name).to_string(), ty));
            }
        }
        out
    }

    /// The flavour of a router type — `*gin.RouterGroup`, `chi.Router` — or nothing. A
    /// type from outside the frameworks whose name says "router" is taken as one too, so
    /// a project's own `Router` interface over Gin still works.
    fn router_type(&self, ty: Node<'_>) -> Option<Flavour> {
        let text = self.text(ty).trim_start_matches('*');
        let (package, name) = split_qualified(text);
        match package.and_then(|p| self.packages.get(p)) {
            Some(flavour) => flavour.is_router_type(name).then_some(*flavour),
            None => {
                let lowered = name.to_ascii_lowercase();
                (lowered.contains("router") || lowered.contains("mux")).then_some(Flavour::Unknown)
            }
        }
    }

    /// Walk a function body in source order, so declarations are seen before their uses.
    /// A closure is read in a copy of the scope: it sees everything outside, and what it
    /// declares stays inside.
    fn visit(&mut self, node: Node<'f>, scope: &mut Scope) {
        if self.consumed.contains(&node.id()) {
            return;
        }
        match node.kind() {
            "short_var_declaration" | "assignment_statement" => {
                let names = node
                    .child_by_field_name("left")
                    .map(named_children)
                    .unwrap_or_default();
                let values = node
                    .child_by_field_name("right")
                    .map(named_children)
                    .unwrap_or_default();
                // `x, err := f()`: one value for several names binds the first only.
                for (index, name) in names.iter().enumerate() {
                    let value = if values.len() == names.len() {
                        values.get(index)
                    } else if index == 0 {
                        values.first()
                    } else {
                        None
                    };
                    if let Some(value) = value {
                        self.bind(scope, *name, *value, node);
                    }
                }
            }
            "var_spec" => self.var_spec(node, scope),
            "call_expression" => self.call(node, scope),
            "composite_literal" => self.literal(node, scope),
            "func_literal" => {
                let mut inner = scope.clone();
                if let Some(list) = node.child_by_field_name("parameters") {
                    for (name, ty) in self.parameters(list) {
                        inner
                            .types
                            .insert(name, self.text(ty).trim_start_matches('*').to_string());
                    }
                }
                for child in named_children(node) {
                    self.visit(child, &mut inner);
                }
                return;
            }
            _ => {}
        }
        for child in named_children(node) {
            self.visit(child, scope);
        }
    }

    /// `var r *gin.Engine`: a router that will be assigned later, or elsewhere.
    fn bind_type(&mut self, scope: &mut Scope, name: Node<'f>, ty: Node<'f>) {
        if name.kind() != "identifier" {
            return;
        }
        let name = self.text(name).to_string();
        match self.router_type(ty) {
            Some(flavour) => {
                let symbol = self.symbol_for(scope, &name);
                scope.routers.insert(
                    name,
                    RouterExpr {
                        name: symbol,
                        flavour,
                        auth: None,
                        trusted: true,
                    },
                );
            }
            None => {
                scope
                    .types
                    .insert(name, self.text(ty).trim_start_matches('*').to_string());
            }
        }
    }

    /// The symbol a local gets: `main.r` inside `main`, `r` at package level. Locals are
    /// never referenced from another file, so the function's name keeps two `r`s apart.
    fn symbol_for(&self, scope: &Scope, name: &str) -> String {
        match &scope.function {
            Some(function) => format!("{function}.{name}"),
            None => name.to_string(),
        }
    }

    /// `name = value`, in every spelling.
    fn bind(&mut self, scope: &mut Scope, name: Node<'f>, value: Node<'f>, at: Node<'f>) {
        let target = match name.kind() {
            "identifier" => self.text(name).to_string(),
            "selector_expression" => {
                let (Some(operand), Some(field)) = (
                    name.child_by_field_name("operand"),
                    name.child_by_field_name("field"),
                ) else {
                    return;
                };
                let field = self.text(field);
                // `srv.Handler = r`: the server serves the router.
                if field == "Handler" {
                    if let Some(router) = self.served(scope, value) {
                        self.mark_root(router, self.span(at));
                    }
                    return;
                }
                // `s.router = chi.NewRouter()`: a field of the receiver's type.
                let Some(ty) = self.type_of(scope, operand) else {
                    return;
                };
                format!("{ty}.{field}")
            }
            _ => return,
        };
        if target == "_" {
            return;
        }
        let symbol = if name.kind() == "identifier" {
            self.symbol_for(scope, &target)
        } else {
            target.clone()
        };

        let value = unwrap(value);
        match value.kind() {
            "call_expression" => {
                if let Some((flavour, is_root)) = self.constructor(value) {
                    self.consumed.insert(value.id());
                    self.declare(scope, &target, symbol, flavour, is_root, self.span(at));
                    return;
                }
                if let Some((parent, prefix, auth, methods)) = self.group_call(scope, value) {
                    self.consumed.insert(value.id());
                    let returned = scope.returned.contains(&target);
                    let child = match (&scope.function, returned) {
                        // `func Routes() chi.Router { r := parent.Group(...); return r }`
                        (Some(function), true) => function.clone(),
                        _ => symbol,
                    };
                    if self.is_own(scope, &parent) {
                        self.activity();
                    }
                    if returned && scope.function.is_some() {
                        self.activity();
                    } else {
                        self.declared.push(Declared {
                            symbol: child.clone(),
                            is_app_root: false,
                            served: false,
                            mounted: true,
                            group: group_from_prefix(&prefix),
                            span: self.span(at),
                        });
                    }
                    self.sink.mount(MountFact {
                        parent: SymbolRef::new(self.module(), parent.name.clone()),
                        child: SymbolRef::new(self.module(), child.clone()),
                        prefix,
                        group: None,
                        auth,
                        methods,
                        replaces_child_prefix: false,
                        span: self.span(at),
                    });
                    scope.routers.insert(
                        target,
                        RouterExpr {
                            name: child,
                            flavour: parent.flavour,
                            auth: None,
                            trusted: true,
                        },
                    );
                    return;
                }
                let Some(function) = value.child_by_field_name("function") else {
                    return;
                };
                // `x := new(T)`
                if self.text(function) == "new" {
                    if let Some(ty) = arguments(value).first() {
                        scope
                            .types
                            .insert(target, split_qualified(self.text(*ty)).1.to_string());
                    }
                    return;
                }
                // `srv := api.New(db)` — whatever it returns, `srv` stands for the call,
                // as long as the call is into this project.
                if let Some(callee) = self.callee_name(scope, value) {
                    if self.is_project_name(&callee) {
                        scope.aliases.insert(target, (callee, true));
                    }
                }
            }
            "composite_literal" => {
                if let Some(ty) = value.child_by_field_name("type") {
                    let ty = self.text(ty).trim_start_matches('*').to_string();
                    let (package, base) = split_qualified(&ty);
                    if package.is_none() && self.structs.contains_key(base) {
                        scope
                            .aliases
                            .insert(target.clone(), (base.to_string(), false));
                    }
                    scope.types.insert(target, ty);
                }
            }
            "identifier" => {
                let source = self.text(value).to_string();
                if let Some(router) = scope.routers.get(&source).cloned() {
                    scope.routers.insert(target, router);
                } else if let Some(alias) = scope.aliases.get(&source).cloned() {
                    scope.aliases.insert(target, alias);
                }
            }
            _ => {
                if let Some(text) = self.string_value(scope, value) {
                    scope.constants.insert(target, text);
                }
            }
        }
    }

    /// Whether a dotted name starts inside this project: an imported package of its own,
    /// a function or type in this file.
    fn is_project_name(&self, name: &str) -> bool {
        let head = name.split('.').next().unwrap_or(name);
        self.project_packages.contains(head)
            || self.function_names.contains(head)
            || self.structs.contains_key(head)
    }

    fn declare(
        &mut self,
        scope: &mut Scope,
        local: &str,
        symbol: String,
        flavour: Flavour,
        is_root: bool,
        span: Span,
    ) {
        // Returned from the function: the function is the router, and it is the caller's
        // to serve or mount.
        if scope.returned.contains(local) {
            if let Some(function) = scope.function.clone() {
                self.activity();
                scope.routers.insert(
                    local.to_string(),
                    RouterExpr {
                        name: function,
                        flavour,
                        auth: None,
                        trusted: true,
                    },
                );
                return;
            }
        }
        match self.declared.iter_mut().find(|d| d.symbol == symbol) {
            Some(existing) => existing.is_app_root |= is_root,
            None => self.declared.push(Declared {
                symbol: symbol.clone(),
                is_app_root: is_root,
                served: false,
                mounted: false,
                group: None,
                span,
            }),
        }
        let router = RouterExpr {
            name: symbol,
            flavour,
            auth: None,
            trusted: true,
        };
        if scope.function.is_some() || local.contains('.') {
            scope.routers.insert(local.to_string(), router);
        } else {
            self.package_routers.insert(local.to_string(), router);
        }
    }

    /// `gin.Default()`, `chi.NewRouter()` — the flavour, and whether it is a root by itself.
    fn constructor(&self, call: Node<'_>) -> Option<(Flavour, bool)> {
        let function = call.child_by_field_name("function")?;
        if function.kind() != "selector_expression" {
            return None;
        }
        let package = self.text(function.child_by_field_name("operand")?);
        let name = self.text(function.child_by_field_name("field")?);
        let flavour = *self.packages.get(package)?;
        flavour.constructor(name).map(|root| (flavour, root))
    }

    /// `r.Group("/v1", mw...)`, `r.PathPrefix("/api").Subrouter()`, `r.With(mw)`,
    /// `r.Methods("GET").Subrouter()`: a router carved out of another, with the prefix,
    /// the guards and the method restriction the carving stated.
    fn group_call(
        &mut self,
        scope: &mut Scope,
        call: Node<'f>,
    ) -> Option<(
        RouterExpr,
        PathTemplate,
        Option<AuthRequirement>,
        Vec<HttpMethod>,
    )> {
        let (root, pieces) = chain(self.file, call);
        let root = self.router_of(scope, root?)?;
        if !root.trusted {
            return None;
        }
        let mut prefix = PathTemplate::empty();
        let mut auth = None;
        let mut methods = Vec::new();
        let mut carved = false;
        for (name, piece) in &pieces {
            let args = arguments(*piece);
            match name.as_str() {
                "Group" | "Route" => {
                    let first = args.first()?;
                    if first.kind() == "func_literal" {
                        // chi's `r.Group(func(r chi.Router) {...})` — no prefix, and the
                        // literal is read where the call is.
                        carved = true;
                        continue;
                    }
                    if !self.is_path_argument(scope, *first, true) {
                        return None;
                    }
                    prefix = prefix.join(&self.path_of(scope, *first));
                    for middleware in &args[1..] {
                        if auth.is_none() {
                            auth = self.auth_of(scope, *middleware);
                        }
                    }
                    carved = true;
                }
                "PathPrefix" | "Path" => {
                    let first = args.first()?;
                    prefix = prefix.join(&self.path_of(scope, *first));
                }
                "Methods" => methods = self.methods_of(&args),
                "Subrouter" => carved = true,
                "With" | "Use" => {
                    for middleware in &args {
                        if auth.is_none() {
                            auth = self.auth_of(scope, *middleware);
                        }
                    }
                    carved |= *name == "With";
                }
                "Name" | "Host" | "Schemes" | "Headers" | "Queries" => {}
                _ => return None,
            }
        }
        carved.then_some((root, prefix, auth, methods))
    }

    /// The router an expression names, if it names one at all.
    fn router_of(&mut self, scope: &mut Scope, node: Node<'f>) -> Option<RouterExpr> {
        let node = unwrap(node);
        match node.kind() {
            "identifier" => {
                let name = self.text(node);
                if let Some(router) = scope.routers.get(name) {
                    return Some(router.clone());
                }
                if let Some(router) = self.package_routers.get(name) {
                    return Some(router.clone());
                }
                if let Some((alias, from_call)) = scope.aliases.get(name) {
                    return from_call.then(|| RouterExpr {
                        name: alias.clone(),
                        flavour: Flavour::Unknown,
                        auth: None,
                        trusted: true,
                    });
                }
                if self.packages.get(name) == Some(&Flavour::Http) {
                    return Some(self.default_mux(node));
                }
                if let Some((receiver, ty)) = &scope.receiver {
                    if receiver == name {
                        return Some(RouterExpr {
                            name: ty.clone(),
                            flavour: Flavour::Unknown,
                            auth: None,
                            trusted: true,
                        });
                    }
                }
                if scope.types.contains_key(name)
                    || scope.constants.contains_key(name)
                    || self.constants.contains_key(name)
                    || self.externals.contains(name)
                    || self.project_packages.contains(name)
                    || self.packages.contains_key(name)
                    || self.function_names.contains(name)
                    || name == "nil"
                {
                    return None;
                }
                // Not declared here: a package-level router in a sibling file, or nothing.
                Some(RouterExpr {
                    name: name.to_string(),
                    flavour: Flavour::Unknown,
                    auth: None,
                    trusted: looks_like_router(name),
                })
            }
            "selector_expression" => {
                let (Some(operand), Some(field)) = (
                    node.child_by_field_name("operand"),
                    node.child_by_field_name("field"),
                ) else {
                    return None;
                };
                let field = self.text(field);
                let operand = unwrap(operand);
                if operand.kind() == "identifier" {
                    let base = self.text(operand);
                    if self.externals.contains(base) {
                        return None;
                    }
                    // A field of a declared router — `e.Logger` — is not a router.
                    if scope.routers.contains_key(base) || self.package_routers.contains_key(base) {
                        return None;
                    }
                    if scope.aliases.contains_key(base) {
                        return None; // `srv.Handler`, whatever `srv` stands for
                    }
                    if self.project_packages.contains(base) {
                        return Some(RouterExpr {
                            name: format!("{base}.{field}"),
                            flavour: Flavour::Unknown,
                            auth: None,
                            trusted: true,
                        });
                    }
                    // `s.router`, on the receiver or a local of known type.
                    if let Some(ty) = self.type_of(scope, operand) {
                        let (package, base) = split_qualified(&ty);
                        if package.is_some() {
                            return None; // a field of `http.Server`, `sql.DB`
                        }
                        return Some(RouterExpr {
                            name: format!("{base}.{field}"),
                            flavour: Flavour::Unknown,
                            auth: None,
                            trusted: true,
                        });
                    }
                }
                None
            }
            "call_expression" => {
                // `Register(r.Group("/users"))`: a group made on the spot is a router
                // mounted at its prefix.
                let (parent, prefix, auth, methods) = self.group_call(scope, node)?;
                self.consumed.insert(node.id());
                let symbol = synthetic_symbol(&parent.name, self.span(node));
                Some(self.synthetic(parent, symbol, prefix, auth, methods, node))
            }
            _ => None,
        }
    }

    /// The normalised text an operand contributes to a dotted reference: `handlers`
    /// stays, `h` becomes what it was assigned, `&Server{}` becomes `Server`,
    /// `NewHandler(db)` becomes `NewHandler`.
    fn qualifier(&self, scope: &Scope, node: Node<'f>) -> Option<String> {
        let node = unwrap(node);
        match node.kind() {
            "identifier" => {
                let name = self.text(node);
                if let Some((alias, _)) = scope.aliases.get(name) {
                    return Some(alias.clone());
                }
                if let Some((receiver, ty)) = &scope.receiver {
                    if receiver == name {
                        return Some(ty.clone());
                    }
                }
                if let Some(ty) = scope.types.get(name) {
                    return Some(split_qualified(ty).1.to_string());
                }
                Some(name.to_string())
            }
            "selector_expression" => {
                let (Some(operand), Some(field)) = (
                    node.child_by_field_name("operand"),
                    node.child_by_field_name("field"),
                ) else {
                    return None;
                };
                let base = self.qualifier(scope, operand)?;
                Some(format!("{base}.{}", self.text(field)))
            }
            "call_expression" => {
                let function = node.child_by_field_name("function")?;
                self.qualifier(scope, function)
            }
            "composite_literal" => {
                let ty = node.child_by_field_name("type")?;
                Some(split_qualified(self.text(ty)).1.to_string())
            }
            _ => None,
        }
    }

    /// `users.Register` for the call `users.Register(v1)`; `NewHandler.Routes` for
    /// `NewHandler(db).Routes()`.
    fn callee_name(&self, scope: &Scope, call: Node<'f>) -> Option<String> {
        let function = call.child_by_field_name("function")?;
        self.qualifier(scope, function)
    }

    /// The type a local was given, when it is known.
    fn type_of(&self, scope: &Scope, node: Node<'_>) -> Option<String> {
        let node = unwrap(node);
        if node.kind() != "identifier" {
            return None;
        }
        let name = self.text(node);
        if let Some((receiver, ty)) = &scope.receiver {
            if receiver == name {
                return Some(ty.clone());
            }
        }
        scope.types.get(name).cloned()
    }

    fn default_mux(&mut self, at: Node<'_>) -> RouterExpr {
        if self.default_mux.is_none() {
            self.default_mux = Some(self.span(at));
        }
        RouterExpr {
            name: "http.DefaultServeMux".to_string(),
            flavour: Flavour::Http,
            auth: None,
            trusted: true,
        }
    }

    /// Declare a router that exists only as an expression — `r.Group("/v1").GET(...)`,
    /// `Register(r.Group("/users"))` — and mount it where it was made.
    fn synthetic(
        &mut self,
        parent: RouterExpr,
        symbol: String,
        prefix: PathTemplate,
        auth: Option<AuthRequirement>,
        methods: Vec<HttpMethod>,
        at: Node<'_>,
    ) -> RouterExpr {
        if self.synthetic.insert(symbol.clone()) {
            self.declared.push(Declared {
                symbol: symbol.clone(),
                is_app_root: false,
                served: false,
                mounted: true,
                group: group_from_prefix(&prefix),
                span: self.span(at),
            });
            self.sink.mount(MountFact {
                parent: SymbolRef::new(self.module(), parent.name.clone()),
                child: SymbolRef::new(self.module(), symbol.clone()),
                prefix,
                group: None,
                auth,
                methods,
                replaces_child_prefix: false,
                span: self.span(at),
            });
        }
        RouterExpr {
            name: symbol,
            flavour: parent.flavour,
            auth: None,
            trusted: true,
        }
    }

    /// Note that something happened on the function being read, so it is declared as a
    /// router in its own right.
    fn activity(&mut self) {
        if let Some(function) = self.functions.last_mut() {
            function.activity = true;
        }
    }

    /// Whether a router expression is the function being read — its parameter, or the
    /// router it returns — so that registering on it is registering on the function.
    fn is_own(&self, scope: &Scope, router: &RouterExpr) -> bool {
        scope.function.as_deref() == Some(router.name.as_str())
    }

    /// A router is being served: the root of everything under it.
    fn mark_root(&mut self, router: RouterExpr, span: Span) {
        if let Some(declared) = self.declared.iter_mut().find(|d| d.symbol == router.name) {
            declared.served = true;
            return;
        }
        if let Some(function) = self.functions.iter_mut().find(|f| f.symbol == router.name) {
            function.activity = true;
            function.is_app_root = true;
            return;
        }
        if router.name == "http.DefaultServeMux" || !router.trusted {
            return;
        }
        // Declared elsewhere — `http.ListenAndServe(":8080", api.New())` — so a server
        // root stands in, with the router mounted at `/` beneath it.
        if self.server_root.is_none() {
            self.server_root = Some(span);
        }
        self.sink.mount(MountFact {
            parent: SymbolRef::new(self.module(), "http.Server"),
            child: SymbolRef::new(self.module(), router.name),
            prefix: PathTemplate::empty(),
            group: None,
            auth: None,
            methods: Vec::new(),
            replaces_child_prefix: false,
            span,
        });
    }

    /// What a serve call is handed: a router, a call that builds one, or a value of a
    /// type that serves one from its `ServeHTTP`.
    fn served(&mut self, scope: &mut Scope, handler: Node<'f>) -> Option<RouterExpr> {
        if let Some(router) = self.router_of(scope, handler) {
            return router.trusted.then_some(router);
        }
        let handler = unwrap(handler);
        let child = match handler.kind() {
            "identifier" => match scope.aliases.get(self.text(handler)) {
                Some((alias, _)) => alias.clone(),
                None => return None,
            },
            _ => self.mount_child_of_call(scope, handler)?,
        };
        Some(RouterExpr {
            name: child,
            flavour: Flavour::Unknown,
            auth: None,
            trusted: true,
        })
    }

    /// `&http.Server{Addr: ":8080", Handler: r}`; `&Server{router: chi.NewRouter()}`.
    fn literal(&mut self, node: Node<'f>, scope: &mut Scope) {
        let (Some(ty), Some(body)) = (
            node.child_by_field_name("type"),
            node.child_by_field_name("body"),
        ) else {
            return;
        };
        let ty = self.text(ty).to_string();
        let (package, base) = split_qualified(&ty);
        let is_server =
            base == "Server" && package.and_then(|p| self.packages.get(p)) == Some(&Flavour::Http);

        for element in named_children(body) {
            if element.kind() != "keyed_element" {
                continue;
            }
            let (Some(key), Some(value)) = (
                element.child_by_field_name("key"),
                element.child_by_field_name("value"),
            ) else {
                continue;
            };
            let key = self.text(key).to_string();
            let value = named_children(value).into_iter().next().unwrap_or(value);
            if is_server {
                match key.as_str() {
                    "Handler" => {
                        if let Some(router) = self.served(scope, value) {
                            self.mark_root(router, self.span(node));
                        }
                    }
                    "Addr" => {
                        self.listen_port(scope, value);
                    }
                    _ => {}
                }
                continue;
            }
            if package.is_none() {
                if let Some((flavour, _)) = self.constructor(unwrap(value)) {
                    self.consumed.insert(unwrap(value).id());
                    let symbol = format!("{base}.{key}");
                    self.declare(
                        scope,
                        &symbol,
                        symbol.clone(),
                        flavour,
                        false,
                        self.span(node),
                    );
                }
            }
        }
    }

    /// A call: a chain of method pieces on a router, or a function handed one.
    fn call(&mut self, node: Node<'f>, scope: &mut Scope) {
        // Only the top of a chain: `r.Route(...).Get(...)` is read once, from `Get`.
        if let Some(parent) = node.parent() {
            if parent.kind() == "selector_expression"
                && parent
                    .parent()
                    .is_some_and(|grand| grand.kind() == "call_expression")
            {
                return;
            }
        }

        let (root, pieces) = chain(self.file, node);
        if pieces.is_empty() {
            return;
        }

        // `http.ListenAndServe(":8080", r)` and friends on the package itself.
        if let Some(root_node) = root {
            if root_node.kind() == "identifier"
                && self.packages.get(self.text(root_node)) == Some(&Flavour::Http)
                && pieces.len() == 1
            {
                let (name, call) = &pieces[0];
                let args = arguments(*call);
                match name.as_str() {
                    "ListenAndServe" | "ListenAndServeTLS" | "Serve" | "ServeTLS" => {
                        let handler_index = if name == "ListenAndServeTLS" { 3 } else { 1 };
                        if name.starts_with("ListenAndServe") {
                            if let Some(addr) = args.first() {
                                self.listen_port(scope, *addr);
                            }
                        }
                        if let Some(handler) = args.get(handler_index) {
                            if let Some(router) = self.served(scope, *handler) {
                                self.mark_root(router, self.span(*call));
                            }
                        }
                    }
                    "HandleFunc" | "Handle" => {
                        let mux = self.default_mux(*call);
                        self.pieces(scope, mux, &pieces, node);
                    }
                    _ => {}
                }
                return;
            }
        }

        let router = root.and_then(|r| self.router_of(scope, r));

        // A function handed a router is a mount of that function — and so is a method
        // on something that only *might* be a router, `h := handlers.New(db); h.Mount(r)`.
        //   users.Register(v1)   h.Routes(api.Group("/x"))   setup(r, db)
        let root_is_alias = root
            .is_some_and(|r| r.kind() == "identifier" && scope.aliases.contains_key(self.text(r)));
        if pieces.len() == 1 && (router.is_none() || root_is_alias) {
            let (_, call) = &pieces[0];
            let external = root
                .is_some_and(|r| r.kind() == "identifier" && self.externals.contains(self.text(r)));
            let args = arguments(*call);
            let parent = if external {
                None
            } else {
                args.iter()
                    .find_map(|arg| self.router_of(scope, *arg).filter(|r| r.trusted))
            };
            if let Some(parent) = parent {
                if let Some(child) = self.callee_name(scope, *call) {
                    if parent.name != child {
                        self.mount(scope, &parent, child, PathTemplate::empty(), None, *call);
                        return;
                    }
                }
            }
        }

        if let Some(router) = router {
            self.pieces(scope, router, &pieces, node);
        }
    }

    /// The pieces of a chain, on a router.
    fn pieces(
        &mut self,
        scope: &mut Scope,
        router: RouterExpr,
        pieces: &[(String, Node<'f>)],
        chain_node: Node<'f>,
    ) {
        let mut current = router;
        let gorilla_style = pieces
            .iter()
            .any(|(name, _)| GORILLA_PIECES.contains(&name.as_str()));
        let mut pending = Pending::default();
        let none = Pending::default();

        for (name, call) in pieces {
            let args = arguments(*call);
            let name = name.as_str();

            if let Some((_, method)) = UPPER_METHODS.iter().find(|(m, _)| *m == name) {
                if args
                    .first()
                    .is_some_and(|first| self.is_path_argument(scope, *first, true))
                {
                    let path = self.path_of(scope, args[0]);
                    self.route(
                        scope,
                        &current,
                        vec![method.clone()],
                        false,
                        path,
                        &args[1..],
                        *call,
                        &none,
                    );
                }
                continue;
            }
            if let Some((_, method)) = TITLE_METHODS.iter().find(|(m, _)| *m == name) {
                // `resty.R().Get(url)` and `http.Get(url)` take one argument; a route takes
                // a handler too, and `cache.Delete("/key", 1)` does not.
                if args.len() >= 2
                    && self.is_path_argument(scope, args[0], false)
                    && is_handler_like(args[1])
                {
                    let path = self.path_of(scope, args[0]);
                    self.route(
                        scope,
                        &current,
                        vec![method.clone()],
                        false,
                        path,
                        &args[1..],
                        *call,
                        &none,
                    );
                }
                continue;
            }

            match name {
                "Any" | "All" => {
                    if args
                        .first()
                        .is_some_and(|f| self.is_path_argument(scope, *f, true))
                    {
                        let path = self.path_of(scope, args[0]);
                        self.route(
                            scope,
                            &current,
                            Vec::new(),
                            true,
                            path,
                            &args[1..],
                            *call,
                            &none,
                        );
                    }
                }
                "Handle" | "HandleFunc" => {
                    // Gin: `Handle("GET", "/x", h)`. Everyone else: `Handle("/x", h)`.
                    if name == "Handle" && args.len() >= 3 {
                        if let Some(method) = self.method_of(&args[0]) {
                            if self.is_path_argument(scope, args[1], true) {
                                let path = self.path_of(scope, args[1]);
                                self.route(
                                    scope,
                                    &current,
                                    vec![method],
                                    false,
                                    path,
                                    &args[2..],
                                    *call,
                                    &none,
                                );
                                continue;
                            }
                        }
                    }
                    if args.len() < 2
                        || !self.is_path_argument(scope, args[0], false)
                        || !is_handler_like(args[1])
                    {
                        continue;
                    }
                    let (methods, path) = self.pattern_of(scope, args[0]);
                    let handler = args[1];
                    if self.text(handler).contains("FileServer(") {
                        continue; // serves files, not an API
                    }
                    if name == "Handle" {
                        if let Some((child, prefix)) = self.mounted_handler(scope, handler, &path) {
                            self.mount(scope, &current, child, prefix, None, *call);
                            continue;
                        }
                    }
                    if gorilla_style {
                        pending.path = Some(path);
                        pending.handler = Some(handler);
                        pending.methods = methods;
                    } else {
                        let unspecified = methods.is_empty();
                        self.route(
                            scope,
                            &current,
                            methods,
                            unspecified,
                            path,
                            &args[1..],
                            *call,
                            &none,
                        );
                    }
                }
                "Method" | "MethodFunc" | "Add" => {
                    if args.len() >= 3 && self.is_path_argument(scope, args[1], true) {
                        let method = self.method_of(&args[0]);
                        let path = self.path_of(scope, args[1]);
                        let unspecified = method.is_none();
                        self.route(
                            scope,
                            &current,
                            method.into_iter().collect(),
                            unspecified,
                            path,
                            &args[2..],
                            *call,
                            &none,
                        );
                    }
                }
                "Match" => {
                    if args.len() >= 3 && self.is_path_argument(scope, args[1], true) {
                        let methods = self.methods_of(&args[..1]);
                        let path = self.path_of(scope, args[1]);
                        let unspecified = methods.is_empty();
                        self.route(
                            scope,
                            &current,
                            methods,
                            unspecified,
                            path,
                            &args[2..],
                            *call,
                            &none,
                        );
                    }
                }
                "Group" | "Route" => {
                    let Some(first) = args.first() else { continue };
                    if first.kind() == "func_literal" {
                        // chi's `r.Group(func(r chi.Router) {...})`: the same router, with
                        // whatever `r.Use` inside applying to the routes inside.
                        let symbol = synthetic_symbol(&current.name, self.span(*call));
                        let group = self.synthetic(
                            current.clone(),
                            symbol,
                            PathTemplate::empty(),
                            None,
                            Vec::new(),
                            *call,
                        );
                        self.literal_router(scope, *first, group);
                        continue;
                    }
                    if !self.is_path_argument(scope, *first, true) {
                        continue;
                    }
                    let prefix = self.path_of(scope, *first);
                    let mut auth = None;
                    let mut literal = None;
                    for arg in &args[1..] {
                        if arg.kind() == "func_literal" {
                            literal = Some(*arg);
                        } else if auth.is_none() {
                            auth = self.auth_of(scope, *arg);
                        }
                    }
                    let symbol = synthetic_symbol(&current.name, self.span(*call));
                    let group =
                        self.synthetic(current.clone(), symbol, prefix, auth, Vec::new(), *call);
                    match literal {
                        // chi and Fiber: `r.Route("/users", func(r chi.Router) {...})`.
                        Some(literal) => self.literal_router(scope, literal, group),
                        // Gin, Echo, Fiber inline: `r.Group("/v1").GET(...)`.
                        None => current = group,
                    }
                }
                "With" => {
                    for arg in &args {
                        if current.auth.is_none() {
                            current.auth = self.auth_of(scope, *arg);
                        }
                    }
                }
                "Use" => {
                    // Fiber: `app.Use("/api", sub)` mounts; otherwise middleware.
                    if args.len() >= 2 && self.is_path_argument(scope, args[0], false) {
                        if let Some(child) = self.router_of(scope, args[1]).filter(|r| r.trusted) {
                            let prefix = self.path_of(scope, args[0]);
                            self.mount(scope, &current, child.name, prefix, None, *call);
                            continue;
                        }
                    }
                    let mut auth = None;
                    for arg in &args {
                        if auth.is_none() {
                            auth = self.auth_of(scope, *arg);
                        }
                        if arg.kind() == "func_literal" {
                            continue;
                        }
                        // Fiber: `app.Use(sub)` — a sub-app at the root.
                        if let Some(child) = self.router_of(scope, *arg).filter(|r| r.trusted) {
                            if child.name != current.name {
                                self.mount(
                                    scope,
                                    &current,
                                    child.name,
                                    PathTemplate::empty(),
                                    None,
                                    *call,
                                );
                            }
                        }
                    }
                    if let Some(auth) = auth {
                        if pieces.len() == 1 {
                            self.router_auth.insert(current.name.clone(), auth);
                        } else {
                            current.auth = Some(auth);
                        }
                    }
                }
                "Mount" => {
                    if args.len() >= 2 && self.is_path_argument(scope, args[0], true) {
                        let prefix = self.path_of(scope, args[0]);
                        if let Some((child, prefix)) = self.mounted_handler(scope, args[1], &prefix)
                        {
                            self.mount(scope, &current, child, prefix, None, *call);
                        } else if let Some(child) = self.mount_child_of_call(scope, args[1]) {
                            self.mount(scope, &current, child, prefix, None, *call);
                        } else if !self.is_external_call(args[1]) {
                            self.sink.warn(format!(
                                "{}:{}: `{}` is mounted at {} but could not be read as a router",
                                self.file.display_path(),
                                self.span(*call).line,
                                self.text(args[1]),
                                prefix
                            ));
                        }
                    }
                }
                // gorilla/mux pieces, folded into the pending route.
                "Path" => {
                    if let Some(first) = args.first() {
                        pending.path = Some(self.path_of(scope, *first));
                    }
                }
                "PathPrefix" => {
                    if let Some(first) = args.first() {
                        let mut path = self.path_of(scope, *first);
                        path.segments.push(PathSegment::Param {
                            name: "path".to_string(),
                            ty: Some(TypeHint::Path),
                            catch_all: true,
                            optional: false,
                        });
                        path.trailing_slash = false;
                        pending.path = Some(path);
                    }
                }
                "Methods" => pending.methods = self.methods_of(&args),
                "HandlerFunc" | "Handler" => pending.handler = args.first().copied(),
                "Queries" => {
                    for key in args.iter().step_by(2) {
                        if let Some(text) = string_literal(self.file, *key) {
                            pending.queries.push(text);
                        }
                    }
                }
                "Headers" => {
                    for key in args.iter().step_by(2) {
                        if let Some(text) = string_literal(self.file, *key) {
                            pending.headers.push(text);
                        }
                    }
                }
                "Subrouter" => {
                    // `r.PathPrefix("/api").Subrouter().HandleFunc(...)` inline.
                    let prefix = pending.path.take().unwrap_or_default();
                    let methods = std::mem::take(&mut pending.methods);
                    let symbol = synthetic_symbol(&current.name, self.span(*call));
                    current = self.synthetic(current.clone(), symbol, prefix, None, methods, *call);
                }
                serve if SERVE_METHODS.contains(&serve) => {
                    if !current.trusted {
                        continue; // `cmd.Run()`, `ln.Serve()` — not a router
                    }
                    let port = args.first().and_then(|a| self.listen_port(scope, *a));
                    if port.is_none() && current.flavour == Flavour::Gin && name == "Run" {
                        self.sink.port(
                            8080,
                            format!(
                                "gin's default, {}:{}",
                                self.file.display_path(),
                                self.span(*call).line
                            ),
                        );
                    }
                    if current.flavour == Flavour::Http {
                        continue;
                    }
                    let router = current.clone();
                    self.mark_root(router, self.span(*call));
                }
                _ => {}
            }
        }

        if gorilla_style {
            if let (Some(path), Some(handler)) = (pending.path.take(), pending.handler) {
                let methods = std::mem::take(&mut pending.methods);
                let unspecified = methods.is_empty();
                self.route(
                    scope,
                    &current,
                    methods,
                    unspecified,
                    path,
                    &[handler],
                    chain_node,
                    &pending,
                );
            }
        }
    }

    /// The body of `func(r chi.Router) {...}` handed to `Route` or `Group`, read with its
    /// parameter bound to the group.
    fn literal_router(&mut self, scope: &mut Scope, literal: Node<'f>, group: RouterExpr) {
        self.consumed.insert(literal.id());
        let Some(body) = literal.child_by_field_name("body") else {
            return;
        };
        let mut inner = scope.clone();
        if let Some(list) = literal.child_by_field_name("parameters") {
            for (name, _) in self.parameters(list) {
                inner.routers.insert(name, group.clone());
            }
        }
        self.visit(body, &mut inner);
    }

    /// For `Handle(path, X)` and `Mount(prefix, X)`: X as a mounted router, when it is
    /// one. `http.StripPrefix("/api", r)` mounts `r` under the stripped prefix.
    fn mounted_handler(
        &mut self,
        scope: &mut Scope,
        handler: Node<'f>,
        prefix: &PathTemplate,
    ) -> Option<(String, PathTemplate)> {
        let handler = unwrap(handler);
        if handler.kind() == "call_expression" {
            let function = handler.child_by_field_name("function")?;
            if !self.text(function).ends_with("StripPrefix") {
                return None;
            }
            let args = arguments(handler);
            let inner = *args.get(1)?;
            let stripped = self.path_of(scope, args[0]);
            if let Some(router) = self.router_of(scope, inner).filter(|r| r.trusted) {
                return Some((router.name, stripped));
            }
            return self
                .mount_child_of_call(scope, inner)
                .map(|child| (child, stripped));
        }
        let router = self.router_of(scope, handler).filter(|r| r.trusted)?;
        Some((router.name, prefix.clone()))
    }

    /// `users.Routes()`, `NewHandler(db).Routes()`: the router a call produces, named by
    /// what was called, for the graph to trace to a function that returns one.
    fn mount_child_of_call(&mut self, scope: &mut Scope, node: Node<'f>) -> Option<String> {
        let node = unwrap(node);
        if node.kind() != "call_expression" || self.is_external_call(node) {
            return None;
        }
        let function = node.child_by_field_name("function")?;
        if self.text(function) == "new" {
            return None;
        }
        self.callee_name(scope, node)
    }

    /// `http.FileServer(...)`, `promhttp.Handler()`: a call into a package outside the
    /// project, which nothing here can follow.
    fn is_external_call(&self, node: Node<'_>) -> bool {
        let node = unwrap(node);
        if node.kind() != "call_expression" {
            return false;
        }
        node.child_by_field_name("function")
            .and_then(leftmost_identifier)
            .is_some_and(|name| self.externals.contains(self.text(name)))
    }

    fn mount(
        &mut self,
        scope: &Scope,
        parent: &RouterExpr,
        child: String,
        prefix: PathTemplate,
        auth: Option<AuthRequirement>,
        at: Node<'_>,
    ) {
        if self.is_own(scope, parent) {
            self.activity();
        }
        if let Some(declared) = self.declared.iter_mut().find(|d| d.symbol == child) {
            declared.mounted = true;
        }
        self.sink.mount(MountFact {
            parent: SymbolRef::new(self.module(), parent.name.clone()),
            child: SymbolRef::new(self.module(), child),
            prefix,
            group: None,
            auth: auth.or_else(|| parent.auth.clone()),
            methods: Vec::new(),
            replaces_child_prefix: false,
            span: self.span(at),
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn route(
        &mut self,
        scope: &Scope,
        router: &RouterExpr,
        methods: Vec<HttpMethod>,
        unspecified: bool,
        path: PathTemplate,
        handlers: &[Node<'f>],
        at: Node<'f>,
        pending: &Pending<'f>,
    ) {
        if self.is_own(scope, router) {
            self.activity();
        }

        let mut usage = Usage::default();
        let mut auth = router.auth.clone();
        for handler in handlers {
            if auth.is_none() && handler.kind() != "func_literal" {
                auth = self.auth_of(scope, *handler);
            }
            if let Some(function) = self.handler_function(*handler) {
                usage.merge(self.usage_of(function));
            }
        }
        // The last argument is the handler; the rest is middleware. Named, it may be
        // declared in the next file over, and the scan joins the two afterwards.
        let named_handler = handlers.last().and_then(|handler| {
            let mut handler = unwrap(*handler);
            if handler.kind() == "call_expression"
                && handler
                    .child_by_field_name("function")
                    .is_some_and(|f| self.text(f).ends_with("HandlerFunc"))
            {
                handler = unwrap(*arguments(handler).first()?);
            }
            match handler.kind() {
                "identifier" | "selector_expression" => self.qualifier(scope, handler),
                _ => None,
            }
        });
        if auth.is_none() {
            auth = self.router_auth.get(&router.name).cloned();
        }

        let (methods, unspecified) = if unspecified {
            if usage.checked.is_empty() {
                (UNSPECIFIED_METHODS.to_vec(), true)
            } else {
                (usage.checked.clone(), false)
            }
        } else {
            (methods, false)
        };

        // The span of the method piece — `.POST` — rather than the whole chain, so a
        // chain written one route per line lands on the right one.
        let span = at
            .child_by_field_name("function")
            .and_then(|f| f.child_by_field_name("field"))
            .map(|f| self.span(f))
            .unwrap_or_else(|| self.span(at));

        let path_params: Vec<String> = path.param_names().iter().map(|s| s.to_string()).collect();
        let mut fact = RouteFact::new(
            SymbolRef::new(self.module(), router.name.clone()),
            methods[0].clone(),
            path,
            span,
        );
        fact.methods = methods;
        let mut query: Vec<String> = pending.queries.clone();
        for name in &usage.query {
            if !query.contains(name) && !path_params.contains(name) {
                query.push(name.clone());
            }
        }
        fact.query_params = query.iter().map(ParamSpec::new).collect();
        let mut headers: Vec<String> = pending.headers.clone();
        for name in &usage.headers {
            if !headers.contains(name) {
                headers.push(name.clone());
            }
        }
        fact.headers = headers.iter().map(ParamSpec::new).collect();
        fact.body = usage.body();
        fact.auth = auth;
        fact.handler = named_handler.map(|name| SymbolRef::new(self.module(), name));
        promote_authorization_header(&mut fact.headers, &mut fact.auth);
        if unspecified {
            fact.summary = Some(
                "registered without a method, and the handler never checks `r.Method`, so any method reaches it".into(),
            );
        }
        self.sink.route(fact);
    }

    /// The function a handler argument names, when it is in this file: a literal, a
    /// function, a method, `http.HandlerFunc(f)`, or a factory call `list(db)` whose
    /// body — closure included — is then read.
    fn handler_function(&self, node: Node<'f>) -> Option<Node<'f>> {
        let node = unwrap(node);
        match node.kind() {
            "func_literal" => Some(node),
            "identifier" => top_level_function(self.file, self.text(node)),
            "selector_expression" => {
                let operand = unwrap(node.child_by_field_name("operand")?);
                let field = node.child_by_field_name("field")?;
                if operand.kind() == "identifier" {
                    let base = self.text(operand);
                    if self.externals.contains(base) || self.project_packages.contains(base) {
                        return None;
                    }
                }
                method_declaration(self.file, self.text(field))
            }
            "call_expression" => {
                let function = node.child_by_field_name("function")?;
                if self.text(function).ends_with("HandlerFunc") {
                    return self.handler_function(*arguments(node).first()?);
                }
                self.handler_function(function)
            }
            _ => None,
        }
    }

    /// What a handler reads off its request.
    fn usage_of(&self, function: Node<'f>) -> Usage {
        let mut usage = Usage::default();
        let mut types: BTreeMap<String, String> = BTreeMap::new();
        let mut query_values: BTreeSet<String> = BTreeSet::new();

        walk(function, &mut |node| match node.kind() {
            "var_spec" => {
                if let Some(ty) = node.child_by_field_name("type") {
                    for name in children_by_field(node, "name") {
                        types.insert(
                            self.text(name).to_string(),
                            self.text(ty).trim_start_matches('*').to_string(),
                        );
                    }
                }
            }
            "short_var_declaration" => {
                let names = node
                    .child_by_field_name("left")
                    .map(named_children)
                    .unwrap_or_default();
                let values = node
                    .child_by_field_name("right")
                    .map(named_children)
                    .unwrap_or_default();
                for (index, name) in names.iter().enumerate() {
                    let Some(value) = values.get(index).or(values.first()) else {
                        continue;
                    };
                    let value = unwrap(*value);
                    let name = self.text(*name).to_string();
                    match value.kind() {
                        "composite_literal" => {
                            if let Some(ty) = value.child_by_field_name("type") {
                                types.insert(name, self.text(ty).to_string());
                            }
                        }
                        "call_expression" => {
                            let text = self.text(value);
                            if text.starts_with("new(") {
                                if let Some(ty) = arguments(value).first() {
                                    types.insert(name, self.text(*ty).to_string());
                                }
                            } else if text.ends_with(".Query()") {
                                query_values.insert(name);
                            }
                        }
                        _ => {}
                    }
                }
            }
            "call_expression" => {
                let Some(function) = node.child_by_field_name("function") else {
                    return;
                };
                if function.kind() != "selector_expression" {
                    return;
                }
                let (Some(operand), Some(field)) = (
                    function.child_by_field_name("operand"),
                    function.child_by_field_name("field"),
                ) else {
                    return;
                };
                let method = self.text(field);
                let receiver = self.text(operand);
                let args = arguments(node);
                let first = args.first().and_then(|a| string_literal(self.file, *a));

                match method {
                    "Query" | "DefaultQuery" | "GetQuery" | "QueryArray" | "GetQueryArray"
                    | "QueryParam" | "QueryInt" | "QueryBool" | "QueryFloat" => {
                        if let Some(name) = first {
                            usage.push_query(name);
                        }
                    }
                    "Get" => {
                        let Some(name) = first else { return };
                        if receiver.ends_with(".Query()") || query_values.contains(receiver) {
                            usage.push_query(name);
                        } else if receiver.ends_with(".Header")
                            || receiver.ends_with(".Header()")
                            || looks_like_header(&name)
                        {
                            usage.push_header(name);
                        }
                    }
                    "GetHeader" => {
                        if let Some(name) = first {
                            usage.push_header(name);
                        }
                    }
                    "PostForm" | "DefaultPostForm" | "GetPostForm" | "PostFormValue"
                    | "FormValue" | "PostFormArray" => {
                        if let Some(name) = first {
                            usage.push_form(name);
                        }
                    }
                    "FormFile" => {
                        if let Some(name) = first {
                            usage.push_file(name);
                        }
                    }
                    "ShouldBindJSON" | "BindJSON" | "ShouldBind" | "Bind"
                    | "ShouldBindBodyWith" | "MustBindWith" | "ShouldBindWith"
                    | "ShouldBindXML" | "BindXML" | "ShouldBindYAML" | "BodyParser"
                    | "DecodeJSON" | "Decode" | "Unmarshal" | "ReadJSON" => {
                        if method == "Decode" && !receiver.contains("Decoder") {
                            return;
                        }
                        usage.reads_body = true;
                        if let Some(target) = args.last() {
                            let target = unwrap(*target);
                            if target.kind() == "identifier" {
                                if let Some(ty) = types.get(self.text(target)) {
                                    usage.body_type = Some(split_qualified(ty).1.to_string());
                                }
                            }
                        }
                    }
                    "ShouldBindQuery" | "BindQuery" => {
                        let Some(target) = args.last() else { return };
                        let target = unwrap(*target);
                        if target.kind() != "identifier" {
                            return;
                        }
                        let Some(ty) = types.get(self.text(target)) else {
                            return;
                        };
                        let Some(fields) = self.structs.get(split_qualified(ty).1) else {
                            return;
                        };
                        for field in fields {
                            let name = field
                                .form
                                .clone()
                                .or_else(|| field.json.clone())
                                .unwrap_or_else(|| field.field.clone());
                            usage.push_query(name);
                        }
                    }
                    "GetRawData" | "Body" => usage.reads_body = true,
                    _ => {}
                }
            }
            "selector_expression" => {
                if node
                    .child_by_field_name("field")
                    .is_some_and(|f| self.text(f) == "Body")
                {
                    usage.reads_body = true;
                }
            }
            "binary_expression" => {
                let (Some(left), Some(right)) = (
                    node.child_by_field_name("left"),
                    node.child_by_field_name("right"),
                ) else {
                    return;
                };
                let other = if self.text(left).ends_with(".Method") {
                    right
                } else if self.text(right).ends_with(".Method") {
                    left
                } else {
                    return;
                };
                if let Some(method) = self.method_of(&other) {
                    usage.push_checked(method);
                }
            }
            "expression_switch_statement" => {
                if !node
                    .child_by_field_name("value")
                    .is_some_and(|v| self.text(v).ends_with(".Method"))
                {
                    return;
                }
                let mut cases = Vec::new();
                walk(node, &mut |n| {
                    if n.kind() == "expression_case" {
                        cases.push(n);
                    }
                });
                for case in cases {
                    if let Some(list) = case.child_by_field_name("value") {
                        for value in named_children(list) {
                            if let Some(method) = self.method_of(&value) {
                                usage.push_checked(method);
                            }
                        }
                    }
                }
            }
            _ => {}
        });
        usage
    }

    /// `"GET"`, `http.MethodGet` → GET.
    fn method_of(&self, node: &Node<'_>) -> Option<HttpMethod> {
        let text = match string_literal(self.file, *node) {
            Some(text) => text,
            None => {
                let (_, name) = split_qualified(self.text(*node));
                name.strip_prefix("Method")?.to_ascii_uppercase()
            }
        };
        let Ok(method) = text.parse::<HttpMethod>();
        (!matches!(method, HttpMethod::Other(_))).then_some(method)
    }

    /// Methods from string arguments, or a `[]string{...}` literal.
    fn methods_of(&self, args: &[Node<'_>]) -> Vec<HttpMethod> {
        let mut methods = Vec::new();
        for arg in args {
            let arg = unwrap(*arg);
            if arg.kind() == "composite_literal" {
                if let Some(body) = arg.child_by_field_name("body") {
                    for element in named_children(body) {
                        let element = named_children(element)
                            .into_iter()
                            .next()
                            .unwrap_or(element);
                        if let Some(method) = self.method_of(&element) {
                            methods.push(method);
                        }
                    }
                }
            } else if let Some(method) = self.method_of(&arg) {
                methods.push(method);
            }
        }
        methods
    }

    /// Auth, from a middleware expression's name: `AuthRequired()`, `middleware.JWT(...)`.
    fn auth_of(&self, scope: &Scope, node: Node<'_>) -> Option<AuthRequirement> {
        let node = unwrap(node);
        if node.kind() == "func_literal" {
            return None;
        }
        if node.kind() == "identifier" {
            let name = self.text(node);
            if scope.routers.contains_key(name) || self.package_routers.contains_key(name) {
                return None; // a router is not middleware
            }
        }
        auth_from_middleware(self.text(node))
    }

    /// Whether an argument is a path — a string, a constant, or an expression that will
    /// be reported as unresolved — rather than a handler or a router.
    ///
    /// `certain` says the position can only hold a path (`GET`'s first argument), so an
    /// identifier declared nowhere in the file is one too, probably a constant in the
    /// package's next file. Where a method name is shared with HTTP clients (`Get`,
    /// `Handle`), only a value that reads as a path counts.
    fn is_path_argument(&self, scope: &Scope, node: Node<'_>, certain: bool) -> bool {
        let node = unwrap(node);
        match node.kind() {
            "interpreted_string_literal" | "raw_string_literal" => {
                string_literal(self.file, node).is_some_and(|text| is_path_text(&text))
            }
            "identifier" => {
                let name = self.text(node);
                match scope
                    .constants
                    .get(name)
                    .or_else(|| self.constants.get(name))
                {
                    Some(value) => is_path_text(value),
                    None => {
                        certain
                            && !scope.routers.contains_key(name)
                            && !scope.types.contains_key(name)
                            && !scope.aliases.contains_key(name)
                            && !self.package_routers.contains_key(name)
                            && !self.function_names.contains(name)
                            && name != "nil"
                    }
                }
            }
            "binary_expression" => match self.string_value(scope, node) {
                Some(text) => is_path_text(&text),
                None => {
                    // `prefix + "/users"`: some part is a string, so it is a path, even if
                    // an unresolved one.
                    let mut has_literal = false;
                    walk(node, &mut |n| {
                        if matches!(
                            n.kind(),
                            "interpreted_string_literal" | "raw_string_literal"
                        ) {
                            has_literal = true;
                        }
                    });
                    has_literal
                }
            },
            "selector_expression" => {
                let text = self.text(node);
                let object = text.split('.').next().unwrap_or("");
                let lowered = text.to_ascii_lowercase();
                certain
                    || matches!(
                        object.to_ascii_lowercase().as_str(),
                        "config"
                            | "cfg"
                            | "settings"
                            | "constants"
                            | "paths"
                            | "prefixes"
                            | "routes"
                    )
                    || lowered.contains("prefix")
                    || lowered.contains("path")
            }
            "call_expression" => {
                // `fmt.Sprintf("/%s/users", version)`, `path.Join(prefix, "/x")`,
                // `os.Getenv("PREFIX")` where only a path can go.
                let text = self.text(node);
                certain || text.starts_with("fmt.Sprintf(\"/") || text.starts_with("path.Join(")
            }
            _ => false,
        }
    }

    fn path_of(&self, scope: &Scope, node: Node<'_>) -> PathTemplate {
        match self.string_value(scope, node) {
            Some(text) => parse_go_path(&text),
            None => PathTemplate::from_segments(vec![PathSegment::unresolved(self.text(node))]),
        }
    }

    /// net/http's `"GET /items/{id}"` pattern: the method, if it states one, and the path.
    fn pattern_of(&self, scope: &Scope, node: Node<'_>) -> (Vec<HttpMethod>, PathTemplate) {
        let Some(text) = self.string_value(scope, node) else {
            return (Vec::new(), self.path_of(scope, node));
        };
        let (methods, path) = match text.split_once(' ') {
            Some((method, rest))
                if !method.is_empty() && method.chars().all(|c| c.is_ascii_uppercase()) =>
            {
                let method: HttpMethod = method.parse().unwrap_or(HttpMethod::Get);
                (vec![method], rest.trim().to_string())
            }
            _ => (Vec::new(), text),
        };
        // `example.com/path` — a host pattern; the path is what follows the host.
        let path = match path.find('/') {
            Some(0) | None => path,
            Some(index) => path[index..].to_string(),
        };
        (methods, parse_go_path(&path))
    }

    /// A string an expression folds to, through constants and `+`.
    fn string_value(&self, scope: &Scope, node: Node<'_>) -> Option<String> {
        let node = unwrap(node);
        match node.kind() {
            "interpreted_string_literal" | "raw_string_literal" => string_literal(self.file, node),
            "identifier" => {
                let name = self.text(node);
                scope
                    .constants
                    .get(name)
                    .or_else(|| self.constants.get(name))
                    .cloned()
            }
            "binary_expression" => {
                let left = node.child_by_field_name("left")?;
                let right = node.child_by_field_name("right")?;
                let operator = node.child_by_field_name("operator")?;
                if self.text(operator) != "+" {
                    return None;
                }
                Some(format!(
                    "{}{}",
                    self.string_value(scope, left)?,
                    self.string_value(scope, right)?
                ))
            }
            _ => None,
        }
    }

    /// `":8080"`, `"0.0.0.0:8080"`, a constant holding one: the port, recorded as a base
    /// URL candidate.
    fn listen_port(&mut self, scope: &Scope, node: Node<'_>) -> Option<u16> {
        let text = self.string_value(scope, node)?;
        let port: u16 = text.rsplit(':').next()?.trim().parse().ok()?;
        self.sink.port(
            port,
            format!(
                "listen address in {}:{}",
                self.file.display_path(),
                self.span(node).line
            ),
        );
        Some(port)
    }
}

/// gorilla/mux route state, and the query and header matchers it declares.
#[derive(Default)]
struct Pending<'f> {
    path: Option<PathTemplate>,
    handler: Option<Node<'f>>,
    methods: Vec<HttpMethod>,
    queries: Vec<String>,
    headers: Vec<String>,
}

#[derive(Default)]
struct Usage {
    query: Vec<String>,
    headers: Vec<String>,
    form: Vec<String>,
    files: Vec<String>,
    body_type: Option<String>,
    reads_body: bool,
    /// `r.Method == http.MethodPost`: what a method-less handler accepts.
    checked: Vec<HttpMethod>,
}

impl Usage {
    fn push_query(&mut self, name: String) {
        if !self.query.contains(&name) {
            self.query.push(name);
        }
    }

    fn push_header(&mut self, name: String) {
        if !self.headers.contains(&name) {
            self.headers.push(name);
        }
    }

    fn push_form(&mut self, name: String) {
        if !self.form.contains(&name) {
            self.form.push(name);
        }
    }

    fn push_file(&mut self, name: String) {
        if !self.files.contains(&name) {
            self.files.push(name);
        }
    }

    fn push_checked(&mut self, method: HttpMethod) {
        if !self.checked.contains(&method) {
            self.checked.push(method);
        }
    }

    fn merge(&mut self, other: Usage) {
        for name in other.query {
            self.push_query(name);
        }
        for name in other.headers {
            self.push_header(name);
        }
        for name in other.form {
            self.push_form(name);
        }
        for name in other.files {
            self.push_file(name);
        }
        for method in other.checked {
            self.push_checked(method);
        }
        self.reads_body |= other.reads_body;
        if self.body_type.is_none() {
            self.body_type = other.body_type;
        }
    }

    /// The body the handler reads, as far as it said: a form, a file upload, a JSON
    /// struct (filled in from the model afterwards), or just "something".
    fn body(&self) -> Option<BodySchema> {
        if !self.files.is_empty() || !self.form.is_empty() {
            let mut schema = BodySchema::json();
            schema.content_type = if self.files.is_empty() {
                "application/x-www-form-urlencoded".to_string()
            } else {
                "multipart/form-data".to_string()
            };
            let mut properties = serde_json::Map::new();
            let mut example = serde_json::Map::new();
            for name in &self.form {
                properties.insert(name.clone(), serde_json::json!({ "type": "string" }));
                example.insert(name.clone(), serde_json::Value::String(String::new()));
            }
            for name in &self.files {
                properties.insert(
                    name.clone(),
                    serde_json::json!({ "type": "string", "format": "binary" }),
                );
                example.insert(name.clone(), serde_json::Value::String(String::new()));
            }
            schema.schema = Some(serde_json::json!({ "type": "object", "properties": properties }));
            schema.example = Some(serde_json::Value::Object(example));
            return Some(schema);
        }
        if let Some(ty) = &self.body_type {
            let mut schema = BodySchema::json();
            schema.schema = Some(serde_json::json!({ "type": "object", "title": ty }));
            return Some(schema);
        }
        self.reads_body.then(BodySchema::json)
    }
}

/// Unwind `a.B(x).C(y)` into its root `a` and the pieces `[(B, call), (C, call)]`,
/// innermost first. A plain `f(x)` has no root and one piece named `f`.
fn chain<'a>(file: &ParsedFile, call: Node<'a>) -> (Option<Node<'a>>, Vec<(String, Node<'a>)>) {
    let mut pieces = Vec::new();
    let mut node = call;
    let root = loop {
        let Some(function) = node.child_by_field_name("function") else {
            break None;
        };
        match function.kind() {
            "selector_expression" => {
                let (Some(operand), Some(field)) = (
                    function.child_by_field_name("operand"),
                    function.child_by_field_name("field"),
                ) else {
                    break None;
                };
                pieces.push((file.text(field).to_string(), node));
                let operand = unwrap(operand);
                if operand.kind() == "call_expression" {
                    node = operand;
                    continue;
                }
                break Some(operand);
            }
            "identifier" => {
                pieces.push((file.text(function).to_string(), node));
                break None;
            }
            _ => break Some(function),
        }
    };
    pieces.reverse();
    (root, pieces)
}

/// A call's arguments, comments dropped.
fn arguments(call: Node<'_>) -> Vec<Node<'_>> {
    call.child_by_field_name("arguments")
        .map(named_children)
        .unwrap_or_default()
}

fn named_children(node: Node<'_>) -> Vec<Node<'_>> {
    (0..node.named_child_count() as u32)
        .filter_map(|i| node.named_child(i))
        .filter(|n| n.kind() != "comment")
        .collect()
}

fn children_by_field<'a>(node: Node<'a>, field: &str) -> Vec<Node<'a>> {
    let mut cursor = node.walk();
    node.children_by_field_name(field, &mut cursor).collect()
}

fn child_of_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    crate::index::child_of_kind(node, kind)
}

/// Strip `&x`, `(x)` and `*x`.
fn unwrap(node: Node<'_>) -> Node<'_> {
    let mut node = node;
    loop {
        match node.kind() {
            "unary_expression" | "parenthesized_expression" => {
                let Some(inner) = node
                    .child_by_field_name("operand")
                    .or_else(|| node.named_child(0))
                else {
                    return node;
                };
                node = inner;
            }
            _ => return node,
        }
    }
}

/// The text of a string literal, interpreted or raw, without its quotes.
fn string_literal(file: &ParsedFile, node: Node<'_>) -> Option<String> {
    let node = unwrap(node);
    match node.kind() {
        "interpreted_string_literal" => {
            let mut out = String::new();
            for child in named_children(node) {
                match child.kind() {
                    "interpreted_string_literal_content" => out.push_str(file.text(child)),
                    "escape_sequence" => out.push_str(match file.text(child) {
                        "\\n" => "\n",
                        "\\t" => "\t",
                        "\\\"" => "\"",
                        "\\\\" => "\\",
                        other => other,
                    }),
                    _ => {}
                }
            }
            Some(out)
        }
        "raw_string_literal" => Some(file.text(node).trim_matches('`').to_string()),
        _ => None,
    }
}

fn leftmost_identifier(node: Node<'_>) -> Option<Node<'_>> {
    let node = unwrap(node);
    match node.kind() {
        "identifier" => Some(node),
        "selector_expression" => leftmost_identifier(node.child_by_field_name("operand")?),
        "call_expression" => leftmost_identifier(node.child_by_field_name("function")?),
        _ => None,
    }
}

/// `pkg.Name` → (`Some("pkg")`, `"Name"`), pointer stripped.
fn split_qualified(text: &str) -> (Option<&str>, &str) {
    let text = text.trim_start_matches('*');
    match text.rsplit_once('.') {
        Some((package, name)) => (Some(package), name),
        None => (None, text),
    }
}

/// The name a package is referred to by: the last path element, except that `echo/v4` is
/// `echo` and `go-chi` would be `chi`.
fn package_name(path: &str) -> String {
    let mut parts: Vec<&str> = path.split('/').collect();
    while let Some(last) = parts.last() {
        let is_version = last.len() > 1
            && last.starts_with('v')
            && last[1..].chars().all(|c| c.is_ascii_digit());
        if is_version && parts.len() > 1 {
            parts.pop();
        } else {
            break;
        }
    }
    let last = parts.last().copied().unwrap_or(path);
    last.rsplit('-').next().unwrap_or(last).to_string()
}

/// `json:"name,omitempty"` → `name`.
fn tag_value(tag: &str, key: &str) -> Option<String> {
    let start = tag.find(&format!("{key}:\""))? + key.len() + 2;
    let rest = &tag[start..];
    let end = rest.find('"')?;
    let value = rest[..end].split(',').next().unwrap_or("");
    (!value.is_empty()).then(|| value.to_string())
}

/// A Go type as the model index reads annotations: `[]string` → `list[str]`.
fn annotation(go: &str) -> String {
    let go = go.trim().trim_start_matches('*');
    if let Some(inner) = go.strip_prefix("[]") {
        if inner == "byte" {
            return "bytes".to_string();
        }
        return format!("list[{}]", annotation(inner));
    }
    if let Some(rest) = go.strip_prefix("map[") {
        if let Some(close) = rest.find(']') {
            return format!(
                "dict[{}, {}]",
                annotation(&rest[..close]),
                annotation(&rest[close + 1..])
            );
        }
    }
    match go {
        "string" => "str".to_string(),
        "int" | "int8" | "int16" | "int32" | "int64" | "uint" | "uint8" | "uint16" | "uint32"
        | "uint64" => "int".to_string(),
        "float32" | "float64" => "float".to_string(),
        "bool" => "bool".to_string(),
        "time.Time" => "datetime".to_string(),
        "uuid.UUID" => "UUID".to_string(),
        "decimal.Decimal" => "Decimal".to_string(),
        "json.RawMessage" | "any" | "interface{}" => "Any".to_string(),
        "sql.NullString" | "null.String" => "str".to_string(),
        "sql.NullInt64" | "null.Int" => "int".to_string(),
        "sql.NullBool" | "null.Bool" => "bool".to_string(),
        "sql.NullTime" | "null.Time" => "datetime".to_string(),
        other => split_qualified(other).1.to_string(),
    }
}

fn is_path_text(text: &str) -> bool {
    text.starts_with('/')
        || text.is_empty()
        // net/http: `"GET /users"`, and a host-rooted `"example.com/"`.
        || text.split_once(' ').is_some_and(|(method, rest)| {
            !method.is_empty()
                && method.chars().all(|c| c.is_ascii_uppercase())
                && rest.contains('/')
        })
}

fn is_version_or_api(segment: &str) -> bool {
    segment == "api"
        || (segment.len() > 1
            && segment.starts_with('v')
            && segment[1..].chars().all(|c| c.is_ascii_digit()))
}

/// `Router`, `apiMux`, `engine`: a name declared elsewhere that is almost certainly a
/// router, and so worth mounting on the strength of the name.
fn looks_like_router(name: &str) -> bool {
    let lowered = name.to_ascii_lowercase();
    lowered.contains("router") || lowered.contains("mux") || lowered.contains("engine")
}

/// Whether an argument could be a handler or a router: a name, a literal function, a
/// call that builds one — not a number or a string.
fn is_handler_like(node: Node<'_>) -> bool {
    matches!(
        unwrap(node).kind(),
        "identifier"
            | "selector_expression"
            | "func_literal"
            | "call_expression"
            | "composite_literal"
    )
}

fn looks_like_header(name: &str) -> bool {
    name.contains('-')
        || matches!(
            name.to_ascii_lowercase().as_str(),
            "authorization" | "accept" | "origin" | "host" | "cookie" | "referer"
        )
}

fn synthetic_symbol(parent: &str, span: Span) -> String {
    format!("{parent}@{}:{}", span.line, span.column)
}

/// The last segment of a group's prefix that says something: `/api/v1/users` → `users`.
fn group_from_prefix(prefix: &PathTemplate) -> Option<String> {
    prefix
        .segments
        .iter()
        .rev()
        .find_map(|segment| match segment {
            PathSegment::Literal { value } if !is_version_or_api(value) => Some(value.clone()),
            _ => None,
        })
}

/// Parse a path in any of the syntaxes the six frameworks use.
///
/// `{id}`, `{id:[0-9]+}` (chi, gorilla), `{path...}` and `{$}` (net/http), `:id`, `:id?`,
/// `:id<int>` (Gin, Echo, Fiber), `*`, `*name`, `+` (catch-alls).
fn parse_go_path(text: &str) -> PathTemplate {
    let trimmed = text.trim();
    let trailing_slash = trimmed.len() > 1 && trimmed.ends_with('/');
    let mut segments = Vec::new();
    for raw in trimmed.split('/').filter(|s| !s.is_empty()) {
        if raw == "{$}" {
            continue;
        }
        if let Some(inner) = raw.strip_prefix('{').and_then(|r| r.strip_suffix('}')) {
            if let Some(name) = inner.strip_suffix("...") {
                segments.push(PathSegment::Param {
                    name: name.to_string(),
                    ty: Some(TypeHint::Path),
                    catch_all: true,
                    optional: false,
                });
                continue;
            }
            let (name, constraint) = match inner.split_once(':') {
                Some((n, c)) => (n.trim(), Some(c.trim())),
                None => (inner.trim(), None),
            };
            segments.push(PathSegment::Param {
                name: name.to_string(),
                ty: simple_constraint(constraint),
                catch_all: false,
                optional: false,
            });
            continue;
        }
        if let Some(rest) = raw.strip_prefix(':') {
            let optional = rest.ends_with('?');
            let rest = rest.trim_end_matches('?');
            let (name, constraint) = match rest.split_once('<') {
                Some((n, c)) => (n, Some(c.trim_end_matches('>'))),
                None => (rest, None),
            };
            segments.push(PathSegment::Param {
                name: name.to_string(),
                ty: simple_constraint(constraint),
                catch_all: false,
                optional,
            });
            continue;
        }
        if raw == "*" || raw == "+" {
            segments.push(PathSegment::Param {
                name: "wildcard".to_string(),
                ty: Some(TypeHint::Path),
                catch_all: true,
                optional: raw == "*",
            });
            continue;
        }
        if let Some(name) = raw.strip_prefix('*') {
            segments.push(PathSegment::Param {
                name: name.to_string(),
                ty: Some(TypeHint::Path),
                catch_all: true,
                optional: false,
            });
            continue;
        }
        segments.push(PathSegment::literal(raw));
    }
    let mut template = PathTemplate::from_segments(segments);
    template.trailing_slash = trailing_slash;
    template
}

/// `{id:int}` says a type; `{id:[0-9]+}` says a regular expression, which is not one.
fn simple_constraint(constraint: Option<&str>) -> Option<TypeHint> {
    constraint
        .filter(|c| !c.is_empty() && c.chars().all(|ch| ch.is_ascii_alphanumeric()))
        .map(TypeHint::parse)
}

/// `routes/users.go` → `users`; `routes/router.go` → the directory, if it says anything.
fn group_from_file(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    let stem = stem.split('.').next().unwrap_or(stem);
    if MEANINGLESS_NAMES.contains(&stem) {
        let dir = path.parent()?.file_name()?.to_str()?;
        return (!MEANINGLESS_NAMES.contains(&dir)).then(|| dir.to_string());
    }
    Some(stem.to_string())
}

/// `RegisterUserRoutes` → `user`; `usersResource.Routes` → `users`;
/// `Server.registerUsers` → `users`.
fn group_from_function(symbol: &str) -> Option<String> {
    let candidates: Vec<&str> = symbol.split('.').collect();
    candidates.iter().find_map(|name| {
        let mut name = *name;
        for prefix in [
            "Register", "register", "Mount", "mount", "Setup", "setup", "Add", "add", "New", "new",
        ] {
            if let Some(rest) = name.strip_prefix(prefix) {
                name = rest;
            }
        }
        for suffix in [
            "Routes",
            "Router",
            "Handlers",
            "Handler",
            "Resource",
            "Controller",
            "Service",
            "API",
            "Api",
        ] {
            if let Some(rest) = name.strip_suffix(suffix) {
                name = rest;
            }
        }
        let mut chars = name.chars();
        let lowered = match chars.next() {
            Some(first) => first.to_ascii_lowercase().to_string() + chars.as_str(),
            None => return None,
        };
        (!MEANINGLESS_NAMES.contains(&lowered.as_str())).then_some(lowered)
    })
}

fn top_level_function<'a>(file: &'a ParsedFile, name: &str) -> Option<Node<'a>> {
    top_level(file, "function_declaration", name)
}

fn method_declaration<'a>(file: &'a ParsedFile, name: &str) -> Option<Node<'a>> {
    top_level(file, "method_declaration", name)
}

fn top_level<'a>(file: &'a ParsedFile, kind: &str, name: &str) -> Option<Node<'a>> {
    named_children(file.root()).into_iter().find(|node| {
        node.kind() == kind
            && node
                .child_by_field_name("name")
                .is_some_and(|n| file.text(n) == name)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::SourceIndex;
    use rl_model::ParamStyle;

    fn extract(path: &str, source: &str) -> FactSink {
        extract_in("example.com/app", path, source)
    }

    /// As a scan would see the file: `module` is what `go.mod` says, so imports under it
    /// are the project's own packages.
    fn extract_in(module: &str, path: &str, source: &str) -> FactSink {
        let file = SourceIndex::new()
            .unwrap()
            .parse(path, Language::Go, source.to_string())
            .unwrap();
        let mut sink = FactSink::new();
        let adapter = GoAdapter {
            module_path: RefCell::new(Some(module.to_string())),
        };
        adapter.extract(&file, &mut sink);
        sink
    }

    /// In source order.
    fn routes(sink: &FactSink) -> Vec<String> {
        let mut facts: Vec<&RouteFact> = sink.routes.iter().collect();
        facts.sort_by_key(|r| (r.span.line, r.span.column));
        facts
            .iter()
            .flat_map(|r| {
                r.methods.iter().map(move |m| {
                    format!(
                        "{} {} on {}",
                        m,
                        r.path.render(ParamStyle::Braces),
                        r.router.name
                    )
                })
            })
            .collect()
    }

    fn mounts(sink: &FactSink) -> Vec<String> {
        sink.mounts
            .iter()
            .map(|m| {
                format!(
                    "{} at {} on {}",
                    m.child.name,
                    m.prefix.render(ParamStyle::Braces),
                    m.parent.name
                )
            })
            .collect()
    }

    #[test]
    fn echo_groups_methods_and_the_listen_address() {
        let sink = extract(
            "main.go",
            r#"package main

import (
	"github.com/labstack/echo/v4"
	"github.com/labstack/echo/v4/middleware"
)

func main() {
	e := echo.New()
	e.GET("/health", health)
	api := e.Group("/api", middleware.JWT([]byte("secret")))
	api.POST("/users", createUser)
	api.Any("/proxy", proxy)
	api.Match([]string{"GET", "HEAD"}, "/files/:name", file)
	e.Add("PATCH", "/x", patch)
	e.Logger.Fatal(e.Start(":1323"))
}
"#,
        );
        assert_eq!(
            routes(&sink),
            vec![
                "GET /health on main.e",
                "POST /users on main.api",
                "GET /proxy on main.api",
                "POST /proxy on main.api",
                "PUT /proxy on main.api",
                "PATCH /proxy on main.api",
                "DELETE /proxy on main.api",
                "GET /files/{name} on main.api",
                "HEAD /files/{name} on main.api",
                "PATCH /x on main.e",
            ]
        );
        assert_eq!(mounts(&sink), vec!["main.api at /api on main.e"]);
        assert!(matches!(
            sink.mounts[0].auth,
            Some(AuthRequirement::Bearer { format: Some(ref f) }) if f == "JWT"
        ));
        assert!(sink
            .routers
            .iter()
            .any(|r| r.symbol.name == "main.e" && r.is_app_root));
        assert_eq!(sink.ports[0].0, 1323);
    }

    #[test]
    fn fiber_mounts_sub_apps_and_reads_optional_params() {
        let sink = extract(
            "main.go",
            r#"package main

import "github.com/gofiber/fiber/v2"

func main() {
	app := fiber.New()
	api := fiber.New()
	api.Get("/users/:id?", getUser)
	api.All("/ping", ping)
	app.Mount("/api", api)
	adminApp := fiber.New()
	app.Use("/admin", adminApp)
	app.Route("/books", func(r fiber.Router) {
		r.Get("/", list)
		r.Post("/", create)
	})
	app.Static("/", "./public")
	app.Listen(":3000")
}
"#,
        );
        let listed = routes(&sink);
        assert_eq!(listed[0], "GET /users/{id} on main.api");
        assert!(matches!(
            sink.routes[0].path.segments[1],
            PathSegment::Param { optional: true, .. }
        ));
        assert!(
            listed.contains(&"GET / on main.app@13:2".to_string()),
            "{listed:?}"
        );
        assert!(listed.contains(&"POST / on main.app@13:2".to_string()));
        assert_eq!(
            mounts(&sink),
            vec![
                "main.api at /api on main.app",
                "main.adminApp at /admin on main.app",
                "main.app@13:2 at /books on main.app",
            ]
        );
        // A sub-app is built like the app, but mounting it makes it not a root.
        let roots: Vec<&str> = sink
            .routers
            .iter()
            .filter(|r| r.is_app_root)
            .map(|r| r.symbol.name.as_str())
            .collect();
        assert_eq!(roots, vec!["main.app"]);
        assert_eq!(sink.ports[0].0, 3000);
    }

    #[test]
    fn gorilla_chains_are_read_as_one_route() {
        let sink = extract(
            "main.go",
            r#"package main

import (
	"net/http"

	"github.com/gorilla/mux"
)

func main() {
	r := mux.NewRouter()
	r.HandleFunc("/users", listUsers).Methods("GET")
	r.HandleFunc("/users", createUser).Methods("POST", "PUT")
	r.Path("/search").Queries("q", "{q}", "page", "{page}").HandlerFunc(search).Methods(http.MethodGet)
	r.Handle("/metrics", metricsHandler)
	api := r.PathPrefix("/api").Subrouter()
	api.HandleFunc("/items/{id:[0-9]+}", getItem).Methods("GET")
	api.Methods("DELETE").Path("/items/{id}").HandlerFunc(deleteItem)
	http.ListenAndServe(":8000", r)
}
"#,
        );
        assert_eq!(
            routes(&sink),
            vec![
                "GET /users on main.r",
                "POST /users on main.r",
                "PUT /users on main.r",
                "GET /search on main.r",
                "GET /metrics on main.r",
                "POST /metrics on main.r",
                "PUT /metrics on main.r",
                "PATCH /metrics on main.r",
                "DELETE /metrics on main.r",
                "GET /items/{id} on main.api",
                "DELETE /items/{id} on main.api",
            ]
        );
        let search = sink
            .routes
            .iter()
            .find(|r| r.path.to_string() == "/search")
            .unwrap();
        let query: Vec<&str> = search
            .query_params
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(query, vec!["q", "page"]);
        let item = sink
            .routes
            .iter()
            .find(|r| r.path.to_string().starts_with("/items"))
            .unwrap();
        assert_eq!(
            item.path.segments[1],
            PathSegment::Param {
                name: "id".into(),
                ty: None,
                catch_all: false,
                optional: false
            },
            "a regular expression constraint is not a type"
        );
        assert_eq!(mounts(&sink), vec!["main.api at /api on main.r"]);
        // Served by ListenAndServe, so a root — not an orphan.
        assert!(sink
            .routers
            .iter()
            .any(|r| r.symbol.name == "main.r" && r.is_app_root));
        assert_eq!(sink.ports[0].0, 8000);
    }

    #[test]
    fn net_http_patterns_default_mux_and_method_checks() {
        let sink = extract(
            "main.go",
            r#"package main

import "net/http"

func handleItems(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "nope", 405)
		return
	}
}

func main() {
	http.HandleFunc("GET /users/{id}", getUser)
	http.HandleFunc("/items", handleItems)
	http.Handle("/static/", http.StripPrefix("/static/", http.FileServer(http.Dir("public"))))
	mux := http.NewServeMux()
	mux.HandleFunc("GET example.com/tenants/{tenant}/files/{path...}", files)
	mux.HandleFunc("/{$}", index)
	http.Handle("/api/", http.StripPrefix("/api", mux))
	http.ListenAndServe(":8080", nil)
}
"#,
        );
        assert_eq!(
            routes(&sink),
            vec![
                "GET /users/{id} on http.DefaultServeMux",
                "POST /items on http.DefaultServeMux",
                "GET /tenants/{tenant}/files/{path} on main.mux",
                "GET / on main.mux",
                "POST / on main.mux",
                "PUT / on main.mux",
                "PATCH / on main.mux",
                "DELETE / on main.mux",
            ]
        );
        assert!(matches!(
            sink.routes[2].path.segments[3],
            PathSegment::Param {
                catch_all: true,
                ..
            }
        ));
        assert_eq!(
            mounts(&sink),
            vec!["main.mux at /api on http.DefaultServeMux"]
        );
        assert_eq!(sink.ports[0].0, 8080);
    }

    #[test]
    fn a_gin_engine_returned_from_a_function_is_the_functions_router() {
        let sink = extract(
            "router.go",
            r#"package api

import "github.com/gin-gonic/gin"

func New(db *DB) *gin.Engine {
	r := gin.Default()
	r.GET("/ping", ping)
	v1 := r.Group("/v1")
	v1.GET("/users", listUsers)
	return r
}
"#,
        );
        assert_eq!(
            routes(&sink),
            vec!["GET /ping on New", "GET /users on New.v1"]
        );
        assert_eq!(mounts(&sink), vec!["New.v1 at /v1 on New"]);
        let function = sink
            .routers
            .iter()
            .find(|r| r.symbol.name == "New")
            .unwrap();
        assert!(!function.is_app_root, "the caller serves it");
        assert!(!function.implicit);
    }

    #[test]
    fn a_router_from_another_package_is_served_through_a_server_root() {
        let sink = extract(
            "main.go",
            r#"package main

import (
	"net/http"

	"example.com/app/internal/api"
)

func main() {
	r := api.New(db)
	srv := &http.Server{Addr: ":9000", Handler: r}
	srv.ListenAndServe()
}
"#,
        );
        assert!(sink.routes.is_empty());
        assert!(sink
            .routers
            .iter()
            .any(|r| r.symbol.name == "http.Server" && r.is_app_root));
        assert_eq!(mounts(&sink), vec!["api.New at / on http.Server"]);
        assert_eq!(sink.ports[0].0, 9000);
    }

    #[test]
    fn a_function_handed_a_router_is_mounted_and_declared_implicitly_when_idle() {
        let sink = extract(
            "main.go",
            r#"package main

import (
	"github.com/gin-gonic/gin"

	"example.com/app/internal/handlers"
	"example.com/app/internal/users"
)

func main() {
	r := gin.Default()
	users.Register(r.Group("/users"))
	setup(r)
	h := handlers.New(db)
	h.Mount(r)
	log.Println(r)
	cmd.Run()
}

func setup(r *gin.Engine) {
	r.Use(gin.Recovery())
}
"#,
        );
        assert_eq!(
            mounts(&sink),
            vec![
                "main.r@12:17 at /users on main.r",
                "users.Register at / on main.r@12:17",
                "setup at / on main.r",
                "handlers.New.Mount at / on main.r",
            ]
        );
        let setup = sink
            .routers
            .iter()
            .find(|r| r.symbol.name == "setup")
            .unwrap();
        assert!(setup.implicit, "nothing was registered through it");
        assert!(sink.warnings.is_empty(), "{:?}", sink.warnings);
    }

    #[test]
    fn a_client_get_is_not_a_route() {
        let sink = extract(
            "client.go",
            r#"package client

import "net/http"

func fetch() {
	resp, _ := http.Get("/users")
	client.Get("/users")
	resty.R().Get("/users")
	cache.Delete("/key", 1)
}
"#,
        );
        assert!(sink.routes.is_empty(), "{:?}", routes(&sink));
        assert!(sink.mounts.is_empty());
        assert!(sink.warnings.is_empty(), "{:?}", sink.warnings);
    }

    #[test]
    fn struct_tags_become_model_fields() {
        let sink = extract(
            "user.go",
            r#"package users

import "time"

type Base struct {
	ID int `json:"id"`
}

type User struct {
	Base
	Name     string            `json:"name" binding:"required"`
	Email    string            `json:"email,omitempty"`
	Tags     []string          `json:"tags"`
	Meta     map[string]string `json:"meta"`
	Born     time.Time         `json:"born"`
	Password string            `json:"-"`
	internal int
	Plain    bool
}
"#,
        );
        let user = sink.models.iter().find(|m| m.name == "User").unwrap();
        assert_eq!(user.bases, vec!["Base"]);
        let fields: Vec<(&str, &str, bool)> = user
            .fields
            .iter()
            .map(|f| (f.name.as_str(), f.annotation.as_str(), f.required))
            .collect();
        assert_eq!(
            fields,
            vec![
                ("name", "str", true),
                ("email", "str", false),
                ("tags", "list[str]", false),
                ("meta", "dict[str, str]", false),
                ("born", "datetime", false),
                ("Plain", "bool", false),
            ]
        );
    }

    #[test]
    fn handlers_are_recorded_by_name_for_routes_elsewhere() {
        let sink = extract(
            "handler.go",
            r#"package users

import "github.com/gin-gonic/gin"

type Handler struct{}

func (h *Handler) Create(c *gin.Context) {
	var req CreateUser
	c.ShouldBindJSON(&req)
	c.GetHeader("X-Request-Id")
	c.Query("dry_run")
}

func helper(x int) {}
"#,
        );
        assert_eq!(sink.handlers.len(), 1);
        let create = &sink.handlers[0];
        assert_eq!(create.name, "Handler.Create");
        assert_eq!(create.query_params[0].name, "dry_run");
        assert_eq!(create.headers[0].name, "X-Request-Id");
        assert_eq!(
            create.body.as_ref().unwrap().schema.as_ref().unwrap()["title"],
            "CreateUser"
        );
    }

    #[test]
    fn a_route_names_its_handler_normalised() {
        let sink = extract(
            "routes.go",
            r#"package users

import "github.com/gin-gonic/gin"

func Register(rg *gin.RouterGroup) {
	h := &Handler{}
	rg.GET("/", h.List)
	rg.POST("/", middleware.Auth(), create)
	rg.PUT("/:id", http.HandlerFunc(update))
}
"#,
        );
        let names: Vec<&str> = sink
            .routes
            .iter()
            .map(|r| r.handler.as_ref().map(|h| h.name.as_str()).unwrap_or("-"))
            .collect();
        assert_eq!(names, vec!["Handler.List", "create", "update"]);
        assert!(sink.routes[1].auth.is_some());
    }

    #[test]
    fn go_path_syntaxes() {
        let render = |raw: &str| parse_go_path(raw).render(ParamStyle::Braces);
        assert_eq!(render("/users/:id"), "/users/{id}");
        assert_eq!(render("/users/{id}"), "/users/{id}");
        assert_eq!(render("/users/{id:[0-9]+}"), "/users/{id}");
        assert_eq!(render("/files/{path...}"), "/files/{path}");
        assert_eq!(render("/files/*filepath"), "/files/{filepath}");
        assert_eq!(render("/files/*"), "/files/{wildcard}");
        assert_eq!(render("/{$}"), "/");
        assert_eq!(render("/notes/"), "/notes/");
        let typed = parse_go_path("/users/:id<int>");
        assert!(matches!(
            &typed.segments[1],
            PathSegment::Param {
                ty: Some(TypeHint::Integer),
                ..
            }
        ));
        let catch_all = parse_go_path("/files/{path...}");
        assert!(matches!(
            &catch_all.segments[1],
            PathSegment::Param {
                catch_all: true,
                ..
            }
        ));
    }

    #[test]
    fn package_names_drop_major_versions_and_hyphens() {
        assert_eq!(package_name("github.com/labstack/echo/v4"), "echo");
        assert_eq!(
            package_name("github.com/go-chi/chi/v5/middleware"),
            "middleware"
        );
        assert_eq!(package_name("github.com/gorilla/mux"), "mux");
        assert_eq!(package_name("net/http"), "http");
    }

    #[test]
    fn tags_and_annotations() {
        let tag = r#"json:"name,omitempty" form:"n""#;
        assert_eq!(tag_value(tag, "json").as_deref(), Some("name"));
        assert_eq!(tag_value(tag, "form").as_deref(), Some("n"));
        assert_eq!(tag_value(r#"json:"-""#, "json").as_deref(), Some("-"));
        assert_eq!(tag_value(r#"form:"n""#, "json"), None);
        assert_eq!(annotation("*[]models.Item"), "list[Item]");
        assert_eq!(annotation("map[string]interface{}"), "dict[str, Any]");
        assert_eq!(annotation("[]byte"), "bytes");
    }

    #[test]
    fn groups_come_from_files_and_function_names() {
        assert_eq!(
            group_from_file(Path::new("internal/users/routes.go")).as_deref(),
            Some("users")
        );
        assert_eq!(
            group_from_file(Path::new("internal/users/handler.go")).as_deref(),
            Some("users")
        );
        assert_eq!(group_from_file(Path::new("cmd/server/main.go")), None);
        assert_eq!(
            group_from_function("RegisterUserRoutes").as_deref(),
            Some("user")
        );
        assert_eq!(
            group_from_function("usersResource.Routes").as_deref(),
            Some("users")
        );
        assert_eq!(
            group_from_function("Server.registerNotes").as_deref(),
            Some("notes")
        );
        assert_eq!(group_from_function("Routes"), None);
    }
}
