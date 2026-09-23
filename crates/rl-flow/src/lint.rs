//! Checking a flow for mistakes that would only show up when it runs.
//!
//! [`Flow::validate`] refuses a graph that cannot run at all — a cycle, a dangling edge.
//! This finds the ones that run and then fail, or worse, pass without testing anything: a
//! `{{token}}` nothing defines, a step that reads a variable captured by a step that runs
//! after it, a request with no assertions. They are warnings rather than errors, because a
//! flow half-written on the canvas is full of them, and because an agent writing a flow
//! can read one and fix it.
//!
//! Variables are followed the way the runner does: in execution order, starting from the
//! environment, each step adding what it declares or extracts.

use rl_model::{Flow, NodeId, NodeKind, VariableContext};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Something that will probably go wrong when the flow runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Warning {
    /// The node it is about, or none for the flow as a whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<NodeId>,
    pub message: String,
}

impl Warning {
    fn at(node: &NodeId, message: impl Into<String>) -> Self {
        Warning {
            node: Some(node.clone()),
            message: message.into(),
        }
    }
}

/// Check `flow` against the variables it will run with and, when a scan has been done, the
/// endpoints the project serves.
///
/// Returns nothing for a flow [`Flow::validate`] refuses; that error comes first.
pub fn lint(
    flow: &Flow,
    variables: &VariableContext,
    endpoints: Option<&BTreeSet<String>>,
) -> Vec<Warning> {
    let Ok(order) = flow.execution_order() else {
        return Vec::new();
    };
    let mut warnings = Vec::new();

    // Who defines each name, so an out-of-order reference can say which step to move.
    let mut defined_by: BTreeMap<String, &NodeId> = BTreeMap::new();
    for node in &flow.nodes {
        for name in defines(&node.kind) {
            defined_by.entry(name).or_insert(&node.id);
        }
    }

    let mut scope = variables.clone();
    for id in &order {
        let Some(node) = flow.node(id) else { continue };

        // A variables block may build on the pairs above it in the same block.
        let mut local = scope.clone();
        for (text, declares) in templates(&node.kind) {
            for name in VariableContext::references(&text) {
                if local.is_defined(&name) {
                    continue;
                }
                let message = match defined_by.get(&name) {
                    Some(later) if *later != id => format!(
                        "`{{{{{name}}}}}` is set by `{later}`, which does not run before this \
                         step; add an edge from `{later}` to `{id}`"
                    ),
                    _ => format!(
                        "`{{{{{name}}}}}` is not defined by the environment or by any step \
                         before this one"
                    ),
                };
                warnings.push(Warning::at(id, message));
            }
            if let Some(name) = declares {
                local.set_local(name, "");
            }
        }

        if let NodeKind::Request {
            request, assert, ..
        } = &node.kind
        {
            if assert.is_empty() {
                warnings.push(Warning::at(
                    id,
                    "has no assertions, so it passes on any response, a 500 included; \
                     assert at least the status",
                ));
            }
            if let (Some(spec), Some(endpoints)) = (&request.spec_ref, endpoints) {
                if !endpoints.is_empty() && !endpoints.contains(spec.as_str()) {
                    warnings.push(Warning::at(
                        id,
                        format!("`spec_ref` names `{spec}`, which the last scan did not find"),
                    ));
                }
            }
        }

        for name in defines(&node.kind) {
            scope.set_local(name, "");
        }
    }

    warnings
}

/// Every template a node resolves, in the order it resolves them, each with the variable
/// it declares once resolved (a variables block's pair).
fn templates(kind: &NodeKind) -> Vec<(String, Option<String>)> {
    match kind {
        NodeKind::Request {
            request, assert, ..
        } => {
            let mut out: Vec<(String, Option<String>)> = request
                .variable_references()
                .into_iter()
                .map(|name| (format!("{{{{{name}}}}}"), None))
                .collect();
            out.extend(assert.iter().map(|a| (a.expected.clone(), None)));
            out
        }
        NodeKind::Condition { left, right, .. } => {
            vec![(left.clone(), None), (right.clone(), None)]
        }
        NodeKind::Variables { variables } => variables
            .iter()
            .map(|v| (v.value.clone(), Some(v.name.trim().to_string())))
            .collect(),
        NodeKind::Display { text } => vec![(text.clone(), None)],
    }
}

