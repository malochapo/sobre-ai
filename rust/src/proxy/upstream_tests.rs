//! Split from `proxy/mod.rs` (#660 LOC gate): `upstream_tests`.

use super::*;

fn upstreams_with_openai(openai: &str) -> Upstreams {
    Upstreams {
        anthropic: "https://api.anthropic.com".into(),
        openai: openai.into(),
    }
}

/// The #449 core wiring: provider handlers read the upstream per request from
/// the watch channel, so a published change is served immediately, without
/// rebuilding the `ProxyState`.
#[tokio::test]
async fn proxy_state_reads_upstream_live_from_watch() {
    let (tx, rx) =
        tokio::sync::watch::channel(Arc::new(upstreams_with_openai("https://old.example")));
    let state = ProxyState {
        client: reqwest::Client::new(),
        port: 0,
        upstreams: rx,
    };
    assert_eq!(state.openai_upstream(), "https://old.example");

    tx.send(Arc::new(upstreams_with_openai("https://new.example")))
        .unwrap();
    assert_eq!(
        state.openai_upstream(),
        "https://new.example",
        "a live handler read must reflect the published change"
    );
    assert_eq!(state.upstream_snapshot().openai, "https://new.example");
}

/// End-to-end #449 repro (in-process, no network): a `config set`-style edit
/// to config.toml is picked up by a *running* proxy's refresh task within the
/// reload interval — without any restart. Before the fix this value stayed
/// frozen at the start-time upstream forever.
///
/// The process-global env lock is intentionally held across the polling
/// `.await`s to keep the data dir isolated for the whole test; safe because
/// each `#[tokio::test]` owns its current-thread runtime, so this std guard
/// only makes *other* test threads wait — it can never deadlock this one.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn config_change_is_picked_up_live_without_restart() {
    use crate::core::config::Config;

    let _lock = crate::core::data_dir::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    crate::test_env::set_var("LEAN_CTX_DATA_DIR", tmp.path());

    // Start state: config.toml points OpenAI at a loopback upstream.
    Config::update_global(|c| {
        c.proxy
            .upstreams
            .insert("openai".into(), "http://127.0.0.1:19101".into());
    })
    .unwrap();
    let initial = Config::load().proxy.resolve_all();
    assert_eq!(initial.openai, "http://127.0.0.1:19101");

    let (tx, rx) = tokio::sync::watch::channel(Arc::new(initial.clone()));
    spawn_upstream_refresh(tx, initial);

    // `lean-ctx config set proxy.upstreams.openai …` (same safe write path).
    Config::update_global(|c| {
        c.proxy
            .upstreams
            .insert("openai".into(), "http://127.0.0.1:19102".into());
    })
    .unwrap();

    // Poll the live value the handlers would read — no restart in between.
    let mut live = rx.borrow().openai.clone();
    for _ in 0..80 {
        if live == "http://127.0.0.1:19102" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        live = rx.borrow().openai.clone();
    }
    assert_eq!(
        live, "http://127.0.0.1:19102",
        "running proxy must serve the new config.toml upstream without a restart"
    );

    crate::test_env::remove_var("LEAN_CTX_DATA_DIR");
}

/// A loopback upstream that answers every request with its name, the path it
/// was asked for and the credential it received.
async fn named_upstream(name: &'static str) -> String {
    let upstream = Router::new().fallback(move |req: Request<Body>| async move {
        let credential = ["x-api-key", "authorization"]
            .iter()
            .find_map(|h| req.headers().get(*h)?.to_str().ok())
            .unwrap_or("none")
            .to_string();
        format!("{name} {} {credential}", req.uri().path())
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, upstream).await });
    format!("http://{addr}")
}

/// `GET /models` and `/v1/models/{id}` are provider routes: normalised to
/// `/v1/...`, let through on the provider key, and passed through unchanged
/// to the upstream the client speaks (`anthropic-version` → Anthropic, any
/// other client → OpenAI). Never a 404, never a local catalog.
#[tokio::test]
async fn models_route_goes_to_the_provider_the_client_speaks() {
    let upstreams = Upstreams {
        anthropic: named_upstream("anthropic").await,
        openai: named_upstream("openai").await,
    };
    let (_tx, rx) = tokio::sync::watch::channel(Arc::new(upstreams));
    let state = ProxyState {
        client: reqwest::Client::new(),
        port: 0,
        upstreams: rx,
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app(state, "proxy-token".into())).await });

    let client = reqwest::Client::new();
    for (path, anthropic, expected) in [
        ("/models", false, "openai /v1/models Bearer sk-test"),
        (
            "/v1/models/gpt-5",
            false,
            "openai /v1/models/gpt-5 Bearer sk-test",
        ),
        ("/models", true, "anthropic /v1/models sk-ant-test"),
        (
            "/v1/models/claude-sonnet-4-5",
            true,
            "anthropic /v1/models/claude-sonnet-4-5 sk-ant-test",
        ),
    ] {
        let req = client.get(format!("{base}{path}"));
        let req = if anthropic {
            req.header("x-api-key", "sk-ant-test")
                .header("anthropic-version", "2023-06-01")
        } else {
            req.bearer_auth("sk-test")
        };
        let resp = req.send().await.unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::OK, "GET {path}");
        assert_eq!(resp.text().await.unwrap(), expected, "GET {path}");
    }
}
