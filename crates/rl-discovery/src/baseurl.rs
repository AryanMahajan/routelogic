//! Where `http://localhost:8000` comes from.
//!
//! Discovered routes have no host, and "zero configuration where possible" has to survive
//! that. The project usually already states the answer somewhere — a run script, a compose
//! file, an `.env` — so this reads it rather than asking.
//!
//! Candidates are offered ranked. Nothing is chosen silently.

use crate::project::ProjectContext;
use serde::{Deserialize, Serialize};

/// A possible base URL, and why it was suggested.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaseUrlCandidate {
    pub url: String,
    /// Shown in the UI so the choice is explicable rather than magic.
    pub source: String,
    /// Higher is a better guess.
    pub confidence: u8,
}

/// What a framework serves on when nothing says otherwise.
fn default_port(framework: &str) -> Option<(u16, &'static str)> {
    match framework {
        "fastapi" => Some((8000, "uvicorn default")),
        "flask" => Some((5000, "Flask default")),
        "django" => Some((8000, "Django default")),
        "nextjs" => Some((3000, "Next.js default")),
        "express" => Some((3000, "the usual Express choice")),
        "go" => Some((8080, "the usual Go choice")),
        _ => None,
    }
}

/// Collect base URL candidates, best first.
///
/// `frameworks` are the detected framework ids, best first, and decide which default port
/// is offered when the project says nothing about how it runs.
pub fn infer(project: &ProjectContext, frameworks: &[&str]) -> Vec<BaseUrlCandidate> {
    infer_with_listen_ports(project, frameworks, &[])
}

/// [`infer`], plus ports the scan read out of the source itself — a Go
/// `http.ListenAndServe(":8080", …)` — each with where it was seen.
pub fn infer_with_listen_ports(
    project: &ProjectContext,
    frameworks: &[&str],
    listen_ports: &[(u16, String)],
) -> Vec<BaseUrlCandidate> {
    let mut found: Vec<BaseUrlCandidate> = Vec::new();

    let mut add = |port: u16, source: String, confidence: u8| {
        let url = format!("http://localhost:{port}");
        if !found.iter().any(|c| c.url == url) {
            found.push(BaseUrlCandidate {
                url,
                source,
                confidence,
            });
        }
    };

    // A run command states the port the developer actually uses — the strongest signal.
    for (name, text) in [
        ("Procfile", project.manifest("Procfile")),
        ("Makefile", project.manifest("Makefile")),
        ("docker-compose.yml", project.manifest("docker-compose.yml")),
        ("Dockerfile", project.manifest("Dockerfile")),
        ("package.json", project.manifest("package.json")),
    ] {
        let Some(text) = text else { continue };
        for port in ports_from_run_commands(text) {
            add(port, format!("run command in {name}"), 9);
        }
    }

    // The address the program binds, stated in code. As good as a run command.
    for (port, seen_at) in listen_ports {
        add(*port, seen_at.clone(), 8);
    }

    // Published container ports: `"8000:8000"` — the left side is what the host sees.
    for name in ["docker-compose.yml", "docker-compose.yaml", "compose.yml"] {
        let Some(text) = project.manifest(name) else {
            continue;
        };
        for port in published_ports(text) {
            add(port, format!("published port in {name}"), 7);
        }
    }

    for name in [".env", ".env.local", ".env.example"] {
        let Some(text) = project.manifest(name) else {
            continue;
        };
        for port in env_ports(text) {
            add(port, format!("PORT in {name}"), 6);
        }
    }

    if let Some(text) = project.manifest("Dockerfile") {
        for port in exposed_ports(text) {
            add(port, "EXPOSE in Dockerfile".to_string(), 4);
        }
    }

    // Always offer a default, so there is something to click even in a project that says
    // nothing about how it runs.
    let mut offered_default = false;
    for framework in frameworks {
        if let Some((port, why)) = default_port(framework) {
            add(port, why.to_string(), 1);
            offered_default = true;
        }
    }
    if !offered_default {
        add(8000, "a common default".to_string(), 1);
    }

    found.sort_by_key(|c| std::cmp::Reverse(c.confidence));
    found
}

