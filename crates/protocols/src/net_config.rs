//! Sprint 12: the unified HTTP client configuration surface — proxy (from
//! Sprint 12's earlier proxy-support commit), DNS mode (`crate::dns`), and
//! the new authentication pieces (custom headers for bearer tokens/API
//! keys, and a raw `Cookie` header for session-cookie auth). One
//! `ClientConfig` -> one `reqwest::Client`, same "one shared client per
//! `Engine`" model `crate::http::build_client_with_proxy` already
//! established — see that function's doc comment, and
//! `sdm_engine::Engine::new_with_proxy`'s, for why per-Job client
//! construction is out of scope here too (this just widens what a single
//! constructed client can be configured with).

use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, Url};

use crate::dns::{self, DnsMode};
use crate::http::{ProtoError, ProxyConfig};

/// A single `name: value` header applied to every request this client
/// makes — the mechanism behind both "Bearer tokens, API keys" (Sprint 12
/// scope) and any other custom header a site's login flow needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthHeader {
    pub name: String,
    pub value: String,
}

impl AuthHeader {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        AuthHeader {
            name: name.into(),
            value: value.into(),
        }
    }

    /// Convenience for the common case: `Authorization: Bearer <token>`.
    pub fn bearer(token: impl Into<String>) -> Self {
        AuthHeader::new("Authorization", format!("Bearer {}", token.into()))
    }
}

/// Everything needed to build one `reqwest::Client`. `Default` is
/// identical to `build_client()`'s current behavior (no proxy, plain DNS,
/// no extra headers/cookies) so existing call sites are unaffected by
/// this type's introduction.
#[derive(Debug, Clone, Default)]
pub struct ClientConfig {
    pub proxy: Option<ProxyConfig>,
    pub dns: DnsMode,
    /// Applied as default headers on every request this client makes —
    /// i.e. per-client (per-Job or per-domain, depending on how the
    /// caller resolved which `AuthHeader`s apply — see
    /// `sdm_storage::auth::resolve_auth_config`), not truly per-domain
    /// within a single client, since `reqwest::Client` has no built-in
    /// per-host default-header routing. A client built with domain-scoped
    /// auth is expected to only be used for requests to that domain.
    pub extra_headers: Vec<AuthHeader>,
    /// Raw `Cookie:` header value (e.g. `"sessionid=abc123; csrftoken=xyz"`)
    /// — either pasted manually or imported from a browser via the
    /// Sprint 11 extension's REST endpoint. Scoped to `cookie_url` (the
    /// origin the cookie should be sent to), matching
    /// `reqwest::cookie::Jar::add_cookie_str`'s URL-scoping requirement.
    pub cookie_header: Option<String>,
    pub cookie_url: Option<Url>,
}

impl ClientConfig {
    pub fn new() -> Self {
        ClientConfig::default()
    }

    pub fn with_proxy(mut self, proxy: ProxyConfig) -> Self {
        self.proxy = Some(proxy);
        self
    }

    pub fn with_dns(mut self, dns: DnsMode) -> Self {
        self.dns = dns;
        self
    }

    pub fn with_header(mut self, header: AuthHeader) -> Self {
        self.extra_headers.push(header);
        self
    }

    /// `url` is any URL on the target site — only its scheme+host+port
    /// (the "origin") is used to scope the cookie.
    pub fn with_cookie(
        mut self,
        cookie_header: impl Into<String>,
        url: &str,
    ) -> Result<Self, ProtoError> {
        let parsed = Url::parse(url).map_err(ProtoError::InvalidCookieUrl)?;
        self.cookie_header = Some(cookie_header.into());
        self.cookie_url = Some(parsed);
        Ok(self)
    }
}

/// Applies the shared timeout/pooling/TLS-session-reuse tuning every
/// client this crate builds should have, regardless of proxy/DNS/auth
/// configuration. Sprint 12 scope: "connection pooling, TLS session
/// resumption tuned in reqwest/hyper client config to cut
/// repeated-handshake overhead across segments/queue items" — segmented
/// downloads open many short-lived connections to the same host in quick
/// succession (one per segment), which is exactly the pattern connection
/// pooling and TLS session resumption are for.
///
/// Happy Eyeballs (RFC 8305) IPv4/IPv6 connection racing is *not*
/// configured here because it doesn't need to be: `hyper-util`'s
/// `HttpConnector` (which every `reqwest::Client` uses under the hood)
/// already races connections across address families by default whenever
/// DNS resolution returns more than one candidate address. This crate's
/// job is just to not get in the way of that — see `crate::dns`'s
/// `DohResolver`, which deliberately returns every address DNS gives it
/// (both families) rather than filtering down to one.
fn tune(builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    builder
        .use_rustls_tls()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(60 * 30))
        // TLS session resumption is a property of the rustls
        // `ClientConfig` reqwest builds internally when `use_rustls_tls()`
        // is set — rustls caches session tickets per `ClientConfig`
        // instance automatically, so the meaningful lever we have here is
        // making sure connections (and therefore sessions) actually get
        // reused instead of torn down between segments:
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(16)
        .tcp_keepalive(Duration::from_secs(60))
        .tcp_nodelay(true)
}

