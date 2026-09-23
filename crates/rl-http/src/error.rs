//! Execution errors.

pub type Result<T> = std::result::Result<T, HttpError>;

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("{url:?} is not a valid URL")]
    InvalidUrl {
        url: String,
        #[source]
        source: url::ParseError,
    },

    /// A placeholder was never filled in, so the request would go somewhere meaningless.
    #[error("the URL still contains an unfilled placeholder: {placeholder}")]
    UnfilledPlaceholder { placeholder: String },

    #[error("{name:?} is not a valid header name or value")]
    InvalidHeader { name: String },

    #[error("could not read {path}")]
    BodyFile {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("the request timed out after {ms}ms")]
    Timeout { ms: u64 },

    #[error("stopped after {limit} redirects")]
    TooManyRedirects { limit: u8 },

    #[error("a redirect to {location:?} could not be resolved")]
    BadRedirect { location: String },

    /// A guard passed to [`crate::HttpEngine::execute_guarded`] would not let this request —
    /// or a redirect it led to — go out. Nothing was sent to the refused destination.
    #[error("refused: {reason}")]
    Refused { reason: String },

    #[error("could not build the HTTP client")]
    Client {
        #[source]
        source: reqwest::Error,
    },

    #[error("request failed")]
    Send {
        #[source]
        source: reqwest::Error,
    },

    #[error("could not read the response body")]
    Body {
        #[source]
        source: reqwest::Error,
    },
}

impl HttpError {
    /// Whether retrying unchanged could plausibly succeed.
    pub fn is_transient(&self) -> bool {
        match self {
            HttpError::Timeout { .. } => true,
            HttpError::Send { source } => source.is_timeout() || source.is_connect(),
            _ => false,
        }
    }
}
