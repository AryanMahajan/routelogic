//! HTTP methods.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// An HTTP method.
///
/// Only the methods RouteLogic can meaningfully discover and send are modelled.
/// Anything else round-trips through [`HttpMethod::Other`] rather than being rejected —
/// an API client that cannot send an unusual method is less useful than one that can.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Head,
    Post,
    Put,
    Patch,
    Delete,
    Options,
    Trace,
    #[serde(untagged)]
    Other(String),
}

/// Written by hand: the catch-all [`HttpMethod::Other`] is untagged, so on the wire every
/// method is simply a string, which is what the schema should say.
impl schemars::JsonSchema for HttpMethod {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "HttpMethod".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "description": "An HTTP method, upper case. Any token is accepted; these are the usual ones.",
            "examples": ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"]
        })
    }
}

impl HttpMethod {
    /// Every method that a framework adapter may emit without an explicit method list.
    pub const COMMON: [HttpMethod; 8] = [
        HttpMethod::Get,
        HttpMethod::Head,
        HttpMethod::Post,
        HttpMethod::Put,
        HttpMethod::Patch,
        HttpMethod::Delete,
        HttpMethod::Options,
        HttpMethod::Trace,
    ];

    pub fn as_str(&self) -> &str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Head => "HEAD",
            HttpMethod::Post => "POST",
            HttpMethod::Put => "PUT",
            HttpMethod::Patch => "PATCH",
            HttpMethod::Delete => "DELETE",
            HttpMethod::Options => "OPTIONS",
            HttpMethod::Trace => "TRACE",
            HttpMethod::Other(s) => s,
        }
    }

    /// Whether a request body is conventional for this method.
    ///
    /// This drives UI defaults only. It never prevents a body from being sent — deliberately
    /// sending a body with `GET` is a legitimate thing to want to do.
    pub fn conventionally_has_body(&self) -> bool {
        matches!(self, HttpMethod::Post | HttpMethod::Put | HttpMethod::Patch)
    }

    /// Safe methods are not expected to modify state.
    pub fn is_safe(&self) -> bool {
        matches!(
            self,
            HttpMethod::Get | HttpMethod::Head | HttpMethod::Options | HttpMethod::Trace
        )
    }

    pub fn is_idempotent(&self) -> bool {
        self.is_safe() || matches!(self, HttpMethod::Put | HttpMethod::Delete)
    }
}

impl FromStr for HttpMethod {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.trim().to_ascii_uppercase().as_str() {
            "GET" => HttpMethod::Get,
            "HEAD" => HttpMethod::Head,
            "POST" => HttpMethod::Post,
            "PUT" => HttpMethod::Put,
            "PATCH" => HttpMethod::Patch,
            "DELETE" => HttpMethod::Delete,
            "OPTIONS" => HttpMethod::Options,
            "TRACE" => HttpMethod::Trace,
            other => HttpMethod::Other(other.to_string()),
        })
    }
}

impl fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_case_insensitively() {
        assert_eq!("get".parse::<HttpMethod>().unwrap(), HttpMethod::Get);
        assert_eq!("  PoSt ".parse::<HttpMethod>().unwrap(), HttpMethod::Post);
    }

    #[test]
    fn unknown_methods_round_trip() {
        let m: HttpMethod = "PURGE".parse().unwrap();
        assert_eq!(m, HttpMethod::Other("PURGE".into()));
        assert_eq!(m.as_str(), "PURGE");
    }

    #[test]
    fn serializes_as_uppercase_string() {
        let json = serde_json::to_string(&HttpMethod::Delete).unwrap();
        assert_eq!(json, "\"DELETE\"");
        let back: HttpMethod = serde_json::from_str(&json).unwrap();
        assert_eq!(back, HttpMethod::Delete);
    }

    #[test]
    fn unknown_method_serializes_without_a_wrapper() {
        let json = serde_json::to_string(&HttpMethod::Other("PURGE".into())).unwrap();
        assert_eq!(json, "\"PURGE\"");
    }

    #[test]
    fn body_convention_does_not_include_get() {
        assert!(!HttpMethod::Get.conventionally_has_body());
        assert!(HttpMethod::Post.conventionally_has_body());
    }
}
