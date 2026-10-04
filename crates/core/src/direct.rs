//! Direct encrypted delivery. Ingress never waits on the outbound endpoint mutex
//! or mutates ratchets: simultaneous first contact cannot deadlock both centers.
use super::*;
use cipherwhisper_protocol::p2p::*;
use reqwest::{Client, Url};
use std::{collections::HashSet, time::Duration};

pub fn http_client(endpoint: &str, ca: Option<&str>) -> Result<Client> {
    let url = Url::parse(endpoint)?;
    let loopback = url.host_str().is_some_and(|h| {
        h == "localhost"
            || h.trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none(),
        "peer address must be an origin"
    );
    ensure!(
        url.scheme() == "https" || (url.scheme() == "http" && loopback),
        "remote P2P requires HTTPS; HTTP is only for local tests"
    );
    let mut builder = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(10))
        .tls_version_min(reqwest::tls::Version::TLS_1_3);
    if let Some(ca) = ca {
        ensure!(
            url.scheme() == "https" && ca.len() <= 256 * 1024,
            "invalid peer CA"
        );
        builder = builder.tls_certs_only([reqwest::Certificate::from_pem(ca.as_bytes())?]);
    }
    Ok(builder.build()?)
}
pub(crate) fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS direct_routes(peer_id TEXT PRIMARY KEY REFERENCES peers(id),profile TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS direct_keys(key TEXT PRIMARY KEY,data TEXT NOT NULL,consumed INTEGER NOT NULL DEFAULT 0);
    CREATE TABLE IF NOT EXISTS direct_nonces(peer TEXT NOT NULL,nonce TEXT NOT NULL,timestamp INTEGER NOT NULL,PRIMARY KEY(peer,nonce));
    CREATE TABLE IF NOT EXISTS direct_inbox(seq INTEGER PRIMARY KEY AUTOINCREMENT,id TEXT UNIQUE NOT NULL,sender TEXT NOT NULL,digest TEXT NOT NULL,envelope TEXT,last_error TEXT);")?;
    Ok(())
}
impl Endpoint {
    pub fn is_direct(&self) -> bool {
        self.transport.is_none()
    }
    pub fn set_direct_address(&mut self, endpoint: &str, ca: Option<&str>) -> Result<()> {
        ensure!(self.is_direct(), "center uses relay transport");
        http_client(endpoint, ca)?;
        let profile = self.make_direct_profile(endpoint, ca)?;
        set_metadata(
            &self.db,
            "direct-profile",
            &serde_json::to_string(&profile)?,
        )
    }
    fn make_direct_profile(&self, endpoint: &str, ca: Option<&str>) -> Result<PeerProfile> {
        let mut profile = PeerProfile {
            version: VERSION,
            identity: self.contact_card()?,
            endpoint: endpoint.trim_end_matches('/').into(),
            ca_pem: ca.map(str::to_owned),
            signature: String::new(),
        };
        profile.signature = load_account(&self.db, &self.key)?
            .sign(profile.signing_bytes())
            .to_base64();
        Ok(profile)
    }
    pub fn direct_profile(&self) -> Result<PeerProfile> {
        ensure!(self.is_direct(), "center uses relay transport");
        Ok(serde_json::from_str(&metadata(
            &self.db,
            "direct-profile",
        )?)?)
    }
    pub fn add_direct_peer(&mut self, profile: PeerProfile) -> Result<()> {
        ensure!(self.is_direct(), "center uses relay transport");
        profile.validate()?;
        http_client(&profile.endpoint, profile.ca_pem.as_deref())?;
        self.add_peer(profile.identity.clone())?;
        self.db.execute("INSERT INTO direct_routes VALUES(?,?) ON CONFLICT(peer_id) DO UPDATE SET profile=excluded.profile",params![profile.identity.user_id,serde_json::to_string(&profile)?])?;
        Ok(())
    }
    pub fn direct_peers(&self) -> Result<Vec<PeerProfile>> {
        if !self.is_direct() {
            return Ok(vec![]);
        }
        let mut stmt = self
            .db
            .prepare("SELECT profile FROM direct_routes ORDER BY peer_id")?;
        stmt.query_map([], |r| r.get::<_, String>(0))?
            .map(|r| Ok(serde_json::from_str(&r?)?))
            .collect()
    }
    pub(crate) fn refresh_direct_prekeys(&mut self) -> Result<()> {
        let upload = self.prekey_upload()?;
        let tx = self.db.transaction()?;
        for key in upload.one_time_prekeys {
            tx.execute("INSERT INTO direct_keys VALUES(?,?,0) ON CONFLICT(key) DO UPDATE SET data=excluded.data WHERE consumed=0", params![key.key,serde_json::to_string(&key)?])?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn direct_auth(
        &self,
        target: &str,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> Result<PeerAuth> {
        let account = load_account(&self.db, &self.key)?;
        let mut auth = PeerAuth {
            signing_key: account.ed25519_key().to_base64(),
            target: target.into(),
            timestamp: now(),
            nonce: Uuid::new_v4().to_string(),
            signature: String::new(),
        };
        auth.signature = account
            .sign(auth.signing_bytes(method, path, body))
            .to_base64();
        Ok(auth)
    }
    pub async fn check_direct_peer(&mut self, peer: &str) -> Result<ContactCard> {
        let card: ContactCard = self
            .direct_request(peer, "POST", "/p2p/v1/ping", vec![])
            .await?;
        ensure!(card == self.peer(peer)?, "peer identity changed");
        Ok(card)
    }
    pub(crate) async fn direct_request<T: serde::de::DeserializeOwned + Serialize>(
        &mut self,
        peer: &str,
        method: &str,
        path: &str,
        body: Vec<u8>,
    ) -> Result<T> {
        let profile: PeerProfile = serde_json::from_str(
            &self
                .db
                .query_row(
                    "SELECT profile FROM direct_routes WHERE peer_id=?",
                    [peer],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
                .ok_or_else(|| {
                    anyhow::anyhow!("import the peer's .peer.json connection card first")
                })?,
        )?;
        profile.validate()?;
        let pinned = self.peer(peer)?;
        ensure!(
            profile.identity.signing_key == pinned.signing_key
                && profile.identity.curve_key == pinned.curve_key,
            "peer route key mismatch"
        );
        let auth = self.direct_auth(peer, method, path, &body)?;
        let mut response = http_client(&profile.endpoint, profile.ca_pem.as_deref())?
            .request(
                reqwest::Method::from_bytes(method.as_bytes())?,
                format!("{}{path}", profile.endpoint.trim_end_matches('/')),
            )
            .header("content-type", "application/json")
            .header("x-peer-key", &auth.signing_key)
            .header("x-peer-target", &auth.target)
            .header("x-peer-time", auth.timestamp.to_string())
            .header("x-peer-nonce", &auth.nonce)
            .header("x-peer-signature", &auth.signature)
            .body(body)
            .send()
            .await?;
        ensure!(
            response.status().is_success(),
            "peer returned {}; queued ciphertext preserved",
            response.status()
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len() + chunk.len() <= 1024 * 1024,
                "peer response too large"
            );
            bytes.extend_from_slice(&chunk);
        }
        let reply: PeerResponse<T> = serde_json::from_slice(&bytes)?;
        reply.validate(&pinned, &self.contact_card()?.user_id, &auth.nonce)?;
        Ok(reply.data)
    }
    pub fn direct_ingress(&self) -> Result<DirectIngress> {
        ensure!(self.is_direct(), "center uses relay transport");
        let db = Connection::open(
            self.db
                .path()
                .ok_or_else(|| anyhow::anyhow!("persistent database required"))?,
        )?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;",
        )?;
        Ok(DirectIngress {
            db,
            key: Zeroizing::new(*self.key),
            card: self.contact_card()?,
        })
    }
    fn drain_direct_inbox(&mut self, report: &mut SyncReport) -> Result<()> {
        let incoming: Vec<(String, String)> = {
            let mut stmt = self.db.prepare("SELECT id,envelope FROM direct_inbox WHERE envelope IS NOT NULL ORDER BY seq LIMIT 100")?;
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        for (id, raw) in incoming {
            let env: Envelope = serde_json::from_str(&raw)?;
            match self.receive(&env) {
                Ok(fresh) => {
                    let tx = self.db.transaction()?;
                    tx.execute(
                        "UPDATE direct_inbox SET envelope=NULL,last_error=NULL WHERE id=?",
                        [&id],
                    )?;
                    tx.execute("DELETE FROM pending_acks WHERE id=?", [&id])?;
                    tx.commit()?;
                    if fresh {
                        report.received += 1;
                    }
                }
                Err(e) => {
                    self.db.execute(
                        "UPDATE direct_inbox SET last_error=? WHERE id=?",
                        params![e.to_string(), id],
                    )?;
                    report.errors.push(format!("receive {id}: {e}"));
                }
            }
        }
        Ok(())
    }
    pub(crate) async fn sync_direct(&mut self, force: bool) -> Result<SyncReport> {
        let mut report = SyncReport::default();
        self.refresh_direct_prekeys()?;
        self.drain_direct_inbox(&mut report)?;
        let mut unavailable = HashSet::new();
        for job in self
            .outbox()?
            .into_iter()
            .filter(|j| force || j.next_attempt <= now())
            .take(100)
        {
            let env: Envelope = serde_json::from_str(&self.db.query_row(
                "SELECT envelope FROM outbox WHERE id=?",
                [&job.id],
                |r| r.get::<_, String>(0),
            )?)?;
            if unavailable.contains(&env.to) {
                continue;
            }
            let result: Result<Delivery> = if job.accepted {
                self.direct_request(
                    &env.to,
                    "GET",
                    &format!("/p2p/v1/messages/{}", env.id),
                    vec![],
                )
                .await
            } else {
                self.direct_request(
                    &env.to,
                    "POST",
                    "/p2p/v1/messages",
                    serde_json::to_vec(&env)?,
                )
                .await
            };
            match result {
                Ok(delivery) => {
                    ensure!(delivery.id == env.id, "peer delivery ID mismatch");
                    self.finish_delivery(&env.id, delivery.acknowledged)?;
                    if delivery.acknowledged {
                        report.delivered += 1;
                    } else if !job.accepted {
                        report.sent += 1;
                    }
                }
                Err(e) => {
                    unavailable.insert(env.to);
                    let delay = 2_i64.pow((job.attempts + 1).min(8) as u32);
                    self.db.execute("UPDATE outbox SET accepted=0,attempts=attempts+1,next_attempt=?,last_error=? WHERE id=?",params![now()+delay,e.to_string(),env.id])?;
                    report.errors.push(format!("outbox {}: {e}", env.id));
                }
            }
        }
        self.drain_direct_inbox(&mut report)?;
        Ok(report)
    }
}

/// Separate connection to the same local database. No plaintext history routes,
/// third-party forwarding, outgoing network, or ratchet mutation.
pub struct DirectIngress {
    db: Connection,
    key: Zeroizing<[u8; 32]>,
    card: ContactCard,
}
impl DirectIngress {
    pub fn authenticate(
        &mut self,
        auth: &PeerAuth,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> Result<String> {
        let peer = auth.validate(&self.card.user_id, method, path, body)?;
        let card: ContactCard = serde_json::from_str(&self.db.query_row(
            "SELECT card FROM peers WHERE id=?",
            [&peer],
            |r| r.get::<_, String>(0),
        )?)?;
        ensure!(card.signing_key == auth.signing_key, "untrusted peer");
        let tx = self.db.transaction()?;
        tx.execute(
            "DELETE FROM direct_nonces WHERE timestamp<?",
            [now() - AUTH_WINDOW - 1],
        )?;
        tx.execute(
            "INSERT INTO direct_nonces VALUES(?,?,?)",
            params![peer, auth.nonce, auth.timestamp],
        )?;
        tx.commit()?;
        Ok(peer)
    }
    pub fn response<T: Serialize>(
        &self,
        peer: &str,
        nonce: &str,
        data: T,
    ) -> Result<PeerResponse<T>> {
        let mut reply = PeerResponse {
            version: VERSION,
            from: self.card.user_id.clone(),
            to: peer.into(),
            nonce: nonce.into(),
            data,
            signature: String::new(),
        };
        reply.signature = load_account(&self.db, &self.key)?
            .sign(reply.signing_bytes())
            .to_base64();
        Ok(reply)
    }
    pub fn identity(&self) -> ContactCard {
        self.card.clone()
    }
    pub fn claim(&mut self) -> Result<PrekeyBundle> {
        let tx = self.db.transaction()?;
        let upload: PrekeyUpload = serde_json::from_str(&metadata(&tx, "prekeys")?)?;
        let selected: Option<(String,String)> = tx.query_row("SELECT key,data FROM direct_keys WHERE consumed=0 AND json_extract(data,'$.expires_at')>? ORDER BY key LIMIT 1",[now()],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        let one_time_prekey = if let Some((key, data)) = selected {
            tx.execute("UPDATE direct_keys SET consumed=1 WHERE key=?", [key])?;
            Some(serde_json::from_str(&data)?)
        } else {
            None
        };
        let bundle = PrekeyBundle {
            identity: upload.identity,
            signed_prekey: upload.signed_prekey,
            one_time_prekey,
        };
        bundle.validate()?;
        tx.commit()?;
        Ok(bundle)
    }
    pub fn enqueue(&mut self, peer: &str, env: Envelope) -> Result<Delivery> {
        let card: ContactCard = serde_json::from_str(&self.db.query_row(
            "SELECT card FROM peers WHERE id=?",
            [peer],
            |r| r.get::<_, String>(0),
        )?)?;
        env.validate(&card.signing_key)?;
        ensure!(
            env.from == peer && env.to == self.card.user_id,
            "direct envelope routing mismatch"
        );
        let tx = self.db.transaction()?;
        let received: Option<String> = tx
            .query_row(
                "SELECT digest FROM received_envelopes WHERE id=?",
                [&env.id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(hash) = &received {
            ensure!(hash == &env.digest(), "received envelope ID collision");
        }
        let old: Option<String> = tx
            .query_row(
                "SELECT digest FROM direct_inbox WHERE id=?",
                [&env.id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(hash) = old {
            ensure!(hash == env.digest(), "envelope ID collision");
        } else {
            let count: i64 = tx.query_row(
                "SELECT COUNT(*) FROM direct_inbox WHERE envelope IS NOT NULL",
                [],
                |r| r.get(0),
            )?;
            ensure!(count < 10000, "local inbox full");
            tx.execute(
                "INSERT INTO direct_inbox(id,sender,digest,envelope) VALUES(?,?,?,?)",
                params![
                    env.id,
                    peer,
                    env.digest(),
                    if received.is_some() {
                        None
                    } else {
                        Some(serde_json::to_string(&env)?)
                    }
                ],
            )?;
        }
        tx.commit()?;
        Ok(Delivery {
            id: env.id,
            acknowledged: received.is_some(),
        })
    }
    pub fn delivery(&self, peer: &str, id: &str) -> Result<Delivery> {
        uuid(id)?;
        let acknowledged = self.db.query_row("SELECT EXISTS(SELECT 1 FROM received_envelopes r WHERE r.id=i.id AND r.digest=i.digest) FROM direct_inbox i WHERE i.id=? AND i.sender=?",params![id,peer],|r|r.get(0))?;
        Ok(Delivery {
            id: id.into(),
            acknowledged,
        })
    }
}
