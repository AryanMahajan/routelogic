//! What an agent connected over MCP is allowed to do, and what it is shown.
//!
//! An agent writing a flow is most useful when it can check its work: send the login, see
//! that the token comes back as `accessToken`, and capture it by that name. That means it
//! sends real requests, so this module decides where they may go and what comes back.
//!
//! ## Where requests may go
//!
//! Loopback — `localhost`, `127.0.0.1`, `::1` — always, with every method: that is the
//! server the developer is running, and a flow that creates a user needs `POST` against
//! it. Anything else only when `workspace.yaml` lists it:
//!
//! ```yaml
//! agent:
//!   allow:
//!     - host: api.staging.example.com
//!       methods: [GET, HEAD, OPTIONS]
//! ```
//!
//! The check is on the host a request resolves to, after `{{base_url}}` is filled in —
//! never on an environment's name, since an environment called `local` can point anywhere.
//! It runs again before every redirect, so an allowed host cannot hand the request on.
//!
//! ## What comes back
//!
//! Every secret value is replaced with `{{secret:NAME}}` before anything reaches the agent
//! — in the request echo, the response, captured variables — so a token the workspace
//! resolved does not leave the machine in an agent's context. The name is not secret, and
//! it is exactly what the agent should write in a flow.

use crate::{CoreError, Result, RouteLogic};
use rl_flow::{FlowRun, NodeResult, Outcome, RunOptions, Sender, SkipReason};
use rl_http::{Exchange, HttpEngine, PreparedRequest};
use rl_model::{Flow, RequestDraft, VariableContext};
use rl_workspace::{AgentAllow, AgentConfig};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::net::IpAddr;
use url::{Host, Url};

/// History rows for requests an agent sent carry this as their source.
pub const AGENT_SOURCE: &str = "agent";

/// Where an agent may send requests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentPolicy {
    allow: Vec<AgentAllow>,
}

/// Why a request was not sent, in terms an agent can act on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refusal {
    pub method: String,
    pub host: String,
    pub reason: String,
    /// What would allow it.
    pub fix: String,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} — {}", self.reason, self.fix)
    }
}

/// The policy as it stands, for the agent to read before it tries anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyView {
    /// Always allowed, with every method.
    pub loopback: Vec<String>,
    pub allow: Vec<AllowView>,
    pub how_to_change: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowView {
    pub host: String,
    /// Empty means every method.
    pub methods: Vec<String>,
}

impl AgentPolicy {
    pub fn new(config: &AgentConfig) -> Self {
        AgentPolicy {
            allow: config.allow.clone(),
        }
    }

    /// Whether `method` may be sent to `url`.
    pub fn check(&self, method: &str, url: &Url) -> std::result::Result<(), Refusal> {
        if is_loopback(url) {
            return Ok(());
        }
        let host = display_host(url);
        let method = method.to_ascii_uppercase();

        let matching: Vec<&AgentAllow> = self
            .allow
            .iter()
            .filter(|rule| host_matches(&rule.host, url))
            .collect();
        if matching.is_empty() {
            return Err(Refusal {
                method: method.clone(),
                reason: format!(
                    "`{host}` is not a loopback host and is not in the agent allow list"
                ),
                fix: format!(
                    "to let an agent reach it, add `- host: {host}` under `agent.allow` in \
                     .routelogic/workspace.yaml"
                ),
                host,
            });
        }
        if matching.iter().any(|rule| {
            rule.methods.is_empty()
                || rule
                    .methods
                    .iter()
                    .any(|m| m.as_str().eq_ignore_ascii_case(&method))
        }) {
            return Ok(());
        }
        Err(Refusal {
            reason: format!("`{host}` is allowed for an agent, but not with {method}"),
            fix: format!(
                "add {method} to that host's `methods` under `agent.allow` in \
                 .routelogic/workspace.yaml"
            ),
            method,
            host,
        })
    }

    pub fn view(&self) -> PolicyView {
        PolicyView {
            loopback: vec!["localhost".into(), "127.0.0.1".into(), "::1".into()],
            allow: self
                .allow
                .iter()
                .map(|rule| AllowView {
                    host: rule.host.clone(),
                    methods: rule.methods.iter().map(|m| m.to_string()).collect(),
                })
                .collect(),
            how_to_change: "edit `agent.allow` in .routelogic/workspace.yaml; loopback hosts \
                            are always allowed"
                .into(),
        }
    }
}

