//! Local LLM reverse proxy.
//!
//! Intercepts Anthropic and OpenAI traffic on loopback, compresses the tool
//! results in each request (every rewrite says so in the result itself), and
//! records the usage the provider reports in the local ledger.

pub(crate) mod anthropic;
#[cfg(test)]
mod auth_tests;
pub(crate) mod ccr;
#[cfg(test)]
mod ccr_robustness_tests;
mod codec;
pub(crate) mod compress;
pub(crate) mod forward;
pub(crate) mod history_prune;
pub(crate) mod openai;
pub(crate) mod openai_responses;
pub(crate) mod tool_kind;
pub(crate) mod tool_output;
#[cfg(test)]
mod upstream_tests;
pub(crate) mod usage;
pub(crate) mod usage_meter;

use std::net::SocketAddr;
use std::sync::Arc;

use crate::core::config::Upstreams;

use axum::{
    Router,
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
    routing::{any, get, post},
};

#[derive(Clone)]
pub struct ProxyState {
    pub client: reqwest::Client,
    pub port: u16,
    /// Live provider upstreams, refreshed from config.toml without a proxy
    /// restart (#449). Read per request via [`ProxyState::openai_upstream`] etc.
    pub upstreams: tokio::sync::watch::Receiver<Arc<Upstreams>>,
}

impl ProxyState {
    /// Consistent snapshot of all upstreams for the current request/response.
    pub fn upstream_snapshot(&self) -> Arc<Upstreams> {
        self.upstreams.borrow().clone()
    }

    /// Current Anthropic upstream (live).
    pub fn anthropic_upstream(&self) -> String {
        self.upstreams.borrow().anthropic.clone()
    }

    /// Current OpenAI upstream (live).
    pub fn openai_upstream(&self) -> String {
        self.upstreams.borrow().openai.clone()
    }
}

/// TCP connect timeout (seconds).
fn connect_timeout_secs() -> u64 {
    15
}

/// Idle read timeout (seconds) between bytes from upstream. Generous by default
/// so long extended-thinking phases (which still emit SSE keepalives) are never
/// cut, while a truly dead connection eventually fails.
fn read_idle_timeout_secs() -> u64 {
    300
}

/// How often (seconds) a running proxy re-reads config.toml for upstream
/// changes.
fn upstream_reload_secs() -> u64 {
    5
}

/// Background task: re-resolves the provider upstreams from config.toml on an
/// interval and publishes any change to the live request handlers (#449). Ends
/// once every receiver (the proxy itself) has been dropped.
///
/// `Config::load()` already keeps an internal content-hash cache, so re-reading
/// an unchanged `config.toml` skips the TOML parse + merge and costs only a small
/// file read; combined with the relaxed default interval (#453) the idle steady
/// state is negligible without needing a separate stat pre-check.
fn spawn_upstream_refresh(tx: tokio::sync::watch::Sender<Arc<Upstreams>>, initial: Upstreams) {
    let interval = std::time::Duration::from_secs(upstream_reload_secs());
    tokio::spawn(async move {
        let mut last = initial;
        loop {
            tokio::time::sleep(interval).await;
            let next = crate::core::config::Config::load()
                .proxy
                .refresh_upstreams(&last);
            if next != last {
                log_upstream_change(&last, &next);
                last = next.clone();
                if tx.send(Arc::new(next)).is_err() {
                    break;
                }
            }
        }
    });
}

/// One stdout line per changed provider, matching the startup banner style so a
/// running proxy's log shows when (and to what) an upstream switched.
fn log_upstream_change(old: &Upstreams, new: &Upstreams) {
    if old.anthropic != new.anthropic {
        println!("  ↻ Anthropic upstream → {}", new.anthropic);
    }
    if old.openai != new.openai {
        println!("  ↻ OpenAI upstream → {}", new.openai);
    }
}

pub async fn start_proxy(port: u16) -> anyhow::Result<()> {
    let token = crate::core::session_token::resolve_proxy_token("LEAN_CTX_PROXY_TOKEN");
    start_proxy_with_token(port, Some(token)).await
}

/// Security invariant: the proxy NEVER runs unauthenticated. `None` does not
/// mean "no auth" — it means "resolve the session token for me". Provider
/// routes additionally accept provider API keys (see `proxy_auth_guard`), so
/// IDE clients keep working without any setup.
fn effective_auth_token(auth_token: Option<String>) -> String {
    auth_token
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| crate::core::session_token::resolve_proxy_token("LEAN_CTX_PROXY_TOKEN"))
}

