//! `docs/api.md` §1 and §3 — the token on the ingress, and the control
//! vocabulary over HTTP.
//!
//! Both halves are real: a real axum router, a real reqwest client. Nothing
//! reaches the network, and no turn is ever served — every assertion here is
//! about who gets past the door.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use pretty_assertions::assert_eq;
use proxenos::auth::store::AccountStore;
use proxenos::auth::store::FileStore;
use proxenos::control::handler::ControlState;
use proxenos::ingress::Access;
use proxenos::ingress::AppState;
use proxenos::ingress::ModelMapping;
use proxenos::ingress::serving_router;
use proxenos::upstream::http::HttpTransport;
use serde_json::Value;
use serde_json::json;
use std::sync::Arc;

const TOKEN: &str = "a-long-random-string";

struct Harness {
    base: String,
    client: reqwest::Client,
    /// The §3 socket, serving the SAME `ControlState` the HTTP endpoint does —
    /// the same `Arc`s, not a second daemon that happens to agree. Parity
    /// asserted against two copies of the state would only prove the two
    /// copies matched.
    socket: std::path::PathBuf,
    _dir: tempfile::TempDir,
}

impl Harness {
    /// A daemon on a loopback port, with or without a token configured.
    async fn start(token: Option<&str>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(FileStore::new(dir.path().join("credentials.json")));
        let switches = Arc::new(proxenos::recorder::Switches::default());
        let usage = Arc::new(proxenos::usage::UsageStore::default());
        let refusals = Arc::new(proxenos::auth::refusals::Refusals::default());
        let policy = Arc::new(proxenos::policy::Policy::new(
            proxenos::policy::Snapshot::routing_only(
                vec![ModelMapping {
                    requested: "claude-sonnet-5".to_owned(),
                    upstream: "gpt-5.6-terra".to_owned(),
                    account: None,
                    missing: None,
                }],
                None,
            ),
        ));
        let catalog = Arc::new(proxenos::catalog::CatalogSource::fixed(
            proxenos::catalog::Catalog::fallback(),
        ));

        let state = AppState {
            policy: Arc::clone(&policy),
            catalog: Arc::clone(&catalog),
            // Never reached: nothing here serves a turn, and no test may touch
            // the network.
            transport: Arc::new(HttpTransport::new("http://127.0.0.1:1")),
            conduits: None,
            recorder: None,
            capture: Arc::clone(&switches),
            usage: Arc::clone(&usage),
            refusals: Arc::clone(&refusals),
            instructions: Arc::new(proxenos::config::InstructionsConfig {
                identity: false,
                append: None,
                working_budget: false,
            }),
            sessions: Arc::new(proxenos::session::SessionStore::new()),
            relay: None,
        };

        let control = ControlState {
            port: 8787,
            policy,
            catalog,
            credentials: Arc::clone(&store) as Arc<dyn AccountStore>,
            capture: switches,
            usage,
            refusals,
            config: Arc::new(proxenos::config::Config::default()),
            shutdown: Arc::new(proxenos::daemon::Shutdown::default()),
            tokens: None,
            usage_endpoint: String::new(),
            anthropic_usage_endpoint: String::new(),
            anthropic_profile_endpoint: String::new(),
            sessions: Arc::new(proxenos::session::SessionStore::new()),
            config_path: Some(dir.path().join("config.toml")),
        };

        let socket = dir.path().join("control.sock");
        let over_socket = control.clone();
        let path = socket.clone();
        tokio::spawn(async move {
            let _ = proxenos::control::serve(&path, over_socket).await;
        });
        for _ in 0..100 {
            if socket.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let router = serving_router(
            state,
            Access {
                token: token.map(str::to_owned),
                control: Some(control),
            },
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = proxenos::daemon::serve_router(listener, router).await;
        });

        Self {
            base: format!("http://{addr}"),
            client: reqwest::Client::new(),
            socket,
            _dir: dir,
        }
    }

    async fn get(&self, path: &str, auth: Option<&str>) -> reqwest::Response {
        let mut request = self.client.get(format!("{}{path}", self.base));
        if let Some(auth) = auth {
            request = request.bearer_auth(auth);
        }
        request.send().await.expect("the request should arrive")
    }

    async fn control(&self, method: &str, auth: Option<&str>) -> reqwest::Response {
        let mut request = self
            .client
            .post(format!("{}/control", self.base))
            .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": method }));
        if let Some(auth) = auth {
            request = request.bearer_auth(auth);
        }
        request.send().await.expect("the request should arrive")
    }
}

