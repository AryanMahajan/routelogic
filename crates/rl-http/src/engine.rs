//! Request execution.

use crate::error::{HttpError, Result};
use crate::response::{Body, Exchange, Hop, Response, SentRequest, Timing, MAX_BODY_PREVIEW};
use base64::Engine as _;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION, COOKIE};
use reqwest::{Client, Method, Url};
use rl_model::{AuthConfig, BodyValue, FormPart, RequestDraft, RequestSettings};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How much of a request body is kept for the record of what was sent.
const MAX_REQUEST_PREVIEW: usize = 64 * 1024;

/// Clients are cached by the settings that affect their construction, so a burst of requests
/// against the same host reuses one connection pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ClientKey {
    accept_invalid_certs: bool,
    timeout_ms: u64,
}

/// Executes requests.
///
/// Configured against most of `reqwest`'s conveniences. An API client's job is to send
/// exactly what was described, including the things a normal HTTP client would helpfully
/// correct.
#[derive(Debug, Default)]
pub struct HttpEngine {
    clients: Mutex<HashMap<ClientKey, Client>>,
}

impl HttpEngine {
    pub fn new() -> Self {
        Self::default()
    }

    fn client(&self, settings: &RequestSettings) -> Result<Client> {
        let key = ClientKey {
            accept_invalid_certs: settings.accept_invalid_certs,
            timeout_ms: settings.timeout_ms,
        };

        let mut clients = self.clients.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(client) = clients.get(&key) {
            return Ok(client.clone());
        }

        let client = Client::builder()
            // Redirects are followed by hand below, so the chain can be recorded and
            // credentials dropped when one crosses origins.
            .redirect(reqwest::redirect::Policy::none())
            .danger_accept_invalid_certs(settings.accept_invalid_certs)
            .timeout(Duration::from_millis(settings.timeout_ms))
            .build()
            .map_err(|source| HttpError::Client { source })?;

        clients.insert(key, client.clone());
        Ok(client)
    }

    /// Send a request.
    ///
    /// The draft is expected to be already resolved — see [`rl_model::RequestDraft::resolve`].
    /// Resolution happens as late as possible, and this is the last stop before the wire.
    pub async fn execute(&self, draft: &RequestDraft) -> Result<Exchange> {
        let client = self.client(&draft.settings)?;
        let method = Method::from_bytes(draft.method.as_str().as_bytes()).unwrap_or(Method::GET);

        let mut url = build_url(draft)?;
        let mut headers = build_headers(draft)?;
        let mut current_method = method.clone();
        let mut redirects: Vec<Hop> = Vec::new();
        // Cleared when a redirect downgrades the method: the body described a POST that has
        // already happened, and replaying it on the follow-up GET would be wrong.
        let mut send_body = true;

        let started = Instant::now();

        loop {
            let mut request = client.request(current_method.clone(), url.clone());
            request = request.headers(headers.clone());

            // The body is rebuilt each hop rather than cloned: a multipart form is not
            // clonable, and a file-backed body has to be re-read anyway.
            if send_body && body_is_sent(&current_method, &draft.body) {
                request = apply_body(request, &draft.body)?;
            }

            let built = request
                .build()
                .map_err(|source| HttpError::Send { source })?;

            let sent = SentRequest {
                method: built.method().to_string(),
                url: built.url().to_string(),
                headers: header_pairs(built.headers()),
                body_size: built
                    .body()
                    .and_then(|b| b.as_bytes())
                    .map_or(0, <[u8]>::len),
                body_preview: built
                    .body()
                    .and_then(|b| b.as_bytes())
                    .map(preview_of_bytes),
            };

            let response = client.execute(built).await.map_err(|source| {
                if source.is_timeout() {
                    HttpError::Timeout {
                        ms: draft.settings.timeout_ms,
                    }
                } else {
                    HttpError::Send { source }
                }
            })?;

            let ttfb_ms = started.elapsed().as_millis() as u64;
            let status = response.status();

            let should_follow = draft.settings.follow_redirects
                && status.is_redirection()
                && response.headers().contains_key(reqwest::header::LOCATION);

            if should_follow {
                if redirects.len() as u8 >= draft.settings.max_redirects {
                    return Err(HttpError::TooManyRedirects {
                        limit: draft.settings.max_redirects,
                    });
                }

                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_string();

                let next = url.join(&location).map_err(|_| HttpError::BadRedirect {
                    location: location.clone(),
                })?;

                // Dropping credentials when the origin changes is the whole reason redirects
                // are followed by hand. Forwarding an Authorization header to a host the
                // user never named is how a token leaks.
                let crossed_origin = !same_origin(&url, &next);
                let had_auth = headers.contains_key(AUTHORIZATION);
                if crossed_origin && had_auth {
                    headers.remove(AUTHORIZATION);
                    headers.remove(COOKIE);
                }

                redirects.push(Hop {
                    status: status.as_u16(),
                    from: url.to_string(),
                    to: next.to_string(),
                    credentials_stripped: crossed_origin && had_auth,
                });

                // 303 always becomes GET; 301 and 302 do so for POST, by long-standing
                // practice. 307 and 308 preserve the method and body.
                if status.as_u16() == 303
                    || (matches!(status.as_u16(), 301 | 302) && current_method == Method::POST)
                {
                    current_method = Method::GET;
                    send_body = false;
                    // The body is gone, so its framing headers must go too, or the next
                    // request advertises a Content-Length it will never send.
                    headers.remove(reqwest::header::CONTENT_TYPE);
                    headers.remove(reqwest::header::CONTENT_LENGTH);
                }

                url = next;
                continue;
            }

            let status_text = status.canonical_reason().unwrap_or("").to_string();
            let headers_out = header_pairs(response.headers());
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            let content_encoding = response
                .headers()
                .get(reqwest::header::CONTENT_ENCODING)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            let reported_length = response.content_length();

            let full = response
                .bytes()
                .await
                .map_err(|source| HttpError::Body { source })?;
            let total_ms = started.elapsed().as_millis() as u64;

            let truncated = full.len() > MAX_BODY_PREVIEW;
            let bytes = if truncated {
                full[..MAX_BODY_PREVIEW].to_vec()
            } else {
                full.to_vec()
            };

            return Ok(Exchange {
                request: sent,
                response: Response {
                    status: status.as_u16(),
                    status_text,
                    headers: headers_out,
                    body: Body {
                        bytes,
                        truncated,
                        reported_length,
                        content_type,
                        content_encoding,
                    },
                    timing: Timing { ttfb_ms, total_ms },
                    redirects,
                    insecure: draft.settings.accept_invalid_certs,
                },
            });
        }
    }
}