/// Ports named by `--port N`, `-p N`, `PORT=N`, or `host:port` in a serving command.
fn ports_from_run_commands(text: &str) -> Vec<u16> {
    let mut ports = Vec::new();

    for line in text.lines() {
        let lowered = line.to_ascii_lowercase();
        let is_run_command = [
            "uvicorn",
            "gunicorn",
            "hypercorn",
            "flask run",
            "flask --app",
            "runserver",
            "daphne",
            "next dev",
            "next start",
            "node ",
            "nodemon",
            "ts-node",
            "tsx ",
        ]
        .iter()
        .any(|needle| lowered.contains(needle));
        if !is_run_command {
            continue;
        }

        // Quotes are stripped because a package.json script is one JSON string, and its
        // first token arrives as `"PORT=4000`.
        let tokens: Vec<&str> = line
            .split_whitespace()
            .map(|t| t.trim_matches(['"', '\'']))
            .collect();
        for (index, token) in tokens.iter().enumerate() {
            // `PORT=4000 node src/server.js`
            if let Some(value) = token.strip_prefix("PORT=") {
                if let Ok(port) = value.trim_matches(|c: char| !c.is_ascii_digit()).parse() {
                    ports.push(port);
                }
            }

            // `--port 8080` and `--port=8080`
            if let Some(value) = token.strip_prefix("--port=").or_else(|| {
                (*token == "--port" || *token == "-p")
                    .then(|| tokens.get(index + 1).copied())
                    .flatten()
            }) {
                if let Ok(port) = value.trim_matches(|c: char| !c.is_ascii_digit()).parse() {
                    ports.push(port);
                }
            }

            // `manage.py runserver 0.0.0.0:8001` and `runserver 8001`.
            if index > 0 && tokens[index - 1] == "runserver" {
                let port = token.rsplit_once(':').map_or(*token, |(_, p)| p);
                if let Ok(port) = port.parse() {
                    ports.push(port);
                }
            }

            // `--bind 0.0.0.0:8080`, gunicorn's form.
            if let Some(value) = token.strip_prefix("--bind=").or_else(|| {
                (*token == "--bind" || *token == "-b")
                    .then(|| tokens.get(index + 1).copied())
                    .flatten()
            }) {
                if let Some((_, port)) = value.rsplit_once(':') {
                    if let Ok(port) = port.trim_matches('"').parse() {
                        ports.push(port);
                    }
                }
            }
        }
    }

    ports
}

/// `"8000:8000"` in a compose file's `ports:` list.
fn published_ports(text: &str) -> Vec<u16> {
    text.lines()
        .map(str::trim)
        .filter(|line| line.starts_with('-'))
        .filter_map(|line| {
            let value = line
                .trim_start_matches('-')
                .trim()
                .trim_matches(['"', '\'']);
            let host_side = value.split(':').next()?;
            host_side.parse().ok()
        })
        .collect()
}

fn env_ports(text: &str) -> Vec<u16> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            let key = key.trim().to_ascii_uppercase();
            if key == "PORT" || key.ends_with("_PORT") {
                value.trim().trim_matches(['"', '\'']).parse().ok()
            } else {
                None
            }
        })
        .collect()
}

