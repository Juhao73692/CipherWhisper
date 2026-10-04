//! Shared loopback API/UI for a center or internal device client.
use anyhow::{Result, ensure};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use cipherwhisper_core::{Endpoint, SyncReport};
use cipherwhisper_protocol::*;
use clap::Parser;
use serde::Deserialize;
use std::{
    io::Write,
    path::{Path as FsPath, PathBuf},
    sync::Arc,
    time::Duration,
};
use subtle::ConstantTimeEq;
use tokio::sync::{Mutex, watch};
use zeroize::Zeroizing;
mod client;
mod device_server;
mod peer_server;
mod ui;
mod workspace;
use cipherwhisper_protocol::device::{DeviceCard, Pairing};
pub use client::{ClientArgs, DeviceInitArgs, init_device, run_client};
use workspace::Workspace;

#[derive(Parser)]
#[command(
    about = "CipherWhisper Personal Trust Domain center with a local browser UI",
    version
)]
pub struct DomainArgs {
    #[arg(long, default_value = "domain-data")]
    pub data: PathBuf,
    #[arg(long)]
    pub name: Option<String>,
    /// Optional legacy relay transport. Default is direct P2P, without a relay.
    #[arg(long)]
    pub relay: Option<String>,
    /// Trust this PEM CA for the relay HTTPS connection (does not disable verification).
    #[arg(long, requires = "relay")]
    pub relay_ca: Option<PathBuf>,
    /// Direct peer listener, independent of the local UI and device ports.
    #[arg(long, default_value = "127.0.0.1:8800", conflicts_with = "relay")]
    pub peer_bind: std::net::SocketAddr,
    /// Advertised direct origin; required for remote HTTPS listeners.
    #[arg(long, conflicts_with = "relay")]
    pub peer_url: Option<String>,
    #[arg(long, requires_all=["peer_tls_key","peer_ca"], conflicts_with="relay")]
    pub peer_tls_cert: Option<PathBuf>,
    #[arg(long, requires_all=["peer_tls_cert","peer_ca"], conflicts_with="relay")]
    pub peer_tls_key: Option<PathBuf>,
    #[arg(long, requires_all=["peer_tls_cert","peer_tls_key"], conflicts_with="relay")]
    pub peer_ca: Option<PathBuf>,
    #[arg(long, default_value = "127.0.0.1:8790")]
    pub bind: std::net::SocketAddr,
    #[arg(
        long,
        env = cipherwhisper_core::passphrase_env(),
        hide_env_values = true,
        hide = true
    )]
    pub passphrase: String,
    #[arg(long, default_value = "5")]
    pub sync_seconds: u64,
    /// Open and unlock the embedded UI using a single-use, 90-second local link.
    #[arg(long)]
    pub open: bool,
    /// Dedicated HTTPS listener for enrolled devices; admin/UI stay on --bind.
    #[arg(long, requires_all=["device_tls_cert","device_tls_key","device_ca","device_url"])]
    pub device_bind: Option<std::net::SocketAddr>,
    #[arg(long, requires = "device_bind")]
    pub device_tls_cert: Option<PathBuf>,
    #[arg(long, requires = "device_bind")]
    pub device_tls_key: Option<PathBuf>,
    #[arg(long, requires = "device_bind")]
    pub device_ca: Option<PathBuf>,
    /// HTTPS origin devices will use to reach this center (IP or DNS).
    #[arg(long, requires = "device_bind")]
    pub device_url: Option<String>,
}
#[derive(Clone)]
struct AppState {
    domain: Arc<Mutex<Workspace>>,
    token_digest: Arc<String>,
    last_sync: Arc<Mutex<Option<SyncReport>>>,
    browser: Arc<Mutex<ui::BrowserAuth>>,
    ui_hosts: Arc<Vec<String>>,
    device_config: Option<device_server::DeviceConfig>,
}
struct ApiError(String);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error":self.0})),
        )
            .into_response()
    }
}
impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        Self(e.to_string())
    }
}
type Api<T> = std::result::Result<Json<T>, ApiError>;