/// Whether a body goes out.
///
/// Only `HEAD` is refused, because a body there is meaningless rather than merely unusual.
/// A body on `GET` is unconventional but legitimate, and several real APIs require it — an
/// API client that silently drops it is the more annoying failure.
fn body_is_sent(method: &Method, body: &BodyValue) -> bool {
    !body.is_none() && method != Method::HEAD
}

pub(crate) fn build_url(draft: &RequestDraft) -> Result<Url> {
    let raw = draft.url_with_path_values();

    // A `{placeholder}` left in the URL means a path parameter was never filled. Sending it
    // would hit a meaningless path and produce a confusing 404, so say what is missing.
    if let Some(placeholder) = find_placeholder(&raw) {
        return Err(HttpError::UnfilledPlaceholder { placeholder });
    }

    let mut url = Url::parse(&raw).map_err(|source| HttpError::InvalidUrl {
        url: raw.clone(),
        source,
    })?;

    {
        let mut pairs = url.query_pairs_mut();
        for row in draft.query.iter().filter(|r| r.enabled) {
            pairs.append_pair(&row.key, &row.value);
        }
        if let AuthConfig::ApiKey {
            key,
            value,
            location: rl_model::ApiKeyLocation::Query,
        } = &draft.auth
        {
            pairs.append_pair(key, value);
        }
    }

    // `query_pairs_mut` leaves a bare `?` behind when nothing was appended to an empty query.
    if url.query() == Some("") {
        url.set_query(None);
    }

    Ok(url)
}

fn find_placeholder(url: &str) -> Option<String> {
    let start = url.find('{')?;
    let end = url[start..].find('}')? + start;
    Some(url[start..=end].to_string())
}

pub(crate) fn build_headers(draft: &RequestDraft) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();

    for row in draft.headers.iter().filter(|r| r.enabled) {
        let name =
            HeaderName::from_bytes(row.key.as_bytes()).map_err(|_| HttpError::InvalidHeader {
                name: row.key.clone(),
            })?;
        let value = HeaderValue::from_str(&row.value).map_err(|_| HttpError::InvalidHeader {
            name: row.key.clone(),
        })?;
        // `append` rather than `insert`: a request may legitimately carry the same header
        // twice, and an API client should not quietly collapse that.
        headers.append(name, value);
    }

    let mut cookies: Vec<String> = draft
        .cookies
        .iter()
        .filter(|c| c.enabled)
        .map(|c| format!("{}={}", c.key, c.value))
        .collect();

    match &draft.auth {
        AuthConfig::Bearer { token } => {
            let value = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| {
                HttpError::InvalidHeader {
                    name: "Authorization".into(),
                }
            })?;
            headers.insert(AUTHORIZATION, value);
        }
        AuthConfig::Basic { username, password } => {
            let encoded =
                base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"));
            let value = HeaderValue::from_str(&format!("Basic {encoded}")).map_err(|_| {
                HttpError::InvalidHeader {
                    name: "Authorization".into(),
                }
            })?;
            headers.insert(AUTHORIZATION, value);
        }
        AuthConfig::ApiKey {
            key,
            value,
            location,
        } => match location {
            rl_model::ApiKeyLocation::Header => {
                let name = HeaderName::from_bytes(key.as_bytes())
                    .map_err(|_| HttpError::InvalidHeader { name: key.clone() })?;
                let value = HeaderValue::from_str(value)
                    .map_err(|_| HttpError::InvalidHeader { name: key.clone() })?;
                headers.insert(name, value);
            }
            rl_model::ApiKeyLocation::Cookie => cookies.push(format!("{key}={value}")),
            // Applied to the URL in `build_url`.
            rl_model::ApiKeyLocation::Query => {}
        },
        AuthConfig::None | AuthConfig::Inherit => {}
    }

    if !cookies.is_empty() {
        let value =
            HeaderValue::from_str(&cookies.join("; ")).map_err(|_| HttpError::InvalidHeader {
                name: "Cookie".into(),
            })?;
        headers.insert(COOKIE, value);
    }

    Ok(headers)
}