/// `localhost`, anything under `.localhost`, the loopback addresses, and the unspecified
/// address (`0.0.0.0` reaches the local machine too).
fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(domain)) => {
            let domain = domain.trim_end_matches('.').to_ascii_lowercase();
            domain == "localhost" || domain.ends_with(".localhost")
        }
        Some(Host::Ipv4(ip)) => IpAddr::V4(ip).is_loopback() || ip.is_unspecified(),
        Some(Host::Ipv6(ip)) => {
            ip.is_loopback()
                || ip.is_unspecified()
                || ip.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
        }
        None => false,
    }
}

fn display_host(url: &Url) -> String {
    match (url.host_str(), url.port()) {
        (Some(host), Some(port)) => format!("{host}:{port}"),
        (Some(host), None) => host.to_string(),
        (None, _) => url.as_str().to_string(),
    }
}

/// `api.example.com` (any port), `api.example.com:8443` (that port), `*.example.com`
/// (subdomains, not the apex). Case-insensitive.
fn host_matches(pattern: &str, url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let pattern = pattern.trim().to_ascii_lowercase();
    let (pattern_host, pattern_port) = match pattern.rsplit_once(':') {
        // `[::1]:80` and bare IPv6 contain colons; only a trailing number is a port.
        Some((h, p))
            if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) && !h.ends_with(':') =>
        {
            (h.to_string(), p.parse::<u16>().ok())
        }
        _ => (pattern, None),
    };
    if let Some(port) = pattern_port {
        if url.port_or_known_default() != Some(port) {
            return false;
        }
    }
    match pattern_host.strip_prefix("*.") {
        Some(suffix) => host.ends_with(&format!(".{suffix}")),
        None => host == pattern_host.trim_start_matches('[').trim_end_matches(']'),
    }
}

// --- masking ----------------------------------------------------------------------------

/// Replace every secret value in `text` with `{{secret:NAME}}`. Longer secrets first, so
/// one that contains another is not half-masked.
pub fn mask(text: &str, variables: &VariableContext) -> String {
    let mut secrets: Vec<(&String, &String)> = variables
        .secrets
        .iter()
        .filter(|(_, value)| !value.is_empty())
        .collect();
    secrets.sort_by_key(|(_, value)| std::cmp::Reverse(value.len()));
    let mut out = text.to_string();
    for (name, value) in secrets {
        out = out.replace(value.as_str(), &format!("{{{{secret:{name}}}}}"));
    }
    out
}

/// [`mask`] applied to every string in a JSON value, keys included.
pub fn mask_json(value: serde_json::Value, variables: &VariableContext) -> serde_json::Value {
    use serde_json::Value;
    match value {
        Value::String(s) => Value::String(mask(&s, variables)),
        Value::Array(items) => {
            Value::Array(items.into_iter().map(|v| mask_json(v, variables)).collect())
        }
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| (mask(&k, variables), mask_json(v, variables)))
                .collect(),
        ),
        other => other,
    }
}

// --- what the agent is shown ------------------------------------------------------------

/// A response, compact and masked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentResponse {
    pub status: u16,
    pub status_text: String,
    pub duration_ms: u64,
    /// The request as it went out, secrets masked.
    pub request: AgentSent,
    pub headers: Vec<(String, String)>,
    /// The body as text — pretty-printed when it is JSON — masked, and cut at the limit
    /// the caller asked for. A binary body is described rather than shown.
    pub body: String,
    pub body_bytes: usize,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub body_truncated: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redirects: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSent {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

impl AgentResponse {
    pub fn of(exchange: &Exchange, variables: &VariableContext, max_body: usize) -> Self {
        let response = &exchange.response;
        let masked_pairs = |pairs: &[(String, String)]| -> Vec<(String, String)> {
            pairs
                .iter()
                .map(|(k, v)| (k.clone(), mask(v, variables)))
                .collect()
        };

        let (text, textual) = match response.body.pretty_json() {
            Some(pretty) => (pretty, true),
            None if response.body.looks_textual() => {
                (response.body.as_text_lossy().into_owned(), true)
            }
            None => (String::new(), false),
        };
        // Masked before cutting, so a secret straddling the cut is still masked.
        let text = mask(&text, variables);
        let (body, body_truncated) = if !textual {
            (
                format!(
                    "<{} bytes of {}>",
                    response.body.len(),
                    response
                        .body
                        .content_type
                        .as_deref()
                        .unwrap_or("binary data")
                ),
                false,
            )
        } else {
            cut(&text, max_body)
        };

        AgentResponse {
            status: response.status,
            status_text: response.status_text.clone(),
            duration_ms: response.timing.total_ms,
            request: AgentSent {
                method: exchange.request.method.clone(),
                url: mask(&exchange.request.url, variables),
                headers: masked_pairs(&exchange.request.headers),
                body: exchange
                    .request
                    .body_preview
                    .as_deref()
                    .map(|b| cut(&mask(b, variables), max_body).0),
            },
            headers: masked_pairs(&response.headers),
            body,
            body_bytes: response.body.len(),
            body_truncated: body_truncated || response.body.truncated,
            redirects: response
                .redirects
                .iter()
                .map(|hop| format!("{} → {}", hop.status, mask(&hop.to, variables)))
                .collect(),
        }
    }
}

/// At most `max` bytes of `text`, on a character boundary, and whether anything was cut.
fn cut(text: &str, max: usize) -> (String, bool) {
    if text.len() <= max {
        return (text.to_string(), false);
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_string(), true)
}

/// A single send, as the agent sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum AgentSend {
    Sent {
        response: Box<AgentResponse>,
    },
    Refused {
        refusal: Refusal,
    },
    /// It went out and never came back: connection refused, a timeout, a bad URL.
    Failed {
        error: String,
    },
}

