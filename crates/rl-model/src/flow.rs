//! [`Flow`] — a multi-step API test, as a graph.
//!
//! A flow is the document a developer builds on the canvas: request nodes wired together so
//! that what one response returns feeds the next request. The model here is *only* the
//! document — nodes, edges, what to extract, what to assert. Running one is `rl-flow`'s job.
//!
//! ## What a node is
//!
//! A [`Node`] is a [`RequestDraft`] plus the two things a test does with its response:
//! [`Extraction`]s that turn parts of it into `{{variables}}` for later nodes, and
//! [`Assertion`]s that decide whether the step passed. Both sit *on* the request rather than
//! being nodes of their own, so a five-call flow is five cards on the canvas, not fifteen.
//!
//! Three other kinds keep a flow self-contained:
//!
//! - [`NodeKind::Condition`] compares two interpolated strings and sends the run down its
//!   `true` or `false` output.
//! - [`NodeKind::Variables`] declares `name = value` pairs — the flow's own inputs, so
//!   changing who a test looks up is one edit on the canvas rather than an environment
//!   change. With nothing wired into it, it runs before everything else.
//! - [`NodeKind::Display`] resolves a template and shows the text, for putting the result
//!   a run was after in plain words on the canvas.
//!
//! ## Order comes from the edges
//!
//! Edges are dependencies. [`Flow::execution_order`] is a topological sort — where two nodes
//! are independent the tie is broken by their position in [`Flow::nodes`], never by where
//! they sit on the canvas. Moving a card around must not change what the test does.
//!
//! ## Provenance
//!
//! A node built from a discovered endpoint keeps [`RequestDraft::spec_ref`], which is what
//! lets the canvas jump from a card to the handler that serves it.

use crate::draft::RequestDraft;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const CURRENT_VERSION: u32 = 1;

/// The output of a [`NodeKind::Condition`] taken when the predicate holds.
pub const HANDLE_TRUE: &str = "true";
/// The output of a [`NodeKind::Condition`] taken when the predicate does not hold.
pub const HANDLE_FALSE: &str = "false";

/// Any string, unique within the flow. The canvas makes UUIDs; a hand-written flow can use
/// words — `login`, `create_user` — which read better in an edge list.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub struct NodeId(String);

impl NodeId {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        NodeId(uuid::Uuid::new_v4().to_string())
    }

    pub fn from_raw(raw: impl Into<String>) -> Self {
        NodeId(raw.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where a node sits on the canvas. Part of the document, never part of its meaning.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
pub struct Position {
    pub x: f64,
    pub y: f64,
}

/// Where a value comes from — for an extraction or the left side of an assertion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "from", rename_all = "snake_case")]
pub enum ValueSource {
    /// The HTTP status code, as a number.
    Status,
    /// A response header, case-insensitively. The first one when repeated. The field is
    /// `header` rather than `name` because this enum is flattened into [`Extraction`],
    /// which already has a `name`.
    Header { header: String },
    /// A path into the response body parsed as JSON: `user.id`, `items[0].name`, `$` for
    /// the whole document. Scalars are extracted as their plain text; objects and arrays as
    /// compact JSON.
    Body { path: String },
    /// The body as text, whole.
    BodyText,
    /// How long the exchange took, in milliseconds.
    Duration,
}

/// Turn part of a response into a variable for the nodes that follow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Extraction {
    /// The variable name later nodes reference as `{{name}}`.
    pub name: String,
    #[serde(flatten)]
    pub source: ValueSource,
}

/// How an assertion or condition compares its two sides.
///
/// When both sides parse as numbers the comparison is numeric, so `"200"` equals `200` and
/// `"9"` is not greater than `"10"`. Otherwise it is a plain string comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Operator {
    Equals,
    NotEquals,
    Contains,
    NotContains,
    /// The value is present at all — a header that was sent, a body path that resolves.
    /// The right-hand side is ignored.
    Exists,
    NotExists,
    GreaterThan,
    LessThan,
}

