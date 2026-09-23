//! End-to-end tests against a real socket.
//!
//! A mock at the `reqwest` layer would not prove the things that actually matter here —
//! whether an `Authorization` header survives a redirect, what arrives on the wire when two
//! headers share a name — so these run a small HTTP server and inspect what it received.

use rl_http::{HttpEngine, HttpError};
use rl_model::{AuthConfig, BodyValue, HttpMethod, KeyValue, RequestDraft};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// One request as the server saw it.
#[derive(Debug, Clone)]
struct Received {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Received {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn header_count(&self, name: &str) -> usize {
        self.headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case(name))
            .count()
    }
}

type Handler = Arc<dyn Fn(&Received) -> String + Send + Sync>;

struct TestServer {
    addr: SocketAddr,
    received: Arc<Mutex<Vec<Received>>>,
}

impl TestServer {
    async fn start<F>(handler: F) -> TestServer
    where
        F: Fn(&Received) -> String + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let received = Arc::new(Mutex::new(Vec::new()));

        let handler: Handler = Arc::new(handler);
        let log = Arc::clone(&received);

        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let handler = Arc::clone(&handler);
                let log = Arc::clone(&log);

                tokio::spawn(async move {
                    let Some(request) = read_request(&mut socket).await else {
                        return;
                    };
                    let response = handler(&request);
                    log.lock().unwrap().push(request);
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.flush().await;
                });
            }
        });

        TestServer { addr, received }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    fn requests(&self) -> Vec<Received> {
        self.received.lock().unwrap().clone()
    }

    fn last(&self) -> Received {
        self.requests()
            .last()
            .cloned()
            .expect("no request received")
    }
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<Received> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];

    // Head first.
    let head_end = loop {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
            break pos;
        }
    };

    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.lines();
    let request_line = lines.next()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();

    let headers: Vec<(String, String)> = lines
        .filter_map(|line| {
            let (k, v) = line.split_once(':')?;
            Some((k.trim().to_string(), v.trim().to_string()))
        })
        .collect();

    let content_length: usize = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);

    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < content_length {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }

    Some(Received {
        method,
        target,
        headers,
        body: String::from_utf8_lossy(&body).to_string(),
    })
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn respond(status: u16, reason: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn redirect(status: u16, location: &str) -> String {
    format!(
        "HTTP/1.1 {status} Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
}

fn get(url: String) -> RequestDraft {
    RequestDraft::new(HttpMethod::Get, url)
}

// --- the basics -------------------------------------------------------------------------

#[tokio::test]
async fn a_request_returns_status_headers_and_body() {
    let server = TestServer::start(|_| respond(200, "OK", r#"{"ok":true}"#)).await;
    let engine = HttpEngine::new();

    let exchange = engine.execute(&get(server.url("/users"))).await.unwrap();

    assert_eq!(exchange.response.status, 200);
    assert_eq!(exchange.response.status_text, "OK");
    assert_eq!(exchange.response.body.as_text().unwrap(), r#"{"ok":true}"#);
    assert_eq!(
        exchange.response.header("content-type"),
        Some("application/json")
    );
    assert!(exchange.response.is_success());
}

#[tokio::test]
async fn query_parameters_and_headers_reach_the_server() {
    let server = TestServer::start(|_| respond(200, "OK", "{}")).await;
    let engine = HttpEngine::new();

    let mut draft = get(server.url("/search"));
    draft.query.push(KeyValue::new("q", "hello world"));
    draft.query.push(KeyValue::new("skip", "1").disabled());
    draft.headers.push(KeyValue::new("X-Custom", "value"));

    engine.execute(&draft).await.unwrap();

    let received = server.last();
    assert!(received.target.contains("q=hello+world"));
    assert!(
        !received.target.contains("skip"),
        "disabled rows must not be sent"
    );
    assert_eq!(received.header("x-custom"), Some("value"));
}

#[tokio::test]
async fn a_json_body_is_sent_with_its_content_type() {
    let server = TestServer::start(|_| respond(201, "Created", "{}")).await;
    let engine = HttpEngine::new();

    let mut draft = RequestDraft::new(HttpMethod::Post, server.url("/users"));
    draft.body = BodyValue::Json {
        content: r#"{"name":"Aryan"}"#.into(),
    };

    let exchange = engine.execute(&draft).await.unwrap();

    let received = server.last();
    assert_eq!(received.method, "POST");
    assert_eq!(received.body, r#"{"name":"Aryan"}"#);
    assert_eq!(received.header("content-type"), Some("application/json"));
    assert_eq!(exchange.request.body_size, 16);
}

#[tokio::test]
async fn duplicate_headers_arrive_twice_rather_than_collapsed() {
    let server = TestServer::start(|_| respond(200, "OK", "{}")).await;
    let engine = HttpEngine::new();

    let mut draft = get(server.url("/"));
    draft.headers.push(KeyValue::new("X-Trace", "one"));
    draft.headers.push(KeyValue::new("X-Trace", "two"));

    engine.execute(&draft).await.unwrap();

    assert_eq!(server.last().header_count("x-trace"), 2);
}

#[tokio::test]
async fn timing_is_recorded() {
    let server = TestServer::start(|_| respond(200, "OK", "{}")).await;
    let engine = HttpEngine::new();

    let exchange = engine.execute(&get(server.url("/"))).await.unwrap();

    assert!(exchange.response.timing.total_ms >= exchange.response.timing.ttfb_ms);
}

#[tokio::test]
async fn an_error_status_is_returned_rather_than_raised() {
    let server = TestServer::start(|_| respond(404, "Not Found", r#"{"detail":"nope"}"#)).await;
    let engine = HttpEngine::new();

    // A 404 is an answer, not a failure. The engine reports it; it does not error.
    let exchange = engine.execute(&get(server.url("/missing"))).await.unwrap();

    assert_eq!(exchange.response.status, 404);
    assert!(exchange.response.is_error());
}

// --- redirects --------------------------------------------------------------------------

#[tokio::test]
async fn redirects_are_not_followed_by_default() {
    let server = TestServer::start(|r| {
        if r.target == "/start" {
            redirect(302, "/end")
        } else {
            respond(200, "OK", r#"{"arrived":true}"#)
        }
    })
    .await;
    let engine = HttpEngine::new();

    let exchange = engine.execute(&get(server.url("/start"))).await.unwrap();

    assert_eq!(exchange.response.status, 302);
    assert!(exchange.response.redirects.is_empty());
    assert_eq!(server.requests().len(), 1, "the second hop must not happen");
}

#[tokio::test]
async fn redirects_are_followed_when_asked_and_the_chain_is_recorded() {
    let server = TestServer::start(|r| match r.target.as_str() {
        "/a" => redirect(302, "/b"),
        "/b" => redirect(302, "/c"),
        _ => respond(200, "OK", r#"{"arrived":true}"#),
    })
    .await;
    let engine = HttpEngine::new();

    let mut draft = get(server.url("/a"));
    draft.settings.follow_redirects = true;

    let exchange = engine.execute(&draft).await.unwrap();

    assert_eq!(exchange.response.status, 200);
    assert_eq!(exchange.response.redirects.len(), 2);
    assert!(exchange.response.redirects[0].to.ends_with("/b"));
    assert!(exchange.response.redirects[1].to.ends_with("/c"));
}

// --- guards -----------------------------------------------------------------------------

#[tokio::test]
async fn a_guard_that_refuses_sends_nothing() {
    let server = TestServer::start(|_| respond(200, "OK", "{}")).await;
    let engine = HttpEngine::new();

    let refused = engine
        .execute_guarded(&get(server.url("/x")), &|method, url| {
            Err(format!("{method} {} is not allowed", url.path()))
        })
        .await;

    match refused {
        Err(HttpError::Refused { reason }) => assert_eq!(reason, "GET /x is not allowed"),
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert!(server.requests().is_empty(), "nothing reached the server");
}

/// The case the guard exists for: an allowed first hop redirecting somewhere that is not.
#[tokio::test]
async fn a_guard_is_asked_again_before_each_redirect() {
    let server = TestServer::start(|r| match r.target.as_str() {
        "/allowed" => redirect(302, "/forbidden"),
        _ => respond(200, "OK", "{}"),
    })
    .await;
    let engine = HttpEngine::new();
    let mut draft = get(server.url("/allowed"));
    draft.settings.follow_redirects = true;

    let refused = engine
        .execute_guarded(&draft, &|_, url| match url.path() {
            "/allowed" => Ok(()),
            other => Err(format!("{other} is off limits")),
        })
        .await;

    assert!(
        matches!(refused, Err(HttpError::Refused { .. })),
        "{refused:?}"
    );
    assert_eq!(
        server.requests().len(),
        1,
        "the redirect target was never requested"
    );
}

#[tokio::test]
async fn a_redirect_loop_stops_at_the_limit() {
    let server = TestServer::start(|_| redirect(302, "/loop")).await;
    let engine = HttpEngine::new();

    let mut draft = get(server.url("/loop"));
    draft.settings.follow_redirects = true;
    draft.settings.max_redirects = 3;

    match engine.execute(&draft).await {
        Err(HttpError::TooManyRedirects { limit }) => assert_eq!(limit, 3),
        other => panic!("expected a redirect limit error, got {other:?}"),
    }
}

#[tokio::test]
async fn a_303_turns_a_post_into_a_get() {
    let server = TestServer::start(|r| {
        if r.target == "/submit" {
            redirect(303, "/result")
        } else {
            respond(200, "OK", "{}")
        }
    })
    .await;
    let engine = HttpEngine::new();

    let mut draft = RequestDraft::new(HttpMethod::Post, server.url("/submit"));
    draft.body = BodyValue::Json {
        content: r#"{"a":1}"#.into(),
    };
    draft.settings.follow_redirects = true;

    engine.execute(&draft).await.unwrap();

    let requests = server.requests();
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[1].method, "GET", "303 must become a GET");
    assert!(requests[1].body.is_empty());
}

// --- the one that matters most ----------------------------------------------------------

#[tokio::test]
async fn credentials_survive_a_same_origin_redirect() {
    let server = TestServer::start(|r| {
        if r.target == "/start" {
            redirect(302, "/end")
        } else {
            respond(200, "OK", "{}")
        }
    })
    .await;
    let engine = HttpEngine::new();

    let mut draft = get(server.url("/start"));
    draft.auth = AuthConfig::Bearer {
        token: "s3cr3t".into(),
    };
    draft.settings.follow_redirects = true;

    let exchange = engine.execute(&draft).await.unwrap();

    let requests = server.requests();
    assert_eq!(requests[1].header("authorization"), Some("Bearer s3cr3t"));
    assert!(!exchange.response.redirects[0].credentials_stripped);
}

#[tokio::test]
async fn credentials_are_dropped_when_a_redirect_crosses_origins() {
    // The receiving server records whatever it is handed, so the test can prove the token
    // never arrived rather than merely that the engine intended not to send it.
    let attacker = TestServer::start(|_| respond(200, "OK", r#"{"captured":true}"#)).await;
    let attacker_url = attacker.url("/collect");

    let origin = TestServer::start(move |_| redirect(302, &attacker_url)).await;
    let engine = HttpEngine::new();

    let mut draft = get(origin.url("/start"));
    draft.auth = AuthConfig::Bearer {
        token: "s3cr3t".into(),
    };
    draft.cookies.push(KeyValue::new("session", "also-secret"));
    draft.settings.follow_redirects = true;

    let exchange = engine.execute(&draft).await.unwrap();

    let leaked = attacker.last();
    assert_eq!(
        leaked.header("authorization"),
        None,
        "a bearer token must never follow a redirect to another origin"
    );
    assert_eq!(leaked.header("cookie"), None, "nor must cookies");

    assert!(exchange.response.redirects[0].credentials_stripped);
}

// --- failure modes ----------------------------------------------------------------------

#[tokio::test]
async fn an_unfilled_path_parameter_fails_before_anything_is_sent() {
    let server = TestServer::start(|_| respond(200, "OK", "{}")).await;
    let engine = HttpEngine::new();

    let draft = get(server.url("/users/{user_id}"));

    match engine.execute(&draft).await {
        Err(HttpError::UnfilledPlaceholder { placeholder }) => {
            assert_eq!(placeholder, "{user_id}");
        }
        other => panic!("expected an unfilled placeholder error, got {other:?}"),
    }
    assert!(
        server.requests().is_empty(),
        "nothing should have been sent"
    );
}

#[tokio::test]
async fn a_connection_failure_is_reported_as_transient() {
    let engine = HttpEngine::new();
    // Port 1 on loopback: nothing is listening.
    let draft = get("http://127.0.0.1:1/".to_string());

    let error = engine.execute(&draft).await.unwrap_err();
    assert!(
        error.is_transient(),
        "a refused connection is worth retrying"
    );
}