/// One step of a flow run, as the agent sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentStep {
    pub node: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `passed`, `failed` or `skipped`.
    pub outcome: String,
    /// Why it failed or was skipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<Box<AgentResponse>>,
    /// Every assertion, with what was actually found.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assertions: Vec<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extracted: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

/// A flow run, as the agent sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRun {
    pub flow: String,
    pub passed: bool,
    pub summary: rl_flow::Summary,
    pub duration_ms: u64,
    pub steps: Vec<AgentStep>,
}

impl AgentRun {
    pub fn of(run: &FlowRun, flow: &Flow, variables: &VariableContext, max_body: usize) -> Self {
        AgentRun {
            flow: run.flow.clone(),
            passed: run.passed(),
            summary: run.summary,
            duration_ms: run.duration_ms,
            steps: run
                .results
                .iter()
                .map(|result| step(result, flow, variables, max_body))
                .collect(),
        }
    }
}

fn step(
    result: &NodeResult,
    flow: &Flow,
    variables: &VariableContext,
    max_body: usize,
) -> AgentStep {
    let (outcome, reason) = match &result.outcome {
        Outcome::Passed => ("passed", None),
        Outcome::Failed { failure } => ("failed", Some(mask(&failure.message(), variables))),
        Outcome::Skipped { reason } => (
            "skipped",
            Some(match reason {
                SkipReason::UpstreamFailed { node } => {
                    format!("`{node}` failed, and this step depends on it")
                }
                SkipReason::BranchNotTaken { node, handle } => {
                    format!("the `{handle}` output of condition `{node}` was not taken")
                }
            }),
        ),
    };
    AgentStep {
        node: result.node.to_string(),
        name: flow.node(&result.node).and_then(|n| n.name.clone()),
        outcome: outcome.to_string(),
        reason,
        request: result.request.as_ref().map(|r| {
            mask(
                &format!("{} {}", r.method, r.url_with_path_values()),
                variables,
            )
        }),
        // A failing step's response is what the agent needs to see; a passing one's is
        // noise at the size of a flow, so only the status survives.
        response: result.exchange.as_ref().map(|exchange| {
            let limit = if result.outcome.is_passed() {
                0
            } else {
                max_body
            };
            Box::new(AgentResponse::of(exchange, variables, limit))
        }),
        assertions: result
            .assertions
            .iter()
            .filter_map(|a| serde_json::to_value(a).ok())
            .map(|v| mask_json(v, variables))
            .collect(),
        extracted: result
            .extracted
            .iter()
            .map(|e| (e.name.clone(), mask(&e.value, variables)))
            .collect(),
        output: result.output.as_ref().map(|o| mask(o, variables)),
    }
}

// --- sending as an agent ----------------------------------------------------------------

/// The engine, asking the policy before every request and every redirect.
struct GatedEngine<'a> {
    engine: &'a HttpEngine,
    policy: &'a AgentPolicy,
}

impl Sender for GatedEngine<'_> {
    fn send(&self, draft: &RequestDraft) -> impl Future<Output = rl_http::Result<Exchange>> + Send {
        let policy = self.policy;
        async move {
            self.engine
                .execute_guarded(draft, &|method, url| {
                    policy.check(method, url).map_err(|r| r.to_string())
                })
                .await
        }
    }
}

impl RouteLogic {
    /// The open workspace's agent policy, read from `workspace.yaml` as it is now — an
    /// agent stays connected for hours, and an edit to the allow list must apply to its
    /// next request. A file that no longer parses is an error, not an empty list: quietly
    /// taking permissions away would be as confusing as quietly granting them.
    pub fn agent_policy(&self) -> Result<AgentPolicy> {
        Ok(AgentPolicy::new(
            &self.workspace()?.manifest_on_disk()?.agent,
        ))
    }