/// Build a `reqwest::Client` from a full [`ClientConfig`]. This is the
/// one real client constructor in this crate now — `build_client` and
/// `build_client_with_proxy` (kept for backward compatibility with their
/// existing call sites) both delegate to this with the rest of
/// `ClientConfig` left at its default.
pub fn build_client_with_config(cfg: &ClientConfig) -> Result<Client, ProtoError> {
    let mut builder = tune(Client::builder());

    if let Some(proxy) = &cfg.proxy {
        builder = builder.proxy(proxy.to_reqwest_proxy()?);
    }

    if let DnsMode::Doh(provider) = &cfg.dns {
        builder = builder.dns_resolver(std::sync::Arc::new(dns::DohResolver::new(*provider)));
    }

    if !cfg.extra_headers.is_empty() {
        let mut headers = HeaderMap::new();
        for h in &cfg.extra_headers {
            let name =
                HeaderName::try_from(h.name.as_str()).map_err(|e| ProtoError::InvalidHeader {
                    name: h.name.clone(),
                    reason: e.to_string(),
                })?;
            let value = HeaderValue::from_str(&h.value).map_err(|e| ProtoError::InvalidHeader {
                name: h.name.clone(),
                reason: e.to_string(),
            })?;
            headers.insert(name, value);
        }
        builder = builder.default_headers(headers);
    }

    if let (Some(cookie_header), Some(url)) = (&cfg.cookie_header, &cfg.cookie_url) {
        let jar = reqwest::cookie::Jar::default();
        jar.add_cookie_str(cookie_header, url);
        builder = builder.cookie_provider(std::sync::Arc::new(jar));
    }

    builder.build().map_err(ProtoError::InvalidProxy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_builds_a_client() {
        let cfg = ClientConfig::default();
        assert!(build_client_with_config(&cfg).is_ok());
    }

    #[test]
    fn bearer_header_has_expected_shape() {
        let h = AuthHeader::bearer("tok123");
        assert_eq!(h.name, "Authorization");
        assert_eq!(h.value, "Bearer tok123");
    }

    #[test]
    fn invalid_header_name_is_rejected_before_building() {
        let cfg = ClientConfig::new().with_header(AuthHeader::new("bad header name", "v"));
        let err = build_client_with_config(&cfg).unwrap_err();
        assert!(matches!(err, ProtoError::InvalidHeader { .. }));
    }

    #[test]
    fn invalid_header_value_is_rejected_before_building() {
        // A bare CR is not a legal header value (header-injection guard).
        let cfg = ClientConfig::new().with_header(AuthHeader::new("X-Test", "line1\rline2"));
        let err = build_client_with_config(&cfg).unwrap_err();
        assert!(matches!(err, ProtoError::InvalidHeader { .. }));
    }

    #[test]
    fn cookie_config_requires_a_parseable_url() {
        let err = ClientConfig::new()
            .with_cookie("session=abc", "not a url")
            .unwrap_err();
        assert!(matches!(err, ProtoError::InvalidCookieUrl(_)));
    }

    #[test]
    fn valid_cookie_config_builds_a_client() {
        let cfg = ClientConfig::new()
            .with_cookie("session=abc123", "https://example.com/")
            .unwrap();
        assert!(build_client_with_config(&cfg).is_ok());
    }

    /// Sprint 12 DoD: "a cookie-authenticated download against a test
    /// site requiring a login session succeeds where an unauthenticated
    /// request would 401." This proves the client this module builds
    /// actually attaches the configured `Authorization` header and
    /// `Cookie` on outgoing requests — not just that the builder accepts
    /// the config without error.
    #[tokio::test]
    async fn built_client_sends_configured_bearer_header_and_cookie() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/protected"))
            .and(header("Authorization", "Bearer secret-token"))
            .and(header("Cookie", "session=abc123"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&server)
            .await;
        // Any request missing either credential 401s — proving the
        // 200 above genuinely depended on both being sent, not just
        // reachability.
        Mock::given(method("GET"))
            .and(path("/protected"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let cfg = ClientConfig::new()
            .with_header(AuthHeader::bearer("secret-token"))
            .with_cookie("session=abc123", &server.uri())
            .unwrap();
        let client = build_client_with_config(&cfg).unwrap();

        let resp = client
            .get(format!("{}/protected", server.uri()))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        // Sanity check the negative case with a client that has neither
        // credential configured.
        let bare_client = build_client_with_config(&ClientConfig::default()).unwrap();
        let resp = bare_client
            .get(format!("{}/protected", server.uri()))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 401);
    }
}
