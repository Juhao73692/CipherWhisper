//! Durable opaque ciphertext queue. No Conversation Layer or private keys.
use anyhow::{Result, ensure};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, OriginalUri, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use cipherwhisper_protocol::*;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use std::{
    path::Path as FsPath,
    sync::{Arc, Mutex},
};

type Shared = Arc<Mutex<Connection>>;
struct ApiError(StatusCode, String);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({"error":self.1}))).into_response()
    }
}
impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        Self(StatusCode::BAD_REQUEST, e.to_string())
    }
}
type Api<T> = std::result::Result<Json<T>, ApiError>;

pub fn router(path: impl AsRef<FsPath>) -> Result<Router> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
    CREATE TABLE IF NOT EXISTS envelopes(seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT UNIQUE NOT NULL, sender TEXT NOT NULL, recipient TEXT NOT NULL, digest TEXT NOT NULL, envelope TEXT, acknowledged INTEGER NOT NULL DEFAULT 0);
    CREATE INDEX IF NOT EXISTS inbox ON envelopes(recipient, acknowledged, seq);
    CREATE TABLE IF NOT EXISTS prekeys(user_id TEXT PRIMARY KEY, upload TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS one_time_keys(user_id TEXT NOT NULL, key TEXT NOT NULL, data TEXT NOT NULL, consumed INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(user_id,key));
    CREATE TABLE IF NOT EXISTS auth_nonces(user_id TEXT NOT NULL, nonce TEXT NOT NULL, timestamp INTEGER NOT NULL, PRIMARY KEY(user_id,nonce));")?;
    Ok(Router::new()
        .route(
            "/health",
            get(|| async { Json(serde_json::json!({"status":"ok", "protocol":VERSION})) }),
        )
        .route("/prekeys", post(publish))
        .route("/prekeys/{user}/claim", post(claim))
        .route("/messages", post(send).get(inbox))
        .route("/messages/{id}/ack", post(ack))
        .route("/messages/{id}/delivery", get(delivery))
        .layer(DefaultBodyLimit::max(MAX_ENVELOPE * 2))
        .with_state(Arc::new(Mutex::new(conn))))
}
fn auth(
    state: &Shared,
    headers: &HeaderMap,
    method: &str,
    uri: &str,
    body: &[u8],
) -> Result<(String, RequestAuth), ApiError> {
    let parsed = (|| -> Result<RequestAuth> {
        let value = |key: &str| -> Result<String> {
            Ok(headers
                .get(key)
                .ok_or_else(|| anyhow::anyhow!("missing authentication"))?
                .to_str()?
                .to_owned())
        };
        Ok(RequestAuth {
            signing_key: value("x-td-key")?,
            timestamp: value("x-td-time")?.parse()?,
            nonce: value("x-td-nonce")?,
            signature: value("x-td-signature")?,
        })
    })()
    .map_err(|_| ApiError(StatusCode::UNAUTHORIZED, "invalid authentication".into()))?;
    let user = parsed.validate(method, uri, body).map_err(|_| {
        ApiError(
            StatusCode::UNAUTHORIZED,
            "invalid or expired signature".into(),
        )
    })?;
    let db = state.lock().map_err(|_| {
        ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "database unavailable".into(),
        )
    })?;
    db.execute(
        "DELETE FROM auth_nonces WHERE timestamp < ?",
        [now() - AUTH_WINDOW - 1],
    )
    .map_err(anyhow::Error::from)?;
    db.execute(
        "INSERT INTO auth_nonces VALUES (?,?,?)",
        params![user, parsed.nonce, parsed.timestamp],
    )
    .map_err(|_| ApiError(StatusCode::UNAUTHORIZED, "replayed request".into()))?;
    Ok((user, parsed))
}
async fn publish(
    State(state): State<Shared>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    body: Bytes,
) -> Api<serde_json::Value> {
    let (user, _) = auth(&state, &headers, "POST", &uri.to_string(), &body)?;
    let upload: PrekeyUpload = serde_json::from_slice(&body).map_err(anyhow::Error::from)?;
    upload.validate()?;
    ensure_api(upload.identity.user_id == user, "prekey owner mismatch")?;
    let mut db = state.lock().unwrap();
    let tx = db.transaction().map_err(anyhow::Error::from)?;
    if let Some(old) = tx
        .query_row("SELECT upload FROM prekeys WHERE user_id=?", [&user], |r| {
            r.get::<_, String>(0)
        })
        .optional()
        .map_err(anyhow::Error::from)?
    {
        let old: PrekeyUpload = serde_json::from_str(&old).map_err(anyhow::Error::from)?;
        ensure_api(
            old.identity.curve_key == upload.identity.curve_key,
            "identity keys cannot silently change",
        )?;
    }
    for key in &upload.one_time_prekeys {
        tx.execute("INSERT INTO one_time_keys(user_id,key,data) VALUES(?,?,?) ON CONFLICT(user_id,key) DO UPDATE SET data=excluded.data WHERE consumed=0", params![user,key.key,serde_json::to_string(key).map_err(anyhow::Error::from)?]).map_err(anyhow::Error::from)?;
    }
    // The upload row only needs the fallback key. Claimed one-time keys live in a separate table.
    let stored = PrekeyUpload {
        one_time_prekeys: vec![],
        ..upload
    };
    tx.execute(
        "INSERT INTO prekeys VALUES(?,?) ON CONFLICT(user_id) DO UPDATE SET upload=excluded.upload",
        params![
            user,
            serde_json::to_string(&stored).map_err(anyhow::Error::from)?
        ],
    )
    .map_err(anyhow::Error::from)?;
    tx.commit().map_err(anyhow::Error::from)?;
    Ok(Json(serde_json::json!({"ok":true})))
}
async fn claim(
    State(state): State<Shared>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    Path(user): Path<String>,
    body: Bytes,
) -> Api<PrekeyBundle> {
    auth(&state, &headers, "POST", &uri.to_string(), &body)?;
    ensure_api(body.is_empty(), "claim body must be empty")?;
    let mut db = state.lock().unwrap();
    let tx = db.transaction().map_err(anyhow::Error::from)?;
    let raw = tx
        .query_row("SELECT upload FROM prekeys WHERE user_id=?", [&user], |r| {
            r.get::<_, String>(0)
        })
        .optional()
        .map_err(anyhow::Error::from)?
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "no prekeys published".into()))?;
    let upload: PrekeyUpload = serde_json::from_str(&raw).map_err(anyhow::Error::from)?;
    let mut selected = None;
    {
        let mut stmt = tx
            .prepare(
                "SELECT key,data FROM one_time_keys WHERE user_id=? AND consumed=0 ORDER BY key",
            )
            .map_err(anyhow::Error::from)?;
        let rows = stmt
            .query_map([&user], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(anyhow::Error::from)?;
        for row in rows {
            let (key, data) = row.map_err(anyhow::Error::from)?;
            let candidate: SignedPrekey =
                serde_json::from_str(&data).map_err(anyhow::Error::from)?;
            if candidate.expires_at > now() {
                selected = Some((key, candidate));
                break;
            }
        }
    }
    if let Some((key, _)) = &selected {
        tx.execute(
            "UPDATE one_time_keys SET consumed=1 WHERE user_id=? AND key=?",
            params![user, key],
        )
        .map_err(anyhow::Error::from)?;
    }
    let bundle = PrekeyBundle {
        identity: upload.identity,
        signed_prekey: upload.signed_prekey,
        one_time_prekey: selected.map(|(_, k)| k),
    };
    bundle.validate()?;
    tx.commit().map_err(anyhow::Error::from)?;
    Ok(Json(bundle))
}
fn ensure_api(ok: bool, message: &str) -> Result<(), ApiError> {
    if ok {
        Ok(())
    } else {
        Err(ApiError(StatusCode::BAD_REQUEST, message.into()))
    }
}
async fn send(
    State(state): State<Shared>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    body: Bytes,
) -> Api<Delivery> {
    let (user, auth) = auth(&state, &headers, "POST", &uri.to_string(), &body)?;
    let env: Envelope = serde_json::from_slice(&body).map_err(anyhow::Error::from)?;
    env.validate(&auth.signing_key)?;
    ensure_api(user == env.from, "sender mismatch")?;
    let mut db = state.lock().unwrap();
    let tx = db.transaction().map_err(anyhow::Error::from)?;
    let old = tx
        .query_row(
            "SELECT digest,acknowledged FROM envelopes WHERE id=?",
            [&env.id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)),
        )
        .optional()
        .map_err(anyhow::Error::from)?;
    let acknowledged = if let Some((hash, delivered)) = old {
        if hash != env.digest() {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "envelope id collision".into(),
            ));
        }
        delivered
    } else {
        let count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM envelopes WHERE recipient=? AND acknowledged=0",
                [&env.to],
                |r| r.get(0),
            )
            .map_err(anyhow::Error::from)?;
        if count >= 10000 {
            return Err(ApiError(
                StatusCode::TOO_MANY_REQUESTS,
                "recipient queue full".into(),
            ));
        }
        tx.execute(
            "INSERT INTO envelopes(id,sender,recipient,digest,envelope) VALUES(?,?,?,?,?)",
            params![
                env.id,
                env.from,
                env.to,
                env.digest(),
                serde_json::to_string(&env).map_err(anyhow::Error::from)?
            ],
        )
        .map_err(anyhow::Error::from)?;
        false
    };
    tx.commit().map_err(anyhow::Error::from)?;
    Ok(Json(Delivery {
        id: env.id,
        acknowledged,
    }))
}
#[derive(Deserialize)]
struct InboxQuery {
    #[serde(default)]
    cursor: i64,
}
async fn inbox(
    State(state): State<Shared>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    Query(query): Query<InboxQuery>,
) -> Api<InboxPage> {
    let (user, _) = auth(&state, &headers, "GET", &uri.to_string(), &[])?;
    ensure_api(query.cursor >= 0, "invalid cursor")?;
    let db = state.lock().unwrap();
    let mut stmt = db.prepare("SELECT seq,envelope FROM envelopes WHERE recipient=? AND acknowledged=0 AND seq>? ORDER BY seq LIMIT 100").map_err(anyhow::Error::from)?;
    let rows = stmt
        .query_map(params![user, query.cursor], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(anyhow::Error::from)?;
    let mut items = vec![];
    for row in rows {
        let (cursor, raw) = row.map_err(anyhow::Error::from)?;
        items.push(QueuedEnvelope {
            cursor,
            envelope: serde_json::from_str(&raw).map_err(anyhow::Error::from)?,
        });
    }
    Ok(Json(InboxPage {
        next_cursor: items.last().map_or(query.cursor, |i| i.cursor),
        items,
    }))
}
async fn ack(
    State(state): State<Shared>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    Path(id): Path<String>,
    body: Bytes,
) -> Api<Delivery> {
    let (user, _) = auth(&state, &headers, "POST", &uri.to_string(), &body)?;
    ensure_api(body.is_empty(), "ack body must be empty")?;
    let db = state.lock().unwrap();
    let changed = db
        .execute(
            "UPDATE envelopes SET acknowledged=1,envelope=NULL WHERE id=? AND recipient=?",
            params![id, user],
        )
        .map_err(anyhow::Error::from)?;
    if changed == 0 {
        return Err(ApiError(StatusCode::NOT_FOUND, "message not found".into()));
    }
    Ok(Json(Delivery {
        id,
        acknowledged: true,
    }))
}
async fn delivery(
    State(state): State<Shared>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    Path(id): Path<String>,
) -> Api<Delivery> {
    let (user, _) = auth(&state, &headers, "GET", &uri.to_string(), &[])?;
    let db = state.lock().unwrap();
    let acknowledged = db
        .query_row(
            "SELECT acknowledged FROM envelopes WHERE id=? AND sender=?",
            params![id, user],
            |r| r.get(0),
        )
        .optional()
        .map_err(anyhow::Error::from)?
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "message not found".into()))?;
    Ok(Json(Delivery { id, acknowledged }))
}

pub fn validate_bind(address: std::net::SocketAddr) -> Result<()> {
    ensure!(
        address.ip().is_loopback(),
        "MVP relay binds to loopback only; use a TLS reverse proxy for remote access"
    );
    Ok(())
}

mod service;
pub use service::{RelayArgs, run};