    /// The request as it would go out, masked. Never sends anything, so never refused.
    pub fn prepare_for_agent(
        &self,
        draft: &RequestDraft,
        environment: Option<&str>,
    ) -> Result<PreparedRequest> {
        let variables = self.variables_for(environment)?;
        let resolved = resolve_in(draft, &variables)?;
        let prepared = rl_http::prepare(&resolved)?;
        let value = mask_json(
            serde_json::to_value(&prepared).map_err(|e| CoreError::Internal {
                message: e.to_string(),
            })?,
            &variables,
        );
        serde_json::from_value(value).map_err(|e| CoreError::Internal {
            message: e.to_string(),
        })
    }

    /// Send a request on an agent's behalf: checked against the policy, recorded in history
    /// as the agent's, and returned masked with the body cut at `max_body` bytes.
    pub async fn send_for_agent(
        &self,
        draft: &RequestDraft,
        environment: Option<&str>,
        max_body: usize,
    ) -> Result<AgentSend> {
        let policy = self.agent_policy()?;
        let variables = self.variables_for(environment)?;
        let resolved = resolve_in(draft, &variables)?;

        // Checked here first so a refusal comes back structured; the engine checks again
        // at every redirect. A refusal is recorded like a failed send, so history shows
        // what an agent tried as well as what it did.
        let first = rl_http::prepare(&resolved)?;
        if let Ok(url) = Url::parse(&first.url) {
            if let Err(refusal) = policy.check(&first.method, &url) {
                self.record_for_agent(
                    &resolved,
                    &Err(rl_http::HttpError::Refused {
                        reason: refusal.reason.clone(),
                    }),
                    &variables,
                );
                return Ok(AgentSend::Refused { refusal });
            }
        }

        let gate = GatedEngine {
            engine: self.engine.as_ref(),
            policy: &policy,
        };
        let outcome = gate.send(&resolved).await;
        self.record_for_agent(&resolved, &outcome, &variables);

        Ok(match outcome {
            Ok(exchange) => AgentSend::Sent {
                response: Box::new(AgentResponse::of(&exchange, &variables, max_body)),
            },
            Err(rl_http::HttpError::Refused { reason }) => AgentSend::Refused {
                refusal: Refusal {
                    method: resolved.method.to_string(),
                    host: String::new(),
                    reason: mask(&reason, &variables),
                    fix: "a redirect led to a host the agent may not reach; see agent.allow in \
                          .routelogic/workspace.yaml"
                        .into(),
                },
            },
            Err(error) => AgentSend::Failed {
                error: mask(&CoreError::from(error).message(), &variables),
            },
        })
    }

    fn record_for_agent(
        &self,
        sent: &RequestDraft,
        outcome: &rl_http::Result<Exchange>,
        variables: &VariableContext,
    ) {
        if let Some(workspace) = self.workspace.as_ref() {
            if let Ok(history) = workspace.history() {
                let mut entry = crate::build_entry(sent, outcome);
                entry.source = Some(AGENT_SOURCE.to_string());
                let _ = history.record(&entry.redacted(variables));
            }
        }
    }

    /// Run a flow on an agent's behalf: every request checked against the policy (a refused
    /// one fails its step and says why), each recorded in history as the agent's, and the
    /// run returned masked.
    pub async fn run_flow_for_agent(
        &self,
        flow: &Flow,
        environment: Option<&str>,
        options: RunOptions,
        max_body: usize,
    ) -> Result<AgentRun> {
        flow.validate()?;
        let policy = self.agent_policy()?;
        let variables = self.variables_for(environment)?;
        let mut history = self.workspace.as_ref().and_then(|w| w.history().ok());

        let gate = GatedEngine {
            engine: self.engine.as_ref(),
            policy: &policy,
        };
        let mut record = |event: rl_flow::FlowEvent| {
            if let rl_flow::FlowEvent::NodeFinished { result } = &event {
                if let (Some(history), Some(mut entry)) =
                    (history.as_mut(), crate::flow_entry(result))
                {
                    entry.source = Some(AGENT_SOURCE.to_string());
                    let _ = history.record(&entry.redacted(&variables));
                }
            }
        };
        let run = rl_flow::run_with(flow, options, &variables, &gate, &mut record).await?;
        Ok(AgentRun::of(&run, flow, &variables, max_body))
    }
}

