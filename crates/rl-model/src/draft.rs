//! [`RequestDraft`] — an executable request.
//!
//! The counterpart to [`crate::EndpointSpec`]. A draft has concrete values and a real URL,
//! but no source location; a spec has a shape and a source location, but no values. Saved
//! requests and history hold drafts.

use crate::method::HttpMethod;
use crate::path::ParamStyle;
use crate::spec::{ApiKeyLocation, EndpointId, EndpointSpec};
use crate::vars::{ResolveError, Resolved, VariableContext};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub struct RequestId(String);

impl RequestId {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        RequestId(uuid::Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One row in a key/value table — query parameters, headers, cookies, form fields.
///
/// `enabled` is stored rather than the row being deleted, so toggling a parameter off
/// produces a one-word diff in a committed collection instead of a removed line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct KeyValue {
    pub key: String,
    #[serde(default)]
    pub value: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

fn default_true() -> bool {
    true
}

/// A generated default is different every time; stating one in the schema would make the
/// published schema change on every build and suggest a value nobody should copy.
fn without_default(schema: &mut schemars::Schema) {
    schema.remove("default");
}

impl KeyValue {
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        KeyValue {
            key: key.into(),
            value: value.into(),
            enabled: true,
            description: None,
        }
    }

    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }
}

/// Concrete authentication for a request.
///
/// Auth is structured rather than written as a raw header. That is what lets a request
/// round-trip: an environment switch can swap the token, and export can re-render the header
/// correctly. A hand-written `Authorization` header cannot be reasoned about.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthConfig {
    #[default]
    None,
    /// Take auth from the enclosing collection or workspace.
    Inherit,
    Bearer {
        token: String,
    },
    Basic {
        username: String,
        password: String,
    },
    ApiKey {
        key: String,
        value: String,
        location: ApiKeyLocation,
    },
}

impl AuthConfig {
    pub fn is_none(&self) -> bool {
        matches!(self, AuthConfig::None)
    }
}

/// One part of a multipart body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FormPart {
    Text {
        name: String,
        value: String,
        #[serde(default = "default_true")]
        enabled: bool,
    },
    File {
        name: String,
        path: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_type: Option<String>,
        #[serde(default = "default_true")]
        enabled: bool,
    },
}

/// A request body.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BodyValue {
    #[default]
    None,
    Json {
        content: String,
    },
    /// Any other textual body, with the content type stated explicitly.
    Text {
        content: String,
        content_type: String,
    },
    /// `application/x-www-form-urlencoded`
    Form {
        fields: Vec<KeyValue>,
    },
    /// `multipart/form-data`
    Multipart {
        parts: Vec<FormPart>,
    },
    /// Sent verbatim from a file.
    Binary {
        path: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_type: Option<String>,
    },
}

impl BodyValue {
    pub fn is_none(&self) -> bool {
        matches!(self, BodyValue::None)
    }

    /// The `Content-Type` this body implies, unless the request states one explicitly.
    pub fn implied_content_type(&self) -> Option<&str> {
        match self {
            BodyValue::None => None,
            BodyValue::Json { .. } => Some("application/json"),
            BodyValue::Text { content_type, .. } => Some(content_type),
            BodyValue::Form { .. } => Some("application/x-www-form-urlencoded"),
            BodyValue::Multipart { .. } => Some("multipart/form-data"),
            BodyValue::Binary { content_type, .. } => {
                content_type.as_deref().or(Some("application/octet-stream"))
            }
        }
    }
}

/// Per-request transport settings.
///
/// Deliberately per-request, never global: disabling certificate verification for one call
/// against localhost must not silently weaken a later call to production.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RequestSettings {
    /// Off by default — an API client should show what the server actually returned, and
    /// following a redirect can forward an `Authorization` header to an unintended host.
    #[serde(default)]
    pub follow_redirects: bool,
    #[serde(default = "default_max_redirects")]
    pub max_redirects: u8,
    /// For local development against self-signed certificates.
    ///
    /// Deliberately **not serialized**. `docs/security.md` promises this toggle is "never
    /// sticky", and persisting it into a committed collection would break that promise in
    /// the worst way: the next person to clone the repository would inherit
    /// certificate verification being off, silently, without ever having chosen it.
    /// It lasts for the session that set it and no longer.
    #[serde(skip)]
    pub accept_invalid_certs: bool,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_max_redirects() -> u8 {
    10
}

fn default_timeout_ms() -> u64 {
    30_000
}

impl Default for RequestSettings {
    fn default() -> Self {
        RequestSettings {
            follow_redirects: false,
            max_redirects: default_max_redirects(),
            accept_invalid_certs: false,
            timeout_ms: default_timeout_ms(),
        }
    }
}

impl RequestSettings {
    /// Whether these are the defaults, and so need not be written down.
    pub fn is_default(&self) -> bool {
        *self == RequestSettings::default()
    }
}

