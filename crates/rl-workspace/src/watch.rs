//! Noticing when a workspace changes underneath the application.
//!
//! Two processes can share a workspace: the app, and an agent's MCP server writing flows
//! through the same files. A `git checkout` or a hand edit changes them too. The watcher
//! reports each change as a [`Change`] — which document, and whether it is gone — so the
//! app can reload what it shows instead of saving a stale copy over someone else's work.
//!
//! ## Our own writes are not changes
//!
//! Saving a flow writes its file, which fires the watcher, which must not tell the app its
//! own save happened elsewhere. Every workspace write in this process goes through one
//! function, which records the content it wrote; a change whose file still holds exactly
//! that content is ours and is dropped. Comparing content rather than timing means a slow
//! disk cannot make an echo look foreign, and another process writing the same bytes is,
//! correctly, no change at all.

use crate::layout::{self, Layout};
use crate::{Result, WorkspaceError};
use notify::{RecursiveMode, Watcher as _};
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeSet, HashMap};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// How long to wait for a burst of events to settle. An editor's save, or a rename into
/// place, is several events for one change.
const SETTLE: Duration = Duration::from_millis(200);

/// Which kind of document changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Flow,
    Collection,
    Environment,
    /// `workspace.yaml` itself.
    Workspace,
}

/// A document that changed on disk, not by this process.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Change {
    pub kind: ChangeKind,
    /// The document's name — the file name without `.yaml`. Empty for the manifest.
    pub name: String,
    /// The file no longer exists.
    pub removed: bool,
}

/// Watches a workspace until dropped.
pub struct Watcher {
    _inner: notify::RecommendedWatcher,
}

impl std::fmt::Debug for Watcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Watcher").finish_non_exhaustive()
    }
}

impl Watcher {
    /// Start watching `layout`'s workspace directory and the user's collections, calling
    /// `on_change` from a background thread for every change this process did not make.
    pub fn start(layout: &Layout, on_change: impl Fn(Change) + Send + 'static) -> Result<Watcher> {
        let roots = [layout.dir(), layout.collections_dir()];
        for root in &roots {
            std::fs::create_dir_all(root)
                .map_err(|e| WorkspaceError::io(format!("creating {}", root.display()), e))?;
        }

        let (sender, events) = mpsc::channel::<PathBuf>();
        let mut inner = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if let Ok(event) = event {
                for path in event.paths {
                    let _ = sender.send(path);
                }
            }
        })
        .map_err(|e| WorkspaceError::io("starting the file watcher", std::io::Error::other(e)))?;

        for root in &roots {
            inner.watch(root, RecursiveMode::Recursive).map_err(|e| {
                WorkspaceError::io(
                    format!("watching {}", root.display()),
                    std::io::Error::other(e),
                )
            })?;
        }

        let layout = layout.clone();
        std::thread::Builder::new()
            .name("workspace-watcher".into())
            .spawn(move || settle(events, &layout, on_change))
            .map_err(|e| WorkspaceError::io("starting the file watcher", e))?;

        Ok(Watcher { _inner: inner })
    }
}

/// Collect a burst of events, then report each document once. Ends when the watcher is
/// dropped, which closes the channel.
fn settle(events: mpsc::Receiver<PathBuf>, layout: &Layout, on_change: impl Fn(Change)) {
    while let Ok(first) = events.recv() {
        let mut paths = BTreeSet::from([first]);
        while let Ok(more) = events.recv_timeout(SETTLE) {
            paths.insert(more);
        }
        let changes: BTreeSet<Change> = paths
            .iter()
            .filter_map(|path| Some((classify(layout, path)?, path)))
            .filter(|(change, path)| !change_is_ours(path, change.removed))
            .map(|(change, _)| change)
            .collect();
        for change in changes {
            on_change(change);
        }
    }
}

/// Which document a path is, and whether it is gone. `None` for anything that is not a
/// document: history, secrets, a temporary file mid-write.
fn classify(layout: &Layout, path: &Path) -> Option<Change> {
    let file_name = path.file_name()?.to_str()?;
    if file_name.starts_with('.') {
        return None;
    }
    let parent = path.parent()?;
    let removed = !path.is_file();

    let kind = if same_dir(parent, &layout.dir()) && file_name == layout::MANIFEST_FILE {
        ChangeKind::Workspace
    } else if same_dir(parent, &layout.flows_dir()) {
        ChangeKind::Flow
    } else if same_dir(parent, &layout.environments_dir()) {
        ChangeKind::Environment
    } else if same_dir(parent, &layout.collections_dir()) {
        ChangeKind::Collection
    } else {
        return None;
    };

    let name = if kind == ChangeKind::Workspace {
        String::new()
    } else {
        layout::name_from_file(path)?
    };
    Some(Change {
        kind,
        name,
        removed,
    })
}

/// Paths from the watcher and from the layout can differ in form — a `\\?\` prefix, a
/// drive letter's case — so directories are compared by their last two components, which
/// is unambiguous within one workspace.
fn same_dir(a: &Path, b: &Path) -> bool {
    tail(a) == tail(b)
}

fn tail(path: &Path) -> Vec<String> {
    let parts: Vec<String> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect();
    parts[parts.len().saturating_sub(2)..].to_vec()
}

// --- our own writes -------------------------------------------------------------------

