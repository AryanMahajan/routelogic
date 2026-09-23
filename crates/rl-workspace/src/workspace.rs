//! Opening, creating, and reading a workspace.

use crate::collection::Collection;
use crate::environment::Environment;
use crate::error::{Result, WorkspaceError};
use crate::layout::{self, Layout};
use crate::secrets::{FileSecretStore, SecretStore};
use rl_model::{Flow, VariableContext};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const CURRENT_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceKind {
    /// Attached to a source repository; discovery applies.
    #[default]
    Project,
    /// A general-purpose API client workspace, with no project attached.
    Standalone,
}

/// What is known about the attached project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectRef {
    /// Relative to the workspace root, so the file stays valid on another machine.
    #[serde(default = "dot")]
    pub root: PathBuf,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frameworks: Vec<String>,
    /// `module:attribute` for runtime enrich, once the user has confirmed it.
    ///
    /// Stored so the consent decision and its target are recorded together and can be
    /// revoked by editing one file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_target: Option<String>,
}

fn dot() -> PathBuf {
    PathBuf::from(".")
}

// Hand-written rather than derived: `PathBuf::default()` is the *empty* path, which would
// write `root: ''` into workspace.yaml and read as "nowhere" rather than "here".
impl Default for ProjectRef {
    fn default() -> Self {
        ProjectRef {
            root: dot(),
            frameworks: Vec::new(),
            app_target: None,
        }
    }
}

/// `workspace.yaml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceManifest {
    #[serde(default = "default_version")]
    pub version: u32,
    pub name: String,
    #[serde(default)]
    pub kind: WorkspaceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<ProjectRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_environment: Option<String>,
    /// Workspace globals — variables shared across every environment.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub variables: BTreeMap<String, String>,
}

fn default_version() -> u32 {
    CURRENT_VERSION
}

impl WorkspaceManifest {
    pub fn new(name: impl Into<String>, kind: WorkspaceKind) -> Self {
        WorkspaceManifest {
            version: CURRENT_VERSION,
            name: name.into(),
            kind,
            project: match kind {
                WorkspaceKind::Project => Some(ProjectRef::default()),
                WorkspaceKind::Standalone => None,
            },
            default_environment: None,
            variables: BTreeMap::new(),
        }
    }
}

/// An open workspace.
#[derive(Debug, Clone)]
pub struct Workspace {
    layout: Layout,
    manifest: WorkspaceManifest,
}

impl Workspace {
    /// Create a workspace, writing the directory skeleton and the `.gitignore`.
    ///
    /// The `.gitignore` is written here — at creation — rather than when someone first
    /// notices a credential in a diff. That ordering is the whole point.
    pub fn create(
        root: impl AsRef<Path>,
        name: impl Into<String>,
        kind: WorkspaceKind,
    ) -> Result<Self> {
        Workspace::create_in(Layout::new(root.as_ref()), name, kind)
    }

    /// [`Workspace::create`] with an explicit layout — a test's temporary data directory.
    pub fn create_in(layout: Layout, name: impl Into<String>, kind: WorkspaceKind) -> Result<Self> {
        if layout.exists() {
            return Err(WorkspaceError::AlreadyExists(layout.dir()));
        }

        for dir in [
            layout.dir(),
            layout.collections_dir(),
            layout.environments_dir(),
            layout.local_dir(),
        ] {
            std::fs::create_dir_all(&dir)
                .map_err(|e| WorkspaceError::io(format!("creating {}", dir.display()), e))?;
        }

        write_file(&layout.gitignore(), layout::GITIGNORE_CONTENTS)?;
        ensure_project_gitignore(&layout)?;

        let workspace = Workspace {
            manifest: WorkspaceManifest::new(name, kind),
            layout,
        };
        workspace.save()?;
        Ok(workspace)
    }