/// A configured token is demanded of every turn-surface request, and the
/// refusal is an Anthropic error shape — a bare 401 from a middleware is not
/// something the client's retry logic can act on (§1.1).
#[tokio::test]
async fn the_ingress_refuses_a_request_with_no_token() {
    let harness = Harness::start(Some(TOKEN)).await;

    let response = harness.get("/v1/models", None).await;

    assert_eq!(response.status(), 401);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["type"], "error");
    assert_eq!(body["error"]["type"], "authentication_error");
}

/// And of a request carrying the wrong one.
#[tokio::test]
async fn the_ingress_refuses_the_wrong_token() {
    let harness = Harness::start(Some(TOKEN)).await;

    let response = harness
        .get("/v1/models", Some("proxenos-token:not-the-secret"))
        .await;

    assert_eq!(response.status(), 401);
}

/// A value that is a bare account tag carries no token, so it is refused too.
/// The tag is a name and was never a credential.
#[tokio::test]
async fn an_account_tag_alone_is_not_a_token() {
    let harness = Harness::start(Some(TOKEN)).await;

    let response = harness
        .get("/v1/models", Some("proxenos-account:work"))
        .await;

    assert_eq!(response.status(), 401);
}

/// The token gets in, and so does the token beside an account tag — the two
/// travel in the one header the client offers.
#[tokio::test]
async fn the_token_is_accepted_alone_and_beside_an_account_tag() {
    let harness = Harness::start(Some(TOKEN)).await;

    let alone = harness
        .get("/v1/models", Some(&format!("proxenos-token:{TOKEN}")))
        .await;
    assert_eq!(alone.status(), 200);

    let tagged = harness
        .get(
            "/v1/models",
            Some(&format!("proxenos-token:{TOKEN} proxenos-account:work")),
        )
        .await;
    assert_eq!(tagged.status(), 200);

    let reversed = harness
        .get(
            "/v1/models",
            Some(&format!("proxenos-account:work proxenos-token:{TOKEN}")),
        )
        .await;
    assert_eq!(reversed.status(), 200);
}

/// A daemon with no token configured is the posture this project shipped with,
/// and it is untouched: nothing is demanded, and a value the client sent is
/// ignored exactly as before.
#[tokio::test]
async fn a_daemon_with_no_token_demands_nothing() {
    let harness = Harness::start(None).await;

    assert_eq!(harness.get("/v1/models", None).await.status(), 200);
    assert_eq!(
        harness.get("/v1/models", Some("unused")).await.status(),
        200
    );
}

/// §3 — the control vocabulary answers over HTTP, in the same JSON-RPC shape
/// the socket uses.
#[tokio::test]
async fn control_answers_over_http() {
    let harness = Harness::start(Some(TOKEN)).await;

    let response = harness
        .control("status", Some(&format!("proxenos-token:{TOKEN}")))
        .await;

    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["jsonrpc"], "2.0");
    assert_eq!(body["result"]["port"], 8787);
}

/// The control endpoint is behind the same token as the turn surface. It
/// carries `accounts.remove` and `accounts.select`; an open one would be worse
/// than an open ingress.
#[tokio::test]
async fn control_over_http_refuses_a_request_with_no_token() {
    let harness = Harness::start(Some(TOKEN)).await;

    let response = harness.control("status", None).await;

    assert_eq!(response.status(), 401);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["type"], "authentication_error");
}

/// An unknown method keeps its JSON-RPC code over HTTP, because "this daemon
/// does not have that method" is the one distinction a caller acts on (§6).
#[tokio::test]
async fn an_unknown_method_keeps_its_code_over_http() {
    let harness = Harness::start(Some(TOKEN)).await;

    let body: Value = harness
        .control(
            "definitely.not.a.method",
            Some(&format!("proxenos-token:{TOKEN}")),
        )
        .await
        .json()
        .await
        .unwrap();

    assert_eq!(body["error"]["code"], -32601);
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("definitely.not.a.method"),
        "{body}"
    );
}

/// A daemon with no token serves the control vocabulary to loopback callers.
/// It cannot be reached from anywhere else — `resolve_listen` refuses that
/// combination — and the endpoint states the rule rather than relying on it.
#[tokio::test]
async fn control_over_http_serves_a_loopback_caller_with_no_token() {
    let harness = Harness::start(None).await;

    let response = harness.control("status", None).await;

    assert_eq!(response.status(), 200);
}