pub async fn start_proxy_with_token(port: u16, auth_token: Option<String>) -> anyhow::Result<()> {
    use crate::core::config::{Config, is_local_proxy_url};

    let auth_token = effective_auth_token(auth_token);

    // A single total timeout aborts long streaming generations (e.g. Opus doing
    // a big refactor) mid-response. Use a connect timeout plus a read (idle)
    // timeout instead: a genuinely hung upstream still fails, but a slow-but-
    // alive stream is never cut off. Both are configurable for edge networks.
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(connect_timeout_secs()))
        .read_timeout(std::time::Duration::from_secs(read_idle_timeout_secs()))
        .build()?;

    let cfg = Config::load();
    // Read once at startup — avoids a Config::load() on every proxied request.
    // The proxy serves this machine only: it always binds loopback.
    let bind_host = std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);
    let initial = cfg.proxy.resolve_all();

    // The proxy reads its upstreams live from a watch channel: a background task
    // re-resolves them from config.toml on an interval and publishes any change,
    // so `lean-ctx config set proxy.upstreams.<id>` (or any config.toml edit) takes
    // effect on the running proxy within seconds, without a restart (#449).
    let (upstream_tx, upstream_rx) = tokio::sync::watch::channel(Arc::new(initial.clone()));
    spawn_upstream_refresh(upstream_tx, initial.clone());

    let Upstreams {
        anthropic: anthropic_upstream,
        openai: openai_upstream,
    } = initial;

    let state = ProxyState {
        client,
        port,
        upstreams: upstream_rx,
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/status", get(status_handler))
        .route("/v1/messages", any(anthropic::handler))
        .route("/v1/messages/{*rest}", any(anthropic::handler))
        .route("/v1/chat/completions", any(openai::handler))
        .route(
            "/v1/responses",
            post(openai_responses::handler).get(responses_websocket_refused),
        )
        .route("/v1/responses/{*rest}", any(openai_responses::handler))
        .route("/v1/models", get(models_handler))
        .route("/v1/models/{*rest}", get(models_handler))
        // Bare provider endpoints (no `/v1` prefix). Clients whose base URL points
        // at the proxy root — notably OpenCode via `@ai-sdk/openai`, whose
        // Responses-API requests hit `/responses` — dispatch here. The
        // `normalize_provider_path` layer rewrites the URI to its canonical
        // `/v1/...` form before the handler forwards upstream (#353).
        .route("/messages", any(anthropic::handler))
        .route("/messages/{*rest}", any(anthropic::handler))
        .route("/chat/completions", any(openai::handler))
        .route(
            "/responses",
            post(openai_responses::handler).get(responses_websocket_refused),
        )
        .route("/responses/{*rest}", any(openai_responses::handler))
        .route("/models", get(models_handler))
        .route("/models/{*rest}", get(models_handler))
        .fallback(fallback);

    let mut app = app
        .layer(axum::middleware::from_fn(host_guard))
        .with_state(state);

    {
        let expected = auth_token.clone();
        app = app.layer(axum::middleware::from_fn(move |req, next| {
            let expected = expected.clone();
            proxy_auth_guard(req, next, expected)
        }));
    }

    // Outermost layer (runs first): normalize bare provider endpoints to their
    // canonical `/v1/...` form so auth, routing and upstream forwarding all agree,
    // regardless of whether the client's base URL includes `/v1` (#353).
    app = app.layer(axum::middleware::from_fn(normalize_provider_path));

    let addr = SocketAddr::from((bind_host, port));
    println!("lean-ctx proxy listening on http://{addr} (token auth enabled)");
    println!("  Anthropic: POST /v1/messages → {anthropic_upstream}");
    println!("  OpenAI:    POST /v1/chat/completions → {openai_upstream}");
    println!(
        "  OpenAI:    POST /v1/responses → {openai_upstream}  (bare /responses also accepted)"
    );
    if openai_upstream.starts_with("http://") && !is_local_proxy_url(&openai_upstream) {
        println!(
            "  ⚠ OpenAI upstream is plaintext HTTP to a non-loopback host \
             — use only on a trusted local network"
        );
    }

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    println!("lean-ctx proxy shut down cleanly.");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    {
        // Fall back to Ctrl-C only if the SIGTERM handler cannot be installed,
        // rather than panicking the proxy on startup.
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sigterm) => {
                tokio::select! {
                    _ = ctrl_c => {},
                    _ = sigterm.recv() => {},
                }
            }
            Err(e) => {
                tracing::warn!("lean-ctx proxy: SIGTERM handler unavailable ({e}); Ctrl-C only");
                ctrl_c.await.ok();
            }
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await.ok();
    }

    println!("lean-ctx proxy: received shutdown signal, draining…");
}

