//! Authenticated direct delivery listener; independently services incoming calls
//! while the center is waiting for an outgoing peer request.
use anyhow::{Result, ensure};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, OriginalUri, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use std::{
    net::TcpListener,
    sync::{Arc, Mutex},
};
use topicairn_core::direct::DirectIngress;
use topicairn_protocol::{p2p::*, *};
type Shared = Arc<Mutex<DirectIngress>>;
struct Error(String);
impl From<anyhow::Error> for Error {
    fn from(e: anyhow::Error) -> Self {
        Self(e.to_string())
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error":self.0})),
        )
            .into_response()
    }
}
fn authenticate(
    ingress: &mut DirectIngress,
    headers: &HeaderMap,
    method: &str,
    path: &str,
    body: &[u8],
) -> Result<(String, String)> {
    ensure!(
        !headers.contains_key("origin"),
        "browser requests forbidden"
    );
    let value = |key: &str| -> Result<String> {
        Ok(headers
            .get(key)
            .ok_or_else(|| anyhow::anyhow!("missing peer authentication"))?
            .to_str()?
            .into())
    };
    let auth = PeerAuth {
        signing_key: value("x-peer-key")?,
        target: value("x-peer-target")?,
        timestamp: value("x-peer-time")?.parse()?,
        nonce: value("x-peer-nonce")?,
        signature: value("x-peer-signature")?,
    };
    let peer = ingress.authenticate(&auth, method, path, body)?;
    Ok((peer, auth.nonce))
}
fn auth_error() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({"error":"peer authentication failed"})),
    )
        .into_response()
}
async fn ping(
    State(shared): State<Shared>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    body: Bytes,
) -> Response {
    let mut ingress = shared.lock().unwrap();
    let Ok((peer, nonce)) = authenticate(&mut ingress, &headers, "POST", &uri.to_string(), &body)
    else {
        return auth_error();
    };
    let reply = (|| -> Result<_> {
        ensure!(body.is_empty(), "ping body must be empty");
        ingress.response(&peer, &nonce, ingress.identity())
    })();
    reply.map(Json).map_err(Error::from).into_response()
}
async fn claim(
    State(shared): State<Shared>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    body: Bytes,
) -> Response {
    let mut ingress = shared.lock().unwrap();
    let Ok((peer, nonce)) = authenticate(&mut ingress, &headers, "POST", &uri.to_string(), &body)
    else {
        return auth_error();
    };
    let reply = (|| -> Result<_> {
        ensure!(body.is_empty(), "claim body must be empty");
        let bundle = ingress.claim()?;
        ingress.response(&peer, &nonce, bundle)
    })();
    reply.map(Json).map_err(Error::from).into_response()
}
async fn send(
    State(shared): State<Shared>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    body: Bytes,
) -> Response {
    let mut ingress = shared.lock().unwrap();
    let Ok((peer, nonce)) = authenticate(&mut ingress, &headers, "POST", &uri.to_string(), &body)
    else {
        return auth_error();
    };
    let reply = (|| -> Result<_> {
        let env = serde_json::from_slice(&body)?;
        let receipt = ingress.enqueue(&peer, env)?;
        ingress.response(&peer, &nonce, receipt)
    })();
    reply.map(Json).map_err(Error::from).into_response()
}
async fn delivery(
    State(shared): State<Shared>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    Path(id): Path<String>,
) -> Response {
    let mut ingress = shared.lock().unwrap();
    let Ok((peer, nonce)) = authenticate(&mut ingress, &headers, "GET", &uri.to_string(), b"")
    else {
        return auth_error();
    };
    let reply = (|| -> Result<_> {
        let receipt = ingress.delivery(&peer, &id)?;
        ingress.response(&peer, &nonce, receipt)
    })();
    reply.map(Json).map_err(Error::from).into_response()
}
pub(crate) fn router(ingress: DirectIngress) -> Router {
    Router::new()
        .route("/p2p/v1/ping", post(ping))
        .route("/p2p/v1/prekeys/claim", post(claim))
        .route("/p2p/v1/messages", post(send))
        .route("/p2p/v1/messages/{id}", get(delivery))
        .layer(DefaultBodyLimit::max(MAX_ENVELOPE * 8))
        .with_state(Arc::new(Mutex::new(ingress)))
}
pub(crate) struct PeerServer {
    pub listener: TcpListener,
    pub tls: Option<axum_server::tls_rustls::RustlsConfig>,
    pub ingress: DirectIngress,
}
impl PeerServer {
    pub fn start(
        self,
        mut stop: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<Result<()>> {
        tokio::spawn(async move {
            let app = router(self.ingress);
            if let Some(tls) = self.tls {
                let handle = axum_server::Handle::new();
                let signal = handle.clone();
                let shutdown = tokio::spawn(async move {
                    let _ = stop.changed().await;
                    signal.graceful_shutdown(Some(std::time::Duration::from_secs(5)));
                });
                let result = axum_server::tls_rustls::from_tcp_rustls(self.listener, tls)?
                    .handle(handle)
                    .serve(app.into_make_service())
                    .await;
                shutdown.abort();
                result?;
            } else {
                axum::serve(tokio::net::TcpListener::from_std(self.listener)?, app)
                    .with_graceful_shutdown(async move {
                        let _ = stop.changed().await;
                    })
                    .await?;
            }
            Ok(())
        })
    }
}