impl Operator {
    pub const ALL: [Operator; 8] = [
        Operator::Equals,
        Operator::NotEquals,
        Operator::Contains,
        Operator::NotContains,
        Operator::Exists,
        Operator::NotExists,
        Operator::GreaterThan,
        Operator::LessThan,
    ];

    /// Whether the operator looks at the right-hand side at all.
    pub fn is_unary(self) -> bool {
        matches!(self, Operator::Exists | Operator::NotExists)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Operator::Equals => "equals",
            Operator::NotEquals => "not_equals",
            Operator::Contains => "contains",
            Operator::NotContains => "not_contains",
            Operator::Exists => "exists",
            Operator::NotExists => "not_exists",
            Operator::GreaterThan => "greater_than",
            Operator::LessThan => "less_than",
        }
    }
}

/// A check on a response. Any failing assertion fails its node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Assertion {
    #[serde(flatten)]
    pub source: ValueSource,
    pub op: Operator,
    /// May contain `{{variables}}`. Ignored by unary operators.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub expected: String,
}

impl Assertion {
    /// The check a freshly added request node starts with: it did not fail.
    pub fn status_ok() -> Self {
        Assertion {
            source: ValueSource::Status,
            op: Operator::LessThan,
            expected: "400".to_string(),
        }
    }
}

/// One `name = value` pair declared by a [`NodeKind::Variables`] block. The value may
/// reference other variables.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Variable {
    pub name: String,
    #[serde(default)]
    pub value: String,
}

/// What a node does.
// Nearly every node is a request; boxing the common case to slim the rare one would be
// the wrong trade, and a flow holds a handful of nodes, not millions.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NodeKind {
    /// Send a request, then extract and assert on what came back.
    Request {
        request: RequestDraft,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        extract: Vec<Extraction>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        assert: Vec<Assertion>,
    },
    /// Compare two interpolated strings and continue down one of two outputs.
    Condition {
        left: String,
        op: Operator,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        right: String,
    },
    /// Declare variables for the steps that follow — the flow's inputs.
    Variables {
        #[serde(default)]
        variables: Vec<Variable>,
    },
    /// Resolve a template and show it.
    Display {
        #[serde(default)]
        text: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Node {
    pub id: NodeId,
    /// A label for the card. Falls back to the request's name or display in the UI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Where the card sits. Leave it out and the node is placed by [`Flow::lay_out`] the
    /// next time the flow is loaded or saved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<Position>,
    #[serde(flatten)]
    pub kind: NodeKind,
}

impl Node {
    pub fn request(request: RequestDraft) -> Self {
        Node {
            id: NodeId::new(),
            name: None,
            position: None,
            kind: NodeKind::Request {
                request,
                extract: Vec::new(),
                assert: Vec::new(),
            },
        }
    }

    pub fn condition(left: impl Into<String>, op: Operator, right: impl Into<String>) -> Self {
        Node {
            id: NodeId::new(),
            name: None,
            position: None,
            kind: NodeKind::Condition {
                left: left.into(),
                op,
                right: right.into(),
            },
        }
    }

    pub fn variables(pairs: &[(&str, &str)]) -> Self {
        Node {
            id: NodeId::new(),
            name: None,
            position: None,
            kind: NodeKind::Variables {
                variables: pairs
                    .iter()
                    .map(|(name, value)| Variable {
                        name: name.to_string(),
                        value: value.to_string(),
                    })
                    .collect(),
            },
        }
    }

    pub fn display(text: impl Into<String>) -> Self {
        Node {
            id: NodeId::new(),
            name: None,
            position: None,
            kind: NodeKind::Display { text: text.into() },
        }
    }

    pub fn at(mut self, x: f64, y: f64) -> Self {
        self.position = Some(Position { x, y });
        self
    }

    pub fn is_condition(&self) -> bool {
        matches!(self.kind, NodeKind::Condition { .. })
    }

    /// The outputs this node has. A request has one, unnamed; a condition has two.
    pub fn handles(&self) -> &'static [&'static str] {
        match self.kind {
            NodeKind::Condition { .. } => &[HANDLE_TRUE, HANDLE_FALSE],
            _ => &[],
        }
    }

    /// A [`NodeKind::Variables`] block with nothing wired into it: the flow's inputs,
    /// which run before anything else regardless of where they were added.
    pub fn is_input_block(&self, flow: &Flow) -> bool {
        matches!(self.kind, NodeKind::Variables { .. }) && flow.upstream(&self.id).next().is_none()
    }
}