async fn health() -> impl IntoResponse {
    let body = serde_json::json!({
        "status": "ok",
        "service": "lean-ctx-proxy",
        "pid": std::process::id(),
    });
    (StatusCode::OK, axum::Json(body))
}

/// `GET /v1/models` and `/v1/models/{id}`: forwarded unchanged to the
/// provider the client speaks. Anthropic clients send `anthropic-version`;
/// every other client is taken to speak OpenAI.
async fn models_handler(
    State(state): State<ProxyState>,
    req: Request<Body>,
) -> Result<Response, StatusCode> {
    let (upstream, provider) = if req.headers().contains_key("anthropic-version") {
        (state.anthropic_upstream(), usage::Provider::Anthropic)
    } else {
        (state.openai_upstream(), usage::Provider::OpenAi)
    };
    forward::forward_request(
        State(state),
        req,
        &upstream,
        "/v1/models",
        |parsed, size| (serde_json::to_vec(&parsed).unwrap_or_default(), size, size),
        provider,
    )
    .await
}

/// `GET /v1/responses`: the WebSocket transport Codex tries first. A 426
/// makes Codex fall back to HTTP for the rest of the session at once, where a
/// 404 or 405 costs seconds of retries and a 400 fails the turn.
async fn responses_websocket_refused() -> StatusCode {
    StatusCode::UPGRADE_REQUIRED
}

async fn status_handler(State(state): State<ProxyState>) -> impl IntoResponse {
    // Live upstreams the proxy is forwarding to right now (#449). This is the
    // single source of truth for "where is my traffic actually going" — it
    // reflects config.toml hot-reloads and any start-time env override.
    let up = state.upstream_snapshot();

    // Usage (calls, tokens, cost, savings) is in the usage ledger, which
    // `lean-ctx gain` reads; this endpoint only says where traffic goes.
    let body = serde_json::json!({
        "status": "running",
        "port": state.port,
        "upstreams": {
            "anthropic": up.anthropic.clone(),
            "openai": up.openai.clone(),
        },
    });
    (StatusCode::OK, axum::Json(body))
}

#[allow(clippy::result_large_err)]
async fn proxy_auth_guard(
    req: axum::extract::Request,
    next: axum::middleware::Next,
    expected_token: String,
) -> Result<Response, Response> {
    let path = req.uri().path();
    if path == "/health" {
        return Ok(next.run(req).await);
    }

    let bearer = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|auth| auth.strip_prefix("Bearer "))
        .map(str::to_string);

    if let Some(token) = bearer.as_deref()
        && constant_time_eq(token.as_bytes(), expected_token.as_bytes())
    {
        return Ok(next.run(req).await);
    }

    // A provider API key on a provider route is enough: a local AI tool
    // authenticates with its own key, without the lean-ctx token.
    if has_provider_api_key(&req) && is_provider_route(path) {
        return Ok(next.run(req).await);
    }

    Err(auth_error_response(path))
}

fn auth_error_response(path: &str) -> Response {
    let hint = if is_provider_route(path) {
        "Provider route requires authentication. Set your API key header or use: lean-ctx proxy token"
    } else {
        "This endpoint requires a lean-ctx Bearer token. Get it with: lean-ctx proxy token"
    };

    let body = serde_json::json!({
        "type": "error",
        "error": {
            "type": "authentication_error",
            "message": format!("401 Unauthorized — {hint}")
        }
    });

    (StatusCode::UNAUTHORIZED, axum::Json(body)).into_response()
}

fn has_provider_api_key(req: &axum::extract::Request) -> bool {
    let headers = req.headers();
    // Provider-specific key headers: Anthropic `x-api-key`, Azure `api-key`.
    // Any non-empty value authenticates.
    for key in ["x-api-key", "api-key"] {
        if headers
            .get(key)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| !v.trim().is_empty())
        {
            return true;
        }
    }
    // OpenAI-style `Authorization` auth. Accept ANY non-empty credential, not
    // just `Bearer sk-`/`gsk_`: OpenAI-*compatible* providers driven through
    // OpenCode/Codex (Azure, OpenRouter, Groq, vLLM/Ollama gateways, project &
    // service-account keys) issue keys that don't carry those prefixes. The proxy
    // binds to loopback only and never injects upstream credentials — it forwards
    // this header verbatim, so an invalid key is rejected by the real upstream,
    // never silently honoured. Gating provider routes on key *shape* only ever
    // produced false 401s for those clients (#362).
    if let Some(auth) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        let auth = auth.trim();
        let credential = auth
            .strip_prefix("Bearer ")
            .or_else(|| auth.strip_prefix("bearer "))
            .unwrap_or(auth)
            .trim();
        // Reject an empty value or a bare scheme keyword carrying no token.
        return !credential.is_empty() && !credential.eq_ignore_ascii_case("bearer");
    }
    false
}