    /// Open an existing workspace.
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        Workspace::open_in(Layout::new(root.as_ref()))
    }

    /// [`Workspace::open`] with an explicit layout.
    pub fn open_in(layout: Layout) -> Result<Self> {
        migrate_renamed_dirs(&layout)?;
        let path = layout.manifest();
        if !path.is_file() {
            return Err(WorkspaceError::NotFound(layout.dir()));
        }

        let manifest: WorkspaceManifest = read_yaml(&path)?;
        if manifest.version > CURRENT_VERSION {
            return Err(WorkspaceError::UnsupportedVersion {
                found: manifest.version,
                supported: CURRENT_VERSION,
            });
        }

        // Self-heal: a workspace cloned before the ignore rule existed, or one whose
        // .gitignore was deleted, must not start writing secrets into a tracked tree.
        let gitignore = layout.gitignore();
        if !gitignore.is_file() {
            write_file(&gitignore, layout::GITIGNORE_CONTENTS)?;
        }
        ensure_project_gitignore(&layout)?;

        migrate_collections(&layout)?;
        migrate_history(&layout)?;

        Ok(Workspace { layout, manifest })
    }

    pub fn open_or_create(
        root: impl AsRef<Path>,
        name: impl Into<String>,
        kind: WorkspaceKind,
    ) -> Result<Self> {
        Workspace::open_or_create_in(Layout::new(root.as_ref()), name, kind)
    }

    pub fn open_or_create_in(
        layout: Layout,
        name: impl Into<String>,
        kind: WorkspaceKind,
    ) -> Result<Self> {
        if layout.exists() {
            Workspace::open_in(layout)
        } else {
            Workspace::create_in(layout, name, kind)
        }
    }

    pub fn exists(root: impl AsRef<Path>) -> bool {
        Layout::new(root.as_ref()).exists()
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    pub fn manifest(&self) -> &WorkspaceManifest {
        &self.manifest
    }

    pub fn manifest_mut(&mut self) -> &mut WorkspaceManifest {
        &mut self.manifest
    }

    pub fn save(&self) -> Result<()> {
        write_yaml(&self.layout.manifest(), &self.manifest, "workspace.yaml")
    }

    // --- collections --------------------------------------------------------------------

    pub fn collection_names(&self) -> Result<Vec<String>> {
        list_names(&self.layout.collections_dir())
    }

    pub fn load_collection(&self, name: &str) -> Result<Collection> {
        let path = self.layout.collection_file(name)?;
        if !path.is_file() {
            return Err(WorkspaceError::NoSuchCollection(name.to_string()));
        }
        read_yaml(&path)
    }

    pub fn save_collection(&self, collection: &Collection) -> Result<()> {
        let path = self.layout.collection_file(&collection.name)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| WorkspaceError::io(format!("creating {}", dir.display()), e))?;
        }
        write_yaml(&path, collection, &collection.name)
    }

    pub fn delete_collection(&self, name: &str) -> Result<()> {
        let path = self.layout.collection_file(name)?;
        if !path.is_file() {
            return Err(WorkspaceError::NoSuchCollection(name.to_string()));
        }
        std::fs::remove_file(&path)
            .map_err(|e| WorkspaceError::io(format!("removing {}", path.display()), e))
    }

    // --- environments -------------------------------------------------------------------

    pub fn environment_names(&self) -> Result<Vec<String>> {
        list_names(&self.layout.environments_dir())
    }

    pub fn load_environment(&self, name: &str) -> Result<Environment> {
        let path = self.layout.environment_file(name)?;
        if !path.is_file() {
            return Err(WorkspaceError::NoSuchEnvironment(name.to_string()));
        }
        read_yaml(&path)
    }

    pub fn save_environment(&self, environment: &Environment) -> Result<()> {
        let path = self.layout.environment_file(&environment.name)?;
        write_yaml(&path, environment, &environment.name)
    }

    pub fn delete_environment(&self, name: &str) -> Result<()> {
        let path = self.layout.environment_file(name)?;
        if !path.is_file() {
            return Err(WorkspaceError::NoSuchEnvironment(name.to_string()));
        }
        std::fs::remove_file(&path)
            .map_err(|e| WorkspaceError::io(format!("deleting {}", path.display()), e))
    }

    // --- flows --------------------------------------------------------------------------

    pub fn flow_names(&self) -> Result<Vec<String>> {
        list_names(&self.layout.flows_dir())
    }

    pub fn load_flow(&self, name: &str) -> Result<Flow> {
        let path = self.layout.flow_file(name)?;
        if !path.is_file() {
            return Err(WorkspaceError::NoSuchFlow(name.to_string()));
        }
        read_yaml(&path)
    }

    /// Write a flow as it is. Nothing is validated here — a half-built graph with a cycle
    /// is still worth saving; it is refused when *run*, with the cycle named.
    pub fn save_flow(&self, flow: &Flow) -> Result<()> {
        let path = self.layout.flow_file(&flow.name)?;
        write_yaml(&path, flow, &flow.name)
    }

    pub fn delete_flow(&self, name: &str) -> Result<()> {
        let path = self.layout.flow_file(name)?;
        if !path.is_file() {
            return Err(WorkspaceError::NoSuchFlow(name.to_string()));
        }
        std::fs::remove_file(&path)
            .map_err(|e| WorkspaceError::io(format!("deleting {}", path.display()), e))
    }

    // --- secrets ------------------------------------------------------------------------

    pub fn secrets(&self) -> FileSecretStore {
        FileSecretStore::new(self.layout.secrets_file())
    }

    // --- history ------------------------------------------------------------------------

    /// Open the request history — per user, shared by every project — creating the
    /// database if needed.
    pub fn history(&self) -> Result<crate::history::History> {
        crate::history::History::open(self.layout.history_db())
    }

    // --- putting it together ------------------------------------------------------------

    /// Assemble the variables a request will resolve against.
    ///
    /// This is where the three tiers meet: globals from the manifest, variables from the
    /// named environment, and values from the private secret store. The result feeds
    /// [`rl_model::RequestDraft::resolve`], so what the UI previews is what gets sent.
    pub fn variable_context(&self, environment: Option<&str>) -> Result<VariableContext> {
        let name = environment
            .map(str::to_string)
            .or_else(|| self.manifest.default_environment.clone());

        let mut ctx = VariableContext::new();
        ctx.globals = self.manifest.variables.clone();

        if let Some(name) = name {
            // A default environment naming a file that no longer exists should not stop the
            // workspace from opening — the user can still send requests with globals alone.
            if let Ok(env) = self.load_environment(&name) {
                ctx.environment = env.variables;
            }
        }

        ctx.secrets = self.secrets().load_all()?;
        Ok(ctx)
    }
}

