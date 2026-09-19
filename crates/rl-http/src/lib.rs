//! # rl-http
//!
//! Request execution. Built on `reqwest`, and configured against most of its conveniences.
//!
//! An API client's job is to send *exactly* what the user described, including the things a
//! normal HTTP client would helpfully correct. Convenience is the wrong default here:
//!
//! - **Redirects are not followed** unless asked. When they are, they are followed by hand so
//!   the chain can be recorded — and so an `Authorization` header is dropped the moment a
//!   redirect crosses origins. Forwarding a token to a host the user never named is how a
//!   credential leaks.
//! - **Duplicate headers survive.** A request may legitimately carry the same header twice.
//! - **A body on `GET` is sent.** Unconventional, occasionally required, and not this
//!   crate's business to veto. Only `HEAD` is refused.
//! - **Certificate verification is per request**, never global, so relaxing it for one call
//!   against localhost cannot weaken a later call to production.
//! - **Bodies are capped, not streamed into memory forever** — see
//!   [`response::MAX_BODY_PREVIEW`].
//! - **An unfilled `{path_param}` is an error**, not a request to a meaningless URL that
//!   comes back as a confusing 404.
//!
//! ## One honest compromise
//!
//! `gzip`, `brotli` and `deflate` are decoded transparently, because an unreadable body helps
//! nobody. The encoding the server applied is recorded in
//! [`response::Body::content_encoding`] so the difference from `curl --raw` is visible rather
//! than mysterious.
//!
//! ## Timing
//!
//! Time-to-first-byte and total. The DNS / TCP / TLS breakdown needs a custom connector and
//! is deferred rather than allowed to hold up the runner.
//!
//! ## Example
//!
//! ```no_run
//! # async fn run() -> Result<(), rl_http::HttpError> {
//! use rl_http::HttpEngine;
//! use rl_model::{HttpMethod, RequestDraft};
//!
//! let engine = HttpEngine::new();
//! let draft = RequestDraft::new(HttpMethod::Get, "https://api.example.com/users");
//! let exchange = engine.execute(&draft).await?;
//!
//! println!("{} in {}ms", exchange.response.status, exchange.response.timing.total_ms);
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]

pub mod engine;
pub mod error;
pub mod prepare;
pub mod response;

pub use engine::HttpEngine;
pub use error::{HttpError, Result};
pub use prepare::{prepare, PreparedBody, PreparedPart, PreparedRequest};
pub use response::{Body, Exchange, Hop, Response, SentRequest, Timing, MAX_BODY_PREVIEW};