fn is_provider_route(path: &str) -> bool {
    path.starts_with("/v1/")
        || path.starts_with("/chat/completions")
        || path.starts_with("/responses")
        || path.starts_with("/messages")
        || path == "/models"
        || path.starts_with("/models/")
}

/// Maps a bare provider endpoint to its canonical `/v1/...` form, preserving any
/// sub-path. Returns `None` when the path is already canonical or not a known
/// provider endpoint.
///
/// Some OpenAI-compatible clients treat the configured base URL as the API root
/// and append the bare endpoint, so they send `POST /responses` or
/// `/chat/completions` instead of `/v1/responses` — notably OpenCode via
/// `@ai-sdk/openai`, whose Responses-API requests land on `/responses`. The proxy
/// and every upstream only know the `/v1/...` paths, so an un-prefixed request
/// would 401 (not a provider route) and then 404 (no handler). (#353)
fn canonical_provider_path(path: &str) -> Option<String> {
    // Inverse case of the bare-endpoint rewrite below: the OpenAI base URL sobre
    // writes includes `/v1` (#366), so a client that treats the base URL as an
    // origin and appends `/v1/...` itself produces `/v1/v1/...`.
    if let Some(rest) = path.strip_prefix("/v1/v1/") {
        return Some(format!("/v1/{rest}"));
    }
    const BARE_TO_CANONICAL: &[(&str, &str, &str)] = &[
        ("/responses", "/v1/responses", "/responses/"),
        (
            "/chat/completions",
            "/v1/chat/completions",
            "/chat/completions/",
        ),
        ("/messages", "/v1/messages", "/messages/"),
        ("/models", "/v1/models", "/models/"),
    ];
    for (bare, canonical, bare_with_slash) in BARE_TO_CANONICAL {
        if path == *bare {
            return Some((*canonical).to_string());
        }
        if let Some(rest) = path.strip_prefix(bare_with_slash) {
            return Some(format!("{canonical}/{rest}"));
        }
    }
    None
}

/// Returns the canonicalized URI for a bare provider endpoint (query preserved),
/// or `None` when no rewrite is needed. Pure, so the rewrite is unit-testable
/// without constructing axum middleware plumbing.
fn normalized_provider_uri(uri: &axum::http::Uri) -> Option<axum::http::Uri> {
    let canonical = canonical_provider_path(uri.path())?;
    let new_path_and_query = match uri.query() {
        Some(q) => format!("{canonical}?{q}"),
        None => canonical,
    };
    new_path_and_query.parse::<axum::http::Uri>().ok()
}

/// Rewrites the request URI in place when it targets a bare provider endpoint, so
/// downstream auth (`is_provider_route`), routing and upstream forwarding all see
/// the canonical `/v1/...` path. (#353)
async fn normalize_provider_path(
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    if let Some(uri) = normalized_provider_uri(req.uri()) {
        *req.uri_mut() = uri;
    }
    next.run(req).await
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    if a.len() != b.len() {
        return false;
    }
    bool::from(a.ct_eq(b))
}

async fn host_guard(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<Response, StatusCode> {
    if let Some(host) = req.headers().get("host").and_then(|v| v.to_str().ok())
        && host_allowed(host)
    {
        return Ok(next.run(req).await);
    }
    Err(StatusCode::FORBIDDEN)
}

/// DNS-rebinding guard: only loopback Host headers pass. Matching is
/// case-insensitive on the host with the port stripped.
fn host_allowed(host_header: &str) -> bool {
    // `[::1]:8080` carries the port after the bracket; plain hosts after `:`.
    let host = host_header.trim();
    let h = if let Some(bracketed) = host.strip_prefix('[') {
        bracketed
            .split(']')
            .next()
            .map(|inner| format!("[{inner}]"))
    } else {
        host.split(':').next().map(str::to_string)
    };
    let Some(h) = h else {
        return false;
    };
    let h = h.trim_end_matches('.').to_ascii_lowercase();
    matches!(h.as_str(), "127.0.0.1" | "localhost" | "[::1]")
}

async fn fallback(req: Request<Body>) -> Response {
    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    eprintln!("lean-ctx proxy: unmatched {method} {path}");
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::from(format!(
            "lean-ctx proxy: no handler for {method} {path}"
        )))
        .expect("BUG: building 404 response should never fail")
}