// --- file helpers -----------------------------------------------------------------------

fn read_yaml<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| WorkspaceError::io(format!("reading {}", path.display()), e))?;

    yaml_serde::from_str(&text).map_err(|source| WorkspaceError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

fn write_yaml<T: Serialize>(path: &Path, value: &T, what: &str) -> Result<()> {
    let text = yaml_serde::to_string(value).map_err(|source| WorkspaceError::Encode {
        what: what.to_string(),
        source,
    })?;
    write_file(path, &text)
}

/// Make sure the project's own `.gitignore` excludes `.routelogic/`.
///
/// Nothing else in the file is touched: an existing `.gitignore` gets one line appended
/// (after a newline if the file does not already end with one), and a missing one is
/// created with just that line. Runs on every open so a rule someone removed, or a
/// project whose `.gitignore` arrived later, is covered again.
fn ensure_project_gitignore(layout: &Layout) -> Result<()> {
    let path = layout.project_gitignore();
    let existing = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(WorkspaceError::io(format!("reading {}", path.display()), e)),
    };

    if ignores_routelogic(&existing) {
        return Ok(());
    }

    let mut text = existing;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(layout::PROJECT_IGNORE_RULE);
    text.push('\n');
    write_file(&path, &text)
}

/// Whether a `.gitignore` already has a rule for the `.routelogic` directory — in any of the
/// spellings git accepts for it, so a hand-written `/.routelogic/` is not duplicated.
fn ignores_routelogic(gitignore: &str) -> bool {
    gitignore.lines().any(|line| {
        let rule = line.trim();
        if rule.starts_with('#') {
            return false;
        }
        let rule = rule.strip_prefix('/').unwrap_or(rule);
        let rule = rule.strip_suffix('/').unwrap_or(rule);
        rule == layout::PROJECT_IGNORE_RULE
    })
}

/// Write a file whole or not at all: into a hidden sibling, then renamed over the target.
///
/// Two processes share a workspace — the app, and an agent's MCP server — and the app
/// watches the directory. A reader must never see half a file, and a crash mid-write must
/// not leave one behind. The sibling starts with a dot and ends in `.tmp`, so neither the
/// flow list nor the watcher mistakes it for a document.
fn write_file(path: &Path, contents: &str) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|e| WorkspaceError::io(format!("creating {}", parent.display()), e))?;

    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temporary = parent.join(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&temporary, contents)
        .map_err(|e| WorkspaceError::io(format!("writing {}", temporary.display()), e))?;
    std::fs::rename(&temporary, path).map_err(|e| {
        let _ = std::fs::remove_file(&temporary);
        WorkspaceError::io(format!("writing {}", path.display()), e)
    })
}