/// An executable request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RequestDraft {
    /// Generated when absent, so a hand-written or agent-written flow need not invent one.
    #[serde(default = "RequestId::new")]
    #[schemars(transform = without_default)]
    pub id: RequestId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Where the request sits inside its collection: `Users/Admin`. A path rather than a
    /// tree, so the collection file stays a flat list and a move is a one-line diff.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,

    /// The spec this came from, when it came from discovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec_ref: Option<EndpointId>,

    pub method: HttpMethod,
    /// May contain `{{variables}}` and `{path_param}` placeholders.
    pub url: String,

    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub path_values: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub query: Vec<KeyValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<KeyValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cookies: Vec<KeyValue>,

    #[serde(default, skip_serializing_if = "AuthConfig::is_none")]
    pub auth: AuthConfig,
    // Bodies and settings are omitted when they carry nothing, so a committed collection
    // stays readable. A `body: {type: none}` block on every GET is noise in a diff.
    #[serde(default, skip_serializing_if = "BodyValue::is_none")]
    pub body: BodyValue,
    #[serde(default, skip_serializing_if = "RequestSettings::is_default")]
    pub settings: RequestSettings,
}

impl RequestDraft {
    pub fn new(method: HttpMethod, url: impl Into<String>) -> Self {
        RequestDraft {
            id: RequestId::new(),
            name: None,
            folder: None,
            spec_ref: None,
            method,
            url: url.into(),
            path_values: BTreeMap::new(),
            query: Vec::new(),
            headers: Vec::new(),
            cookies: Vec::new(),
            auth: AuthConfig::None,
            body: BodyValue::None,
            settings: RequestSettings::default(),
        }
    }

    /// Build a draft from a discovered endpoint.
    ///
    /// The base URL is prepended and the path is rendered in `{brace}` form, leaving
    /// placeholders for the caller to fill. Known query parameters and headers are seeded as
    /// enabled rows, so what discovery found is what gets sent; untick one to leave it out.
    pub fn from_spec(spec: &EndpointSpec, base_url: &str) -> Self {
        let path = spec.path.render(ParamStyle::Braces);
        let url = format!("{}{}", base_url.trim_end_matches('/'), path);

        let mut draft = RequestDraft::new(spec.method.clone(), url);
        draft.spec_ref = Some(spec.id.clone());
        draft.name = spec.summary.clone();

        draft.path_values = spec
            .path_params
            .iter()
            .map(|p| (p.name.clone(), String::new()))
            .collect();

        draft.query = spec
            .query_params
            .iter()
            .map(|p| {
                let value = p
                    .default
                    .as_ref()
                    .map(render_json_scalar)
                    .unwrap_or_default();
                KeyValue {
                    key: p.name.clone(),
                    value,
                    enabled: true,
                    description: p.description.clone(),
                }
            })
            .collect();

        draft.headers = spec
            .headers
            .iter()
            .map(|p| KeyValue {
                key: p.name.clone(),
                value: String::new(),
                enabled: true,
                description: p.description.clone(),
            })
            .collect();

        if let Some(body) = &spec.body {
            let content = body
                .example
                .as_ref()
                .map(|e| serde_json::to_string_pretty(e).unwrap_or_default())
                .unwrap_or_default();

            draft.body = if body.content_type.contains("json") {
                BodyValue::Json { content }
            } else {
                BodyValue::Text {
                    content,
                    content_type: body.content_type.clone(),
                }
            };
        }

        draft
    }

    /// Substitute path parameters into the URL. Missing values are left as placeholders so
    /// the gap stays visible rather than collapsing into a malformed path.
    pub fn url_with_path_values(&self) -> String {
        let mut url = self.url.clone();
        for (name, value) in &self.path_values {
            if !value.is_empty() {
                url = url.replace(&format!("{{{name}}}"), value);
            }
        }
        url
    }