/// The token never appears in what the daemon says about itself. `status` is
/// what a front-end prints and what an operator pastes into an issue.
#[tokio::test]
async fn the_token_is_absent_from_status_and_env() {
    let harness = Harness::start(Some(TOKEN)).await;
    let auth = format!("proxenos-token:{TOKEN}");

    for method in ["status", "env", "tiers", "accounts", "usage"] {
        let body = harness
            .control(method, Some(&auth))
            .await
            .text()
            .await
            .unwrap();
        assert!(!body.contains(TOKEN), "`{method}` leaked the token: {body}");
    }
}

/// The tag grammar, as a pure function. A value with no token part is read
/// exactly as it was before tokens existed — including one holding a space,
/// which splitting would have silently truncated.
#[test]
fn a_value_with_no_token_is_read_as_it_always_was() {
    use proxenos::ingress::parse_tags;

    assert_eq!(parse_tags("unused").account, None);
    assert_eq!(
        parse_tags("proxenos-account:work").account.as_deref(),
        Some("work")
    );
    assert_eq!(
        parse_tags("Bearer proxenos-account:my account")
            .account
            .as_deref(),
        Some("my account")
    );
    assert_eq!(parse_tags("proxenos-account:work").token, None);
}

/// A value that announces a token is read as parts, in either order.
#[test]
fn a_value_with_a_token_is_read_as_parts() {
    use proxenos::ingress::parse_tags;

    let tags = parse_tags("Bearer proxenos-token:secret proxenos-account:work");
    assert_eq!(tags.token.as_deref(), Some("secret"));
    assert_eq!(tags.account.as_deref(), Some("work"));

    let reversed = parse_tags("proxenos-account:work proxenos-token:secret");
    assert_eq!(reversed.token.as_deref(), Some("secret"));
    assert_eq!(reversed.account.as_deref(), Some("work"));
}

/// One place builds the value, so the launcher and the parser cannot disagree
/// about the separator.
#[test]
fn the_launch_value_round_trips_through_the_parser() {
    use proxenos::ingress::auth_token_value;
    use proxenos::ingress::parse_tags;

    assert_eq!(auth_token_value(None, None), "unused");

    let both = auth_token_value(Some("secret"), Some("work"));
    let tags = parse_tags(&both);
    assert_eq!(tags.token.as_deref(), Some("secret"));
    assert_eq!(tags.account.as_deref(), Some("work"));

    // The shape a launch has always produced, unchanged where there is no
    // token: an older daemon reads it the way it always did.
    assert_eq!(
        auth_token_value(None, Some("work")),
        "proxenos-account:work"
    );
}

/// §3 — every documented method answers identically over both transports.
///
/// The state is shared, so this is not two daemons agreeing: it is one
/// daemon's vocabulary reached two ways. `shutdown` is left out because it
/// releases the run loop, and asking twice is not a question with one answer.
#[tokio::test]
async fn every_method_answers_the_same_over_both_transports() {
    use proxenos::control::protocol::METHODS;

    let harness = Harness::start(None).await;

    for method in METHODS {
        if method == "shutdown" {
            continue;
        }

        let over_socket = proxenos::control::call(&harness.socket, method, None).await;
        let over_http = proxenos::control::call_http(&harness.base, None, method, None).await;

        match (over_socket, over_http) {
            (Ok(socket), Ok(http)) => assert_eq!(socket, http, "`{method}` differs by transport"),
            (Err(socket), Err(http)) => {
                assert_eq!(
                    (socket.message, socket.status),
                    (http.message, http.status),
                    "`{method}` fails differently by transport"
                );
            }
            (socket, http) => panic!(
                "`{method}` succeeded on one transport and failed on the other: \
                 socket {socket:?}, http {http:?}"
            ),
        }
    }
}

/// The setters move the daemon over HTTP, not merely answer about it — the
/// same policy the ingress routes turns from.
#[tokio::test]
async fn a_setter_over_http_moves_the_running_daemon() {
    let harness = Harness::start(Some(TOKEN)).await;

    let answer = proxenos::control::call_http(
        &harness.base,
        Some(TOKEN),
        "effort.set",
        Some(json!({ "effort": "low" })),
    )
    .await
    .expect("the setter should answer");
    assert_eq!(answer["effort"], "low");

    let status = proxenos::control::call_http(&harness.base, Some(TOKEN), "status", None)
        .await
        .expect("status should answer");
    assert_eq!(status["effort_ceiling"], "low");
}
