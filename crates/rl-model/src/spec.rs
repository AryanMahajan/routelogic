//! [`EndpointSpec`] — a route discovered from a project.
//!
//! A spec is a *template*: it describes what a project exposes. It has no host and no
//! values. The executable counterpart is [`crate::RequestDraft`]. Keeping the two apart is
//! deliberate — see `docs/concepts.md`.

use crate::method::HttpMethod;
use crate::path::{ParamStyle, PathTemplate, TypeHint};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A stable identifier for a discovered endpoint.
///
/// Derived from method and path *shape*, so it survives a rescan: renaming a handler or
/// moving it to another file does not break a saved request's `spec_ref`.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
pub struct EndpointId(String);

impl EndpointId {
    pub fn new(method: &HttpMethod, path: &PathTemplate) -> Self {
        EndpointId(format!("{} {}", method.as_str(), path.normalized()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for EndpointId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// How a spec was learned. Recorded so the UI can be honest about provenance, and so the
/// merge in `SpecMerger` knows which source to trust for which field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum Origin {
    /// Read from source without executing anything. Always available, sometimes incomplete.
    StaticScan { framework: String },
    /// Obtained by importing the application and asking it directly. Requires opt-in.
    Runtime { framework: String },
    /// Imported from a specification document.
    OpenApi {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        document: Option<String>,
    },
    /// Entered by hand.
    Manual,
}

impl Origin {
    pub fn framework(&self) -> Option<&str> {
        match self {
            Origin::StaticScan { framework } | Origin::Runtime { framework } => Some(framework),
            _ => None,
        }
    }

    /// Runtime and OpenAPI sources describe the API exactly; static analysis infers it.
    pub fn is_authoritative(&self) -> bool {
        matches!(self, Origin::Runtime { .. } | Origin::OpenApi { .. })
    }
}

/// How much to trust a discovered endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// Fully resolved, from an authoritative source or an unambiguous registration.
    High,
    /// Resolved, but something was inferred rather than stated.
    Medium,
    /// Contains unresolved segments, or was reached by a heuristic.
    Low,
}

/// Where an endpoint is defined. Absent for endpoints known only at runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceLocation {
    /// Relative to the project root, so it stays stable across machines.
    pub file: PathBuf,
    /// 1-indexed.
    pub line: u32,
    /// 1-indexed.
    #[serde(default = "one")]
    pub column: u32,
}

fn one() -> u32 {
    1
}

impl SourceLocation {
    pub fn new(file: impl Into<PathBuf>, line: u32) -> Self {
        SourceLocation {
            file: file.into(),
            line,
            column: 1,
        }
    }
}

impl std::fmt::Display for SourceLocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.file.display(), self.line)
    }
}

/// Where a parameter travels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamLocation {
    Path,
    Query,
    Header,
    Cookie,
}

/// A parameter an endpoint accepts. Everything but the name is best-effort.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParamSpec {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ty: Option<TypeHint>,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Enumerated values, where the source states them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enum_values: Vec<serde_json::Value>,
}

impl ParamSpec {
    pub fn new(name: impl Into<String>) -> Self {
        ParamSpec {
            name: name.into(),
            ty: None,
            required: false,
            default: None,
            description: None,
            enum_values: Vec::new(),
        }
    }

    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    pub fn with_type(mut self, ty: TypeHint) -> Self {
        self.ty = Some(ty);
        self
    }
}

/// A request body shape, where one could be detected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BodySchema {
    pub content_type: String,
    /// JSON Schema, as far as the source allowed it to be reconstructed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<serde_json::Value>,
    /// A worked example, used to pre-fill the body editor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,
    #[serde(default)]
    pub required: bool,
}

impl BodySchema {
    pub fn json() -> Self {
        BodySchema {
            content_type: "application/json".to_string(),
            schema: None,
            example: None,
            required: false,
        }
    }
}

/// Where an API key travels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApiKeyLocation {
    Header,
    Query,
    Cookie,
}

/// What an endpoint requires in order to authenticate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthRequirement {
    Bearer {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        format: Option<String>,
    },
    Basic,
    ApiKey {
        name: String,
        location: ApiKeyLocation,
    },
    OAuth2 {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        scopes: Vec<String>,
    },
    Cookie {
        name: String,
    },
    /// Auth is required, but the scheme could not be identified.
    ///
    /// Better than claiming there is none: `hint` carries what was seen, such as the name of
    /// a dependency or a middleware function.
    Unknown {
        hint: String,
    },
}

/// A route a project exposes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EndpointSpec {
    pub id: EndpointId,
    pub method: HttpMethod,
    pub path: PathTemplate,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path_params: Vec<ParamSpec>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub query_params: Vec<ParamSpec>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<ParamSpec>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<BodySchema>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthRequirement>,

    /// Absent for endpoints known only at runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceLocation>,
    pub origin: Origin,

    /// Tag, router name, or folder. Drives the explorer tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    pub confidence: Confidence,

    /// A router was declared but never mounted, so this path may be unreachable.
    ///
    /// Kept rather than dropped: an unmounted router is usually a bug in the project, and
    /// saying so is more useful than silently omitting the routes.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub orphaned: bool,

    /// Anything a framework adapter wants to carry that the model does not name.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