fn token(dir: &FsPath) -> Result<Zeroizing<String>> {
    let path = dir.join("admin.token");
    ensure!(!path.is_symlink(), "admin token cannot be a symlink");
    if path.exists() {
        let token = Zeroizing::new(std::fs::read_to_string(path)?);
        ensure!(token.trim().len() == 64, "invalid admin token file");
        return Ok(Zeroizing::new(token.trim().into()));
    }
    let mut random = Zeroizing::new([0u8; 32]);
    getrandom::fill(random.as_mut())?;
    let token = Zeroizing::new(hex::encode(random.as_slice()));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(path)?;
    file.write_all(token.as_bytes())?;
    file.sync_all()?;
    Ok(token)
}
async fn authenticate(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Response {
    let supplied = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "));
    let valid = if let Some(t) = supplied.filter(|t| t.len() == 64) {
        let hash = digest(t.as_bytes());
        bool::from(hash.as_bytes().ct_eq(state.token_digest.as_bytes()))
            || state.browser.lock().await.accepts(&hash)
    } else {
        false
    };
    if !valid {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error":"invalid admin token"})),
        )
            .into_response();
    }
    next.run(request).await
}
async fn identity(State(state): State<AppState>) -> Api<ContactCard> {
    Ok(Json(state.domain.lock().await.contact_card()?))
}
async fn peers(State(state): State<AppState>) -> Api<Vec<ContactCard>> {
    Ok(Json(state.domain.lock().await.peers()?))
}
async fn add_peer(
    State(state): State<AppState>,
    Json(card): Json<ContactCard>,
) -> Api<serde_json::Value> {
    state.domain.lock().await.add_peer(card).await?;
    Ok(Json(serde_json::json!({"ok":true})))
}
#[derive(Deserialize)]
struct PeerFilter {
    peer: Option<String>,
}
async fn topics(
    State(state): State<AppState>,
    Query(filter): Query<PeerFilter>,
) -> Api<Vec<Topic>> {
    Ok(Json(
        state.domain.lock().await.topics(filter.peer.as_deref())?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewTopic {
    peer_id: String,
    title: String,
}
async fn create_topic(State(state): State<AppState>, Json(input): Json<NewTopic>) -> Api<Topic> {
    Ok(Json(
        state
            .domain
            .lock()
            .await
            .create_topic(&input.peer_id, &input.title)
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicUpdate {
    title: String,
    archived: bool,
}
async fn update_topic(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(input): Json<TopicUpdate>,
) -> Api<Topic> {
    let mut domain = state.domain.lock().await;
    domain
        .update_topic(&id, &input.title, input.archived)
        .await?;
    Ok(Json(domain.topic(&id)?))
}
async fn history(State(state): State<AppState>, Path(id): Path<String>) -> Api<Vec<Message>> {
    Ok(Json(state.domain.lock().await.messages(&id)?))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SendMessage {
    body: String,
    reply_to: Option<String>,
}
async fn send(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(input): Json<SendMessage>,
) -> Api<Message> {
    Ok(Json(
        state
            .domain
            .lock()
            .await
            .send_message(&id, &input.body, input.reply_to)
            .await?,
    ))
}
#[derive(Deserialize)]
struct Search {
    q: String,
}
async fn search(State(state): State<AppState>, Query(input): Query<Search>) -> Api<Vec<Message>> {
    Ok(Json(state.domain.lock().await.search(&input.q)?))
}
async fn sync(State(state): State<AppState>) -> Api<SyncReport> {
    let report = state.domain.lock().await.sync(true).await?;
    *state.last_sync.lock().await = Some(report.clone());
    Ok(Json(report))
}
async fn outbox(State(state): State<AppState>) -> Api<serde_json::Value> {
    Ok(Json(
        serde_json::to_value(state.domain.lock().await.outbox()?).map_err(anyhow::Error::from)?,
    ))
}
async fn status(State(state): State<AppState>) -> Json<serde_json::Value> {
    let workspace = state.domain.lock().await;
    Json(
        serde_json::json!({"protocol":VERSION,"mode":workspace.mode(),"transport":workspace.transport_kind(),"device":workspace.device_info().ok().flatten(),"deviceServer":state.device_config.as_ref().map(|c|&c.server),"lastSync":*state.last_sync.lock().await}),
    )
}
async fn peer_profile(
    State(state): State<AppState>,
) -> Api<cipherwhisper_protocol::p2p::PeerProfile> {
    Ok(Json(state.domain.lock().await.center()?.direct_profile()?))
}
async fn peer_routes(State(state): State<AppState>) -> Api<serde_json::Value> {
    Ok(Json(
        serde_json::to_value(state.domain.lock().await.center()?.direct_peers()?)
            .map_err(anyhow::Error::from)?,
    ))
}
async fn import_direct_peer(
    State(state): State<AppState>,
    Json(profile): Json<cipherwhisper_protocol::p2p::PeerProfile>,
) -> Api<serde_json::Value> {
    state
        .domain
        .lock()
        .await
        .center()?
        .add_direct_peer(profile)?;
    Ok(Json(serde_json::json!({"ok":true})))
}
async fn check_direct_peer(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Api<ContactCard> {
    Ok(Json(
        state
            .domain
            .lock()
            .await
            .center()?
            .check_direct_peer(&id)
            .await?,
    ))
}
async fn devices(State(state): State<AppState>) -> Api<serde_json::Value> {
    let mut w = state.domain.lock().await;
    let records = if w.mode() == "server" {
        w.devices()?
    } else {
        vec![]
    };
    Ok(Json(
        serde_json::json!({"enabled":state.device_config.is_some(),"devices":records}),
    ))
}
async fn enroll(State(state): State<AppState>, Json(card): Json<DeviceCard>) -> Api<Pairing> {
    let config = state.device_config.as_ref().ok_or_else(|| {
        anyhow::anyhow!("start the center with --device-bind and its TLS configuration first")
    })?;
    Ok(Json(state.domain.lock().await.center()?.authorize_device(
        card,
        &config.server,
        &config.ca_pem,
    )?))
}
async fn revoke(State(state): State<AppState>, Path(id): Path<String>) -> Api<serde_json::Value> {
    state.domain.lock().await.center()?.revoke_device(&id)?;
    Ok(Json(serde_json::json!({"ok":true})))
}
async fn device_pending(State(state): State<AppState>) -> Api<serde_json::Value> {
    Ok(Json(
        serde_json::to_value(state.domain.lock().await.pending()?).map_err(anyhow::Error::from)?,
    ))
}
async fn discard(State(state): State<AppState>, Path(id): Path<String>) -> Api<serde_json::Value> {
    state.domain.lock().await.discard(&id)?;
    Ok(Json(serde_json::json!({"ok":true})))
}
struct Remote {
    config: device_server::DeviceConfig,
    listener: std::net::TcpListener,
    tls: axum_server::tls_rustls::RustlsConfig,
}
pub async fn run(args: DomainArgs) -> Result<()> {
    let remote = if let Some(bind) = args.device_bind {
        let ca = std::fs::read(args.device_ca.as_ref().unwrap())?;
        ensure!(ca.len() <= 256 * 1024, "device CA too large");
        let ca_pem = String::from_utf8(ca)?;
        let server = args
            .device_url
            .as_ref()
            .unwrap()
            .trim_end_matches('/')
            .to_owned();
        cipherwhisper_core::device::validate_server(&server, &ca_pem)?;
        let tls = device_server::tls(
            args.device_tls_cert.as_ref().unwrap(),
            args.device_tls_key.as_ref().unwrap(),
        )
        .await?;
        let listener = std::net::TcpListener::bind(bind)?;
        listener.set_nonblocking(true)?;
        Some(Remote {
            config: device_server::DeviceConfig { server, ca_pem },
            listener,
            tls,
        })
    } else {
        None
    };
    let passphrase = Zeroizing::new(args.passphrase);
    let mut domain = if let Some(relay) = &args.relay {
        Endpoint::open_with_ca(
            &args.data,
            &passphrase,
            args.name.as_deref(),
            relay,
            args.relay_ca.as_deref(),
        )?
    } else {
        Endpoint::open_direct(&args.data, &passphrase, args.name.as_deref())?
    };
    let peer_server = if domain.is_direct() {
        ensure!(
            args.peer_bind.ip().is_loopback() || args.peer_tls_cert.is_some(),
            "remote P2P listener requires TLS"
        );
        let listener = std::net::TcpListener::bind(args.peer_bind)?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let url = args.peer_url.unwrap_or_else(|| format!("http://{address}"));
        let (tls, ca) = if let Some(cert) = args.peer_tls_cert {
            ensure!(
                url.starts_with("https://"),
                "TLS peer listener requires --peer-url https://..."
            );
            (
                Some(device_server::tls(&cert, args.peer_tls_key.as_ref().unwrap()).await?),
                Some(std::fs::read_to_string(args.peer_ca.as_ref().unwrap())?),
            )
        } else {
            ensure!(
                url.starts_with("http://"),
                "HTTPS peer URL requires TLS credentials"
            );
            (None, None)
        };
        domain.set_direct_address(&url, ca.as_deref())?;
        println!("Direct P2P: {url}");
        Some(peer_server::PeerServer {
            listener,
            tls,
            ingress: domain.direct_ingress()?,
        })
    } else {
        None
    };
    drop(passphrase);
    serve_workspace(
        args.data,
        args.bind,
        args.sync_seconds,
        args.open,
        Workspace::Center(domain),
        remote,
        peer_server,
    )
    .await
}
async fn serve_workspace(
    data: PathBuf,
    bind: std::net::SocketAddr,
    sync_seconds: u64,
    open: bool,
    domain: Workspace,
    remote: Option<Remote>,
    peer_server: Option<peer_server::PeerServer>,
) -> Result<()> {
    ensure!(
        bind.ip().is_loopback(),
        "management API/UI are loopback only; use the separate HTTPS device listener"
    );
    ensure!(
        (1..=300).contains(&sync_seconds),
        "sync interval must be 1..300 seconds"
    );
    let token = token(&data)?;
    let listener = tokio::net::TcpListener::bind(bind).await?;
    let address = listener.local_addr()?;
    let state = AppState {
        domain: Arc::new(Mutex::new(domain)),
        token_digest: Arc::new(digest(token.as_bytes())),
        last_sync: Arc::new(Mutex::new(None)),
        browser: ui::auth_state(),
        ui_hosts: Arc::new(vec![
            address.to_string(),
            format!("localhost:{}", address.port()),
        ]),
        device_config: remote.as_ref().map(|r| r.config.clone()),
    };
    drop(token);
    let app = Router::new()
        .route("/identity", get(identity))
        .route("/peers", get(peers).post(add_peer))
        .route("/topics", get(topics).post(create_topic))
        .route("/topics/{id}", post(update_topic))
        .route("/topics/{id}/messages", get(history).post(send))
        .route("/search", get(search))
        .route("/sync", post(sync))
        .route("/outbox", get(outbox))
        .route("/status", get(status))
        .route("/p2p/contact", get(peer_profile))
        .route("/p2p/peers", get(peer_routes).post(import_direct_peer))
        .route("/p2p/peers/{id}/check", post(check_direct_peer))
        .route("/devices", get(devices).post(enroll))
        .route("/devices/{id}/revoke", post(revoke))
        .route("/device-pending", get(device_pending))
        .route("/device-pending/{id}/discard", post(discard))
        .layer(DefaultBodyLimit::max(MAX_BODY * 8))
        .layer(middleware::from_fn_with_state(state.clone(), authenticate))
        .route("/ui/session", post(ui::session))
        .fallback(get(ui::assets))
        .layer(middleware::from_fn_with_state(state.clone(), ui::guard))
        .with_state(state.clone());
    let handle = remote.as_ref().map(|_| axum_server::Handle::new());
    let remote_task = if let Some(remote) = remote {
        println!(
            "Enrolled-device HTTPS: {} (listener {})",
            remote.config.server,
            remote.listener.local_addr()?
        );
        let app = device_server::router(state.clone());
        let handle = handle.clone().unwrap();
        Some(tokio::spawn(async move {
            axum_server::tls_rustls::from_tcp_rustls(remote.listener, remote.tls)?
                .handle(handle)
                .serve(app.into_make_service())
                .await
        }))
    } else {
        None
    };
    println!("Local UI: http://{address}/");
    println!(
        "Local admin token file: {}",
        data.join("admin.token").display()
    );
    if open {
        let code = state.browser.lock().await.bootstrap()?;
        let url = Zeroizing::new(format!("http://{address}/#bootstrap={}", code.as_str()));
        if let Err(e) = ui::open(&url) {
            eprintln!("Could not open browser: {e}; open the Local UI URL and use admin.token.");
        }
    }
    let (shutdown, mut stop) = watch::channel(false);
    let peer_task = peer_server.map(|server| server.start(stop.clone()));
    let signal_shutdown = shutdown.clone();
    let signal_handle = handle.clone();
    let worker = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(sync_seconds));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = stop.changed() => break,
                _ = interval.tick() => {
                    match state.domain.lock().await.sync(false).await {
                        Ok(report) => {
                            if !report.errors.is_empty() {
                                eprintln!("Sync has {} error(s); inspect authenticated /status", report.errors.len());
                            }
                            *state.last_sync.lock().await = Some(report);
                        }
                        Err(e) => eprintln!("Sync failed: {e}"),
                    }
                }
            }
        }
    });
    let served = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            let _ = signal_shutdown.send(true);
            if let Some(handle) = signal_handle {
                handle.graceful_shutdown(Some(Duration::from_secs(10)));
            }
        })
        .await;
    let _ = shutdown.send(true);
    if let Some(handle) = handle {
        handle.graceful_shutdown(Some(Duration::from_secs(10)));
    }
    worker.await?;
    if let Some(task) = peer_task {
        task.await??;
    }
    if let Some(task) = remote_task {
        task.await??;
    }
    served?;
    Ok(())
}