    /// Every `{{variable}}` this request references, across all its fields.
    pub fn variable_references(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        let mut push = |text: &str| {
            for name in VariableContext::references(text) {
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        };

        push(&self.url);
        for value in self.path_values.values() {
            push(value);
        }
        for row in self
            .query
            .iter()
            .chain(&self.headers)
            .chain(&self.cookies)
            .filter(|r| r.enabled)
        {
            push(&row.key);
            push(&row.value);
        }
        match &self.auth {
            AuthConfig::Bearer { token } => push(token),
            AuthConfig::Basic { username, password } => {
                push(username);
                push(password);
            }
            AuthConfig::ApiKey { key, value, .. } => {
                push(key);
                push(value);
            }
            _ => {}
        }
        match &self.body {
            BodyValue::Json { content } => push(content),
            BodyValue::Text { content, .. } => push(content),
            BodyValue::Form { fields } => {
                for f in fields.iter().filter(|f| f.enabled) {
                    push(&f.key);
                    push(&f.value);
                }
            }
            _ => {}
        }

        names
    }

    /// Resolve every `{{variable}}` in this request.
    ///
    /// Returns the resolved draft alongside the set of secrets that went into it, so the
    /// caller can redact before writing anything to history.
    pub fn resolve(
        &self,
        ctx: &VariableContext,
    ) -> Result<(RequestDraft, BTreeSet<String>), ResolveError> {
        let mut secrets_used = BTreeSet::new();
        let mut resolve = |text: &str| -> Result<String, ResolveError> {
            let Resolved {
                value,
                secrets_used: used,
            } = ctx.resolve(text)?;
            secrets_used.extend(used);
            Ok(value)
        };

        let mut out = self.clone();
        out.url = resolve(&self.url)?;

        for value in out.path_values.values_mut() {
            *value = resolve(&value.clone())?;
        }

        for row in out
            .query
            .iter_mut()
            .chain(out.headers.iter_mut())
            .chain(out.cookies.iter_mut())
            .filter(|r| r.enabled)
        {
            row.key = resolve(&row.key.clone())?;
            row.value = resolve(&row.value.clone())?;
        }

        out.auth = match &self.auth {
            AuthConfig::Bearer { token } => AuthConfig::Bearer {
                token: resolve(token)?,
            },
            AuthConfig::Basic { username, password } => AuthConfig::Basic {
                username: resolve(username)?,
                password: resolve(password)?,
            },
            AuthConfig::ApiKey {
                key,
                value,
                location,
            } => AuthConfig::ApiKey {
                key: resolve(key)?,
                value: resolve(value)?,
                location: *location,
            },
            other => other.clone(),
        };

        out.body = match &self.body {
            BodyValue::Json { content } => BodyValue::Json {
                content: resolve(content)?,
            },
            BodyValue::Text {
                content,
                content_type,
            } => BodyValue::Text {
                content: resolve(content)?,
                content_type: content_type.clone(),
            },
            BodyValue::Form { fields } => {
                let mut out_fields = fields.clone();
                for f in out_fields.iter_mut().filter(|f| f.enabled) {
                    f.key = resolve(&f.key.clone())?;
                    f.value = resolve(&f.value.clone())?;
                }
                BodyValue::Form { fields: out_fields }
            }
            other => other.clone(),
        };

        Ok((out, secrets_used))
    }
}

fn render_json_scalar(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::{PathTemplate, TypeHint};
    use crate::spec::{BodySchema, Origin, ParamSpec};

    fn spec() -> EndpointSpec {
        let mut s = EndpointSpec::new(
            HttpMethod::Get,
            PathTemplate::parse("/api/v1/users/{user_id}", ParamStyle::Braces),
            Origin::StaticScan {
                framework: "fastapi".into(),
            },
        );
        s.query_params = vec![
            ParamSpec::new("page")
                .with_type(TypeHint::Integer)
                .required(),
            ParamSpec::new("search"),
        ];
        s
    }

    fn ctx() -> VariableContext {
        let mut c = VariableContext::new();
        c.environment
            .insert("base_url".into(), "https://api.example.com".into());
        c.secrets.insert("api_token".into(), "s3cr3t".into());
        c
    }

    #[test]
    fn from_spec_builds_a_usable_url() {
        let draft = RequestDraft::from_spec(&spec(), "http://localhost:8000");
        assert_eq!(draft.url, "http://localhost:8000/api/v1/users/{user_id}");
        assert_eq!(draft.spec_ref, Some(spec().id));
    }

    #[test]
    fn from_spec_does_not_double_the_slash() {
        let draft = RequestDraft::from_spec(&spec(), "http://localhost:8000/");
        assert_eq!(draft.url, "http://localhost:8000/api/v1/users/{user_id}");
    }

    #[test]
    fn from_spec_seeds_path_params_as_empty_placeholders() {
        let draft = RequestDraft::from_spec(&spec(), "http://x");
        assert_eq!(draft.path_values.get("user_id"), Some(&String::new()));
    }

    #[test]
    fn discovered_query_params_start_enabled() {
        let draft = RequestDraft::from_spec(&spec(), "http://x");
        let page = draft.query.iter().find(|q| q.key == "page").unwrap();
        let search = draft.query.iter().find(|q| q.key == "search").unwrap();
        assert!(page.enabled);
        assert!(
            search.enabled,
            "optional ones too — untick to drop, rather than tick to add"
        );
    }

    #[test]
    fn a_json_body_schema_becomes_a_json_body() {
        let mut s = spec();
        s.body = Some(BodySchema {
            content_type: "application/json".into(),
            schema: None,
            example: Some(serde_json::json!({"name": "Aryan"})),
            required: true,
        });
        let draft = RequestDraft::from_spec(&s, "http://x");
        match draft.body {
            BodyValue::Json { content } => assert!(content.contains("Aryan")),
            other => panic!("expected a json body, got {other:?}"),
        }
    }

    #[test]
    fn path_values_substitute_into_the_url() {
        let mut draft = RequestDraft::from_spec(&spec(), "http://localhost:8000");
        draft.path_values.insert("user_id".into(), "42".into());
        assert_eq!(
            draft.url_with_path_values(),
            "http://localhost:8000/api/v1/users/42"
        );
    }

    #[test]
    fn an_unfilled_path_value_stays_a_visible_placeholder() {
        let draft = RequestDraft::from_spec(&spec(), "http://localhost:8000");
        assert!(draft.url_with_path_values().contains("{user_id}"));
    }

    #[test]
    fn resolution_covers_url_headers_auth_and_body() {
        let mut draft = RequestDraft::new(HttpMethod::Post, "{{base_url}}/users");
        draft.headers.push(KeyValue::new("X-Env", "{{base_url}}"));
        draft.auth = AuthConfig::Bearer {
            token: "{{secret:api_token}}".into(),
        };
        draft.body = BodyValue::Json {
            content: r#"{"host": "{{base_url}}"}"#.into(),
        };

        let (resolved, secrets) = draft.resolve(&ctx()).unwrap();

        assert_eq!(resolved.url, "https://api.example.com/users");
        assert_eq!(resolved.headers[0].value, "https://api.example.com");
        assert_eq!(
            resolved.auth,
            AuthConfig::Bearer {
                token: "s3cr3t".into()
            }
        );
        match resolved.body {
            BodyValue::Json { content } => assert!(content.contains("https://api.example.com")),
            other => panic!("expected json, got {other:?}"),
        }
        assert!(secrets.contains("api_token"));
    }

    #[test]
    fn disabled_rows_are_not_resolved_so_a_broken_one_cannot_block_a_send() {
        let mut draft = RequestDraft::new(HttpMethod::Get, "{{base_url}}/x");
        draft
            .query
            .push(KeyValue::new("broken", "{{does_not_exist}}").disabled());

        let (resolved, _) = draft.resolve(&ctx()).unwrap();
        assert_eq!(resolved.query[0].value, "{{does_not_exist}}");
    }

    #[test]
    fn an_undefined_variable_in_an_enabled_row_fails_the_send() {
        let mut draft = RequestDraft::new(HttpMethod::Get, "{{base_url}}/x");
        draft.query.push(KeyValue::new("q", "{{nope}}"));
        assert!(draft.resolve(&ctx()).is_err());
    }

    #[test]
    fn variable_references_are_collected_from_every_field() {
        let mut draft = RequestDraft::new(HttpMethod::Post, "{{base_url}}/x");
        draft.headers.push(KeyValue::new("X-A", "{{token}}"));
        draft.body = BodyValue::Json {
            content: "{{payload}}".into(),
        };

        let refs = draft.variable_references();
        assert!(refs.contains(&"base_url".to_string()));
        assert!(refs.contains(&"token".to_string()));
        assert!(refs.contains(&"payload".to_string()));
    }

    #[test]
    fn redirects_are_off_and_certs_verified_by_default() {
        let s = RequestSettings::default();
        assert!(!s.follow_redirects);
        assert!(!s.accept_invalid_certs);
    }

    #[test]
    fn disabling_cert_verification_never_persists() {
        // A saved collection must not be able to hand the next person who clones the repo a
        // request with certificate verification silently switched off.
        let mut draft = RequestDraft::new(HttpMethod::Get, "https://localhost:8443/x");
        draft.settings.accept_invalid_certs = true;

        let json = serde_json::to_string(&draft).unwrap();
        assert!(!json.contains("accept_invalid_certs"));

        let reloaded: RequestDraft = serde_json::from_str(&json).unwrap();
        assert!(!reloaded.settings.accept_invalid_certs);
    }

    #[test]
    fn round_trips_through_json() {
        let mut draft = RequestDraft::from_spec(&spec(), "http://localhost:8000");
        draft.auth = AuthConfig::Bearer {
            token: "{{secret:api_token}}".into(),
        };
        let json = serde_json::to_string(&draft).unwrap();
        let back: RequestDraft = serde_json::from_str(&json).unwrap();
        assert_eq!(draft, back);
    }

    #[test]
    fn a_saved_draft_keeps_the_secret_reference_not_the_value() {
        let mut draft = RequestDraft::new(HttpMethod::Get, "{{base_url}}/x");
        draft.auth = AuthConfig::Bearer {
            token: "{{secret:api_token}}".into(),
        };
        let json = serde_json::to_string(&draft).unwrap();
        assert!(json.contains("{{secret:api_token}}"));
        assert!(!json.contains("s3cr3t"));
    }
}