fn apply_body(
    request: reqwest::RequestBuilder,
    body: &BodyValue,
) -> Result<reqwest::RequestBuilder> {
    Ok(match body {
        BodyValue::None => request,
        BodyValue::Json { content } => request
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(content.clone()),
        BodyValue::Text {
            content,
            content_type,
        } => request
            .header(reqwest::header::CONTENT_TYPE, content_type.clone())
            .body(content.clone()),
        BodyValue::Form { fields } => {
            let pairs: Vec<(&str, &str)> = fields
                .iter()
                .filter(|f| f.enabled)
                .map(|f| (f.key.as_str(), f.value.as_str()))
                .collect();
            request.form(&pairs)
        }
        BodyValue::Binary { path, content_type } => {
            let bytes = std::fs::read(path).map_err(|source| HttpError::BodyFile {
                path: path.display().to_string(),
                source,
            })?;
            let request = match content_type {
                Some(ct) => request.header(reqwest::header::CONTENT_TYPE, ct.clone()),
                None => request,
            };
            request.body(bytes)
        }
        BodyValue::Multipart { parts } => {
            let mut form = reqwest::multipart::Form::new();
            for part in parts {
                match part {
                    FormPart::Text {
                        name,
                        value,
                        enabled,
                    } if *enabled => {
                        form = form.text(name.clone(), value.clone());
                    }
                    FormPart::File {
                        name,
                        path,
                        content_type,
                        enabled,
                    } if *enabled => {
                        let bytes = std::fs::read(path).map_err(|source| HttpError::BodyFile {
                            path: path.display().to_string(),
                            source,
                        })?;
                        let file_name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "file".to_string());
                        let mut p = reqwest::multipart::Part::bytes(bytes).file_name(file_name);
                        if let Some(ct) = content_type {
                            p = p
                                .mime_str(ct)
                                .map_err(|source| HttpError::Send { source })?;
                        }
                        form = form.part(name.clone(), p);
                    }
                    _ => {}
                }
            }
            request.multipart(form)
        }
    })
}

pub(crate) fn header_pairs(headers: &HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                value.to_str().unwrap_or("<binary>").to_string(),
            )
        })
        .collect()
}

fn preview_of_bytes(bytes: &[u8]) -> String {
    let slice = &bytes[..bytes.len().min(MAX_REQUEST_PREVIEW)];
    String::from_utf8_lossy(slice).into_owned()
}