impl EndpointSpec {
    /// Build a spec, deriving the id and inferring confidence from the path.
    pub fn new(method: HttpMethod, path: PathTemplate, origin: Origin) -> Self {
        let id = EndpointId::new(&method, &path);
        let confidence = if !path.is_resolved() {
            Confidence::Low
        } else if origin.is_authoritative() {
            Confidence::High
        } else {
            Confidence::Medium
        };

        // Path parameters are implied by the template itself, so seed them here rather than
        // making every adapter remember to.
        let path_params = path
            .param_names()
            .into_iter()
            .map(|name| ParamSpec::new(name).required())
            .collect();

        EndpointSpec {
            id,
            method,
            path,
            path_params,
            query_params: Vec::new(),
            headers: Vec::new(),
            body: None,
            auth: None,
            source: None,
            origin,
            group: None,
            summary: None,
            description: None,
            confidence,
            orphaned: false,
            metadata: serde_json::Map::new(),
        }
    }

    pub fn with_source(mut self, source: SourceLocation) -> Self {
        self.source = Some(source);
        self
    }

    pub fn with_group(mut self, group: impl Into<String>) -> Self {
        self.group = Some(group.into());
        self
    }

    /// How this endpoint reads in the explorer: `GET /api/v1/users/{user_id}`.
    pub fn display(&self) -> String {
        format!("{} {}", self.method, self.path.render(ParamStyle::Braces))
    }

    /// Whether anything about this endpoint is unknown, and therefore worth flagging.
    pub fn has_gaps(&self) -> bool {
        !self.path.is_resolved() || self.orphaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::PathSegment;

    fn fastapi() -> Origin {
        Origin::StaticScan {
            framework: "fastapi".into(),
        }
    }

    #[test]
    fn id_is_stable_across_param_renames() {
        let a = EndpointSpec::new(
            HttpMethod::Get,
            PathTemplate::parse("/users/{id}", ParamStyle::Braces),
            fastapi(),
        );
        let b = EndpointSpec::new(
            HttpMethod::Get,
            PathTemplate::parse("/users/{user_id}", ParamStyle::Braces),
            fastapi(),
        );
        assert_eq!(a.id, b.id);
    }

    #[test]
    fn id_distinguishes_method_and_path() {
        let get = EndpointSpec::new(
            HttpMethod::Get,
            PathTemplate::parse("/users", ParamStyle::Braces),
            fastapi(),
        );
        let post = EndpointSpec::new(
            HttpMethod::Post,
            PathTemplate::parse("/users", ParamStyle::Braces),
            fastapi(),
        );
        assert_ne!(get.id, post.id);
    }

    #[test]
    fn path_params_are_seeded_from_the_template() {
        let spec = EndpointSpec::new(
            HttpMethod::Get,
            PathTemplate::parse("/orgs/{org}/users/{user_id}", ParamStyle::Braces),
            fastapi(),
        );
        let names: Vec<_> = spec.path_params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["org", "user_id"]);
        assert!(spec.path_params.iter().all(|p| p.required));
    }

    #[test]
    fn unresolved_paths_are_low_confidence_and_flagged() {
        let path = PathTemplate::from_segments(vec![
            PathSegment::unresolved("settings.API_PREFIX"),
            PathSegment::literal("users"),
        ]);
        let spec = EndpointSpec::new(HttpMethod::Get, path, fastapi());
        assert_eq!(spec.confidence, Confidence::Low);
        assert!(spec.has_gaps());
    }

    #[test]
    fn authoritative_origins_rank_higher_than_inference() {
        let path = PathTemplate::parse("/users", ParamStyle::Braces);
        let static_spec = EndpointSpec::new(HttpMethod::Get, path.clone(), fastapi());
        let runtime_spec = EndpointSpec::new(
            HttpMethod::Get,
            path,
            Origin::Runtime {
                framework: "fastapi".into(),
            },
        );
        assert_eq!(static_spec.confidence, Confidence::Medium);
        assert_eq!(runtime_spec.confidence, Confidence::High);
    }

    #[test]
    fn round_trips_through_json() {
        let spec = EndpointSpec::new(
            HttpMethod::Post,
            PathTemplate::parse("/users/{id}", ParamStyle::Braces),
            fastapi(),
        )
        .with_source(SourceLocation::new("app/api/users.py", 42))
        .with_group("users");

        let json = serde_json::to_string(&spec).unwrap();
        let back: EndpointSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(spec, back);
    }

    #[test]
    fn display_reads_as_method_then_path() {
        let spec = EndpointSpec::new(
            HttpMethod::Delete,
            PathTemplate::parse("/users/{id}", ParamStyle::Braces),
            fastapi(),
        );
        assert_eq!(spec.display(), "DELETE /users/{id}");
    }
}
