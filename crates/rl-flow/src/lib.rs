//! # rl-flow
//!
//! Runs a [`rl_model::Flow`]: a graph of requests where each response can feed the next.
//!
//! ```text
//! Flow ──▶ execution order ──▶ for each node:
//!            (rl-model)          gate on upstream outcomes
//!                                resolve {{variables}}   ← environment + values extracted so far
//!                                send                    ← Sender (rl-http, or a fake in tests)
//!                                extract → new variables
//!                                assert  → pass / fail
//!                                          │
//!                                    FlowEvent stream ──▶ UI lights the card up
//! ```
//!
//! The crate owns three things, and nothing else:
//!
//! - [`extract`] — reading a value out of a response by [`rl_model::ValueSource`].
//! - [`compare`] — deciding whether `actual op expected` holds.
//! - [`run`] — the runner itself, with the skip semantics the module docs spell out.
//! - [`lint`] — the mistakes a flow would only reveal by running: variables nothing
//!   defines, captures used before they happen, requests that assert nothing.
//!
//! It does not know about workspaces, history or the UI. The application wraps
//! [`run::run`] with the variables of the active environment and records each exchange the
//! same way a single send is recorded.

#![forbid(unsafe_code)]

pub mod compare;
pub mod extract;
pub mod lint;
pub mod run;

pub use extract::ExtractError;
pub use lint::{lint, Warning};
pub use run::{
    run, run_with, AssertionResult, Extracted, Failure, FlowEvent, FlowRun, NodeResult, Outcome,
    RunOptions, Sender, SkipReason, Summary,
};
