//! Project detection — what is here, and which files are worth looking at.

use crate::error::{DiscoveryError, Result};
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A pathological repository must not hang the scan. Hitting this is reported rather than
/// silently truncating the result.
pub const MAX_FILES: usize = 40_000;

/// Directories that are never source, and are frequently *not* in `.gitignore`.
const ALWAYS_SKIP: &[&str] = &[
    "node_modules",
    "__pycache__",
    ".venv",
    "venv",
    ".git",
    ".hg",
    ".svn",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".tox",
    "target",
    "vendor",
    "dist",
    "build",
    ".next",
    ".nuxt",
    ".svelte-kit",
    "coverage",
    ".routelogic",
    ".routelens",
];

/// Manifests worth reading in full, because framework detection consults them repeatedly.
const MANIFESTS: &[&str] = &[
    "pyproject.toml",
    "requirements.txt",
    "requirements-dev.txt",
    "setup.py",
    "setup.cfg",
    "Pipfile",
    "poetry.lock",
    "package.json",
    "go.mod",
    "next.config.js",
    "next.config.mjs",
    "next.config.ts",
    "manage.py",
    "Dockerfile",
    "docker-compose.yml",
    "docker-compose.yaml",
    "compose.yml",
    "Procfile",
    "Makefile",
    ".env",
    ".env.example",
    ".env.local",
];

/// A project-relative path as text, with forward slashes on every platform.
///
/// Warnings, evidence and snapshots all carry paths, and none of them should read
/// differently on Windows.
pub fn display(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    Python,
    JavaScript,
    TypeScript,
    Go,
}

impl Language {
    pub fn of(path: &Path) -> Option<Language> {
        match path.extension()?.to_str()? {
            "py" | "pyi" => Some(Language::Python),
            "js" | "jsx" | "mjs" | "cjs" => Some(Language::JavaScript),
            "ts" | "tsx" | "mts" | "cts" => Some(Language::TypeScript),
            "go" => Some(Language::Go),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Language::Python => "python",
            Language::JavaScript => "javascript",
            Language::TypeScript => "typescript",
            Language::Go => "go",
        }
    }
}

/// Everything an adapter needs to know about a project without touching the filesystem again.
///
/// Built once per scan. Manifests are read eagerly because framework detection consults them
/// repeatedly and they are small; source files are only listed, and read on demand.
#[derive(Debug, Clone)]
pub struct ProjectContext {
    root: PathBuf,
    /// Relative to `root`, so a snapshot test is stable across machines.
    files: Vec<PathBuf>,
    manifests: BTreeMap<String, String>,
    truncated: bool,
}

impl ProjectContext {
    pub fn scan(root: impl AsRef<Path>) -> Result<ProjectContext> {
        let root = root.as_ref();
        if !root.is_dir() {
            return Err(DiscoveryError::NotADirectory(root.to_path_buf()));
        }
        // Canonicalize so `strip_prefix` below always matches, whatever form the caller used.
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());

        let mut files = Vec::new();
        let mut manifests = BTreeMap::new();
        let mut truncated = false;

        let walker = WalkBuilder::new(&root)
            .hidden(false) // dotfiles like `.env` matter; ALWAYS_SKIP handles the noise
            .git_ignore(true)
            // Honour .gitignore even outside a git repository: a user may open an extracted
            // archive or a directory that simply has not been initialised yet, and the
            // ignore rules are still the project's own statement about what is not source.
            .require_git(false)
            .git_global(true)
            .git_exclude(true)
            .parents(true)
            .filter_entry(|entry| {
                let name = entry.file_name().to_string_lossy();
                !ALWAYS_SKIP.contains(&name.as_ref())
            })
            .build();

        for entry in walker {
            let entry = entry.map_err(|source| DiscoveryError::Walk { source })?;
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }

            if files.len() >= MAX_FILES {
                truncated = true;
                break;
            }

