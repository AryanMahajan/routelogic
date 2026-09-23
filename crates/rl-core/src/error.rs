//! Errors surfaced to a shell.

pub type Result<T> = std::result::Result<T, CoreError>;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("no workspace is open")]
    NoWorkspace,

    /// Reported together rather than one failed request at a time, so the user fixes every
    /// missing variable in one pass.
    #[error("undefined variable(s): {}", .names.join(", "))]
    UndefinedVariables { names: Vec<String> },

    #[error("this workspace has no project attached, so there is nothing to scan")]
    NotAProject,

    #[error("no scan has been run yet")]
    NoScan,

    #[error("no endpoint with id {id:?}")]
    NoSuchEndpoint { id: String },

    /// Refused deliberately: sending a request to a path RouteLogic could not work out would
    /// hit a meaningless URL and fail in a way the developer blames on their own code.
    #[error("`{id}` has a path RouteLogic could not resolve: {}", .expressions.join(", "))]
    UnresolvedEndpoint {
        id: String,
        expressions: Vec<String>,
    },

    #[error("a collection named {name:?} already exists")]
    CollectionExists { name: String },

    #[error("a flow named {name:?} already exists")]
    FlowExists { name: String },

    #[error("no environment named {name:?}")]
    NoSuchEnvironment { name: String },

    /// The graph cannot be run as drawn. Refused before the first request goes out.
    #[error("the flow cannot run: {0}")]
    Flow(#[from] rl_model::FlowError),

    #[error("no source file at {}", .path.display())]
    NoSuchSource { path: std::path::PathBuf },

    #[error(transparent)]
    Workspace(#[from] rl_workspace::WorkspaceError),

    #[error(transparent)]
    Discovery(#[from] rl_discovery::DiscoveryError),

    #[error(transparent)]
    Enrich(#[from] rl_discovery::EnrichError),

    /// Runtime enrich asks the application; there has to be one to ask.
    #[error("runtime enrich needs a FastAPI or Flask project; this one is {frameworks}")]
    NotEnrichable { frameworks: String },

    #[error(transparent)]
    Http(#[from] rl_http::HttpError),

    #[error(transparent)]
    Import(#[from] rl_import::ImportError),

    #[error(transparent)]
    Resolve(#[from] rl_model::ResolveError),
}

impl CoreError {
    /// A message fit to put in front of a user, with the source chain flattened.
    ///
    /// `thiserror` prints only the outermost message by default, which strips exactly the
    /// detail that makes an error actionable — "request failed" rather than "connection
    /// refused".
    pub fn message(&self) -> String {
        let mut parts = vec![self.to_string()];
        let mut source = std::error::Error::source(self);
        while let Some(current) = source {
            let text = current.to_string();
            if !parts.contains(&text) {
                parts.push(text);
            }
            source = current.source();
        }
        parts.join(": ")
    }
}
