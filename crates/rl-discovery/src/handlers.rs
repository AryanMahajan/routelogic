//! Handlers declared apart from their routes.
//!
//! A Go route names its handler — `r.GET("/users", h.List)` — and the function lives in
//! another file of the same package as often as not. The adapter records what every
//! handler-shaped function reads from its request, by name, and the scan joins the two
//! here once the graph has resolved the routes: the same shape as [`crate::models`].

use crate::adapters::UNSPECIFIED_METHODS;
use crate::facts::{HandlerFact, RouteFact};
use rl_model::EndpointSpec;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Every handler a scan saw, by package and name.
#[derive(Debug, Default)]
pub struct HandlerIndex {
    by_package: BTreeMap<(PathBuf, String), HandlerFact>,
    by_name: BTreeMap<String, Vec<HandlerFact>>,
}

impl HandlerIndex {
    pub fn new(handlers: Vec<HandlerFact>) -> Self {
        let mut index = HandlerIndex::default();
        for handler in handlers {
            let package = package_of(&handler.module);
            index
                .by_name
                .entry(handler.name.clone())
                .or_default()
                .push(handler.clone());
            index
                .by_package
                .entry((package, handler.name.clone()))
                .or_insert(handler);
        }
        index
    }

    pub fn is_empty(&self) -> bool {
        self.by_package.is_empty()
    }

    /// The handler a route names: in the route's own package first, then anywhere it is
    /// the only one by that name — `users.List` from another package is `List` there.
    fn find(&self, route: &RouteFact) -> Option<&HandlerFact> {
        let reference = route.handler.as_ref()?;
        let package = package_of(&reference.module);
        if let Some(found) = self.by_package.get(&(package, reference.name.clone())) {
            return Some(found);
        }
        let mut candidates: Vec<&str> = vec![reference.name.as_str()];
        if let Some((_, rest)) = reference.name.split_once('.') {
            candidates.push(rest);
        }
        candidates
            .into_iter()
            .find_map(|name| match self.by_name.get(name) {
                Some(found) if found.len() == 1 => found.first(),
                _ => None,
            })
    }

    /// Give a spec what its handler reads, where the registration said nothing. Returns
    /// `false` when the spec should be dropped: the route was registered without a
    /// method, and the handler checks `r.Method` for others.
    pub fn fill(&self, spec: &mut EndpointSpec, route: &RouteFact) -> bool {
        let Some(handler) = self.find(route) else {
            return true;
        };
        if route.methods == UNSPECIFIED_METHODS && !handler.methods.is_empty() {
            if !handler.methods.contains(&spec.method) {
                return false;
            }
            spec.summary = None;
        }
        if spec.query_params.is_empty() {
            spec.query_params = handler
                .query_params
                .iter()
                .filter(|p| !spec.path.param_names().contains(&p.name.as_str()))
                .cloned()
                .collect();
        }
        if spec.headers.is_empty() {
            spec.headers = handler.headers.clone();
        }
        if spec.body.is_none() {
            spec.body = handler.body.clone();
        }
        if spec.auth.is_none() {
            spec.auth = handler.auth.clone();
        }
        true
    }
}

fn package_of(module: &Path) -> PathBuf {
    module.parent().map(Path::to_path_buf).unwrap_or_default()
}
