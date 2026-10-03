//! Single-computer Personal Trust Domain endpoint. No device sync or rendering.
mod storage;
pub mod transport;
mod vault;
use anyhow::{Result, ensure};
use argon2::{Algorithm, Argon2, Params, Version};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use std::{fs::File, path::Path};
use topicairn_protocol::*;
use transport::RelayClient;
use uuid::Uuid;
use vodozemac::{
    Curve25519PublicKey,
    olm::{Account, OlmMessage, Session, SessionConfig},
};
use zeroize::Zeroizing;

pub struct Endpoint {
    db: Connection,
    key: Zeroizing<[u8; 32]>,
    _lock: File,
    transport: RelayClient,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ciphertext {
    session_id: String,
    message: OlmMessage,
}
#[derive(Default, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncReport {
    pub sent: usize,
    pub received: usize,
    pub acknowledged: usize,
    pub delivered: usize,
    pub errors: Vec<String>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxStatus {
    pub id: String,
    pub accepted: bool,
    pub attempts: i64,
    pub next_attempt: i64,
    pub last_error: Option<String>,
}

fn load_account(db: &Connection, key: &[u8; 32]) -> Result<Account> {
    let pickle: String =
        db.query_row("SELECT pickle FROM identity WHERE id=1", [], |r| r.get(0))?;
    Ok(Account::from_pickle(
        vault::unseal(&pickle, key, b"topicairn.local.account.v1").map_err(|_| {
            anyhow::anyhow!("cannot unlock identity: wrong passphrase or corrupted storage")
        })?,
    ))
}
fn persist_account(db: &Connection, account: &Account, key: &[u8; 32]) -> Result<()> {
    db.execute(
        "INSERT INTO identity VALUES(1,?) ON CONFLICT(id) DO UPDATE SET pickle=excluded.pickle",
        [vault::seal(
            &account.pickle(),
            key,
            b"topicairn.local.account.v1",
        )?],
    )?;
    Ok(())
}
fn session_context(peer: &str, id: &str) -> Vec<u8> {
    serde_json::to_vec(&("topicairn.local.session.v1", peer, id)).expect("serializable")
}
fn persist_session(db: &Connection, peer: &str, session: &Session, key: &[u8; 32]) -> Result<()> {
    let id = session.session_id();
    let sealed = vault::seal(&session.pickle(), key, &session_context(peer, &id))?;
    db.execute("INSERT INTO sessions VALUES(?,?,?) ON CONFLICT(peer_id,id) DO UPDATE SET pickle=excluded.pickle", params![peer, id, sealed])?;
    Ok(())
}
fn load_session(
    db: &Connection,
    peer: &str,
    id: Option<&str>,
    key: &[u8; 32],
) -> Result<Option<Session>> {
    let raw: Option<(String, String)> = if let Some(id) = id {
        db.query_row(
            "SELECT id,pickle FROM sessions WHERE peer_id=? AND id=?",
            params![peer, id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
    } else {
        db.query_row(
            "SELECT id,pickle FROM sessions WHERE peer_id=? ORDER BY id LIMIT 1",
            [peer],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
    };
    raw.map(|(id, pickle)| {
        Ok(Session::from_pickle(vault::unseal(
            &pickle,
            key,
            &session_context(peer, &id),
        )?))
    })
    .transpose()
}
fn metadata(db: &Connection, name: &str) -> Result<String> {
    Ok(
        db.query_row("SELECT value FROM metadata WHERE key=?", [name], |r| {
            r.get(0)
        })?,
    )
}
fn set_metadata(db: &Connection, name: &str, value: &str) -> Result<()> {
    db.execute(
        "INSERT INTO metadata VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![name, value],
    )?;
    Ok(())
}
fn make_card(account: &Account, label: &str) -> Result<ContactCard> {
    let signing_key = account.ed25519_key().to_base64();
    let mut card = ContactCard {
        version: VERSION,
        user_id: user_id(&signing_key)?,
        signing_key,
        curve_key: account.curve25519_key().to_base64(),
        label: label.into(),
        signature: String::new(),
    };
    card.signature = account.sign(card.signing_bytes()).to_base64();
    card.validate()?;
    Ok(card)
}
fn signed_key(account: &Account, card: &ContactCard, key: String, fallback: bool) -> SignedPrekey {
    let mut prekey = SignedPrekey {
        key,
        expires_at: now() + 30 * 86400,
        signature: String::new(),
    };
    prekey.signature = account
        .sign(prekey.signing_bytes(card, fallback))
        .to_base64();
    prekey
}
fn message_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Message> {
    Ok(Message {
        id: row.get(0)?,
        topic_id: row.get(1)?,
        sender_id: row.get(2)?,
        timestamp: row.get(3)?,
        body: row.get(4)?,
        format: row.get(5)?,
        reply_to: row.get(6)?,
        delivery: row.get(7)?,
    })
}
fn topic_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Topic> {
    Ok(Topic {
        id: row.get(0)?,
        peer_id: row.get(1)?,
        title: row.get(2)?,
        created_at: row.get(3)?,
        updated_at: row.get(4)?,
        archived: row.get(5)?,
    })
}

impl Endpoint {
    /// Init creates a single stable identity. Reopen decrypts the same account and ratchets.
    pub fn open(
        dir: impl AsRef<Path>,
        passphrase: &str,
        label: Option<&str>,
        relay: &str,
    ) -> Result<Self> {
        ensure!(
            passphrase.len() >= 12,
            "use a passphrase of at least 12 bytes"
        );
        let transport = RelayClient::new(relay)?;
        let (mut db, lock) = storage::open(dir.as_ref())?;
        let existing: Option<String> = db
            .query_row("SELECT value FROM metadata WHERE key='salt'", [], |r| {
                r.get(0)
            })
            .optional()?;
        let salt = if let Some(ref salt) = existing {
            hex::decode(salt)?
        } else {
            let mut salt = [0u8; 32];
            getrandom::fill(&mut salt)?;
            salt.to_vec()
        };
        ensure!(salt.len() == 32, "invalid local KDF salt");
        let mut key = Zeroizing::new([0u8; 32]);
        let params = Params::new(65536, 3, 1, Some(32))
            .map_err(|e| anyhow::anyhow!("Argon2 parameters: {e}"))?;
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
            .hash_password_into(passphrase.as_bytes(), &salt, key.as_mut())
            .map_err(|e| anyhow::anyhow!("Argon2 failed: {e}"))?;
        if existing.is_none() {
            let label =
                label.ok_or_else(|| anyhow::anyhow!("new domain requires an identity label"))?;
            let mut account = Account::new();
            let card = make_card(&account, label)?;
            account.generate_fallback_key();
            account.generate_one_time_keys(32);
            let fallback = account
                .fallback_key()
                .values()
                .next()
                .ok_or_else(|| anyhow::anyhow!("missing fallback prekey"))?
                .to_base64();
            let upload = PrekeyUpload {
                identity: card.clone(),
                signed_prekey: signed_key(&account, &card, fallback, true),
                one_time_prekeys: account
                    .one_time_keys()
                    .values()
                    .map(|k| signed_key(&account, &card, k.to_base64(), false))
                    .collect(),
            };
            account.mark_keys_as_published();
            let tx = db.transaction()?;
            set_metadata(&tx, "salt", &hex::encode(salt))?;
            set_metadata(&tx, "schema_version", "1")?;
            set_metadata(&tx, "card", &serde_json::to_string(&card)?)?;
            set_metadata(&tx, "prekeys", &serde_json::to_string(&upload)?)?;
            persist_account(&tx, &account, &key)?;
            tx.commit()?;
        } else {
            ensure!(
                metadata(&db, "schema_version")? == "1",
                "unsupported local schema"
            );
            let account = load_account(&db, &key)?;
            let card: ContactCard = serde_json::from_str(&metadata(&db, "card")?)?;
            card.validate()?;
            ensure!(
                card.signing_key == account.ed25519_key().to_base64()
                    && card.curve_key == account.curve25519_key().to_base64(),
                "stored identity metadata mismatch"
            );
        }
        Ok(Self {
            db,
            key,
            _lock: lock,
            transport,
        })
    }
    pub fn contact_card(&self) -> Result<ContactCard> {
        Ok(serde_json::from_str(&metadata(&self.db, "card")?)?)
    }
    pub fn add_peer(&mut self, card: ContactCard) -> Result<()> {
        card.validate()?;
        ensure!(
            card.user_id != self.contact_card()?.user_id,
            "cannot add self as peer"
        );
        if let Some(old) = self.peer_optional(&card.user_id)? {
            ensure!(
                old.signing_key == card.signing_key && old.curve_key == card.curve_key,
                "peer key replacement requires explicit future identity migration"
            );
        }
        self.db.execute(
            "INSERT INTO peers VALUES(?,?) ON CONFLICT(id) DO UPDATE SET card=excluded.card",
            params![card.user_id, serde_json::to_string(&card)?],
        )?;
        Ok(())
    }
    fn peer_optional(&self, id: &str) -> Result<Option<ContactCard>> {
        self.db
            .query_row("SELECT card FROM peers WHERE id=?", [id], |r| {
                r.get::<_, String>(0)
            })
            .optional()?
            .map(|r| Ok(serde_json::from_str(&r)?))
            .transpose()
    }
    pub fn peer(&self, id: &str) -> Result<ContactCard> {
        self.peer_optional(id)?.ok_or_else(|| {
            anyhow::anyhow!("unknown peer; import and verify their contact card first")
        })
    }
    pub fn peers(&self) -> Result<Vec<ContactCard>> {
        let mut stmt = self.db.prepare("SELECT card FROM peers ORDER BY id")?;
        let mut cards = vec![];
        for row in stmt.query_map([], |r| r.get::<_, String>(0))? {
            cards.push(serde_json::from_str(&row?)?);
        }
        Ok(cards)
    }
    pub fn prekey_upload(&mut self) -> Result<PrekeyUpload> {
        let mut upload: PrekeyUpload = serde_json::from_str(&metadata(&self.db, "prekeys")?)?;
        let mut account = load_account(&self.db, &self.key)?;
        // Refill only when locally consumed. Never discard outstanding private one-time keys.
        if account.stored_one_time_key_count() < 16 {
            account.generate_one_time_keys(16);
            upload.one_time_prekeys.extend(
                account
                    .one_time_keys()
                    .values()
                    .map(|k| signed_key(&account, &upload.identity, k.to_base64(), false)),
            );
            account.mark_keys_as_published();
        }
        if upload.signed_prekey.expires_at < now() + 7 * 86400 {
            upload.signed_prekey = signed_key(
                &account,
                &upload.identity,
                upload.signed_prekey.key.clone(),
                true,
            );
            upload.one_time_prekeys = upload
                .one_time_prekeys
                .iter()
                .map(|k| signed_key(&account, &upload.identity, k.key.clone(), false))
                .collect();
        }
        upload.validate()?;
        let tx = self.db.transaction()?;
        persist_account(&tx, &account, &self.key)?;
        set_metadata(&tx, "prekeys", &serde_json::to_string(&upload)?)?;
        tx.commit()?;
        Ok(upload)
    }
    pub fn request_auth(&self, method: &str, path: &str, body: &[u8]) -> Result<RequestAuth> {
        let account = load_account(&self.db, &self.key)?;
        let mut auth = RequestAuth {
            signing_key: account.ed25519_key().to_base64(),
            timestamp: now(),
            nonce: Uuid::new_v4().to_string(),
            signature: String::new(),
        };
        auth.signature = account
            .sign(auth.signing_bytes(method, path, body))
            .to_base64();
        Ok(auth)
    }
    async fn request<T: serde::de::DeserializeOwned>(
        &mut self,
        method: &str,
        path: &str,
        body: Vec<u8>,
    ) -> Result<T> {
        let auth = self.request_auth(method, path, &body)?;
        self.transport.request(method, path, body, auth).await
    }
    pub async fn publish(&mut self) -> Result<()> {
        let upload = self.prekey_upload()?;
        let body = serde_json::to_vec(&upload)?;
        let _: serde_json::Value = self.request("POST", "/prekeys", body).await?;
        Ok(())
    }
    /// Authenticate fetched prekeys against a contact imported through a trusted channel.
    pub fn establish(&mut self, peer_id: &str, bundle: PrekeyBundle) -> Result<()> {
        bundle.validate()?;
        let pinned = self.peer(peer_id)?;
        ensure!(
            bundle.identity.user_id == pinned.user_id
                && bundle.identity.signing_key == pinned.signing_key
                && bundle.identity.curve_key == pinned.curve_key,
            "prekey identity differs from pinned peer"
        );
        if load_session(&self.db, peer_id, None, &self.key)?.is_some() {
            return Ok(());
        }
        let account = load_account(&self.db, &self.key)?;
        let prekey = bundle
            .one_time_prekey
            .as_ref()
            .unwrap_or(&bundle.signed_prekey);
        let session = account.create_outbound_session(
            SessionConfig::version_1(),
            Curve25519PublicKey::from_base64(&pinned.curve_key)?,
            Curve25519PublicKey::from_base64(&prekey.key)?,
        )?;
        persist_session(&self.db, peer_id, &session, &self.key)
    }
    async fn ensure_session(&mut self, peer: &str) -> Result<()> {
        self.peer(peer)?;
        if load_session(&self.db, peer, None, &self.key)?.is_none() {
            let path = format!("/prekeys/{peer}/claim");
            let bundle: PrekeyBundle = self.request("POST", &path, vec![]).await?;
            self.establish(peer, bundle)?;
        }
        Ok(())
    }
    pub fn create_topic(&mut self, peer: &str, title: &str) -> Result<Topic> {
        self.peer(peer)?;
        let topic = Topic {
            id: Uuid::new_v4().to_string(),
            peer_id: peer.into(),
            title: title.trim().into(),
            created_at: now(),
            updated_at: now(),
            archived: false,
        };
        Event::TopicUpdate {
            topic_id: topic.id.clone(),
            title: topic.title.clone(),
            created_at: topic.created_at,
            archived: false,
        }
        .validate()?;
        self.db.execute(
            "INSERT INTO topics VALUES(?,?,?,?,?,?)",
            params![
                topic.id,
                peer,
                topic.title,
                topic.created_at,
                topic.updated_at,
                false
            ],
        )?;
        Ok(topic)
    }
    pub fn topic(&self, id: &str) -> Result<Topic> {
        Ok(self
            .db
            .query_row("SELECT * FROM topics WHERE id=?", [id], topic_row)?)
    }
    pub fn topics(&self, peer: Option<&str>) -> Result<Vec<Topic>> {
        let mut stmt = self.db.prepare(
            "SELECT * FROM topics WHERE (?1 IS NULL OR peer_id=?1) ORDER BY updated_at DESC,id",
        )?;
        Ok(stmt
            .query_map([peer], topic_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }
    pub fn messages(&self, topic: &str) -> Result<Vec<Message>> {
        self.topic(topic)?;
        let mut stmt = self
            .db
            .prepare("SELECT * FROM messages WHERE topic_id=? ORDER BY timestamp,rowid")?;
        Ok(stmt
            .query_map([topic], message_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }
    pub fn search(&self, query: &str) -> Result<Vec<Message>> {
        ensure!(
            !query.trim().is_empty() && query.len() <= 512,
            "invalid search query"
        );
        let mut stmt=self.db.prepare("SELECT m.* FROM messages m JOIN messages_fts f ON m.rowid=f.rowid WHERE messages_fts MATCH ? ORDER BY rank LIMIT 100")?;
        // Quote as a phrase so untrusted user input is never an FTS expression or SQL.
        let phrase = format!("\"{}\"", query.replace('"', "\"\""));
        Ok(stmt
            .query_map([phrase], message_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }
    pub async fn send_message(
        &mut self,
        topic_id: &str,
        body: &str,
        reply_to: Option<String>,
    ) -> Result<Message> {
        let topic = self.topic(topic_id)?;
        ensure!(!topic.archived, "topic is archived");
        let message_id = Uuid::new_v4().to_string();
        let event = Event::Message {
            message_id: message_id.clone(),
            topic_id: topic.id.clone(),
            topic_title: topic.title,
            created_at: topic.created_at,
            body: body.into(),
            format: "markdown".into(),
            reply_to,
        };
        event.validate()?;
        self.ensure_session(&topic.peer_id).await?;
        self.queue_event(&topic.peer_id, event)?;
        Ok(self.db.query_row(
            "SELECT * FROM messages WHERE id=?",
            [message_id],
            message_row,
        )?)
    }
    pub async fn update_topic(&mut self, id: &str, title: &str, archived: bool) -> Result<()> {
        let topic = self.topic(id)?;
        let event = Event::TopicUpdate {
            topic_id: id.into(),
            title: title.trim().into(),
            created_at: topic.created_at,
            archived,
        };
        event.validate()?;
        self.ensure_session(&topic.peer_id).await?;
        self.queue_event(&topic.peer_id, event)?;
        Ok(())
    }
    /// Atomic ratchet advance + immutable outgoing ciphertext + local plaintext.
    pub fn queue_event(&mut self, peer: &str, event: Event) -> Result<Envelope> {
        event.validate()?;
        self.peer(peer)?;
        let card = self.contact_card()?;
        let account = load_account(&self.db, &self.key)?;
        let tx = self.db.transaction()?;
        let mut session = load_session(&tx, peer, None, &self.key)?
            .ok_or_else(|| anyhow::anyhow!("no session established"))?;
        let id = Uuid::new_v4().to_string();
        let timestamp = now();
        let payload = Payload {
            version: VERSION,
            envelope_id: id.clone(),
            sender: card.user_id.clone(),
            recipient: peer.into(),
            timestamp,
            event,
        };
        let message = session.encrypt(Zeroizing::new(serde_json::to_vec(&payload)?).as_slice())?;
        let ciphertext = serde_json::to_string(&Ciphertext {
            session_id: session.session_id(),
            message,
        })?;
        let mut env = Envelope {
            version: VERSION,
            id,
            from: card.user_id,
            to: peer.into(),
            ciphertext,
            timestamp,
            signature: String::new(),
        };
        env.signature = account.sign(env.signing_bytes()).to_base64();
        let message_id = match &payload.event {
            Event::Message { message_id, .. } => Some(message_id.clone()),
            _ => None,
        };
        apply_event(&tx, &env, &payload.event, true)?;
        persist_session(&tx, peer, &session, &self.key)?;
        tx.execute(
            "INSERT INTO outbox(id,envelope,message_id) VALUES(?,?,?)",
            params![env.id, serde_json::to_string(&env)?, message_id],
        )?;
        tx.commit()?;
        Ok(env)
    }
    /// Reject failures without persisting any mutated ratchet/account or partial plaintext.
    pub fn receive(&mut self, env: &Envelope) -> Result<bool> {
        let peer = self.peer(&env.from)?;
        env.validate(&peer.signing_key)?;
        ensure!(env.to == self.contact_card()?.user_id, "wrong recipient");
        let tx = self.db.transaction()?;
        if let Some(hash) = tx
            .query_row(
                "SELECT digest FROM received_envelopes WHERE id=?",
                [&env.id],
                |r| r.get::<_, String>(0),
            )
            .optional()?
        {
            ensure!(
                hash == env.digest(),
                "replayed envelope id with changed content"
            );
            tx.execute("INSERT OR IGNORE INTO pending_acks VALUES(?)", [&env.id])?;
            tx.commit()?;
            return Ok(false);
        }
        let cipher: Ciphertext = serde_json::from_str(&env.ciphertext)?;
        ensure!(cipher.session_id.len() <= 128, "invalid session identifier");
        let mut account = load_account(&tx, &self.key)?;
        let existing = load_session(&tx, &env.from, Some(&cipher.session_id), &self.key)?;
        let (session, plaintext) = if let Some(mut session) = existing {
            let plain = session.decrypt(&cipher.message)?;
            (session, plain)
        } else if let OlmMessage::PreKey(message) = &cipher.message {
            ensure!(
                cipher.session_id == message.session_id(),
                "prekey session id mismatch"
            );
            let count: i64 = tx.query_row(
                "SELECT COUNT(*) FROM sessions WHERE peer_id=?",
                [&env.from],
                |r| r.get(0),
            )?;
            ensure!(count < 32, "too many sessions for peer");
            let result = account.create_inbound_session(
                SessionConfig::version_1(),
                Curve25519PublicKey::from_base64(&peer.curve_key)?,
                message,
            )?;
            let mut upload: PrekeyUpload = serde_json::from_str(&metadata(&tx, "prekeys")?)?;
            upload
                .one_time_prekeys
                .retain(|key| key.key != message.one_time_key().to_base64());
            set_metadata(&tx, "prekeys", &serde_json::to_string(&upload)?)?;
            (result.session, result.plaintext)
        } else {
            anyhow::bail!("unknown ratchet session; prekey message required")
        };
        let plaintext = Zeroizing::new(plaintext);
        let payload: Payload = serde_json::from_slice(&plaintext)?;
        payload.validate_for(env)?;
        apply_event(&tx, env, &payload.event, false)?;
        persist_account(&tx, &account, &self.key)?;
        persist_session(&tx, &env.from, &session, &self.key)?;
        tx.execute(
            "INSERT INTO received_envelopes VALUES(?,?)",
            params![env.id, env.digest()],
        )?;
        tx.execute("INSERT INTO pending_acks VALUES(?)", [&env.id])?;
        tx.commit()?;
        Ok(true)
    }
    pub fn pending_envelopes(&self) -> Result<Vec<Envelope>> {
        let mut stmt = self
            .db
            .prepare("SELECT envelope FROM outbox ORDER BY rowid")?;
        let mut result = vec![];
        for row in stmt.query_map([], |r| r.get::<_, String>(0))? {
            result.push(serde_json::from_str(&row?)?);
        }
        Ok(result)
    }
    pub fn outbox(&self) -> Result<Vec<OutboxStatus>> {
        let mut stmt = self.db.prepare(
            "SELECT id,accepted,attempts,next_attempt,last_error FROM outbox ORDER BY rowid",
        )?;
        Ok(stmt
            .query_map([], |r| {
                Ok(OutboxStatus {
                    id: r.get(0)?,
                    accepted: r.get(1)?,
                    attempts: r.get(2)?,
                    next_attempt: r.get(3)?,
                    last_error: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }
    fn finish_delivery(&mut self, id: &str, acknowledged: bool) -> Result<()> {
        let tx = self.db.transaction()?;
        let message: Option<String> =
            tx.query_row("SELECT message_id FROM outbox WHERE id=?", [id], |r| {
                r.get(0)
            })?;
        if let Some(message) = message {
            tx.execute(
                "UPDATE messages SET delivery=? WHERE id=?",
                params![if acknowledged { "delivered" } else { "sent" }, message],
            )?;
        }
        if acknowledged {
            tx.execute("DELETE FROM outbox WHERE id=?", [id])?;
        } else {
            tx.execute(
                "UPDATE outbox SET accepted=1,last_error=NULL,next_attempt=? WHERE id=?",
                params![now() + 5, id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    /// Retries reuse committed ciphertext. ACK occurs only after durable local commit.
    pub async fn sync(&mut self, force: bool) -> Result<SyncReport> {
        let mut report = SyncReport::default();
        if let Err(e) = self.publish().await {
            report.errors.push(format!("prekeys: {e}"));
        }
        let jobs = self.outbox()?;
        for job in jobs
            .into_iter()
            .filter(|j| force || j.next_attempt <= now())
        {
            let env: Envelope = serde_json::from_str(&self.db.query_row(
                "SELECT envelope FROM outbox WHERE id=?",
                [&job.id],
                |r| r.get::<_, String>(0),
            )?)?;
            let result: Result<Delivery> = if job.accepted {
                self.request("GET", &format!("/messages/{}/delivery", env.id), vec![])
                    .await
            } else {
                self.request("POST", "/messages", serde_json::to_vec(&env)?)
                    .await
            };
            match result {
                Ok(delivery) => {
                    ensure!(delivery.id == job.id, "relay delivery id mismatch");
                    self.finish_delivery(&job.id, delivery.acknowledged)?;
                    if delivery.acknowledged {
                        report.delivered += 1;
                    } else if !job.accepted {
                        report.sent += 1;
                    }
                }
                Err(e) => {
                    // A restarted/replaced relay may have lost the item: resubmit the same ciphertext.
                    let delay = 2_i64.pow((job.attempts + 1).min(8) as u32);
                    self.db.execute("UPDATE outbox SET accepted=0,attempts=attempts+1,next_attempt=?,last_error=? WHERE id=?",params![now()+delay,e.to_string(),job.id])?;
                    report.errors.push(format!("outbox {}: {e}", job.id));
                }
            }
        }
        // Walk all pages each pass. Failed/unknown messages remain unacked without blocking later pages.
        let mut cursor = 0;
        for _ in 0..100 {
            let page: InboxPage = match self
                .request("GET", &format!("/messages?cursor={cursor}"), vec![])
                .await
            {
                Ok(p) => p,
                Err(e) => {
                    report.errors.push(format!("inbox: {e}"));
                    break;
                }
            };
            if page.items.is_empty() {
                break;
            }
            let next = page.next_cursor;
            for item in page.items {
                match self.receive(&item.envelope) {
                    Ok(true) => report.received += 1,
                    Ok(false) => {}
                    Err(e) => report
                        .errors
                        .push(format!("receive {}: {e}", item.envelope.id)),
                }
            }
            ensure!(next > cursor, "relay cursor did not advance");
            cursor = next;
        }
        let acks: Vec<String> = {
            let mut stmt = self.db.prepare("SELECT id FROM pending_acks")?;
            stmt.query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        for id in acks {
            match self
                .request::<Delivery>("POST", &format!("/messages/{id}/ack"), vec![])
                .await
            {
                Ok(delivery) => {
                    ensure!(
                        delivery.id == id && delivery.acknowledged,
                        "invalid relay acknowledgment"
                    );
                    self.db
                        .execute("DELETE FROM pending_acks WHERE id=?", [id])?;
                    report.acknowledged += 1;
                }
                Err(e) => report.errors.push(format!("ack {id}: {e}")),
            }
        }
        Ok(report)
    }
}

fn apply_event(tx: &Transaction<'_>, env: &Envelope, event: &Event, outgoing: bool) -> Result<()> {
    let peer = if outgoing { &env.to } else { &env.from };
    let (id, title, created) = match event {
        Event::Message {
            topic_id,
            topic_title,
            created_at,
            ..
        } => (topic_id, topic_title, created_at),
        Event::TopicUpdate {
            topic_id,
            title,
            created_at,
            ..
        } => (topic_id, title, created_at),
    };
    let existing: Option<Topic> = tx
        .query_row("SELECT * FROM topics WHERE id=?", [id], topic_row)
        .optional()?;
    if let Some(topic) = existing {
        ensure!(topic.peer_id == *peer, "topic belongs to another peer");
    } else {
        tx.execute(
            "INSERT INTO topics VALUES(?,?,?,?,?,0)",
            params![id, peer, title, created, env.timestamp],
        )?;
    }
    match event {
        Event::Message {
            message_id,
            body,
            format,
            reply_to,
            ..
        } => {
            if let Some(reply) = reply_to {
                // Missing replies may arrive later; existing targets must belong to this topic.
                let target: Option<String> = tx
                    .query_row("SELECT topic_id FROM messages WHERE id=?", [reply], |r| {
                        r.get(0)
                    })
                    .optional()?;
                ensure!(
                    target.as_ref().is_none_or(|topic| topic == id),
                    "reply target belongs to another topic"
                );
            }
            let references: i64 = tx.query_row(
                "SELECT COUNT(*) FROM messages WHERE reply_to=? AND topic_id<>?",
                params![message_id, id],
                |r| r.get(0),
            )?;
            ensure!(references == 0, "out-of-order reply crosses topics");
            ensure!(
                !tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM messages WHERE id=?)",
                    [message_id],
                    |r| r.get::<_, bool>(0)
                )?,
                "message id collision"
            );
            tx.execute(
                "INSERT INTO messages VALUES(?,?,?,?,?,?,?,?)",
                params![
                    message_id,
                    id,
                    env.from,
                    env.timestamp,
                    body,
                    format,
                    reply_to,
                    if outgoing { "queued" } else { "received" }
                ],
            )?;
            tx.execute(
                "UPDATE topics SET updated_at=MAX(updated_at,?) WHERE id=?",
                params![env.timestamp, id],
            )?;
        }
        Event::TopicUpdate {
            title, archived, ..
        } => {
            tx.execute(
                "UPDATE topics SET title=?,archived=?,updated_at=? WHERE id=? AND updated_at<=?",
                params![title, archived, env.timestamp, id, env.timestamp],
            )?;
        }
    }
    Ok(())
}
