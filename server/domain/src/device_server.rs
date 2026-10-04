//! Dedicated HTTPS interface for enrolled devices. Never exposes admin/UI routes.
use crate::{ApiError, AppState};
use anyhow::{Result, ensure};
use axum::{
    Extension, Json, Router,
    body::{Body, to_bytes},
    extract::{DefaultBodyLimit, Query, Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use cipherwhisper_protocol::{MAX_BODY, device::*};
use serde::Deserialize;
use std::{path::Path, sync::Arc};
use zeroize::Zeroizing;
#[derive(Clone)]
pub(crate) struct DeviceConfig {
    pub server: String,
    pub ca_pem: String,
}
#[derive(Clone)]
struct Authorized {
    id: String,
    nonce: String,
}
fn denied() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({"error":"device authentication failed"})),
    )
        .into_response()
}
async fn authenticate(State(state): State<AppState>, request: Request, next: Next) -> Response {
    if request.headers().contains_key(header::ORIGIN) {
        return denied();
    }
    let (mut parts, body) = request.into_parts();
    let header = |key: &str| {
        parts
            .headers
            .get(key)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    let auth = (|| -> Option<DeviceAuth> {
        Some(DeviceAuth {
            signing_key: header("x-device-key")?,
            domain_id: header("x-device-domain")?,
            timestamp: header("x-device-time")?.parse().ok()?,
            nonce: header("x-device-nonce")?,
            signature: header("x-device-signature")?,
        })
    })();
    let Some(auth) = auth else {
        return denied();
    };
    let body = match to_bytes(body, MAX_BODY * 8).await {
        Ok(body) => body,
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    let path = parts.uri.path_and_query().map_or("/", |p| p.as_str());
    let id = match state
        .domain
        .lock()
        .await
        .center()
        .and_then(|e| e.authenticate_device(&auth, parts.method.as_str(), path, &body))
    {
        Ok(id) => id,
        Err(_) => return denied(),
    };
    parts.extensions.insert(Authorized {
        id,
        nonce: auth.nonce,
    });
    let mut response = next.run(Request::from_parts(parts, Body::from(body))).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pull {
    epoch: String,
    cursor: i64,
    limit: Option<usize>,
}
async fn changes(
    State(state): State<AppState>,
    Extension(auth): Extension<Authorized>,
    Query(query): Query<Pull>,
) -> Result<Json<SignedResponse<Page>>, ApiError> {
    let mut workspace = state.domain.lock().await;
    let center = workspace.center()?;
    let page = center.device_page(
        &auth.id,
        &query.epoch,
        query.cursor,
        query.limit.unwrap_or(PAGE_LIMIT),
    )?;
    Ok(Json(center.sign_device_response(
        &auth.id,
        &auth.nonce,
        page,
    )?))
}
async fn ack(
    State(state): State<AppState>,
    Extension(auth): Extension<Authorized>,
    Json(ack): Json<Ack>,
) -> Result<Json<SignedResponse<Ack>>, ApiError> {
    let mut w = state.domain.lock().await;
    let e = w.center()?;
    let ack = e.device_ack(&auth.id, &ack)?;
    Ok(Json(e.sign_device_response(&auth.id, &auth.nonce, ack)?))
}
async fn command(
    State(state): State<AppState>,
    Extension(auth): Extension<Authorized>,
    Json(command): Json<Command>,
) -> Result<Json<SignedResponse<CommandReply>>, ApiError> {
    let mut w = state.domain.lock().await;
    let e = w.center()?;
    let result = e.device_command(&auth.id, command).await?;
    Ok(Json(e.sign_device_response(
        &auth.id,
        &auth.nonce,
        result,
    )?))
}
pub(crate) fn router(state: AppState) -> Router {
    Router::new()
        .route("/device/v1/changes", get(changes))
        .route("/device/v1/ack", post(ack))
        .route("/device/v1/commands", post(command))
        .layer(DefaultBodyLimit::max(MAX_BODY * 8))
        .layer(middleware::from_fn_with_state(state.clone(), authenticate))
        .with_state(state)
}
pub(crate) async fn tls(cert: &Path, key: &Path) -> Result<axum_server::tls_rustls::RustlsConfig> {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
    ensure!(!key.is_symlink(), "device TLS key cannot be a symlink");
    let cert = tokio::fs::read(cert).await?;
    let key = Zeroizing::new(tokio::fs::read(key).await?);
    let certs =
        CertificateDer::pem_slice_iter(&cert).collect::<std::result::Result<Vec<_>, _>>()?;
    let key = PrivateKeyDer::from_pem_slice(&key)?;
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])?
    .with_no_client_auth()
    .with_single_cert(certs, key)?;
    Ok(axum_server::tls_rustls::RustlsConfig::from_config(
        Arc::new(config),
    ))
}
