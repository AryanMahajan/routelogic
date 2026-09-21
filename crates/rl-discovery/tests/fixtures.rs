//! Fixture snapshot tests.
//!
//! The highest-value tests in the project. A discovery regression produces no error — just a
//! slightly shorter list of routes that nobody notices — so the only way to catch one is to
//! pin the expected output and compare.
//!
//! Snapshots also record **expected misses**. Documenting what an adapter cannot see is as
//! valuable as documenting what it can: it marks the boundary, and turns a silent change in
//! that boundary into a failing test.
//!
//! Regenerate with `UPDATE_SNAPSHOTS=1 cargo test -p rl-discovery --test fixtures`, then read
//! the diff before committing it. A snapshot accepted without being read proves nothing.

use rl_discovery::scan;
use rl_model::ParamStyle;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Snapshot {
    frameworks: Vec<String>,
    base_url: String,
    routes: Vec<Route>,
    /// Substrings expected to appear among the warnings.
    warnings: Vec<String>,
    /// Paths static analysis is expected *not* to find, and why.
    expected_misses: Vec<Miss>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
struct Route {
    method: String,
    path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    group: Option<String>,
    source: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    orphaned: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    unresolved: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    query: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    headers: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    has_body: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    has_auth: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Miss {
    path: String,
    reason: String,
}

fn fixtures_dir() -> PathBuf {
    // crates/rl-discovery/tests → repo root
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tests")
        .join("fixtures")
}

fn capture(fixture: &str) -> Snapshot {
    let root = fixtures_dir().join(fixture);
    let result = scan(&root).unwrap_or_else(|e| panic!("scanning {fixture} failed: {e}"));

    let mut routes: Vec<Route> = result
        .endpoints
        .iter()
        .map(|spec| Route {
            method: spec.method.to_string(),
            path: spec.path.render(ParamStyle::Braces),
            group: spec.group.clone(),
            source: spec
                .source
                .as_ref()
                .map(|s| {
                    // Forward slashes, so the snapshot is identical on every platform.
                    format!(
                        "{}:{}",
                        s.file.display().to_string().replace('\\', "/"),
                        s.line
                    )
                })
                .unwrap_or_default(),
            orphaned: spec.orphaned,
            unresolved: !spec.path.is_resolved(),
            query: spec.query_params.iter().map(|p| p.name.clone()).collect(),
            headers: spec.headers.iter().map(|p| p.name.clone()).collect(),
            has_body: spec.body.is_some(),
            has_auth: spec.auth.is_some(),
        })
        .collect();
    routes.sort();

    Snapshot {
        frameworks: result.frameworks.iter().map(|f| f.id.clone()).collect(),
        base_url: result
            .base_urls
            .first()
            .map(|c| c.url.clone())
            .unwrap_or_default(),
        routes,
        warnings: Vec::new(),
        expected_misses: Vec::new(),
    }
}

fn check(fixture: &str) {
    let snapshot_path = fixtures_dir().join(fixture).join("expected-routes.json");
    let mut actual = capture(fixture);

    if std::env::var("UPDATE_SNAPSHOTS").is_ok() || !snapshot_path.exists() {
        // Preserve the hand-written sections; only the observed parts are regenerated.
        if let Ok(text) = std::fs::read_to_string(&snapshot_path) {
            if let Ok(existing) = serde_json::from_str::<Snapshot>(&text) {
                actual.warnings = existing.warnings;
                actual.expected_misses = existing.expected_misses;
            }
        }
        let json = serde_json::to_string_pretty(&actual).unwrap();
        std::fs::write(&snapshot_path, json + "\n").unwrap();
        if std::env::var("UPDATE_SNAPSHOTS").is_err() {
            panic!(
                "no snapshot for `{fixture}`; one was written to {} — read it before committing",
                snapshot_path.display()
            );
        }
        return;
    }

    let text = std::fs::read_to_string(&snapshot_path).unwrap();
    let expected: Snapshot = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("{} is not a valid snapshot: {e}", snapshot_path.display()));

    assert_eq!(
        actual.frameworks, expected.frameworks,
        "framework detection changed for `{fixture}`"
    );
    assert_eq!(
        actual.base_url, expected.base_url,
        "base URL inference changed for `{fixture}`"
    );

    // Compare route by route, so a failure names the route rather than dumping both lists.
    let actual_paths: Vec<String> = actual.routes.iter().map(describe).collect();
    let expected_paths: Vec<String> = expected.routes.iter().map(describe).collect();

    // Every difference at once, rather than one per run — fixing a discovery change is much
    // easier when the whole delta is visible.
    let missing: Vec<&String> = expected_paths
        .iter()
        .filter(|p| !actual_paths.contains(p))
        .collect();
    let added: Vec<&String> = actual_paths
        .iter()
        .filter(|p| !expected_paths.contains(p))
        .collect();

    assert!(
        missing.is_empty() && added.is_empty(),
        "`{fixture}` route set changed.\n  no longer found: {missing:?}\n  newly found: {added:?}\n\
         (if this is correct, regenerate with UPDATE_SNAPSHOTS=1 and read the diff)"
    );

    assert_eq!(actual.routes, expected.routes, "route detail changed");

    // Warnings are compared as substrings, so their wording may improve while the facts
    // they report stay pinned.
    let root = fixtures_dir().join(fixture);
    let reported = scan(&root).unwrap().warnings.join(
        "
",
    );
    for expected_warning in &expected.warnings {
        assert!(
            reported.contains(expected_warning),
            "`{fixture}` no longer warns about {expected_warning:?}; got:
{reported}"
        );
    }

    // The recorded misses must still be misses. If one starts being found, that is good news
    // — but the snapshot has to say so deliberately.
    for miss in &expected.expected_misses {
        assert!(
            !actual.routes.iter().any(|r| r.path == miss.path),
            "`{}` is now discovered, which is an improvement — remove it from expected_misses",
            miss.path
        );
    }
}

fn describe(route: &Route) -> String {
    format!("{} {}", route.method, route.path)
}

#[test]
fn fastapi_fixture_matches_snapshot() {
    check("fastapi");
}

/// The warnings a developer would see. Checked separately, because their wording is allowed
/// to improve while the *facts* they report must not change.
#[test]
fn fastapi_fixture_reports_the_expected_problems() {
    let root = fixtures_dir().join("fastapi");
    let result = scan(&root).unwrap();
    let warnings = result.warnings.join("\n");

    assert!(
        warnings.contains("never mounted"),
        "the unmounted router should be reported, got:\n{warnings}"
    );
    assert!(
        result.endpoints.iter().any(|e| e.orphaned),
        "the unmounted router's route should be kept and flagged"
    );
    assert!(
        result.endpoints.iter().any(|e| !e.path.is_resolved()),
        "the settings-derived prefix should be an unresolved gap, not a guess"
    );
}

#[test]
fn nextjs_fixture_matches_snapshot() {
    check("nextjs");
}

#[test]
fn express_fixture_matches_snapshot() {
    check("express");
}

/// The Express fixture has one of everything the graph is meant to handle.
#[test]
fn express_fixture_reports_the_expected_problems() {
    let root = fixtures_dir().join("express");
    let result = scan(&root).unwrap();
    let warnings = result.warnings.join("\n");

    assert!(
        warnings.contains("legacy.js:router") && warnings.contains("never mounted"),
        "the unmounted router should be reported, got:\n{warnings}"
    );
    assert!(
        warnings.contains("createV2Router()"),
        "the factory-built router should be reported as unfollowable, got:\n{warnings}"
    );
    assert!(
        result
            .endpoints
            .iter()
            .any(|e| e.path.unresolved_exprs() == vec!["process.env.API_PREFIX"]),
        "the env prefix should be an unresolved gap, not a guess"
    );
    assert!(
        !result
            .endpoints
            .iter()
            .any(|e| e.path.to_string().contains("from-a-test")),
        "test files must not contribute routes"
    );
}

#[test]
fn flask_fixture_matches_snapshot() {
    check("flask");
}

#[test]
fn flask_fixture_reports_the_expected_problems() {
    let root = fixtures_dir().join("flask");
    let result = scan(&root).unwrap();
    let warnings = result.warnings.join("\n");

    assert!(
        warnings.contains("orphan.py:bp") && warnings.contains("never mounted"),
        "the unregistered blueprint should be reported, got:\n{warnings}"
    );
    assert!(
        result
            .endpoints
            .iter()
            .any(|e| e.path.unresolved_exprs() == vec!["settings.ADMIN_PREFIX"]),
        "the config-derived prefix should be an unresolved gap, not a guess"
    );
    // Static analysis can see that a route is registered in a loop, but not what it serves:
    // the registration is kept as an unresolved orphan and the reason is stated.
    assert!(
        !result
            .endpoints
            .iter()
            .any(|e| e.path.to_string().contains("widgets")),
        "loop-registered routes are a documented miss"
    );
    assert!(
        warnings.contains("dynamic.py:app`, which is not declared there"),
        "routes on a parameter should be reported, got:
{warnings}"
    );
    let class_routes: Vec<_> = result
        .endpoints
        .iter()
        .filter(|e| e.path.to_string().starts_with("/notes"))
        .map(|e| e.display())
        .collect();
    assert_eq!(
        class_routes,
        vec![
            "GET /notes",
            "DELETE /notes/{note_id}",
            "GET /notes/{note_id}",
            "PUT /notes/{note_id}"
        ],
        "the MethodView in views.py is linked to its rules in __init__.py, and `methods=` narrows /notes"
    );
}

#[test]
fn django_fixture_matches_snapshot() {
    check("django");
}

#[test]
fn go_fixture_matches_snapshot() {
    check("go");
}

/// The Gin fixture: routes registered through functions handed a group, handlers in the
/// file next door, and one of each thing that cannot be known statically.
#[test]
fn go_fixture_reports_the_expected_problems() {
    let root = fixtures_dir().join("go");
    let result = scan(&root).unwrap();
    let warnings = result.warnings.join("\n");

    assert!(
        warnings.contains("legacy.go:Register") && warnings.contains("never mounted"),
        "the Register nobody calls should be reported, got:\n{warnings}"
    );
    assert!(
        result
            .endpoints
            .iter()
            .any(|e| e.path.unresolved_exprs() == vec!["cfg.Prefix"]),
        "the configured prefix should be an unresolved gap, not a guess"
    );
    assert!(
        !result
            .endpoints
            .iter()
            .any(|e| e.path.to_string().contains("summary")),
        "a method held in a variable is a documented miss"
    );
    // orders.Register is handed two groups, so its routes appear under both.
    let orders: Vec<String> = result
        .endpoints
        .iter()
        .filter(|e| e.path.to_string().ends_with("/orders"))
        .map(|e| e.display())
        .collect();
    assert_eq!(
        orders,
        vec![
            "GET /api/v1/orders",
            "POST /api/v1/orders",
            "GET /legacy/orders",
            "POST /legacy/orders"
        ]
    );
    // The handler is in handler.go; the route in routes.go names it.
    let create = result
        .endpoints
        .iter()
        .find(|e| e.display() == "POST /api/v1/users")
        .expect("POST /users");
    let body = create.body.as_ref().expect("a body from CreateUser");
    assert_eq!(
        body.example,
        Some(serde_json::json!({
            "name": "string",
            "email": "string",
            "age": 0,
            "tags": ["string"]
        })),
        "the struct's json tags, minus the `json:\"-\"` field"
    );
    assert!(create.auth.is_some(), "AuthRequired() in the chain");
    let list = result
        .endpoints
        .iter()
        .find(|e| e.display() == "GET /api/v1/users")
        .expect("GET /users");
    let query: Vec<&str> = list.query_params.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(query, vec!["limit", "q"]);
    assert_eq!(result.base_urls[0].url, "http://localhost:8080");
}

#[test]
fn go_chi_fixture_matches_snapshot() {
    check("go-chi");
}

/// The chi fixture: a server type serving its router, resources returning routers,
/// `Route` closures, a mount that reaches another package through a constructor, and a
/// Go 1.22 mux behind `http.StripPrefix`.
#[test]
fn go_chi_fixture_reports_the_expected_problems() {
    let root = fixtures_dir().join("go-chi");
    let result = scan(&root).unwrap();
    let warnings = result.warnings.join("\n");

    assert!(
        warnings.contains("legacy.go:Mux") && warnings.contains("never mounted"),
        "the mux nothing serves should be reported, got:\n{warnings}"
    );
    assert!(
        !result
            .endpoints
            .iter()
            .any(|e| e.orphaned && !e.path.to_string().contains("/v0/")),
        "everything but the legacy mux is reached from main's ListenAndServe"
    );
    assert!(
        result
            .endpoints
            .iter()
            .any(|e| e.path.unresolved_exprs() == vec!["os.Getenv(\"ADMIN_PREFIX\")"]),
        "the environment prefix should be an unresolved gap, not a guess"
    );
    assert!(
        !result
            .endpoints
            .iter()
            .any(|e| e.path.to_string().contains("recent")),
        "table-driven registration is a documented miss"
    );
    // A method-less HandleFunc keeps only the methods its handler checks for.
    let export: Vec<String> = result
        .endpoints
        .iter()
        .filter(|e| e.path.to_string() == "/v2/export")
        .map(|e| e.display())
        .collect();
    assert_eq!(export, vec!["GET /v2/export", "POST /v2/export"]);
    let guarded = result
        .endpoints
        .iter()
        .find(|e| e.display() == "GET /notes/{id}")
        .expect("GET /notes/{id}");
    assert!(
        matches!(guarded.auth, Some(rl_model::AuthRequirement::Bearer { .. })),
        "jwtauth.Verifier in the Route closure's Use"
    );
    assert_eq!(result.base_urls[0].url, "http://localhost:3000");
}

#[test]
fn django_fixture_reports_the_expected_problems() {
    let root = fixtures_dir().join("django");
    let result = scan(&root).unwrap();
    let warnings = result.warnings.join("\n");

    assert!(
        warnings.contains("legacy/urls.py:urlpatterns") && warnings.contains("never mounted"),
        "the URL conf nothing includes should be reported, got:\n{warnings}"
    );
    assert!(
        result
            .endpoints
            .iter()
            .any(|e| e.path.unresolved_exprs() == vec!["settings.INTERNAL_PREFIX"]),
        "the settings-derived prefix should be an unresolved gap, not a guess"
    );
    assert!(
        !result
            .endpoints
            .iter()
            .any(|e| e.path.to_string().contains("reports/daily")),
        "list-comprehension patterns are a documented miss"
    );
    // The ViewSet in views.py is expanded at the router registration in urls.py.
    let users: Vec<String> = result
        .endpoints
        .iter()
        .filter(|e| e.path.to_string().starts_with("/api/v1/users"))
        .map(|e| e.display())
        .collect();
    assert_eq!(
        users,
        vec![
            "GET /api/v1/users/",
            "POST /api/v1/users/",
            "GET /api/v1/users/recent/",
            "DELETE /api/v1/users/{pk}/",
            "GET /api/v1/users/{pk}/",
            "POST /api/v1/users/{pk}/set-password/",
        ]
    );
}