/// Resolve against a given set of variables, reporting every undefined one together.
fn resolve_in(draft: &RequestDraft, variables: &VariableContext) -> Result<RequestDraft> {
    let undefined: Vec<String> = draft
        .variable_references()
        .into_iter()
        .filter(|name| !variables.is_defined(name))
        .collect();
    if !undefined.is_empty() {
        return Err(CoreError::UndefinedVariables { names: undefined });
    }
    Ok(draft.resolve(variables)?.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rl_model::HttpMethod;

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    fn policy(rules: &[(&str, &[&str])]) -> AgentPolicy {
        AgentPolicy::new(&AgentConfig {
            allow: rules
                .iter()
                .map(|(host, methods)| AgentAllow {
                    host: host.to_string(),
                    methods: methods
                        .iter()
                        .map(|m| m.parse::<HttpMethod>().unwrap())
                        .collect(),
                })
                .collect(),
        })
    }

    #[test]
    fn loopback_is_always_allowed_with_every_method() {
        let none = AgentPolicy::default();
        for target in [
            "http://localhost:8000/users",
            "http://LOCALHOST/x",
            "http://api.localhost:3000/x",
            "http://127.0.0.1:5000/x",
            "http://127.1.2.3/x",
            "http://[::1]:8080/x",
            "http://0.0.0.0:8000/x",
        ] {
            assert_eq!(none.check("DELETE", &url(target)), Ok(()), "{target}");
        }
    }

    #[test]
    fn anything_else_is_refused_with_the_line_that_would_allow_it() {
        let refusal = AgentPolicy::default()
            .check("GET", &url("https://api.example.com/users"))
            .unwrap_err();
        assert_eq!(refusal.host, "api.example.com");
        assert!(refusal.fix.contains("- host: api.example.com"));
        assert!(refusal.fix.contains("agent.allow"));

        // Lookalikes of loopback are not loopback.
        for target in [
            "http://localhost.example.com/x",
            "http://mylocalhost/x",
            "http://10.0.0.1/x",
            "http://192.168.1.10/x",
        ] {
            assert!(
                AgentPolicy::default().check("GET", &url(target)).is_err(),
                "{target}"
            );
        }
    }

    #[test]
    fn an_allowed_host_is_limited_to_its_methods() {
        let policy = policy(&[("api.staging.example.com", &["GET", "HEAD"])]);
        let staging = url("https://api.staging.example.com/users");
        assert_eq!(policy.check("GET", &staging), Ok(()));
        assert_eq!(policy.check("get", &staging), Ok(()), "case-insensitive");
        let refusal = policy.check("DELETE", &staging).unwrap_err();
        assert!(refusal.reason.contains("not with DELETE"));
        assert!(refusal.fix.contains("add DELETE"));
    }

    #[test]
    fn host_patterns_ports_and_wildcards() {
        let policy = policy(&[
            ("api.example.com:8443", &[]),
            ("*.internal.test", &[]),
            ("plain.test", &[]),
        ]);
        assert!(policy
            .check("POST", &url("https://api.example.com:8443/x"))
            .is_ok());
        assert!(
            policy
                .check("POST", &url("https://api.example.com/x"))
                .is_err(),
            "the rule names a port, and 443 is not it"
        );
        assert!(policy
            .check("GET", &url("http://a.internal.test/x"))
            .is_ok());
        assert!(policy
            .check("GET", &url("http://deep.a.internal.test/x"))
            .is_ok());
        assert!(
            policy.check("GET", &url("http://internal.test/x")).is_err(),
            "a wildcard covers subdomains, not the apex"
        );
        assert!(
            policy
                .check("GET", &url("http://PLAIN.test:9999/x"))
                .is_ok(),
            "any port"
        );
        assert!(policy.check("GET", &url("http://notplain.test/x")).is_err());
    }

    #[test]
    fn secrets_are_masked_by_name_longest_first() {
        let mut variables = VariableContext::new();
        variables.secrets.insert("short".into(), "abc".into());
        variables.secrets.insert("long".into(), "abcdef".into());
        variables.secrets.insert("empty".into(), String::new());
        assert_eq!(
            mask("token=abcdef, id=abc, other=xyz", &variables),
            "token={{secret:long}}, id={{secret:short}}, other=xyz"
        );
        let json = mask_json(
            serde_json::json!({"auth": "Bearer abcdef", "list": ["abc"], "n": 3}),
            &variables,
        );
        assert_eq!(
            json,
            serde_json::json!({"auth": "Bearer {{secret:long}}", "list": ["{{secret:short}}"], "n": 3})
        );
    }

    #[test]
    fn a_cut_lands_on_a_character_boundary() {
        assert_eq!(cut("héllo", 2), ("h".to_string(), true));
        assert_eq!(cut("hi", 10), ("hi".to_string(), false));
    }
}