/// A dependency: `to` runs after `from`, and only if `from` passed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Edge {
    pub from: NodeId,
    pub to: NodeId,
    /// Which output of `from` this leaves — [`HANDLE_TRUE`] or [`HANDLE_FALSE`] on a
    /// condition, absent on a request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
}

impl Edge {
    pub fn new(from: &NodeId, to: &NodeId) -> Self {
        Edge {
            from: from.clone(),
            to: to.clone(),
            handle: None,
        }
    }

    pub fn via(mut self, handle: &str) -> Self {
        self.handle = Some(handle.to_string());
        self
    }
}

/// A saved flow: one file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Flow {
    #[serde(default = "default_version")]
    pub version: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub edges: Vec<Edge>,
}

fn default_version() -> u32 {
    CURRENT_VERSION
}

/// Why a flow cannot run as written.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FlowError {
    #[error("two nodes share the id {0}")]
    DuplicateNode(NodeId),

    #[error("an edge refers to a node that does not exist: {0}")]
    DanglingEdge(NodeId),

    #[error("node {node} has no output named {handle:?}")]
    UnknownHandle { node: NodeId, handle: String },

    #[error("node {0} is connected to itself")]
    SelfLoop(NodeId),

    /// A flow with a cycle has no first step. The nodes named are the ones left over once
    /// everything that could be ordered was — the cycle, and whatever hangs off it.
    #[error("the flow has a cycle through {}", .0.iter().map(|n| n.as_str()).collect::<Vec<_>>().join(", "))]
    Cycle(Vec<NodeId>),
}

impl Flow {
    pub fn new(name: impl Into<String>) -> Self {
        Flow {
            version: CURRENT_VERSION,
            name: name.into(),
            description: None,
            nodes: Vec::new(),
            edges: Vec::new(),
        }
    }

    /// Add a node, returning its id.
    pub fn add(&mut self, node: Node) -> NodeId {
        let id = node.id.clone();
        self.nodes.push(node);
        id
    }

    /// Wire `from` into `to`.
    pub fn connect(&mut self, from: &NodeId, to: &NodeId) {
        self.edges.push(Edge::new(from, to));
    }

    pub fn node(&self, id: &NodeId) -> Option<&Node> {
        self.nodes.iter().find(|n| &n.id == id)
    }