fn exposed_ports(text: &str) -> Vec<u16> {
    text.lines()
        .map(str::trim)
        .filter(|line| line.to_ascii_uppercase().starts_with("EXPOSE"))
        .flat_map(|line| {
            line.split_whitespace()
                .skip(1)
                .filter_map(|token| token.split('/').next()?.parse().ok())
                .collect::<Vec<u16>>()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn project(files: &[(&str, &str)]) -> (TempDir, ProjectContext) {
        let dir = TempDir::new().unwrap();
        for (path, contents) in files {
            fs::write(dir.path().join(path), contents).unwrap();
        }
        let ctx = ProjectContext::scan(dir.path()).unwrap();
        (dir, ctx)
    }

    fn urls(candidates: &[BaseUrlCandidate]) -> Vec<&str> {
        candidates.iter().map(|c| c.url.as_str()).collect()
    }

    #[test]
    fn a_uvicorn_port_flag_is_the_strongest_signal() {
        let (_dir, project) = project(&[(
            "Procfile",
            "web: uvicorn app.main:app --host 0.0.0.0 --port 9001\n",
        )]);
        let candidates = infer(&project, &["fastapi"]);

        assert_eq!(candidates[0].url, "http://localhost:9001");
        assert!(candidates[0].source.contains("Procfile"));
    }

    #[test]
    fn handles_the_equals_form_and_gunicorn_bind() {
        let (_dir, equals) = project(&[("Makefile", "run:\n\tuvicorn main:app --port=7000\n")]);
        assert_eq!(infer(&equals, &["fastapi"])[0].url, "http://localhost:7000");

        let (_dir, bind) = project(&[("Procfile", "web: gunicorn app:app --bind 0.0.0.0:5050\n")]);
        assert_eq!(infer(&bind, &["fastapi"])[0].url, "http://localhost:5050");
    }

    #[test]
    fn published_compose_ports_are_offered() {
        let (_dir, project) = project(&[(
            "docker-compose.yml",
            "services:\n  api:\n    ports:\n      - \"8080:8000\"\n",
        )]);
        assert!(urls(&infer(&project, &["fastapi"])).contains(&"http://localhost:8080"));
    }

    #[test]
    fn env_port_is_read() {
        let (_dir, project) = project(&[(".env", "# comment\nPORT=3333\nOTHER=x\n")]);
        assert!(urls(&infer(&project, &["fastapi"])).contains(&"http://localhost:3333"));
    }

    #[test]
    fn a_commented_out_port_is_not_used() {
        let (_dir, project) = project(&[(".env", "#PORT=9999\n")]);
        assert!(!urls(&infer(&project, &["fastapi"])).contains(&"http://localhost:9999"));
    }

    #[test]
    fn dockerfile_expose_is_a_weak_candidate() {
        let (_dir, project) = project(&[("Dockerfile", "FROM python\nEXPOSE 8000/tcp\n")]);
        let candidates = infer(&project, &["fastapi"]);
        let exposed = candidates
            .iter()
            .find(|c| c.source.contains("EXPOSE"))
            .unwrap();
        assert_eq!(exposed.url, "http://localhost:8000");
    }

    #[test]
    fn there_is_always_a_default_to_click() {
        let (_dir, project) = project(&[("README.md", "nothing useful here")]);
        let candidates = infer(&project, &[]);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].url, "http://localhost:8000");
    }

    #[test]
    fn the_default_follows_the_detected_framework() {
        let (_dir, project) = project(&[("README.md", "")]);
        assert_eq!(
            urls(&infer(&project, &["nextjs"])),
            vec!["http://localhost:3000"]
        );
        assert_eq!(
            urls(&infer(&project, &["fastapi"])),
            vec!["http://localhost:8000"]
        );
    }

    #[test]
    fn next_and_node_run_scripts_are_read() {
        let (_dir, project) = project(&[(
            "package.json",
            "{ \"scripts\": { \"dev\": \"next dev -p 3100\", \"start\": \"PORT=4000 node server.js\" } }",
        )]);
        let candidates = infer(&project, &["nextjs"]);
        let urls = urls(&candidates);
        assert!(urls.contains(&"http://localhost:3100"));
        assert!(urls.contains(&"http://localhost:4000"));
    }

    #[test]
    fn candidates_are_ranked_and_not_duplicated() {
        let (_dir, project) = project(&[
            ("Procfile", "web: uvicorn main:app --port 8000\n"),
            ("Dockerfile", "EXPOSE 8000\n"),
        ]);
        let candidates = infer(&project, &["fastapi"]);

        assert_eq!(
            candidates
                .iter()
                .filter(|c| c.url.ends_with(":8000"))
                .count(),
            1,
            "the same port from two sources should appear once"
        );
        assert!(candidates[0].confidence >= candidates[candidates.len() - 1].confidence);
    }
}
