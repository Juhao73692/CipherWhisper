//! Embedded loopback-only browser UI. No private material is injected into assets.
use crate::AppState;
use axum::{
    Json,
    extract::{Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use tokio::sync::Mutex;
use topicairn_protocol::digest;
use zeroize::Zeroizing;
include!(concat!(env!("OUT_DIR"), "/ui_assets.rs"));

#[derive(Default)]
pub(crate) struct BrowserAuth {
    bootstrap: Option<(String, Instant)>,
    session_digest: Option<String>,
}
fn random_token() -> anyhow::Result<Zeroizing<String>> {
    let mut random = Zeroizing::new([0u8; 32]);
    getrandom::fill(random.as_mut())?;
    Ok(Zeroizing::new(hex::encode(random.as_slice())))
}
impl BrowserAuth {
    pub(crate) fn bootstrap(&mut self) -> anyhow::Result<Zeroizing<String>> {
        let code = random_token()?;
        self.bootstrap = Some((
            digest(code.as_bytes()),
            Instant::now() + Duration::from_secs(90),
        ));
        Ok(code)
    }
    fn exchange(&mut self, code: &str) -> anyhow::Result<Option<Zeroizing<String>>> {
        let valid = self.bootstrap.as_ref().is_some_and(|(hash, expires)| {
            Instant::now() < *expires
                && bool::from(digest(code.as_bytes()).as_bytes().ct_eq(hash.as_bytes()))
        });
        if !valid {
            return Ok(None);
        }
        self.bootstrap = None;
        let session = random_token()?;
        self.session_digest = Some(digest(session.as_bytes()));
        Ok(Some(session))
    }
    pub(crate) fn accepts(&self, hash: &str) -> bool {
        self.session_digest
            .as_ref()
            .is_some_and(|stored| bool::from(hash.as_bytes().ct_eq(stored.as_bytes())))
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Unlock {
    code: String,
}
pub(crate) async fn session(State(state): State<AppState>, Json(input): Json<Unlock>) -> Response {
    if input.code.len() != 64 {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match state.browser.lock().await.exchange(&input.code) {
        Ok(Some(token)) => Json(serde_json::json!({"token":token.as_str()})).into_response(),
        Ok(None) => StatusCode::UNAUTHORIZED.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
pub(crate) async fn assets(request: Request) -> Response {
    let path = request.uri().path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    match asset(path) {
        Some((bytes, mime)) => ([(header::CONTENT_TYPE, mime)], bytes).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
fn trusted_request(headers: &HeaderMap, hosts: &[String]) -> bool {
    let Some(host) = headers.get(header::HOST).and_then(|h| h.to_str().ok()) else {
        return false;
    };
    if !hosts.iter().any(|allowed| allowed == host) {
        return false;
    }
    if headers
        .get("sec-fetch-site")
        .is_some_and(|value| value == "cross-site")
    {
        return false;
    }
    if let Some(origin) = headers.get(header::ORIGIN)
        && origin.to_str().ok() != Some(format!("http://{host}").as_str())
    {
        return false;
    }
    true
}
pub(crate) async fn guard(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let mut response = if trusted_request(request.headers(), &state.ui_hosts) {
        next.run(request).await
    } else {
        (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error":"local same-origin requests only"})),
        )
            .into_response()
    };
    let headers = response.headers_mut();
    // Shiki and KaTeX require generated inline styles. Scripts/resources remain local.
    headers.insert(header::CONTENT_SECURITY_POLICY, "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; font-src 'self'; connect-src 'self'; img-src 'none'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'".parse().unwrap());
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    headers.insert(header::X_FRAME_OPTIONS, "DENY".parse().unwrap());
    headers.insert(header::REFERRER_POLICY, "no-referrer".parse().unwrap());
    headers.insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
}
pub(crate) fn auth_state() -> Arc<Mutex<BrowserAuth>> {
    Arc::new(Mutex::new(BrowserAuth::default()))
}
pub(crate) fn open(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let status = std::process::Command::new("open").arg(url).status()?;
    #[cfg(target_os = "linux")]
    let status = std::process::Command::new("xdg-open").arg(url).status()?;
    #[cfg(target_os = "windows")]
    let status = std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other("browser launcher failed"))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bootstrap_is_one_use_and_separate_from_session() {
        let mut auth = BrowserAuth::default();
        let code = auth.bootstrap().unwrap();
        assert!(!auth.accepts(&digest(code.as_bytes())));
        assert!(auth.exchange(&"f".repeat(64)).unwrap().is_none());
        let token = auth.exchange(&code).unwrap().unwrap();
        assert!(auth.accepts(&digest(token.as_bytes())));
        assert!(auth.exchange(&code).unwrap().is_none());
        auth.bootstrap = Some((
            digest(code.as_bytes()),
            Instant::now() - Duration::from_secs(1),
        ));
        assert!(auth.exchange(&code).unwrap().is_none());
        assert!(!BrowserAuth::default().accepts(&digest(token.as_bytes())));
    }
    #[test]
    fn reject_cross_site_and_dns_rebinding() {
        let hosts = vec!["127.0.0.1:8790".to_owned(), "localhost:8790".to_owned()];
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "127.0.0.1:8790".parse().unwrap());
        assert!(trusted_request(&headers, &hosts));
        headers.insert(header::ORIGIN, "http://evil.example".parse().unwrap());
        assert!(!trusted_request(&headers, &hosts));
        headers.insert(header::ORIGIN, "http://127.0.0.1:8790".parse().unwrap());
        assert!(trusted_request(&headers, &hosts));
        headers.insert(header::HOST, "evil.example:8790".parse().unwrap());
        assert!(!trusted_request(&headers, &hosts));
        headers.insert(header::HOST, "127.0.0.1:8790".parse().unwrap());
        headers.insert("sec-fetch-site", "cross-site".parse().unwrap());
        assert!(!trusted_request(&headers, &hosts));
    }
    #[test]
    fn only_embedded_assets_are_public() {
        assert!(asset("index.html").is_some());
        assert!(asset("../admin.token").is_none());
        assert!(asset("identity").is_none());
        assert!(!String::from_utf8_lossy(asset("index.html").unwrap().0).contains("Bearer"));
    }
}