    /// The nodes with an edge into `id`, in edge order.
    pub fn upstream<'a>(&'a self, id: &'a NodeId) -> impl Iterator<Item = &'a Edge> + 'a {
        self.edges.iter().filter(move |e| &e.to == id)
    }

    /// Check the graph is something that can be run.
    pub fn validate(&self) -> Result<(), FlowError> {
        let mut seen = BTreeSet::new();
        for node in &self.nodes {
            if !seen.insert(&node.id) {
                return Err(FlowError::DuplicateNode(node.id.clone()));
            }
        }

        for edge in &self.edges {
            let from = self
                .node(&edge.from)
                .ok_or_else(|| FlowError::DanglingEdge(edge.from.clone()))?;
            if self.node(&edge.to).is_none() {
                return Err(FlowError::DanglingEdge(edge.to.clone()));
            }
            if edge.from == edge.to {
                return Err(FlowError::SelfLoop(edge.from.clone()));
            }
            let handles = from.handles();
            match &edge.handle {
                Some(handle) if !handles.contains(&handle.as_str()) => {
                    return Err(FlowError::UnknownHandle {
                        node: edge.from.clone(),
                        handle: handle.clone(),
                    });
                }
                None if !handles.is_empty() => {
                    return Err(FlowError::UnknownHandle {
                        node: edge.from.clone(),
                        handle: String::new(),
                    });
                }
                _ => {}
            }
        }

        self.execution_order().map(|_| ())
    }

    /// The order nodes run in: every node after everything it depends on.
    ///
    /// Kahn's algorithm, with ties broken by position in [`Flow::nodes`] so the order is
    /// stable across runs and independent of where the cards sit on the canvas. The one
    /// exception is deliberate: unconnected [`NodeKind::Variables`] blocks are the flow's
    /// inputs and go first, so a variable declared on the canvas is there for every step
    /// whether or not anyone thought to draw an edge from it.
    pub fn execution_order(&self) -> Result<Vec<NodeId>, FlowError> {
        let order = self.topological_order()?;
        let (inputs, rest): (Vec<NodeId>, Vec<NodeId>) = order
            .into_iter()
            .partition(|id| self.node(id).is_some_and(|n| n.is_input_block(self)));
        Ok(inputs.into_iter().chain(rest).collect())
    }

    fn topological_order(&self) -> Result<Vec<NodeId>, FlowError> {
        let index: BTreeMap<&NodeId, usize> = self
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (&n.id, i))
            .collect();

        let mut indegree: Vec<usize> = vec![0; self.nodes.len()];
        let mut outgoing: Vec<Vec<usize>> = vec![Vec::new(); self.nodes.len()];
        for edge in &self.edges {
            let (Some(&from), Some(&to)) = (index.get(&edge.from), index.get(&edge.to)) else {
                return Err(FlowError::DanglingEdge(edge.from.clone()));
            };
            // The same dependency drawn twice is still one dependency.
            if !outgoing[from].contains(&to) {
                outgoing[from].push(to);
                indegree[to] += 1;
            }
        }

        // The ready set is ordered by document index, so whenever several nodes could go
        // next the one added first does — not the one whose edge happened to be drawn first.
        let mut ready: BTreeSet<usize> = (0..self.nodes.len())
            .filter(|&i| indegree[i] == 0)
            .collect();
        let mut order = Vec::with_capacity(self.nodes.len());

        while let Some(current) = ready.pop_first() {
            order.push(current);
            for &next in &outgoing[current] {
                indegree[next] -= 1;
                if indegree[next] == 0 {
                    ready.insert(next);
                }
            }
        }

        if order.len() != self.nodes.len() {
            let stuck = (0..self.nodes.len())
                .filter(|i| !order.contains(i))
                .map(|i| self.nodes[i].id.clone())
                .collect();
            return Err(FlowError::Cycle(stuck));
        }

        Ok(order
            .into_iter()
            .map(|i| self.nodes[i].id.clone())
            .collect())
    }

    /// Give every node without a [`Position`] one, and return how many were placed.
    ///
    /// Columns follow dependency depth — the longest chain of edges leading into a node —
    /// and rows follow document order within a column, so a chain reads left to right and
    /// a fan-out stacks. Nodes that already have a position are never moved: a flow laid out
    /// by hand stays as it was, and anything new is placed below it. A graph with a cycle
    /// cannot be layered, and is laid out as one column in document order instead.
    pub fn lay_out(&mut self) -> usize {
        const COLUMN: f64 = 340.0;
        const ROW: f64 = 190.0;
        const GAP: f64 = 260.0;

        let unplaced: Vec<usize> = (0..self.nodes.len())
            .filter(|&i| self.nodes[i].position.is_none())
            .collect();
        if unplaced.is_empty() {
            return 0;
        }

        let depth = self.depths();

        // Rows are counted among the nodes being placed, so new cards do not leave gaps
        // for placed ones that sit elsewhere.
        let mut rows: BTreeMap<usize, usize> = BTreeMap::new();
        let mut layered = Vec::with_capacity(unplaced.len());
        for &i in &unplaced {
            let column = depth[i];
            let row = rows.entry(column).or_insert(0);
            layered.push((i, column as f64 * COLUMN, *row as f64 * ROW));
            *row += 1;
        }

        // Beside nothing, the layout starts at the origin; beside a flow already laid out,
        // it starts at that flow's left edge, a gap below its lowest card.
        let placed: Vec<Position> = self.nodes.iter().filter_map(|n| n.position).collect();
        let origin = match placed.iter().map(|p| p.x).reduce(f64::min) {
            Some(left) => Position {
                x: left,
                y: placed.iter().map(|p| p.y).fold(f64::MIN, f64::max) + GAP,
            },
            None => Position::default(),
        };

        for (i, x, y) in &layered {
            self.nodes[*i].position = Some(Position {
                x: origin.x + x,
                y: origin.y + y,
            });
        }
        layered.len()
    }

    /// Each node's column: the length of the longest edge chain leading into it. Every node
    /// is column 0 when the graph has a cycle, since then there is no chain to measure.
    fn depths(&self) -> Vec<usize> {
        let mut depth = vec![0usize; self.nodes.len()];
        let Ok(order) = self.topological_order() else {
            return depth;
        };
        let index: BTreeMap<&NodeId, usize> = self
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (&n.id, i))
            .collect();
        for id in &order {
            let Some(&to) = index.get(id) else { continue };
            for edge in self.upstream(id) {
                if let Some(&from) = index.get(&edge.from) {
                    depth[to] = depth[to].max(depth[from] + 1);
                }
            }
        }
        depth
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HttpMethod;

    fn get(url: &str) -> Node {
        Node::request(RequestDraft::new(HttpMethod::Get, url))
    }

    /// login → me → user, the shape from the feature brief.
    fn chain() -> (Flow, NodeId, NodeId, NodeId) {
        let mut flow = Flow::new("smoke");
        let login = flow.add(get("{{base_url}}/auth/login"));
        let me = flow.add(get("{{base_url}}/me"));
        let user = flow.add(get("{{base_url}}/users/{{user_id}}"));
        flow.connect(&login, &me);
        flow.connect(&me, &user);
        (flow, login, me, user)
    }

    #[test]
    fn a_chain_runs_in_dependency_order() {
        let (flow, login, me, user) = chain();
        assert_eq!(flow.execution_order().unwrap(), vec![login, me, user]);
        flow.validate().unwrap();
    }

    #[test]
    fn order_follows_edges_not_document_or_canvas_position() {
        let mut flow = Flow::new("reversed");
        // Added last and drawn at the top: still runs first, because the edges say so.
        let second = flow.add(get("/second").at(0.0, 100.0));
        let first = flow.add(get("/first").at(0.0, 0.0));
        flow.connect(&first, &second);
        assert_eq!(flow.execution_order().unwrap(), vec![first, second]);
    }

    #[test]
    fn independent_nodes_keep_document_order() {
        let mut flow = Flow::new("fan-out");
        let root = flow.add(get("/root"));
        let a = flow.add(get("/a"));
        let b = flow.add(get("/b"));
        // Edges drawn b-first must not make b run first.
        flow.connect(&root, &b);
        flow.connect(&root, &a);
        assert_eq!(flow.execution_order().unwrap(), vec![root, a, b]);
    }

    #[test]
    fn a_duplicate_edge_is_one_dependency() {
        let mut flow = Flow::new("dup");
        let a = flow.add(get("/a"));
        let b = flow.add(get("/b"));
        flow.connect(&a, &b);
        flow.connect(&a, &b);
        assert_eq!(flow.execution_order().unwrap(), vec![a, b]);
    }

    #[test]
    fn a_cycle_is_refused_and_named() {
        let (mut flow, login, _me, user) = chain();
        flow.connect(&user, &login);
        match flow.validate() {
            Err(FlowError::Cycle(nodes)) => assert_eq!(nodes.len(), 3),
            other => panic!("expected a cycle, got {other:?}"),
        }
    }

    #[test]
    fn a_cycle_off_to_the_side_does_not_hide_the_nodes_before_it() {
        let mut flow = Flow::new("partial");
        let start = flow.add(get("/start"));
        let a = flow.add(get("/a"));
        let b = flow.add(get("/b"));
        flow.connect(&start, &a);
        flow.connect(&a, &b);
        flow.connect(&b, &a);
        match flow.execution_order() {
            Err(FlowError::Cycle(nodes)) => {
                assert!(!nodes.contains(&start));
                assert_eq!(nodes, vec![a, b]);
            }
            other => panic!("expected a cycle, got {other:?}"),
        }
    }

    #[test]
    fn structural_problems_are_reported_precisely() {
        let mut flow = Flow::new("bad");
        let a = flow.add(get("/a"));
        flow.nodes.push(flow.nodes[0].clone());
        assert!(matches!(flow.validate(), Err(FlowError::DuplicateNode(_))));
        flow.nodes.pop();

        flow.edges.push(Edge::new(&a, &NodeId::from_raw("ghost")));
        assert!(matches!(flow.validate(), Err(FlowError::DanglingEdge(_))));
        flow.edges.clear();

        flow.edges.push(Edge::new(&a, &a));
        assert!(matches!(flow.validate(), Err(FlowError::SelfLoop(_))));
        flow.edges.clear();

        // A request has one unnamed output; a named handle on it is a mistake.
        let b = flow.add(get("/b"));
        flow.edges.push(Edge::new(&a, &b).via("true"));
        assert!(matches!(
            flow.validate(),
            Err(FlowError::UnknownHandle { .. })
        ));
        flow.edges.clear();

        // A condition has two named outputs; an unnamed edge does not say which.
        let cond = flow.add(Node::condition("{{x}}", Operator::Equals, "1"));
        flow.edges.push(Edge::new(&cond, &b));
        assert!(matches!(
            flow.validate(),
            Err(FlowError::UnknownHandle { .. })
        ));
        flow.edges.clear();
        flow.edges.push(Edge::new(&cond, &b).via(HANDLE_TRUE));
        flow.validate().unwrap();
    }

    /// The YAML on disk is what a reviewer reads in a pull request, so it must stay flat
    /// and obvious: a node is its request plus `extract` and `assert` lists.
    #[test]
    fn round_trips_through_json_with_a_readable_shape() {
        let (mut flow, login, ..) = chain();
        if let NodeKind::Request {
            extract, assert, ..
        } = &mut flow.nodes[0].kind
        {
            extract.push(Extraction {
                name: "token".into(),
                source: ValueSource::Body {
                    path: "access_token".into(),
                },
            });
            assert.push(Assertion::status_ok());
        }
        flow.nodes[0].id = login;

        let text = serde_json::to_string_pretty(&flow).unwrap();
        assert!(text.contains(r#""type": "request""#));
        assert!(text.contains(r#""from": "body""#));
        assert!(text.contains(r#""path": "access_token""#));
        assert!(text.contains(r#""op": "less_than""#));

        let back: Flow = serde_json::from_str(&text).unwrap();
        assert_eq!(back, flow);
    }

    /// `Extraction` flattens its source, so no source variant may carry a field called
    /// `name` — the JSON would have two of them and one would silently win.
    #[test]
    fn a_header_extraction_serializes_both_its_name_and_its_header() {
        let e = Extraction {
            name: "location".into(),
            source: ValueSource::Header {
                header: "Location".into(),
            },
        };
        let json = serde_json::to_value(&e).unwrap();
        assert_eq!(json["name"], "location");
        assert_eq!(json["header"], "Location");
        assert_eq!(json["from"], "header");
        let back: Extraction = serde_json::from_value(json).unwrap();
        assert_eq!(back, e);
    }

    /// A Variables block added last, with no edges, still runs first: it is the flow's
    /// input, not a step. One with an edge into it runs where the edge puts it.
    #[test]
    fn unconnected_variable_blocks_run_before_everything_else() {
        let (mut flow, login, me, user) = chain();
        let inputs = flow.add(Node::variables(&[("who", "ann")]));
        let mid = flow.add(Node::variables(&[("token", "{{t}}")]));
        flow.connect(&me, &mid);
        flow.connect(&mid, &user);
        assert_eq!(
            flow.execution_order().unwrap(),
            vec![inputs, login, me, mid, user]
        );
    }

    #[test]
    fn a_flow_without_positions_is_laid_out_left_to_right_by_dependency() {
        let mut flow = Flow::new("fan");
        let login = flow.add(get("/login"));
        let a = flow.add(get("/a"));
        let b = flow.add(get("/b"));
        let done = flow.add(Node::display("done"));
        flow.connect(&login, &a);
        flow.connect(&login, &b);
        flow.connect(&a, &done);
        flow.connect(&b, &done);

        assert_eq!(flow.lay_out(), 4);
        let at = |id: &NodeId| flow.node(id).unwrap().position.unwrap();
        assert_eq!((at(&login).x, at(&login).y), (0.0, 0.0));
        assert_eq!(at(&a).x, at(&b).x, "siblings share a column");
        assert!(at(&b).y > at(&a).y, "and stack in document order");
        assert!(
            at(&done).x > at(&a).x,
            "what depends on them sits to the right"
        );
        assert_eq!(flow.lay_out(), 0, "nothing left to place");
    }

    #[test]
    fn laying_out_never_moves_a_placed_node_and_puts_new_ones_below() {
        let mut flow = Flow::new("mixed");
        let kept = flow.add(get("/kept").at(500.0, 40.0));
        let new = flow.add(get("/new"));
        flow.lay_out();
        assert_eq!(
            flow.node(&kept).unwrap().position,
            Some(Position { x: 500.0, y: 40.0 })
        );
        let placed = flow.node(&new).unwrap().position.unwrap();
        assert_eq!(placed.x, 500.0, "aligned with the flow already there");
        assert!(placed.y > 40.0, "below it, not on top of it");
    }

    #[test]
    fn a_hand_written_flow_needs_no_ids_or_positions_beyond_its_own_names() {
        let yaml = r#"{
            "name": "written by hand",
            "nodes": [
                {"id": "login", "type": "request",
                 "request": {"method": "POST", "url": "{{base_url}}/login"},
                 "extract": [{"name": "token", "from": "body", "path": "token"}],
                 "assert": [{"from": "status", "op": "equals", "expected": "200"}]},
                {"id": "me", "type": "request",
                 "request": {"method": "GET", "url": "{{base_url}}/me"}}
            ],
            "edges": [{"from": "login", "to": "me"}]
        }"#;
        let mut flow: Flow = serde_json::from_str(yaml).unwrap();
        flow.validate().unwrap();
        assert!(flow.nodes.iter().all(|n| n.position.is_none()));
        let NodeKind::Request { request, .. } = &flow.nodes[0].kind else {
            panic!("a request")
        };
        assert!(!request.id.as_str().is_empty(), "a request id is generated");
        flow.lay_out();
        assert!(flow.nodes.iter().all(|n| n.position.is_some()));
    }

    /// `docs/flow.schema.json` is what agents and editors validate against, so it must be
    /// what the code actually accepts. Regenerate with `UPDATE_SNAPSHOTS=1`.
    #[test]
    fn the_published_schema_matches_the_model() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/flow.schema.json");
        let schema = serde_json::to_string_pretty(&schemars::schema_for!(Flow)).unwrap() + "\n";
        if std::env::var("UPDATE_SNAPSHOTS").is_ok() {
            std::fs::write(&path, &schema).unwrap();
            return;
        }
        let published = std::fs::read_to_string(&path)
            .unwrap_or_default()
            .replace("\r\n", "\n");
        assert!(
            published == schema,
            "docs/flow.schema.json is out of date; regenerate with UPDATE_SNAPSHOTS=1 cargo test -p rl-model"
        );
    }

    #[test]
    fn operators_know_which_ignore_the_right_hand_side() {
        assert!(Operator::Exists.is_unary());
        assert!(!Operator::Equals.is_unary());
        assert_eq!(Operator::ALL.len(), 8);
    }
}