fn same_origin(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rl_model::{HttpMethod, KeyValue};

    fn draft(url: &str) -> RequestDraft {
        RequestDraft::new(HttpMethod::Get, url)
    }

    #[test]
    fn query_rows_are_appended_and_disabled_ones_skipped() {
        let mut d = draft("https://api.example.com/users");
        d.query.push(KeyValue::new("page", "2"));
        d.query.push(KeyValue::new("debug", "1").disabled());

        let url = build_url(&d).unwrap();
        assert_eq!(url.query(), Some("page=2"));
    }

    #[test]
    fn existing_query_in_the_url_is_preserved() {
        let mut d = draft("https://api.example.com/users?sort=name");
        d.query.push(KeyValue::new("page", "2"));

        let url = build_url(&d).unwrap();
        let q = url.query().unwrap();
        assert!(q.contains("sort=name"));
        assert!(q.contains("page=2"));
    }

    #[test]
    fn a_url_with_no_query_does_not_gain_a_bare_question_mark() {
        let url = build_url(&draft("https://api.example.com/users")).unwrap();
        assert_eq!(url.as_str(), "https://api.example.com/users");
    }

    #[test]
    fn query_values_are_percent_encoded() {
        let mut d = draft("https://api.example.com/search");
        d.query.push(KeyValue::new("q", "a b&c=d"));

        let url = build_url(&d).unwrap();
        assert_eq!(url.query(), Some("q=a+b%26c%3Dd"));
    }

    #[test]
    fn an_unfilled_path_parameter_is_refused_with_the_placeholder_named() {
        let d = draft("https://api.example.com/users/{user_id}");
        match build_url(&d) {
            Err(HttpError::UnfilledPlaceholder { placeholder }) => {
                assert_eq!(placeholder, "{user_id}");
            }
            other => panic!("expected an unfilled placeholder error, got {other:?}"),
        }
    }

    #[test]
    fn a_filled_path_parameter_passes() {
        let mut d = draft("https://api.example.com/users/{user_id}");
        d.path_values.insert("user_id".into(), "42".into());
        let url = build_url(&d).unwrap();
        assert_eq!(url.path(), "/users/42");
    }

    #[test]
    fn a_malformed_url_names_itself_in_the_error() {
        let d = draft("not a url");
        assert!(matches!(build_url(&d), Err(HttpError::InvalidUrl { .. })));
    }

    #[test]
    fn bearer_auth_becomes_an_authorization_header() {
        let mut d = draft("https://x.test/");
        d.auth = AuthConfig::Bearer {
            token: "abc123".into(),
        };
        let headers = build_headers(&d).unwrap();
        assert_eq!(headers[AUTHORIZATION], "Bearer abc123");
    }

    #[test]
    fn basic_auth_is_base64_encoded() {
        let mut d = draft("https://x.test/");
        d.auth = AuthConfig::Basic {
            username: "aladdin".into(),
            password: "opensesame".into(),
        };
        let headers = build_headers(&d).unwrap();
        assert_eq!(headers[AUTHORIZATION], "Basic YWxhZGRpbjpvcGVuc2VzYW1l");
    }

    #[test]
    fn an_api_key_can_travel_in_a_header_a_query_or_a_cookie() {
        let mut d = draft("https://x.test/");

        d.auth = AuthConfig::ApiKey {
            key: "X-API-Key".into(),
            value: "k".into(),
            location: rl_model::ApiKeyLocation::Header,
        };
        assert_eq!(build_headers(&d).unwrap()["x-api-key"], "k");

        d.auth = AuthConfig::ApiKey {
            key: "api_key".into(),
            value: "k".into(),
            location: rl_model::ApiKeyLocation::Query,
        };
        assert_eq!(build_url(&d).unwrap().query(), Some("api_key=k"));

        d.auth = AuthConfig::ApiKey {
            key: "session".into(),
            value: "k".into(),
            location: rl_model::ApiKeyLocation::Cookie,
        };
        assert_eq!(build_headers(&d).unwrap()[COOKIE], "session=k");
    }

    #[test]
    fn duplicate_headers_are_kept_rather_than_collapsed() {
        let mut d = draft("https://x.test/");
        d.headers.push(KeyValue::new("X-Trace", "one"));
        d.headers.push(KeyValue::new("X-Trace", "two"));

        let headers = build_headers(&d).unwrap();
        let values: Vec<_> = headers.get_all("x-trace").iter().collect();
        assert_eq!(values.len(), 2);
    }

    #[test]
    fn disabled_headers_and_cookies_are_omitted() {
        let mut d = draft("https://x.test/");
        d.headers.push(KeyValue::new("X-Off", "no").disabled());
        d.cookies.push(KeyValue::new("c", "no").disabled());

        let headers = build_headers(&d).unwrap();
        assert!(!headers.contains_key("x-off"));
        assert!(!headers.contains_key(COOKIE));
    }

    #[test]
    fn cookies_are_joined_into_one_header() {
        let mut d = draft("https://x.test/");
        d.cookies.push(KeyValue::new("a", "1"));
        d.cookies.push(KeyValue::new("b", "2"));
        assert_eq!(build_headers(&d).unwrap()[COOKIE], "a=1; b=2");
    }

    #[test]
    fn an_invalid_header_name_is_reported_not_dropped() {
        let mut d = draft("https://x.test/");
        d.headers.push(KeyValue::new("bad header", "x"));
        assert!(matches!(
            build_headers(&d),
            Err(HttpError::InvalidHeader { .. })
        ));
    }

    #[test]
    fn origin_comparison_covers_scheme_host_and_port() {
        let a = Url::parse("https://api.example.com/x").unwrap();
        assert!(same_origin(
            &a,
            &Url::parse("https://api.example.com/y").unwrap()
        ));
        assert!(!same_origin(
            &a,
            &Url::parse("http://api.example.com/y").unwrap()
        ));
        assert!(!same_origin(
            &a,
            &Url::parse("https://evil.example/y").unwrap()
        ));
        assert!(!same_origin(
            &a,
            &Url::parse("https://api.example.com:8443/y").unwrap()
        ));
    }
}