/// The display names of every YAML file in a directory, sorted.
fn list_names(dir: &Path) -> Result<Vec<String>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(WorkspaceError::io(format!("reading {}", dir.display()), e)),
    };

    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| layout::name_from_file(&entry.path()))
        .collect();

    names.sort();
    Ok(names)
}

/// Carry a workspace and a data directory written under the project's old name over to the
/// new one: `.routelens/` becomes `.routelogic/`, and `<data dir>/routelens` becomes
/// `<data dir>/routelogic`. Each only when nothing exists under the new name yet — two
/// directories cannot be merged blindly, and the newer one is the one in use.
fn migrate_renamed_dirs(layout: &Layout) -> Result<()> {
    let mut moves = vec![(layout.legacy_dir(), layout.dir())];
    if let Some(legacy) = layout.legacy_data_dir() {
        moves.push((legacy, layout.data_dir().to_path_buf()));
    }
    for (from, to) in moves {
        if !from.is_dir() || to.exists() {
            continue;
        }
        std::fs::rename(&from, &to).map_err(|e| {
            WorkspaceError::io(format!("moving {} to {}", from.display(), to.display()), e)
        })?;
    }
    Ok(())
}

/// Move a history database left under the project's `.routelogic/local/` — where it lived
/// before history became per-user — into the user's data directory.
///
/// Only when there is no per-user database yet: two SQLite files cannot simply be merged,
/// and the newer one is the one being written to. A project database that stays behind
/// is in `local/`, which is ignored, and harmless.
fn migrate_history(layout: &Layout) -> Result<()> {
    let legacy = layout.legacy_history_db();
    let current = layout.history_db();
    if !legacy.is_file() || current.exists() {
        return Ok(());
    }
    if let Some(parent) = current.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| WorkspaceError::io(format!("creating {}", parent.display()), e))?;
    }
    // A rename across drives fails; copy-then-remove covers both.
    match std::fs::rename(&legacy, &current) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(&legacy, &current)
                .map_err(|e| WorkspaceError::io(format!("copying {}", legacy.display()), e))?;
            let _ = std::fs::remove_file(&legacy);
            Ok(())
        }
    }
}

