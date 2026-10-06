use std::time::Duration;

/// Shared TLS config for every ureq client so OS/enterprise root CAs are honored
/// (#643). ureq's default `RootCerts::WebPki` ignores the system store, so requests
/// fail with `UnknownIssuer` behind TLS-intercepting corporate proxies. Inject this
/// into a `ureq::config::Config::builder().tls_config(platform_tls_config())` at each
/// call site — ureq's builder scope typestate is private, so a shared *builder*
/// cannot be returned from a function; the shared piece is this `TlsConfig`.
pub fn platform_tls_config() -> ureq::tls::TlsConfig {
    ureq::tls::TlsConfig::builder()
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .build()
}

/// Agent that honors platform roots and bounds only the connection-setup phases
/// (DNS/connect/first-byte) — a large but progressing download stays uncapped.
pub fn ureq_agent_with_timeouts(
    timeout_resolve: Option<Duration>,
    timeout_connect: Option<Duration>,
    timeout_recv_response: Option<Duration>,
) -> ureq::Agent {
    ureq::config::Config::builder()
        .tls_config(platform_tls_config())
        .timeout_resolve(timeout_resolve)
        .timeout_connect(timeout_connect)
        .timeout_recv_response(timeout_recv_response)
        .build()
        .into()
}