/// The variables a node puts in scope when it passes.
fn defines(kind: &NodeKind) -> Vec<String> {
    match kind {
        NodeKind::Request { extract, .. } => extract.iter().map(|e| e.name.clone()).collect(),
        NodeKind::Variables { variables } => variables
            .iter()
            .map(|v| v.name.trim().to_string())
            .filter(|n| !n.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rl_model::{
        Assertion, EndpointId, Extraction, HttpMethod, Node, Operator, PathTemplate, RequestDraft,
        ValueSource,
    };

    fn request(url: &str) -> Node {
        let mut node = Node::request(RequestDraft::new(HttpMethod::Get, url));
        if let NodeKind::Request { assert, .. } = &mut node.kind {
            assert.push(Assertion::status_ok());
        }
        node
    }

    fn extracting(mut node: Node, name: &str) -> Node {
        if let NodeKind::Request { extract, .. } = &mut node.kind {
            extract.push(Extraction {
                name: name.into(),
                source: ValueSource::Body { path: name.into() },
            });
        }
        node
    }

    fn env() -> VariableContext {
        let mut ctx = VariableContext::new();
        ctx.environment
            .insert("base_url".into(), "http://localhost".into());
        ctx
    }

    fn messages(warnings: &[Warning]) -> Vec<String> {
        warnings.iter().map(|w| w.message.clone()).collect()
    }

    #[test]
    fn a_well_formed_chain_is_clean() {
        let mut flow = Flow::new("ok");
        let login = flow.add(extracting(request("{{base_url}}/login"), "token"));
        let me = flow.add(request("{{base_url}}/me?t={{token}}"));
        flow.connect(&login, &me);
        assert_eq!(lint(&flow, &env(), None), vec![]);
    }

    #[test]
    fn an_undefined_variable_is_named() {
        let mut flow = Flow::new("undefined");
        flow.add(request("{{base_url}}/users/{{user_id}}"));
        let warnings = lint(&flow, &env(), None);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("`{{user_id}}` is not defined"));
    }

    /// The mistake an agent makes most: capture in one step, use in another, forget the
    /// edge. Without the edge the reader runs first, in document order.
    #[test]
    fn a_variable_captured_later_says_which_edge_is_missing() {
        let mut flow = Flow::new("unwired");
        let me = flow.add(request("{{base_url}}/me?t={{token}}"));
        let login = flow.add(extracting(request("{{base_url}}/login"), "token"));
        let warnings = lint(&flow, &env(), None);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].node.as_ref(), Some(&me));
        assert!(warnings[0]
            .message
            .contains(&format!("add an edge from `{login}` to `{me}`")));
    }

    #[test]
    fn a_request_without_assertions_is_flagged() {
        let mut flow = Flow::new("unchecked");
        flow.add(Node::request(RequestDraft::new(
            HttpMethod::Get,
            "{{base_url}}/health",
        )));
        assert!(messages(&lint(&flow, &env(), None))[0].contains("has no assertions"));
    }

    #[test]
    fn variables_blocks_build_on_their_own_pairs_and_feed_later_steps() {
        let mut flow = Flow::new("inputs");
        flow.add(Node::variables(&[
            ("name", "ann"),
            ("email", "{{name}}@example.com"),
        ]));
        flow.add(request("{{base_url}}/users?email={{email}}"));
        assert_eq!(lint(&flow, &env(), None), vec![]);
    }

    #[test]
    fn assertion_expectations_and_display_text_are_checked_too() {
        let mut flow = Flow::new("templates");
        let mut node = request("{{base_url}}/x");
        if let NodeKind::Request { assert, .. } = &mut node.kind {
            assert.push(Assertion {
                source: ValueSource::Body { path: "id".into() },
                op: Operator::Equals,
                expected: "{{expected_id}}".into(),
            });
        }
        flow.add(node);
        flow.add(Node::display("made {{made}}"));
        let found = messages(&lint(&flow, &env(), None));
        assert!(found.iter().any(|m| m.contains("{{expected_id}}")));
        assert!(found.iter().any(|m| m.contains("{{made}}")));
    }

    #[test]
    fn a_secret_reference_resolves_against_the_secret_store() {
        let mut ctx = env();
        ctx.secrets.insert("api_token".into(), "s3cret".into());
        let mut flow = Flow::new("secret");
        flow.add(request("{{base_url}}/x?k={{secret:api_token}}"));
        assert_eq!(lint(&flow, &ctx, None), vec![]);
    }

    #[test]
    fn a_spec_ref_the_scan_did_not_find_is_flagged() {
        let mut flow = Flow::new("stale");
        let mut node = request("{{base_url}}/gone");
        if let NodeKind::Request { request, .. } = &mut node.kind {
            request.spec_ref = Some(EndpointId::new(
                &HttpMethod::Get,
                &PathTemplate::parse("/gone", rl_model::ParamStyle::Braces),
            ));
        }
        flow.add(node);
        let known: BTreeSet<String> = ["GET /users".to_string()].into();
        assert!(messages(&lint(&flow, &env(), Some(&known)))[0].contains("did not find"));
        assert_eq!(
            lint(&flow, &env(), None),
            vec![],
            "no scan, nothing to compare"
        );
    }

    #[test]
    fn a_graph_that_cannot_run_is_left_to_validate() {
        let mut flow = Flow::new("cycle");
        let a = flow.add(request("/a"));
        let b = flow.add(request("/b"));
        flow.connect(&a, &b);
        flow.connect(&b, &a);
        assert_eq!(lint(&flow, &env(), None), vec![]);
    }
}
