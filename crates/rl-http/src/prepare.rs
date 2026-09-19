//! A request described rather than sent — for "copy as cURL" and friends.
//!
//! Uses the same URL, header and auth assembly as [`crate::HttpEngine::execute`], so the
//! command copied is the request that would go out, not an approximation of it.

use crate::engine::{build_headers, build_url, header_pairs};
use crate::error::Result;
use reqwest::header::CONTENT_TYPE;
use rl_model::{BodyValue, FormPart, RequestDraft};
use serde::{Deserialize, Serialize};

/// A request as it would go on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedRequest {
    pub method: String,
    /// Path values filled, query and API-key parameters appended.
    pub url: String,
    /// In the order they would be sent, auth and cookies applied, the body's content type
    /// included unless the request already states one.
    pub headers: Vec<(String, String)>,
    pub body: PreparedBody,
}

/// The body, kept in the shape a shell or a script can express directly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PreparedBody {
    None,
    /// JSON, plain text, or a form already URL-encoded.
    Text {
        content: String,
    },
    /// A file sent verbatim; the caller references it rather than inlining it.
    File {
        path: String,
    },
    /// `multipart/form-data`. The parts stay separate because every target (curl `-F`,
    /// `FormData`) builds the boundary itself.
    Multipart {
        parts: Vec<PreparedPart>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PreparedPart {
    Text {
        name: String,
        value: String,
    },
    File {
        name: String,
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_type: Option<String>,
    },
}

/// Describe a request without sending it.
///
/// The draft is expected to be already resolved, as for [`crate::HttpEngine::execute`]. A
/// body on `HEAD` is left out, as it would be on the wire.
pub fn prepare(draft: &RequestDraft) -> Result<PreparedRequest> {
    let url = build_url(draft)?;
    let mut headers = header_pairs(&build_headers(draft)?);
    let method = draft.method.as_str().to_string();

    let body = if method.eq_ignore_ascii_case("HEAD") {
        PreparedBody::None
    } else {
        match &draft.body {
            BodyValue::None => PreparedBody::None,
            BodyValue::Json { content } | BodyValue::Text { content, .. } => PreparedBody::Text {
                content: content.clone(),
            },
            BodyValue::Form { fields } => PreparedBody::Text {
                content: url::form_urlencoded::Serializer::new(String::new())
                    .extend_pairs(
                        fields
                            .iter()
                            .filter(|f| f.enabled)
                            .map(|f| (f.key.as_str(), f.value.as_str())),
                    )
                    .finish(),
            },
            BodyValue::Binary { path, .. } => PreparedBody::File {
                path: path.display().to_string(),
            },
            BodyValue::Multipart { parts } => PreparedBody::Multipart {
                parts: parts
                    .iter()
                    .filter_map(|part| match part {
                        FormPart::Text {
                            name,
                            value,
                            enabled: true,
                        } => Some(PreparedPart::Text {
                            name: name.clone(),
                            value: value.clone(),
                        }),
                        FormPart::File {
                            name,
                            path,
                            content_type,
                            enabled: true,
                        } => Some(PreparedPart::File {
                            name: name.clone(),
                            path: path.display().to_string(),
                            content_type: content_type.clone(),
                        }),
                        _ => None,
                    })
                    .collect(),
            },
        }
    };

    // Multipart is the one body whose content type carries a boundary the target tool
    // makes up itself, so it is the one left for the tool to add. A file with no stated
    // type goes out with none, as `execute` sends it.
    let stated = headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case(CONTENT_TYPE.as_str()));
    let implied = match &draft.body {
        BodyValue::Multipart { .. }
        | BodyValue::Binary {
            content_type: None, ..
        } => None,
        other => other.implied_content_type(),
    };
    if let (false, Some(content_type), false) =
        (stated, implied, matches!(body, PreparedBody::None))
    {
        headers.push((CONTENT_TYPE.as_str().to_string(), content_type.to_string()));
    }

    Ok(PreparedRequest {
        method,
        url: url.to_string(),
        headers,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rl_model::{AuthConfig, HttpMethod, KeyValue};

    #[test]
    fn a_json_post_carries_its_url_query_auth_and_content_type() {
        let mut d = RequestDraft::new(HttpMethod::Post, "http://localhost:9000/items/{id}");
        d.path_values.insert("id".into(), "7".into());
        d.query.push(KeyValue::new("verbose", "1"));
        d.headers.push(KeyValue::new("X-Trace", "abc"));
        d.auth = AuthConfig::Bearer {
            token: "t0k".into(),
        };
        d.body = BodyValue::Json {
            content: "{\"a\":1}".into(),
        };

        let p = prepare(&d).unwrap();
        assert_eq!(p.method, "POST");
        assert_eq!(p.url, "http://localhost:9000/items/7?verbose=1");
        assert!(p.headers.contains(&("x-trace".into(), "abc".into())));
        assert!(p
            .headers
            .contains(&("authorization".into(), "Bearer t0k".into())));
        assert!(p
            .headers
            .contains(&("content-type".into(), "application/json".into())));
        assert_eq!(
            p.body,
            PreparedBody::Text {
                content: "{\"a\":1}".into()
            }
        );
    }

    #[test]
    fn a_form_is_encoded_and_a_stated_content_type_is_not_doubled() {
        let mut d = RequestDraft::new(HttpMethod::Post, "http://h/login");
        d.headers.push(KeyValue::new(
            "Content-Type",
            "application/x-www-form-urlencoded",
        ));
        d.body = BodyValue::Form {
            fields: vec![KeyValue::new("user", "a b"), KeyValue::new("pw", "x&y")],
        };
        let p = prepare(&d).unwrap();
        assert_eq!(
            p.body,
            PreparedBody::Text {
                content: "user=a+b&pw=x%26y".into()
            }
        );
        assert_eq!(
            p.headers
                .iter()
                .filter(|(n, _)| n == "content-type")
                .count(),
            1
        );
    }

    #[test]
    fn multipart_keeps_its_enabled_parts_and_states_no_content_type() {
        let mut d = RequestDraft::new(HttpMethod::Post, "http://h/upload");
        d.body = BodyValue::Multipart {
            parts: vec![
                FormPart::Text {
                    name: "title".into(),
                    value: "hi".into(),
                    enabled: true,
                },
                FormPart::Text {
                    name: "off".into(),
                    value: "no".into(),
                    enabled: false,
                },
            ],
        };
        let p = prepare(&d).unwrap();
        assert_eq!(
            p.body,
            PreparedBody::Multipart {
                parts: vec![PreparedPart::Text {
                    name: "title".into(),
                    value: "hi".into()
                }]
            }
        );
        assert!(p.headers.is_empty());
    }

    #[test]
    fn head_drops_the_body() {
        let mut d = RequestDraft::new(HttpMethod::Head, "http://h/");
        d.body = BodyValue::Json {
            content: "{}".into(),
        };
        let p = prepare(&d).unwrap();
        assert_eq!(p.body, PreparedBody::None);
        assert!(p.headers.is_empty());
    }
}