/// Move collections saved under the project's `.routelogic/collections/` — where they lived
/// before becoming per-user — into the user's data directory.
///
/// A file whose name already exists there is left where it is rather than overwritten,
/// and reported through the returned names so the caller can say so.
fn migrate_collections(layout: &Layout) -> Result<Vec<String>> {
    let legacy = layout.legacy_collections_dir();
    if !legacy.is_dir() {
        return Ok(Vec::new());
    }
    let entries = std::fs::read_dir(&legacy)
        .map_err(|e| WorkspaceError::io(format!("reading {}", legacy.display()), e))?;

    let mut moved = Vec::new();
    let mut kept = false;
    for entry in entries.flatten() {
        let from = entry.path();
        let Some(name) = layout::name_from_file(&from) else {
            kept = true;
            continue;
        };
        let to = layout.collection_file(&name)?;
        if to.exists() {
            kept = true;
            continue;
        }
        if let Some(dir) = to.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| WorkspaceError::io(format!("creating {}", dir.display()), e))?;
        }
        std::fs::rename(&from, &to)
            .or_else(|_| std::fs::copy(&from, &to).and_then(|_| std::fs::remove_file(&from)))
            .map_err(|e| WorkspaceError::io(format!("moving {}", from.display()), e))?;
        moved.push(name);
    }
    if !kept {
        let _ = std::fs::remove_dir(&legacy);
    }
    Ok(moved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::SecretStore;
    use rl_model::{AuthConfig, HttpMethod, RequestDraft};
    use tempfile::TempDir;

    /// A workspace whose per-user data directory is inside the temp dir, so tests never
    /// see each other's collections — or the developer's.
    fn isolated(dir: &Path) -> Layout {
        Layout::with_data_dir(dir, dir.join("_data"))
    }

    fn workspace() -> (TempDir, Workspace) {
        let dir = TempDir::new().unwrap();
        let ws = Workspace::create_in(isolated(dir.path()), "myproject", WorkspaceKind::Project)
            .unwrap();
        (dir, ws)
    }

    #[test]
    fn collections_are_shared_across_projects_and_migrated_from_older_workspaces() {
        let dir = TempDir::new().unwrap();
        let data = dir.path().join("_data");
        let a = Workspace::create_in(
            Layout::with_data_dir(dir.path().join("a"), &data),
            "a",
            WorkspaceKind::Project,
        )
        .unwrap();
        let b = Workspace::create_in(
            Layout::with_data_dir(dir.path().join("b"), &data),
            "b",
            WorkspaceKind::Project,
        )
        .unwrap();

        a.save_collection(&Collection::new("Shared")).unwrap();
        assert_eq!(
            b.collection_names().unwrap(),
            vec!["Shared"],
            "same user, same list"
        );
        assert!(
            !a.layout().legacy_collections_dir().exists(),
            "nothing is written under the project"
        );

        // A workspace from before collections were per-user carries them along on open.
        let old = dir.path().join("old");
        std::fs::create_dir_all(old.join(".routelogic/collections")).unwrap();
        std::fs::write(
            old.join(".routelogic/workspace.yaml"),
            "version: 1\nname: old\n",
        )
        .unwrap();
        std::fs::write(
            old.join(".routelogic/collections/Legacy.yaml"),
            "version: 1\nname: Legacy\nrequests: []\n",
        )
        .unwrap();
        let reopened = Workspace::open_in(Layout::with_data_dir(&old, &data)).unwrap();
        assert_eq!(
            reopened.collection_names().unwrap(),
            vec!["Legacy", "Shared"]
        );
        assert!(
            !old.join(".routelogic/collections").exists(),
            "moved, not copied"
        );
    }

    /// History follows the user: what was sent from one project is listed in another, and
    /// a database left behind by an older RouteLogic is picked up on open.
    #[test]
    fn history_is_shared_across_projects_and_migrated_from_older_workspaces() {
        use crate::history::{History, NewEntry};
        use rl_model::VariableContext;

        let home = TempDir::new().unwrap();
        let a = TempDir::new().unwrap();
        let b = TempDir::new().unwrap();
        let layout_a = Layout::with_data_dir(a.path(), home.path());
        let layout_b = Layout::with_data_dir(b.path(), home.path());

        let ws_a = Workspace::create_in(layout_a, "a", WorkspaceKind::Project).unwrap();
        ws_a.history()
            .unwrap()
            .record(&NewEntry::new("GET", "http://a.test/").redacted(&VariableContext::new()))
            .unwrap();

        let ws_b = Workspace::create_in(layout_b, "b", WorkspaceKind::Project).unwrap();
        let seen = ws_b.history().unwrap().recent(10).unwrap();
        assert_eq!(seen.len(), 1, "project b sees what project a sent");
        assert_eq!(seen[0].url, "http://a.test/");

        // An older workspace with history under local/, and no per-user database yet.
        let old_home = TempDir::new().unwrap();
        let c = TempDir::new().unwrap();
        let layout_c = Layout::with_data_dir(c.path(), old_home.path());
        Workspace::create_in(layout_c.clone(), "c", WorkspaceKind::Project).unwrap();
        History::open(layout_c.legacy_history_db())
            .unwrap()
            .record(&NewEntry::new("POST", "http://old.test/").redacted(&VariableContext::new()))
            .unwrap();
        assert!(!layout_c.history_db().exists());

        let reopened = Workspace::open_in(layout_c.clone()).unwrap();
        assert!(
            layout_c.history_db().is_file(),
            "moved to the user's data directory"
        );
        assert!(!layout_c.legacy_history_db().exists());
        assert_eq!(
            reopened.history().unwrap().recent(10).unwrap()[0].url,
            "http://old.test/"
        );
    }

    #[test]
    fn creation_writes_the_skeleton() {
        let (_dir, ws) = workspace();
        let l = ws.layout();
        assert!(l.manifest().is_file());
        assert!(l.collections_dir().is_dir());
        assert!(l.environments_dir().is_dir());
        assert!(l.local_dir().is_dir());
    }

    #[test]
    fn creation_writes_the_gitignore_before_anything_can_land_in_local() {
        let (_dir, ws) = workspace();
        let text = std::fs::read_to_string(ws.layout().gitignore()).unwrap();
        assert!(text.lines().any(|l| l.trim() == "local/"));
    }

    #[test]
    fn opening_restores_a_deleted_gitignore() {
        let (dir, ws) = workspace();
        std::fs::remove_file(ws.layout().gitignore()).unwrap();

        let reopened = Workspace::open_in(isolated(dir.path())).unwrap();
        assert!(
            reopened.layout().gitignore().is_file(),
            "a workspace must never start writing secrets into a tracked tree"
        );
    }

    /// A project and a data directory written before the rename are picked up under the
    /// new names, and nothing is left behind under the old ones.
    #[test]
    fn a_workspace_written_under_the_old_name_is_moved_to_the_new_one_on_open() {
        let home = tempfile::tempdir().unwrap();
        let project = home.path().join("project");
        let data = home.path().join("data").join("routelogic");
        let old_data = home.path().join("data").join("routelens");

        std::fs::create_dir_all(project.join(".routelens/flows")).unwrap();
        std::fs::write(
            project.join(".routelens/workspace.yaml"),
            "version: 1\nname: old\n",
        )
        .unwrap();
        std::fs::write(project.join(".routelens/flows/smoke.yaml"), "name: smoke\n").unwrap();
        std::fs::create_dir_all(old_data.join("collections")).unwrap();
        std::fs::write(old_data.join("collections/Kept.yaml"), "name: Kept\n").unwrap();

        let layout = Layout::with_data_dir(&project, &data);
        assert!(layout.exists(), "the old name still counts as a workspace");
        let workspace = Workspace::open_in(layout).unwrap();

        assert_eq!(workspace.manifest().name, "old");
        assert!(project.join(".routelogic/flows/smoke.yaml").is_file());
        assert!(!project.join(".routelens").exists());
        assert!(data.join("collections/Kept.yaml").is_file());
        assert!(!old_data.exists());
        assert_eq!(workspace.flow_names().unwrap(), vec!["smoke".to_string()]);
    }

    #[test]
    fn creation_adds_routelogic_to_a_missing_project_gitignore() {
        let (_dir, ws) = workspace();
        let text = std::fs::read_to_string(ws.layout().project_gitignore()).unwrap();
        assert_eq!(text, ".routelogic\n");
    }

    #[test]
    fn an_existing_project_gitignore_is_appended_to_not_rewritten() {
        let dir = TempDir::new().unwrap();
        let gitignore = dir.path().join(".gitignore");
        std::fs::write(&gitignore, "node_modules/\n*.log").unwrap();

        Workspace::create_in(isolated(dir.path()), "demo", WorkspaceKind::Project).unwrap();

        let text = std::fs::read_to_string(&gitignore).unwrap();
        assert_eq!(text, "node_modules/\n*.log\n.routelogic\n");
    }

    #[test]
    fn a_project_gitignore_that_already_ignores_routelogic_is_left_alone() {
        for spelling in [
            ".routelogic",
            ".routelogic/",
            "/.routelogic",
            "/.routelogic/",
        ] {
            let dir = TempDir::new().unwrap();
            let gitignore = dir.path().join(".gitignore");
            let original = format!("dist/\n{spelling}\n");
            std::fs::write(&gitignore, &original).unwrap();

            Workspace::create_in(isolated(dir.path()), "demo", WorkspaceKind::Project).unwrap();

            assert_eq!(
                std::fs::read_to_string(&gitignore).unwrap(),
                original,
                "{spelling} already covers the directory"
            );
        }
    }

    #[test]
    fn opening_restores_a_removed_project_ignore_rule() {
        let (dir, ws) = workspace();
        std::fs::write(ws.layout().project_gitignore(), "# nothing\n").unwrap();

        Workspace::open_in(isolated(dir.path())).unwrap();

        let text = std::fs::read_to_string(ws.layout().project_gitignore()).unwrap();
        assert_eq!(text, "# nothing\n.routelogic\n");
    }

    #[test]
    fn creating_twice_is_refused_rather_than_overwriting() {
        let (dir, _ws) = workspace();
        assert!(matches!(
            Workspace::create_in(isolated(dir.path()), "other", WorkspaceKind::Project),
            Err(WorkspaceError::AlreadyExists(_))
        ));
    }

    #[test]
    fn opening_a_directory_without_a_workspace_reports_not_found() {
        let dir = TempDir::new().unwrap();
        assert!(matches!(
            Workspace::open_in(isolated(dir.path())),
            Err(WorkspaceError::NotFound(_))
        ));
    }

    #[test]
    fn a_newer_workspace_version_is_refused_with_an_explanation() {
        let (dir, ws) = workspace();
        let path = ws.layout().manifest();
        let text = std::fs::read_to_string(&path)
            .unwrap()
            .replace("version: 1", "version: 999");
        std::fs::write(&path, text).unwrap();

        assert!(matches!(
            Workspace::open_in(isolated(dir.path())),
            Err(WorkspaceError::UnsupportedVersion {
                found: 999,
                supported: 1
            })
        ));
    }

    #[test]
    fn manifest_round_trips() {
        let (dir, mut ws) = workspace();
        ws.manifest_mut()
            .variables
            .insert("org".into(), "acme".into());
        ws.manifest_mut().default_environment = Some("local".into());
        ws.save().unwrap();

        let reopened = Workspace::open_in(isolated(dir.path())).unwrap();
        assert_eq!(reopened.manifest(), ws.manifest());
    }

    #[test]
    fn collections_round_trip_and_list() {
        let (_dir, ws) = workspace();
        let mut users = Collection::new("Users");
        let mut req = RequestDraft::new(HttpMethod::Get, "{{base_url}}/users");
        req.name = Some("List users".into());
        users.push(req);

        ws.save_collection(&users).unwrap();
        ws.save_collection(&Collection::new("Auth")).unwrap();

        assert_eq!(ws.collection_names().unwrap(), vec!["Auth", "Users"]);
        assert_eq!(ws.load_collection("Users").unwrap(), users);
    }

    #[test]
    fn a_missing_collection_is_named_in_the_error() {
        let (_dir, ws) = workspace();
        assert!(matches!(
            ws.load_collection("Nope"),
            Err(WorkspaceError::NoSuchCollection(name)) if name == "Nope"
        ));
    }

    #[test]
    fn listing_an_empty_workspace_yields_nothing_rather_than_failing() {
        let (_dir, ws) = workspace();
        assert!(ws.collection_names().unwrap().is_empty());
        assert!(ws.environment_names().unwrap().is_empty());
    }

    #[test]
    fn a_traversing_collection_name_cannot_escape_the_workspace() {
        let (_dir, ws) = workspace();
        let evil = Collection::new("../../escaped");
        assert!(matches!(
            ws.save_collection(&evil),
            Err(WorkspaceError::InvalidName(_))
        ));
    }

    #[test]
    fn environments_round_trip() {
        let (_dir, ws) = workspace();
        let mut env = Environment::new("local");
        env.set("base_url", "http://localhost:8000")
            .expect_secret("api_token");

        ws.save_environment(&env).unwrap();
        assert_eq!(ws.load_environment("local").unwrap(), env);
        assert_eq!(ws.environment_names().unwrap(), vec!["local"]);
    }

    /// A flow file is reviewed in pull requests like a collection is, so it must read as
    /// what it is: a list of requests with `extract` and `assert` beside each, and edges.
    #[test]
    fn flows_round_trip_as_readable_yaml_and_list_and_delete() {
        use rl_model::{
            Assertion, Extraction, Flow, HttpMethod, Node, NodeKind, RequestDraft, ValueSource,
        };

        let (_dir, ws) = workspace();
        assert!(ws.flow_names().unwrap().is_empty());

        let mut flow = Flow::new("login smoke");
        let mut login = Node::request(RequestDraft::new(
            HttpMethod::Post,
            "{{base_url}}/auth/login",
        ))
        .at(40.0, 80.0);
        if let NodeKind::Request {
            extract, assert, ..
        } = &mut login.kind
        {
            extract.push(Extraction {
                name: "auth_token".into(),
                source: ValueSource::Body {
                    path: "access_token".into(),
                },
            });
            assert.push(Assertion::status_ok());
        }
        let login = flow.add(login);
        let me = flow.add(Node::request(RequestDraft::new(
            HttpMethod::Get,
            "{{base_url}}/me",
        )));
        flow.connect(&login, &me);

        ws.save_flow(&flow).unwrap();
        assert_eq!(ws.flow_names().unwrap(), vec!["login smoke"]);
        assert_eq!(ws.load_flow("login smoke").unwrap(), flow);

        let text = std::fs::read_to_string(ws.layout().flow_file("login smoke").unwrap()).unwrap();
        for expected in [
            "type: request",
            "extract:",
            "name: auth_token",
            "from: body",
            "path: access_token",
            "assert:",
            "op: less_than",
            "expected: '400'",
            "edges:",
        ] {
            assert!(
                text.contains(expected),
                "expected {expected:?} in:
{text}"
            );
        }

        ws.delete_flow("login smoke").unwrap();
        assert!(matches!(
            ws.load_flow("login smoke"),
            Err(WorkspaceError::NoSuchFlow(_))
        ));
        assert!(ws.flow_names().unwrap().is_empty());
    }

    #[test]
    fn variable_context_layers_globals_environment_and_secrets() {
        let (_dir, mut ws) = workspace();
        ws.manifest_mut()
            .variables
            .insert("org".into(), "acme".into());
        ws.manifest_mut().default_environment = Some("local".into());
        ws.save().unwrap();

        let mut env = Environment::new("local");
        env.set("base_url", "http://localhost:8000");
        ws.save_environment(&env).unwrap();

        ws.secrets().set("api_token", "s3cr3t").unwrap();

        let ctx = ws.variable_context(None).unwrap();
        let resolved = ctx
            .resolve("{{base_url}}/{{org}}?t={{secret:api_token}}")
            .unwrap();

        assert_eq!(resolved.value, "http://localhost:8000/acme?t=s3cr3t");
        assert!(resolved.secrets_used.contains("api_token"));
    }

    #[test]
    fn a_default_environment_that_no_longer_exists_does_not_break_opening() {
        let (_dir, mut ws) = workspace();
        ws.manifest_mut().default_environment = Some("deleted".into());
        ws.save().unwrap();

        let ctx = ws.variable_context(None).unwrap();
        assert!(ctx.environment.is_empty());
    }

    /// The end-to-end promise of the three tiers: a committed collection carries the secret
    /// *reference*, the private tier carries the value, and only resolution brings them
    /// together.
    #[test]
    fn a_secret_value_never_reaches_a_committed_file() {
        let (_dir, ws) = workspace();
        ws.secrets().set("api_token", "s3cr3t").unwrap();

        let mut req = RequestDraft::new(HttpMethod::Get, "{{base_url}}/me");
        req.name = Some("Whoami".into());
        req.auth = AuthConfig::Bearer {
            token: "{{secret:api_token}}".into(),
        };
        let mut collection = Collection::new("Auth");
        collection.push(req.clone());
        ws.save_collection(&collection).unwrap();

        let on_disk =
            std::fs::read_to_string(ws.layout().collection_file("Auth").unwrap()).unwrap();
        assert!(on_disk.contains("secret:api_token"));
        assert!(!on_disk.contains("s3cr3t"));

        let mut env = Environment::new("local");
        env.set("base_url", "https://api.example.com");
        ws.save_environment(&env).unwrap();

        let ctx = ws.variable_context(Some("local")).unwrap();
        let (resolved, secrets_used) = req.resolve(&ctx).unwrap();
        assert_eq!(
            resolved.auth,
            AuthConfig::Bearer {
                token: "s3cr3t".into()
            }
        );
        assert!(secrets_used.contains("api_token"));
    }

    /// Committed files are meant to be read and reviewed by a person, so their shape is part
    /// of the contract rather than an implementation detail.
    #[test]
    fn committed_files_stay_readable() {
        let (_dir, ws) = workspace();

        let mut req = RequestDraft::new(HttpMethod::Get, "{{base_url}}/api/v1/users");
        req.name = Some("List users".into());
        req.query.push(rl_model::KeyValue::new("page", "1"));
        let mut collection = Collection::new("Users");
        collection.push(req);
        ws.save_collection(&collection).unwrap();

        let text = std::fs::read_to_string(ws.layout().collection_file("Users").unwrap()).unwrap();

        // Defaults are omitted rather than written out. A `body: {type: none}` block and a
        // full settings block on every GET turn a one-line change into a noisy diff.
        assert!(!text.contains("type: none"), "empty body should be omitted");
        assert!(
            !text.contains("max_redirects"),
            "default settings should be omitted"
        );
        assert!(
            !text.contains("accept_invalid_certs"),
            "never persisted at all"
        );

        // Block-structured YAML, so nested values sit on their own indented lines and a
        // one-field change shows up as a one-line diff. (Braces alone prove nothing here —
        // `{{base_url}}` is a variable reference, not flow style.)
        assert!(
            text.contains("\n  query:\n"),
            "expected block YAML:\n{text}"
        );
        assert!(
            text.contains("\n  - key: page\n"),
            "expected block YAML:\n{text}"
        );

        // The variable reference survives verbatim; nothing resolved it on the way to disk.
        assert!(text.contains("{{base_url}}"));
    }

    #[test]
    fn a_project_workspace_records_its_root_as_here_not_nowhere() {
        let (_dir, ws) = workspace();
        let text = std::fs::read_to_string(ws.layout().manifest()).unwrap();
        assert!(
            !text.contains("root: ''"),
            "an empty path reads as nowhere:\n{text}"
        );
        assert_eq!(
            ws.manifest().project.as_ref().unwrap().root,
            std::path::PathBuf::from(".")
        );
    }
}
