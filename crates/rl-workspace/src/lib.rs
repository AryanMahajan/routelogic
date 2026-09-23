//! # rl-workspace
//!
//! Everything RouteLogic puts on disk, split into three tiers by how the data should be
//! treated.
//!
//! ```text
//! .routelogic/
//! ├── workspace.yaml       committed
//! ├── collections/*.yaml   committed
//! ├── environments/*.yaml  committed — secret NAMES only, never values
//! ├── flows/*.yaml         committed — multi-step API tests, one graph per file
//! ├── .gitignore           committed — written automatically, ignores local/
//! └── local/               never committed
//!     ├── secrets.json     private   — or an OS keychain reference
//!     └── index.sqlite     disposable — rebuildable source index cache
//!
//! <user data dir>/routelogic/   per user, every project
//! ├── collections/*.yaml   saved requests
//! └── history.sqlite       private — what was sent, across every project
//! ```
//!
//! ## Why three tiers and not two
//!
//! "Committed" and "not committed" leaves nowhere to express the difference between *a
//! secret you must protect* and *a cache you can delete*. Conflating them means either
//! caches get backed up or secrets get treated as disposable.
//!
//! | Tier | Committed | Safe to delete |
//! |---|---|---|
//! | Shared — collections, environments, flows | yes | no, it is real work |
//! | Private — secret values | no | no, they are real credentials |
//! | Disposable — history, index | no | yes, it rebuilds |
//!
//! `.routelogic/.gitignore` is written at creation, and restored on open if it went missing.
//! Git friendliness plus bearer tokens is exactly how credentials reach version control.
//! The project's own `.gitignore` also gains a `.routelogic` line on create and open —
//! appended, never rewritten; created if absent — so the directory stays out of the repo.
//!
//! Committed files use sorted keys, so a diff reflects a real edit rather than serializer
//! churn — these files are meant to be reviewed in a pull request.
//!
//! ## Example
//!
//! ```no_run
//! use rl_workspace::{Workspace, WorkspaceKind, Environment};
//! use rl_workspace::secrets::SecretStore;
//!
//! let ws = Workspace::open_or_create(".", "myproject", WorkspaceKind::Project)?;
//!
//! let mut env = Environment::new("local");
//! env.set("base_url", "http://localhost:8000").expect_secret("api_token");
//! ws.save_environment(&env)?;
//!
//! ws.secrets().set("api_token", "s3cr3t")?;
//!
//! // Globals, environment, and secrets, assembled for a request to resolve against.
//! let ctx = ws.variable_context(Some("local"))?;
//! # Ok::<(), rl_workspace::WorkspaceError>(())
//! ```
//!
//! Status: P0 complete — layout, manifest, collections, environments, secrets.
//! History and the source index (both SQLite) arrive with P1 and P3.

#![forbid(unsafe_code)]

pub mod collection;
pub mod environment;
pub mod error;
pub mod history;
pub mod layout;
pub mod secrets;
pub mod watch;
pub mod workspace;

pub use collection::Collection;
pub use environment::Environment;
pub use error::{Result, WorkspaceError};
pub use history::{History, HistoryEntry, NewEntry, RedactedEntry};
pub use layout::{default_data_dir, Layout, DATA_DIR_ENV};
pub use secrets::{FileSecretStore, SecretStore};
pub use watch::{Change, ChangeKind, Watcher};
pub use workspace::{
    AgentAllow, AgentConfig, ProjectRef, Workspace, WorkspaceKind, WorkspaceManifest,
};
