//! Durable client-side mirror. Own device signing key; no center key or peer ratchets.
use super::*;
use cipherwhisper_protocol::device::*;
use reqwest::{Client, Url};
use std::time::Duration;

pub fn validate_server(server: &str, ca: &str) -> Result<Client> {
    let url = Url::parse(server)?;
    ensure!(
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && url.path() == "/",
        "device server must be an HTTPS origin"
    );
    ensure!(ca.len() <= 256 * 1024, "CA too large");
    Ok(Client::builder()
        .tls_certs_only([reqwest::Certificate::from_pem(ca.as_bytes())?])
        .tls_version_min(reqwest::tls::Version::TLS_1_3)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(30))
        .build()?)
}
pub struct Replica {
    pub(crate) db: Connection,
    pub(crate) key: Zeroizing<[u8; 32]>,
    _lock: File,
    pair: Option<Pairing>,
    http: Option<Client>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pending {
    pub id: String,
    pub operation: Operation,
    pub state: String,
    pub error: Option<String>,
    pub attempts: i64,
    pub next_attempt: i64,
    pub retry_paused: bool,
    pub retry_limit: Option<i64>,
}
impl Replica {
    pub fn open(dir: impl AsRef<Path>, passphrase: &str, label: Option<&str>) -> Result<Self> {
        ensure!(
            passphrase.len() >= 12,
            "use a passphrase of at least 12 bytes"
        );
        let (mut db, lock) = storage::open(dir.as_ref())?;
        let salt: Option<String> = db
            .query_row("SELECT value FROM metadata WHERE key='salt'", [], |r| {
                r.get(0)
            })
            .optional()?;
        if salt.is_some() {
            ensure!(
                metadata(&db, "local-role").ok().as_deref() == Some("device"),
                "center database cannot be used as a device replica"
            );
        }
        let salt = if let Some(s) = salt {
            hex::decode(s)?
        } else {
            let mut b = [0u8; 32];
            getrandom::fill(&mut b)?;
            b.to_vec()
        };
        ensure!(salt.len() == 32, "invalid KDF salt");
        let mut key = Zeroizing::new([0u8; 32]);
        Argon2::new(
            Algorithm::Argon2id,
            Version::V0x13,
            Params::new(65536, 3, 1, Some(32)).map_err(|e| anyhow::anyhow!("Argon2: {e}"))?,
        )
        .hash_password_into(passphrase.as_bytes(), &salt, key.as_mut())
        .map_err(|e| anyhow::anyhow!("Argon2: {e}"))?;
        if db.query_row("SELECT EXISTS(SELECT 1 FROM identity)", [], |r| {
            r.get::<_, bool>(0)
        })? {
            let account = load_account(&db, &key)?;
            let card: DeviceCard = serde_json::from_str(&metadata(&db, "device-card")?)?;
            card.validate()?;
            ensure!(
                card.signing_key == account.ed25519_key().to_base64(),
                "device key mismatch"
            );
        } else {
            let label =
                label.ok_or_else(|| anyhow::anyhow!("new replica requires device-init --name"))?;
            let account = Account::new();
            let signing_key = account.ed25519_key().to_base64();
            let mut card = DeviceCard {
                version: DEVICE_VERSION,
                id: device_id(&signing_key)?,
                label: label.into(),
                signing_key,
                signature: String::new(),
            };
            card.signature = account.sign(card.signing_bytes()).to_base64();
            card.validate()?;
            let tx = db.transaction()?;
            set_metadata(&tx, "salt", &hex::encode(salt))?;
            set_metadata(&tx, "local-role", "device")?;
            set_metadata(&tx, "device-card", &serde_json::to_string(&card)?)?;
            persist_account(&tx, &account, &key)?;
            tx.commit()?;
        }
        db.execute_batch("CREATE TABLE IF NOT EXISTS device_pending(id TEXT PRIMARY KEY,operation TEXT NOT NULL,state TEXT NOT NULL DEFAULT 'pending',attempts INTEGER NOT NULL DEFAULT 0,last_error TEXT,result TEXT,created_at INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS replica_versions(kind TEXT NOT NULL,id TEXT NOT NULL,seq INTEGER NOT NULL,digest TEXT NOT NULL,PRIMARY KEY(kind,id)); CREATE TABLE IF NOT EXISTS replica_message_order(id TEXT PRIMARY KEY,seq INTEGER NOT NULL);
        INSERT OR IGNORE INTO replica_message_order SELECT id,seq FROM replica_versions WHERE kind='message';")?;
        if !db.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('device_pending') WHERE name='next_attempt')", [], |r| r.get::<_, bool>(0))? {
            db.execute_batch("ALTER TABLE device_pending ADD COLUMN next_attempt INTEGER NOT NULL DEFAULT 0;")?;
        }
        let pair: Option<Pairing> = db
            .query_row(
                "SELECT value FROM metadata WHERE key='device-pair'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .map(|s| vault::unseal(&s, &key, b"topicairn.device.pair.v1"))
            .transpose()?;
        let http = if let Some(pair) = &pair {
            pair.validate()?;
            ensure!(
                pair.device.signing_key == load_account(&db, &key)?.ed25519_key().to_base64(),
                "pairing belongs to another device"
            );
            Some(validate_server(&pair.server, &pair.ca_pem)?)
        } else {
            None
        };
        Ok(Self {
            db,
            key,
            _lock: lock,
            pair,
            http,
        })
    }
    pub fn device_card(&self) -> Result<DeviceCard> {
        Ok(serde_json::from_str(&metadata(&self.db, "device-card")?)?)
    }
    pub fn pair(&mut self, pair: Pairing, trusted_domain: &str) -> Result<()> {
        pair.validate()?;
        ensure!(
            pair.domain.user_id == trusted_domain,
            "pairing fingerprint does not match --trust-domain"
        );
        ensure!(
            pair.device == self.device_card()?,
            "pairing is for another device"
        );
        if let Some(old) = &self.pair {
            ensure!(
                old.domain.user_id == pair.domain.user_id
                    && old.domain.signing_key == pair.domain.signing_key
                    && old.domain.curve_key == pair.domain.curve_key
                    && old.epoch == pair.epoch,
                "cannot switch a replica to another identity or journal epoch; use a fresh device directory"
            );
        }
        let http = validate_server(&pair.server, &pair.ca_pem)?;
        let first = self.pair.is_none();
        let tx = self.db.transaction()?;
        set_metadata(
            &tx,
            "device-pair",
            &vault::seal(&pair, &self.key, b"topicairn.device.pair.v1")?,
        )?;
        set_metadata(&tx, "replica-domain-id", &pair.domain.user_id)?;
        if first {
            set_metadata(&tx, "device-cursor", "0")?;
            set_metadata(&tx, "device-ack", "0")?;
            set_metadata(&tx, "device-high-water", "0")?;
        }
        tx.commit()?;
        self.pair = Some(pair);
        self.http = Some(http);
        Ok(())
    }
    pub fn pairing(&self) -> Result<&Pairing> {
        self.pair.as_ref().ok_or_else(|| {
            anyhow::anyhow!("device is not paired; import a verified center pairing file")
        })
    }
    pub fn contact_card(&self) -> Result<ContactCard> {
        Ok(self.pairing()?.domain.clone())
    }
    pub fn cursor(&self) -> Result<i64> {
        Ok(metadata(&self.db, "device-cursor")?.parse()?)
    }
    pub fn info(&self) -> Result<serde_json::Value> {
        Ok(
            serde_json::json!({"card":self.device_card()?,"domainId":self.pairing()?.domain.user_id,"server":self.pairing()?.server,"cursor":self.cursor()?,"acknowledgedCursor":metadata(&self.db,"device-ack")?.parse::<i64>()?}),
        )
    }
    pub fn request_auth(&self, method: &str, path: &str, body: &[u8]) -> Result<DeviceAuth> {
        let account = load_account(&self.db, &self.key)?;
        let mut auth = DeviceAuth {
            signing_key: account.ed25519_key().to_base64(),
            domain_id: self.pairing()?.domain.user_id.clone(),
            timestamp: now(),
            nonce: Uuid::new_v4().to_string(),
            signature: String::new(),
        };
        auth.signature = account
            .sign(auth.signing_bytes(method, path, body))
            .to_base64();
        Ok(auth)
    }
    async fn request<T: serde::de::DeserializeOwned + Serialize>(
        &mut self,
        method: &str,
        path: &str,
        body: Vec<u8>,
    ) -> Result<T> {
        let auth = self.request_auth(method, path, &body)?;
        let pair = self.pairing()?.clone();
        let http = self.http.as_ref().unwrap().clone();
        let mut response = http
            .request(
                reqwest::Method::from_bytes(method.as_bytes())?,
                format!("{}{path}", pair.server),
            )
            .header("content-type", "application/json")
            .header("x-device-key", &auth.signing_key)
            .header("x-device-domain", &auth.domain_id)
            .header("x-device-time", auth.timestamp.to_string())
            .header("x-device-nonce", &auth.nonce)
            .header("x-device-signature", &auth.signature)
            .body(body)
            .send()
            .await?;
        let status = response.status();
        ensure!(
            response.content_length().unwrap_or(0) <= 16 * 1024 * 1024,
            "device response too large"
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len() + chunk.len() <= 16 * 1024 * 1024,
                "device response too large"
            );
            bytes.extend_from_slice(&chunk);
        }
        ensure!(
            status.is_success(),
            "center returned {status}; operation remains durable for retry"
        );
        let reply: SignedResponse<T> = serde_json::from_slice(&bytes)?;
        reply.validate(&pair, &auth.nonce)?;
        Ok(reply.data)
    }
    pub fn apply_page(&mut self, page: &Page) -> Result<usize> {
        ensure!(page.epoch == self.pairing()?.epoch, "sync epoch mismatch");
        let cursor = self.cursor()?;
        let observed: i64 = metadata(&self.db, "device-high-water")?.parse()?;
        ensure!(page.high_water >= observed, "server journal rolled back");
        ensure!(
            page.from_cursor >= 0
                && page.from_cursor <= page.next_cursor
                && page.next_cursor <= page.high_water
                && page.changes.len() <= PAGE_LIMIT,
            "invalid sync page bounds"
        );
        if page.next_cursor < cursor {
            anyhow::bail!("stale sync page");
        }
        if page.next_cursor == cursor && page.from_cursor < cursor {
            return Ok(0);
        }
        ensure!(page.from_cursor == cursor, "out-of-order sync page");
        let mut previous = cursor;
        for change in &page.changes {
            ensure!(
                change.seq > previous && change.seq <= page.next_cursor,
                "invalid change order"
            );
            previous = change.seq;
        }
        ensure!(
            previous == page.next_cursor,
            "page cursor does not match committed changes"
        );
        let tx = self.db.transaction()?;
        for change in &page.changes {
            apply_revision(&tx, &change.entity, change.seq)?;
        }
        set_metadata(&tx, "device-cursor", &page.next_cursor.to_string())?;
        set_metadata(&tx, "device-high-water", &page.high_water.to_string())?;
        tx.commit()?;
        Ok(page.changes.len())
    }
    pub fn pending(&self) -> Result<Vec<Pending>> {
        let mut stmt=self.db.prepare("SELECT id,operation,state,last_error,attempts,next_attempt FROM device_pending WHERE state<>'accepted' ORDER BY rowid")?;
        stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?,
            ))
        })?
        .map(|r| {
            let (id, op, state, error, attempts, next_attempt) = r?;
            let operation: Operation = serde_json::from_str(&op)?;
            let is_message = matches!(&operation, Operation::Send { .. });
            Ok(Pending {
                id,
                operation,
                retry_paused: is_message && state == "pending" && attempts >= MAX_DELIVERY_FAILURES,
                state,
                error,
                attempts,
                next_attempt,
                retry_limit: is_message.then_some(MAX_DELIVERY_FAILURES),
            })
        })
        .collect()
    }
    pub fn discard_failed(&mut self, id: &str) -> Result<()> {
        ensure!(
            self.db.execute(
                "DELETE FROM device_pending WHERE id=? AND state='failed'",
                [id]
            )? == 1,
            "only permanently rejected operations can be discarded; uncertain network outcomes must be retried"
        );
        Ok(())
    }
    pub async fn retry_outbox(&mut self, id: &str) -> Result<Message> {
        let job = self
            .pending()?
            .into_iter()
            .find(|job| job.id == id && job.retry_paused)
            .ok_or_else(|| anyhow::anyhow!("只能重新发送已暂停的消息"))?;
        let Operation::Send {
            message_id,
            timestamp,
            topic_id,
            body,
            format,
            reply_to,
            ..
        } = job.operation
        else {
            anyhow::bail!("此任务不是消息");
        };
        if format == "file" {
            self.db.execute(
                "UPDATE device_pending SET attempts=0,next_attempt=0,last_error=NULL WHERE id=?",
                [id],
            )?;
            return Ok(Message {
                id: message_id,
                topic_id,
                sender_id: self.contact_card()?.user_id,
                timestamp,
                body,
                format,
                reply_to,
                delivery: "queued".into(),
            });
        }
        self.send_formatted(&topic_id, &body, reply_to, &format)
            .await
    }
    pub(crate) fn enqueue(&mut self, operation: Operation) -> Result<String> {
        let encoded = serde_json::to_string(&operation)?;
        if !matches!(operation, Operation::Send { .. })
            && let Some(id) = self
                .db
                .query_row(
                    "SELECT id FROM device_pending WHERE operation=? AND state='pending'",
                    [&encoded],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
        {
            return Ok(id);
        }
        let id = Uuid::new_v4().to_string();
        self.db.execute(
            "INSERT INTO device_pending(id,operation,created_at) VALUES(?,?,?)",
            params![id, encoded, now()],
        )?;
        Ok(id)
    }
    async fn flush(&mut self, report: &mut SyncReport, force: bool) -> Result<()> {
        let jobs = self.pending()?;
        for job in jobs
            .into_iter()
            .filter(|j| {
                j.state == "pending" && !j.retry_paused && (force || j.next_attempt <= now())
            })
            .take(100)
        {
            let command = Command {
                id: job.id.clone(),
                operation: job.operation,
            };
            match self
                .request::<CommandReply>(
                    "POST",
                    "/device/v1/commands",
                    serde_json::to_vec(&command)?,
                )
                .await
                .and_then(|reply| {
                    self.apply_command_reply(&command, &reply)?;
                    Ok(reply)
                }) {
                Ok(reply) => {
                    match reply.result {
                        CommandResult::Accepted { .. } => report.sent += 1,
                        CommandResult::Rejected { error, .. } => report
                            .errors
                            .push(format!("operation {} rejected: {error}", command.id)),
                    };
                }
                Err(e) => {
                    self.db.execute(
                        "UPDATE device_pending SET attempts=attempts+1,next_attempt=?,last_error=? WHERE id=?",
                        params![now()+2_i64.pow((job.attempts + 1).min(8) as u32), e.to_string(), command.id],
                    )?;
                    report.errors.push(format!("operation {}: {e}", command.id));
                    break;
                }
            }
        }
        Ok(())
    }
    /// Called only after verifying a nonce-bound, center-signed HTTPS response.
    pub fn apply_command_reply(&mut self, command: &Command, reply: &CommandReply) -> Result<()> {
        ensure!(
            reply.id == command.id && reply.body_digest == digest(&serde_json::to_vec(command)?),
            "command response mismatch"
        );
        let operation: String = self.db.query_row(
            "SELECT operation FROM device_pending WHERE id=?",
            [&command.id],
            |r| r.get(0),
        )?;
        ensure!(
            serde_json::from_str::<Operation>(&operation)? == command.operation,
            "local operation content changed"
        );
        let tx = self.db.transaction()?;
        match &reply.result {
            CommandResult::Accepted { entity, revision } => {
                apply_revision(&tx, entity, *revision)?;
                tx.execute("UPDATE device_pending SET state='accepted',last_error=NULL,result=? WHERE id=?",params![serde_json::to_string(&reply.result)?,command.id])?;
            }
            CommandResult::Rejected { error, current } => {
                if let Some(change) = current {
                    apply_revision(&tx, &change.entity, change.seq)?;
                }
                tx.execute(
                    "UPDATE device_pending SET state='failed',last_error=?,result=? WHERE id=?",
                    params![error, serde_json::to_string(&reply.result)?, command.id],
                )?;
            }
        };
        tx.commit()?;
        Ok(())
    }
    pub async fn sync(&mut self, force: bool) -> Result<SyncReport> {
        self.pump_files().await?;
        let mut report = SyncReport::default();
        self.flush(&mut report, force).await?;
        for _ in 0..100 {
            let cursor = self.cursor()?;
            let path = format!(
                "/device/v1/changes?epoch={}&cursor={cursor}&limit={PAGE_LIMIT}",
                self.pairing()?.epoch
            );
            let page: Page = match self.request("GET", &path, vec![]).await {
                Ok(page) => page,
                Err(e) => {
                    report.errors.push(format!("pull: {e}"));
                    return Ok(report);
                }
            };
            let high = page.high_water;
            report.received += self.apply_page(&page)?;
            if self.cursor()? == high {
                break;
            }
        }
        let ack = Ack {
            epoch: self.pairing()?.epoch.clone(),
            cursor: self.cursor()?,
        };
        match self
            .request::<Ack>("POST", "/device/v1/ack", serde_json::to_vec(&ack)?)
            .await
        {
            Ok(received) => {
                ensure!(
                    received.epoch == ack.epoch && received.cursor == ack.cursor,
                    "ACK mismatch"
                );
                set_metadata(&self.db, "device-ack", &ack.cursor.to_string())?;
                report.acknowledged = 1;
            }
            Err(e) => report.errors.push(format!("ACK: {e}")),
        };
        Ok(report)
    }
    async fn submit_online(&mut self, op: Operation) -> Result<Entity> {
        let id = self.enqueue(op)?;
        let report = self.sync(true).await?;
        let result: Option<String> =
            self.db
                .query_row("SELECT result FROM device_pending WHERE id=?", [&id], |r| {
                    r.get(0)
                })?;
        match result
            .map(|s| serde_json::from_str::<CommandResult>(&s))
            .transpose()?
        {
            Some(CommandResult::Accepted { entity, .. }) => Ok(entity),
            Some(CommandResult::Rejected { error, .. }) => anyhow::bail!("{error}"),
            None => anyhow::bail!(
                "operation {id} is saved for retry; do not resubmit. {}",
                report
                    .errors
                    .first()
                    .map_or("waiting for earlier queued operations", String::as_str)
            ),
        }
    }
    pub fn peers(&self) -> Result<Vec<ContactCard>> {
        let mut s = self.db.prepare("SELECT card FROM peers ORDER BY id")?;
        s.query_map([], |r| r.get::<_, String>(0))?
            .map(|r| Ok(serde_json::from_str(&r?)?))
            .collect()
    }
    pub fn topic(&self, id: &str) -> Result<Topic> {
        chat::decorate_topic(
            &self.db,
            self.db
                .query_row("SELECT * FROM topics WHERE id=?", [id], topic_row)?,
        )
    }
    pub fn topics(&self, peer: Option<&str>) -> Result<Vec<Topic>> {
        let mut s = self.db.prepare(
            "SELECT * FROM topics WHERE (?1 IS NULL OR peer_id=?1) ORDER BY updated_at DESC,id",
        )?;
        s.query_map([peer], topic_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .map(|t| chat::decorate_topic(&self.db, t))
            .collect()
    }
    pub fn unread(&self) -> Result<Vec<crate::unread::UnreadTopic>> {
        crate::unread::summary(&self.db, &self.pairing()?.domain.user_id)
    }
    pub fn mark_read(&self, topic: &str, through: &str) -> Result<()> {
        crate::unread::mark_read(&self.db, &self.pairing()?.domain.user_id, topic, through)
    }
    pub(crate) fn queued_messages(&self) -> Result<Vec<Message>> {
        let mut stmt = self
            .db
            .prepare("SELECT operation,state,attempts FROM device_pending WHERE state<>'accepted' ORDER BY rowid")?;
        let domain = &self.pairing()?.domain.user_id;
        let mut messages = Vec::new();
        for r in stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })? {
            let (raw, state, attempts) = r?;
            if let Operation::Send {
                message_id,
                topic_id,
                body,
                format,
                reply_to,
                timestamp,
            } = serde_json::from_str(&raw)?
            {
                messages.push(Message {
                    id: message_id,
                    topic_id,
                    sender_id: domain.clone(),
                    timestamp,
                    body,
                    format,
                    reply_to,
                    delivery: if state == "failed" {
                        "failed"
                    } else if state == "pending" && attempts >= MAX_DELIVERY_FAILURES {
                        "paused"
                    } else {
                        "queued"
                    }
                    .into(),
                });
            }
        }
        Ok(messages)
    }
    pub fn messages(&self, id: &str) -> Result<Vec<Message>> {
        self.topic(id)?;
        let mut stmt = self.db.prepare(
            "SELECT messages.* FROM messages LEFT JOIN replica_message_order o ON o.id=messages.id WHERE topic_id=? ORDER BY timestamp,o.seq,messages.rowid",
        )?;
        let mut messages = stmt
            .query_map([id], message_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for m in self
            .queued_messages()?
            .into_iter()
            .filter(|m| m.topic_id == id)
        {
            if !messages.iter().any(|old| old.id == m.id) {
                messages.push(m);
            }
        }
        // Preserve canonical journal insertion order when timestamps share a second.
        messages.sort_by_key(|m| m.timestamp);
        Ok(messages)
    }
    pub fn search(&self, query: &str) -> Result<Vec<Message>> {
        ensure!(
            !query.trim().is_empty() && query.len() <= 512,
            "invalid search query"
        );
        let q = query.trim();
        let mut messages = if q.chars().count() < 3 {
            let mut s=self.db.prepare("SELECT * FROM messages WHERE instr(lower(body),lower(?))>0 ORDER BY timestamp DESC,rowid DESC LIMIT 100")?;
            s.query_map([q], message_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        } else {
            let mut s=self.db.prepare("SELECT m.* FROM messages m JOIN messages_fts f ON m.rowid=f.rowid WHERE messages_fts MATCH ? ORDER BY rank LIMIT 100")?;
            s.query_map([format!("\"{}\"", q.replace('"', "\"\""))], message_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for m in self
            .queued_messages()?
            .into_iter()
            .filter(|m| m.body.to_lowercase().contains(&q.to_lowercase()))
        {
            if !messages.iter().any(|old| old.id == m.id) && messages.len() < 100 {
                messages.push(m);
            }
        }
        Ok(messages)
    }
    pub async fn add_peer(&mut self, card: ContactCard) -> Result<()> {
        card.validate()?;
        ensure!(
            card.user_id != self.contact_card()?.user_id,
            "cannot add self as peer"
        );
        self.submit_online(Operation::AddPeer { card }).await?;
        Ok(())
    }
    pub async fn create_topic(&mut self, peer: &str, title: &str) -> Result<Topic> {
        let id = Uuid::new_v4().to_string();
        Event::TopicUpdate {
            topic_id: id.clone(),
            title: title.trim().into(),
            created_at: now(),
            archived: false,
        }
        .validate()?;
        let existing:Option<String>=self.db.query_row("SELECT operation FROM device_pending WHERE state='pending' AND json_extract(operation,'$.type')='create_topic' AND json_extract(operation,'$.peer_id')=? AND json_extract(operation,'$.title')=? ORDER BY rowid LIMIT 1",params![peer,title.trim()],|r|r.get(0)).optional()?;
        let op = if let Some(encoded) = existing {
            serde_json::from_str(&encoded)?
        } else {
            Operation::CreateTopic {
                topic_id: id,
                peer_id: peer.into(),
                title: title.trim().into(),
            }
        };
        let Entity::Topic(topic) = self.submit_online(op).await? else {
            anyhow::bail!("wrong topic result");
        };
        Ok(topic)
    }
    pub async fn update_topic(&mut self, id: &str, title: &str, archived: bool) -> Result<()> {
        let t = self.topic(id)?;
        self.submit_online(Operation::UpdateTopic {
            topic_id: id.into(),
            title: title.trim().into(),
            archived,
            base_title: t.title,
            base_archived: t.archived,
        })
        .await?;
        Ok(())
    }
    pub async fn send_message(
        &mut self,
        id: &str,
        body: &str,
        reply_to: Option<String>,
    ) -> Result<Message> {
        self.send_formatted(id, body, reply_to, "markdown").await
    }
    pub(crate) async fn send_formatted(
        &mut self,
        id: &str,
        body: &str,
        reply_to: Option<String>,
        format: &str,
    ) -> Result<Message> {
        let t = self.topic(id)?;
        ensure!(!t.archived, "topic is archived");
        let message_id = Uuid::new_v4().to_string();
        Event::Message {
            message_id: message_id.clone(),
            topic_id: id.into(),
            topic_title: t.title,
            created_at: t.created_at,
            body: body.into(),
            format: format.into(),
            reply_to: reply_to.clone(),
        }
        .validate()?;
        if let Some(reply) = &reply_to {
            let target = self
                .db
                .query_row("SELECT topic_id FROM messages WHERE id=?", [reply], |r| {
                    r.get::<_, String>(0)
                })
                .optional()?;
            ensure!(
                target.is_none_or(|topic| topic == id),
                "reply target belongs to another topic"
            );
        }
        let timestamp = now();
        self.enqueue(Operation::Send {
            format: format.into(),
            message_id: message_id.clone(),
            topic_id: id.into(),
            body: body.into(),
            reply_to: reply_to.clone(),
            timestamp,
        })?;
        Ok(Message {
            id: message_id,
            topic_id: id.into(),
            sender_id: self.contact_card()?.user_id,
            timestamp,
            body: body.into(),
            format: format.into(),
            reply_to,
            delivery: "queued".into(),
        })
    }
    pub fn outbox(&self) -> Result<Vec<OutboxStatus>> {
        Ok(self
            .pending()?
            .into_iter()
            .map(|job| OutboxStatus {
                message_id: match &job.operation {
                    Operation::Send { message_id, .. } => Some(message_id.clone()),
                    _ => None,
                },
                id: job.id,
                accepted: false,
                attempts: job.attempts,
                next_attempt: job.next_attempt,
                last_error: job.error,
                retry_paused: job.retry_paused,
                retry_limit: job.retry_limit,
            })
            .collect())
    }
}

fn apply_revision(db: &Connection, entity: &Entity, seq: i64) -> Result<()> {
    ensure!(seq > 0, "invalid entity revision");
    // A command receipt can insert its message ahead of an earlier journal
    // message. Local rowid must not then determine their canonical order.
    if let Entity::Message(m) = entity {
        db.execute("INSERT INTO replica_message_order VALUES(?,?) ON CONFLICT(id) DO UPDATE SET seq=MIN(seq,excluded.seq)",params![m.id,seq])?;
    }
    let (kind, id) = entity.key();
    let hash = digest(&serde_json::to_vec(entity)?);
    if let Some((old, old_hash)) = db
        .query_row(
            "SELECT seq,digest FROM replica_versions WHERE kind=? AND id=?",
            params![kind, id],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
        )
        .optional()?
    {
        if seq < old {
            return Ok(());
        }
        if seq == old {
            ensure!(hash == old_hash, "same revision changed content");
            return Ok(());
        }
    }
    apply_entity(db, entity)?;
    db.execute("INSERT INTO replica_versions VALUES(?,?,?,?) ON CONFLICT(kind,id) DO UPDATE SET seq=excluded.seq,digest=excluded.digest",params![kind,id,seq,hash])?;
    let high: i64 = metadata(db, "device-high-water")?.parse()?;
    if seq > high {
        set_metadata(db, "device-high-water", &seq.to_string())?;
    }
    Ok(())
}

fn apply_entity(db: &Connection, entity: &Entity) -> Result<()> {
    match entity {
        Entity::FilePart(p) => {
            chat::store_part(db, p)?;
        }
        Entity::Peer(card) => {
            card.validate()?;
            if let Some(old) = db
                .query_row("SELECT card FROM peers WHERE id=?", [&card.user_id], |r| {
                    r.get::<_, String>(0)
                })
                .optional()?
            {
                let old: ContactCard = serde_json::from_str(&old)?;
                ensure!(
                    old.signing_key == card.signing_key && old.curve_key == card.curve_key,
                    "peer key changed in sync"
                );
            }
            db.execute(
                "INSERT INTO peers VALUES(?,?) ON CONFLICT(id) DO UPDATE SET card=excluded.card",
                params![card.user_id, serde_json::to_string(card)?],
            )?;
        }
        Entity::Topic(t) => {
            Event::TopicUpdate {
                topic_id: t.id.clone(),
                title: t.title.clone(),
                created_at: t.created_at,
                archived: t.archived,
            }
            .validate()?;
            if let Some(old) = db
                .query_row("SELECT peer_id FROM topics WHERE id=?", [&t.id], |r| {
                    r.get::<_, String>(0)
                })
                .optional()?
            {
                ensure!(old == t.peer_id, "topic peer changed in sync");
            }
            db.execute("INSERT INTO topics VALUES(?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET title=excluded.title,updated_at=excluded.updated_at,archived=excluded.archived",params![t.id,t.peer_id,t.title,t.created_at,t.updated_at,t.archived])?;
            let meta = chat::TopicMeta {
                pinned: t.pinned,
                tags: t.tags.clone(),
                status: t.status.clone(),
            };
            cipherwhisper_protocol::special::Special::new(
                "topic.meta",
                serde_json::to_value(&meta)?,
            )
            .validate()?;
            db.execute("INSERT INTO chat_topic_meta VALUES(?,?) ON CONFLICT(topic_id) DO UPDATE SET data=excluded.data",params![t.id,serde_json::to_string(&meta)?])?;
        }
        Entity::Message(m) => {
            let peer: String = db.query_row(
                "SELECT peer_id FROM topics WHERE id=?",
                [&m.topic_id],
                |r| r.get(0),
            )?;
            ensure!(
                m.sender_id == peer || m.sender_id == metadata(db, "replica-domain-id")?,
                "synced sender is outside topic"
            );
            if let Some(reply) = &m.reply_to {
                let target: Option<String> = db
                    .query_row("SELECT topic_id FROM messages WHERE id=?", [reply], |r| {
                        r.get(0)
                    })
                    .optional()?;
                ensure!(
                    target.is_none_or(|t| t == m.topic_id),
                    "synced reply crosses topics"
                );
            }
            let references: i64 = db.query_row(
                "SELECT COUNT(*) FROM messages WHERE reply_to=? AND topic_id<>?",
                params![m.id, m.topic_id],
                |r| r.get(0),
            )?;
            ensure!(references == 0, "out-of-order synced reply crosses topics");
            uuid(&m.id)?;
            uuid(&m.topic_id)?;
            ensure!(
                [
                    "markdown",
                    "markdown.edited",
                    "withdrawn",
                    "file",
                    "unknown",
                    "control.unknown"
                ]
                .contains(&m.format.as_str())
                    && (m.format == "withdrawn" || !m.body.trim().is_empty())
                    && m.body.len() <= MAX_BODY
                    && m.timestamp >= 0
                    && ["queued", "sent", "delivered", "received"].contains(&m.delivery.as_str()),
                "invalid synced message"
            );
            if let Some(reply) = &m.reply_to {
                uuid(reply)?;
            }
            if let Some(old) = db
                .query_row("SELECT * FROM messages WHERE id=?", [&m.id], message_row)
                .optional()?
            {
                ensure!(
                    old.topic_id == m.topic_id
                        && old.sender_id == m.sender_id
                        && old.timestamp == m.timestamp
                        && old.reply_to == m.reply_to,
                    "message identity changed in sync"
                );
            }
            db.execute("INSERT INTO messages VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET delivery=excluded.delivery,body=excluded.body,format=excluded.format",params![m.id,m.topic_id,m.sender_id,m.timestamp,m.body,m.format,m.reply_to,m.delivery])?;
        }
    };
    Ok(())
}