fn written() -> &'static Mutex<HashMap<Vec<String>, u64>> {
    static WRITTEN: OnceLock<Mutex<HashMap<Vec<String>, u64>>> = OnceLock::new();
    WRITTEN.get_or_init(Default::default)
}

fn key(path: &Path) -> Vec<String> {
    let mut parts = tail(path.parent().unwrap_or(path));
    parts.push(
        path.file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default(),
    );
    parts
}

fn fingerprint(contents: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    contents.hash(&mut hasher);
    hasher.finish()
}

/// Record that this process wrote `contents` to `path`, so the watcher can tell the echo
/// of its own save from a change made elsewhere.
pub(crate) fn remember_write(path: &Path, contents: &[u8]) {
    let mut written = written().lock().unwrap_or_else(|e| e.into_inner());
    written.insert(key(path), fingerprint(contents));
}

/// Record that this process deleted `path`.
pub(crate) fn remember_delete(path: &Path) {
    let mut written = written().lock().unwrap_or_else(|e| e.into_inner());
    written.insert(key(path), 0);
}

/// Whether the file at `path` is exactly as this process last left it.
fn change_is_ours(path: &Path, removed: bool) -> bool {
    let written = written().lock().unwrap_or_else(|e| e.into_inner());
    let Some(&expected) = written.get(&key(path)) else {
        return false;
    };
    if removed {
        return expected == 0;
    }
    std::fs::read(path).is_ok_and(|contents| expected != 0 && fingerprint(&contents) == expected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Workspace;
    use rl_model::{Flow, HttpMethod, Node, RequestDraft};
    use std::sync::mpsc::Receiver;
    use tempfile::TempDir;

    fn workspace() -> (TempDir, TempDir, Workspace) {
        let project = TempDir::new().unwrap();
        let data = TempDir::new().unwrap();
        let layout = Layout::with_data_dir(project.path(), data.path());
        let workspace =
            Workspace::create_in(layout, "watched", crate::WorkspaceKind::Project).unwrap();
        for dir in [
            workspace.layout().flows_dir(),
            workspace.layout().environments_dir(),
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }
        (project, data, workspace)
    }

    fn watch(workspace: &Workspace) -> (Watcher, Receiver<Change>) {
        let (tx, rx) = mpsc::channel();
        let watcher = Watcher::start(workspace.layout(), move |change| {
            let _ = tx.send(change);
        })
        .unwrap();
        (watcher, rx)
    }

    fn next(rx: &Receiver<Change>) -> Option<Change> {
        rx.recv_timeout(Duration::from_secs(5)).ok()
    }

    fn flow(name: &str) -> Flow {
        let mut flow = Flow::new(name);
        flow.add(Node::request(RequestDraft::new(HttpMethod::Get, "/x")));
        flow
    }

    #[test]
    fn a_flow_written_by_someone_else_is_reported() {
        let (_p, _d, workspace) = workspace();
        let (_watcher, rx) = watch(&workspace);

        let path = workspace.layout().flow_file("from outside").unwrap();
        std::fs::write(&path, "name: from outside\nnodes: []\n").unwrap();

        assert_eq!(
            next(&rx),
            Some(Change {
                kind: ChangeKind::Flow,
                name: "from outside".into(),
                removed: false,
            })
        );
    }

    #[test]
    fn our_own_save_is_not_reported_but_a_later_foreign_edit_is() {
        let (_p, _d, workspace) = workspace();
        let (_watcher, rx) = watch(&workspace);

        workspace.save_flow(&flow("ours")).unwrap();
        assert_eq!(
            rx.recv_timeout(SETTLE * 4).ok(),
            None,
            "the echo of our own save"
        );

        let path = workspace.layout().flow_file("ours").unwrap();
        std::fs::write(&path, "name: ours\nnodes: []\n").unwrap();
        assert_eq!(next(&rx).map(|c| c.name), Some("ours".to_string()));
    }

    #[test]
    fn a_removed_flow_says_so() {
        let (_p, _d, workspace) = workspace();
        let path = workspace.layout().flow_file("doomed").unwrap();
        std::fs::write(&path, "name: doomed\nnodes: []\n").unwrap();
        let (_watcher, rx) = watch(&workspace);

        std::fs::remove_file(&path).unwrap();
        let change = next(&rx).unwrap();
        assert_eq!((change.name.as_str(), change.removed), ("doomed", true));
    }

    #[test]
    fn environments_and_the_manifest_are_classified_and_local_state_is_not() {
        let (_p, _d, workspace) = workspace();
        let (_watcher, rx) = watch(&workspace);
        let layout = workspace.layout();

        std::fs::write(
            layout.environments_dir().join("staging.yaml"),
            "name: staging\n",
        )
        .unwrap();
        assert_eq!(next(&rx).map(|c| c.kind), Some(ChangeKind::Environment));

        std::fs::create_dir_all(layout.dir().join("local")).unwrap();
        std::fs::write(layout.dir().join("local").join("secrets.json"), "{}").unwrap();
        std::fs::write(
            layout.flows_dir().join(".half-written.yaml.1.tmp"),
            "partial",
        )
        .unwrap();
        assert_eq!(
            rx.recv_timeout(SETTLE * 4).ok(),
            None,
            "secrets and temporary files are not documents"
        );
    }
}
