//! Loopback administration API for a headless Trust Domain center.
use anyhow::{Result, ensure};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
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
use topicairn_core::{Endpoint, SyncReport};
use topicairn_protocol::*;
use zeroize::Zeroizing;

#[derive(Parser)]
#[command(about = "Topicairn headless Personal Trust Domain center", version)]
pub struct DomainArgs {
    #[arg(long, default_value = "domain-data")]
    pub data: PathBuf,
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long, default_value = "http://127.0.0.1:8787")]
    pub relay: String,
    /// Trust this PEM CA for the relay HTTPS connection (does not disable verification).
    #[arg(long)]
    pub relay_ca: Option<PathBuf>,
    #[arg(long, default_value = "127.0.0.1:8790")]
    pub bind: std::net::SocketAddr,
    #[arg(
        long,
        env = "TOPICAIRN_PASSPHRASE",
        hide_env_values = true,
        hide = true
    )]
    pub passphrase: String,
    #[arg(long, default_value = "5")]
    pub sync_seconds: u64,
}
#[derive(Clone)]
struct AppState {
    domain: Arc<Mutex<Endpoint>>,
    token_digest: Arc<String>,
    last_sync: Arc<Mutex<Option<SyncReport>>>,
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
    let valid = supplied.is_some_and(|t| {
        bool::from(
            digest(t.as_bytes())
                .as_bytes()
                .ct_eq(state.token_digest.as_bytes()),
        )
    });
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
    state.domain.lock().await.add_peer(card)?;
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
            .create_topic(&input.peer_id, &input.title)?,
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
    Ok(Json(state.domain.lock().await.sync(true).await?))
}
async fn outbox(State(state): State<AppState>) -> Api<serde_json::Value> {
    Ok(Json(
        serde_json::to_value(state.domain.lock().await.outbox()?).map_err(anyhow::Error::from)?,
    ))
}
async fn status(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({"protocol":VERSION,"lastSync":*state.last_sync.lock().await}))
}
pub async fn run(args: DomainArgs) -> Result<()> {
    ensure!(
        args.bind.ip().is_loopback(),
        "management API is loopback only; domain-internal device networking is out of scope"
    );
    ensure!(
        args.sync_seconds >= 1 && args.sync_seconds <= 300,
        "sync interval must be 1..300 seconds"
    );
    let passphrase = Zeroizing::new(args.passphrase);
    let domain = Endpoint::open_with_ca(
        &args.data,
        &passphrase,
        args.name.as_deref(),
        &args.relay,
        args.relay_ca.as_deref(),
    )?;
    drop(passphrase);
    let token = token(&args.data)?;
    let state = AppState {
        domain: Arc::new(Mutex::new(domain)),
        token_digest: Arc::new(digest(token.as_bytes())),
        last_sync: Arc::new(Mutex::new(None)),
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
        .layer(DefaultBodyLimit::max(MAX_BODY * 2))
        .layer(middleware::from_fn_with_state(state.clone(), authenticate))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind(args.bind).await?;
    let (shutdown, mut stop) = watch::channel(false);
    let worker = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(args.sync_seconds));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _=stop.changed()=>break,
                _=interval.tick()=>{
                    let report=state.domain.lock().await.sync(false).await;
                    match report{Ok(report)=>{if !report.errors.is_empty(){eprintln!("Sync has {} error(s); inspect authenticated /status",report.errors.len());}*state.last_sync.lock().await=Some(report);},Err(e)=>eprintln!("Sync failed: {e}")}
                }
            }
        }
    });
    println!(
        "Topicairn Trust Domain listening on {}",
        listener.local_addr()?
    );
    println!(
        "Local admin token file: {}",
        args.data.join("admin.token").display()
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    let _ = shutdown.send(true);
    worker.await?;
    Ok(())
}