            let path = entry.path();
            let Ok(relative) = path.strip_prefix(&root) else {
                continue;
            };

            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if MANIFESTS.contains(&name) {
                    if let Ok(text) = std::fs::read_to_string(path) {
                        // Keyed by name, not path, so a nested manifest does not shadow the
                        // root one; the first (shallowest) wins.
                        manifests.entry(name.to_string()).or_insert(text);
                    }
                }
            }

            files.push(relative.to_path_buf());
        }

        files.sort();

        Ok(ProjectContext {
            root,
            files,
            manifests,
            truncated,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    /// The scan stopped at [`MAX_FILES`]; results are incomplete.
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    pub fn manifest(&self, name: &str) -> Option<&str> {
        self.manifests.get(name).map(String::as_str)
    }

    pub fn has_manifest(&self, name: &str) -> bool {
        self.manifests.contains_key(name)
    }

    pub fn files_of(&self, language: Language) -> impl Iterator<Item = &PathBuf> {
        self.files
            .iter()
            .filter(move |path| Language::of(path) == Some(language))
    }

    pub fn languages(&self) -> Vec<Language> {
        let mut found: Vec<Language> = self.files.iter().filter_map(|p| Language::of(p)).collect();
        found.sort();
        found.dedup();
        found
    }

    pub fn read(&self, relative: &Path) -> Result<String> {
        let full = self.root.join(relative);
        std::fs::read_to_string(&full)
            .map_err(|e| DiscoveryError::io(format!("reading {}", full.display()), e))
    }

    /// Whether a dependency is declared anywhere.
    ///
    /// Deliberately crude — a substring match across manifests. This is *weak* evidence by
    /// design: a package appearing in a lockfile says little, while an actual import in
    /// source says a lot. [`crate::framework`] weights them accordingly.
    pub fn declares_dependency(&self, name: &str) -> bool {
        self.manifests.values().any(|text| {
            text.lines().any(|line| {
                let line = line.trim();
                // Skip comment lines so a "# we removed fastapi" note is not evidence.
                if line.starts_with('#') {
                    return false;
                }
                line.to_ascii_lowercase()
                    .contains(&name.to_ascii_lowercase())
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn project(files: &[(&str, &str)]) -> TempDir {
        let dir = TempDir::new().unwrap();
        for (path, contents) in files {
            let full = dir.path().join(path);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(full, contents).unwrap();
        }
        dir
    }

    #[test]
    fn recognises_languages_by_extension() {
        assert_eq!(Language::of(Path::new("a/b.py")), Some(Language::Python));
        assert_eq!(
            Language::of(Path::new("a/b.tsx")),
            Some(Language::TypeScript)
        );
        assert_eq!(
            Language::of(Path::new("a/b.mjs")),
            Some(Language::JavaScript)
        );
        assert_eq!(Language::of(Path::new("README.md")), None);
    }

    #[test]
    fn lists_source_files_relative_to_the_root() {
        let dir = project(&[("app/main.py", ""), ("app/api/users.py", "")]);
        let ctx = ProjectContext::scan(dir.path()).unwrap();

        let names: Vec<String> = ctx
            .files()
            .iter()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect();
        assert!(names.contains(&"app/main.py".to_string()));
        assert!(names.contains(&"app/api/users.py".to_string()));
    }

    #[test]
    fn skips_directories_that_are_never_source() {
        let dir = project(&[
            ("app/main.py", ""),
            ("node_modules/pkg/index.js", ""),
            (".venv/lib/thing.py", ""),
            ("__pycache__/main.cpython-311.pyc", ""),
        ]);
        let ctx = ProjectContext::scan(dir.path()).unwrap();

        let joined = ctx
            .files()
            .iter()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(joined.contains("app/main.py"));
        assert!(!joined.contains("node_modules"));
        assert!(!joined.contains(".venv"));
        assert!(!joined.contains("__pycache__"));
    }

    #[test]
    fn honours_gitignore() {
        let dir = project(&[
            ("app/main.py", ""),
            ("app/generated.py", ""),
            (".gitignore", "generated.py\n"),
        ]);
        let ctx = ProjectContext::scan(dir.path()).unwrap();

        let joined = ctx
            .files()
            .iter()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(joined.contains("app/main.py"));
        assert!(!joined.contains("generated.py"));
    }

    #[test]
    fn reads_manifests_eagerly_including_dotfiles() {
        let dir = project(&[
            (
                "pyproject.toml",
                "[project]\ndependencies = [\"fastapi\"]\n",
            ),
            (".env", "PORT=8000\n"),
        ]);
        let ctx = ProjectContext::scan(dir.path()).unwrap();

        assert!(ctx.has_manifest("pyproject.toml"));
        assert!(ctx.manifest(".env").unwrap().contains("PORT=8000"));
    }

    #[test]
    fn dependency_detection_ignores_commented_out_lines() {
        let dir = project(&[("requirements.txt", "# fastapi was removed\nflask==3.0\n")]);
        let ctx = ProjectContext::scan(dir.path()).unwrap();

        assert!(ctx.declares_dependency("flask"));
        assert!(
            !ctx.declares_dependency("fastapi"),
            "a comment is not a declaration"
        );
    }

    #[test]
    fn languages_are_reported_from_what_is_actually_present() {
        let dir = project(&[("a.py", ""), ("b.ts", ""), ("c.md", "")]);
        let ctx = ProjectContext::scan(dir.path()).unwrap();
        assert_eq!(
            ctx.languages(),
            vec![Language::Python, Language::TypeScript]
        );
    }

    #[test]
    fn a_file_path_is_refused_rather_than_scanned() {
        let dir = project(&[("a.py", "")]);
        assert!(matches!(
            ProjectContext::scan(dir.path().join("a.py")),
            Err(DiscoveryError::NotADirectory(_))
        ));
    }

    #[test]
    fn files_can_be_read_back_by_relative_path() {
        let dir = project(&[("app/main.py", "print('hi')")]);
        let ctx = ProjectContext::scan(dir.path()).unwrap();
        assert_eq!(ctx.read(Path::new("app/main.py")).unwrap(), "print('hi')");
    }
}
