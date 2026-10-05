//! Center-owned journal and device authorization; all journal triggers share the data transaction.
use super::*;
use cipherwhisper_protocol::device::*;
const TOPIC_JSON: &str = "json_object('id',new.id,'peerId',new.peer_id,'title',new.title,'createdAt',new.created_at,'updatedAt',new.updated_at,'archived',json(CASE new.archived WHEN 1 THEN 'true' ELSE 'false' END),'pinned',json(CASE COALESCE((SELECT json_extract(data,'$.pinned') FROM chat_topic_meta WHERE topic_id=new.id),0) WHEN 1 THEN 'true' ELSE 'false' END),'tags',json(COALESCE((SELECT json_extract(data,'$.tags') FROM chat_topic_meta WHERE topic_id=new.id),'[]')),'status',COALESCE((SELECT json_extract(data,'$.status') FROM chat_topic_meta WHERE topic_id=new.id),'open'))";
const MESSAGE_JSON: &str = "json_object('id',new.id,'topicId',new.topic_id,'senderId',new.sender_id,'timestamp',new.timestamp,'body',new.body,'format',new.format,'replyTo',new.reply_to,'delivery',new.delivery)";
pub(crate) fn initialize(db: &mut Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS devices(id TEXT PRIMARY KEY,card TEXT NOT NULL,revoked INTEGER NOT NULL DEFAULT 0,created_at INTEGER NOT NULL,last_seen INTEGER NOT NULL DEFAULT 0,ack_cursor INTEGER NOT NULL DEFAULT 0,served_cursor INTEGER NOT NULL DEFAULT 0);
    CREATE TABLE IF NOT EXISTS device_nonces(device_id TEXT NOT NULL,nonce TEXT NOT NULL,timestamp INTEGER NOT NULL,PRIMARY KEY(device_id,nonce));
    CREATE TABLE IF NOT EXISTS device_commands(device_id TEXT NOT NULL,id TEXT NOT NULL,digest TEXT NOT NULL,result TEXT NOT NULL,PRIMARY KEY(device_id,id));
    CREATE TABLE IF NOT EXISTS sync_log(seq INTEGER PRIMARY KEY AUTOINCREMENT,kind TEXT NOT NULL,entity_id TEXT NOT NULL,data TEXT NOT NULL); CREATE INDEX IF NOT EXISTS sync_entity ON sync_log(kind,entity_id,seq);")?;
    // Refresh topic snapshots without resetting the existing journal epoch.
    db.execute_batch(&format!("DROP TRIGGER IF EXISTS sync_topics_INSERT; DROP TRIGGER IF EXISTS sync_topics_UPDATE;
        CREATE TRIGGER sync_topics_INSERT AFTER INSERT ON topics BEGIN INSERT INTO sync_log(kind,entity_id,data) VALUES('topic',new.id,{TOPIC_JSON}); END;
        CREATE TRIGGER sync_topics_UPDATE AFTER UPDATE ON topics BEGIN INSERT INTO sync_log(kind,entity_id,data) VALUES('topic',new.id,{TOPIC_JSON}); END;
        CREATE TRIGGER IF NOT EXISTS sync_file_parts_INSERT AFTER INSERT ON chat_file_parts BEGIN INSERT INTO sync_log(kind,entity_id,data) VALUES('file_part',new.offer_id||':'||new.part,json_object('id',new.offer_id||':'||new.part,'offerId',new.offer_id,'part',new.part,'hex',new.hex)); END;"))?;
    if db.query_row(
        "SELECT EXISTS(SELECT 1 FROM metadata WHERE key='device-sync-journal' AND value='1')",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(());
    }
    let tx = db.transaction()?;
    for (table, kind, json) in [
        ("peers", "peer", "json(new.card)"),
        ("topics", "topic", TOPIC_JSON),
        ("messages", "message", MESSAGE_JSON),
    ] {
        for event in ["INSERT", "UPDATE"] {
            tx.execute_batch(&format!("CREATE TRIGGER IF NOT EXISTS sync_{table}_{event} AFTER {event} ON {table} BEGIN INSERT INTO sync_log(kind,entity_id,data) VALUES('{kind}',new.id,{json}); END;"))?;
        }
        let json = json.replace("new.", "");
        tx.execute_batch(&format!(
            "INSERT INTO sync_log(kind,entity_id,data) SELECT '{kind}',id,{json} FROM {table} ORDER BY rowid;"
        ))?;
    }
    set_metadata(&tx, "sync-epoch", &Uuid::new_v4().to_string())?;
    set_metadata(&tx, "device-sync-journal", "1")?;
    tx.commit()?;
    Ok(())
}

fn revision(db: &Connection, entity: &Entity) -> Result<i64> {
    let (kind, id) = entity.key();
    Ok(db.query_row(
        "SELECT MAX(seq) FROM sync_log WHERE kind=? AND entity_id=?",
        params![kind, id],
        |r| r.get(0),
    )?)
}
fn accepted(db: &Connection, entity: Entity) -> Result<CommandResult> {
    Ok(CommandResult::Accepted {
        revision: revision(db, &entity)?,
        entity,
    })
}

fn record(
    db: &Connection,
    device: &str,
    id: &str,
    hash: &str,
    result: &CommandResult,
) -> Result<()> {
    db.execute(
        "INSERT INTO device_commands VALUES(?,?,?,?)",
        params![device, id, hash, serde_json::to_string(result)?],
    )?;
    Ok(())
}
impl Endpoint {
    pub fn devices(&self) -> Result<Vec<DeviceStatus>> {
        let mut stmt=self.db.prepare("SELECT card,revoked,created_at,last_seen,ack_cursor FROM devices ORDER BY created_at,id")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, bool>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })?;
        rows.map(|r| {
            let (card, revoked, created_at, last_seen, acknowledged_cursor) = r?;
            Ok(DeviceStatus {
                card: serde_json::from_str(&card)?,
                revoked,
                created_at,
                last_seen,
                acknowledged_cursor,
            })
        })
        .collect()
    }
    pub fn authorize_device(
        &mut self,
        card: DeviceCard,
        server: &str,
        ca_pem: &str,
    ) -> Result<Pairing> {
        card.validate()?;
        super::device::validate_server(server, ca_pem)?;
        if let Some((old, revoked)) = self
            .db
            .query_row(
                "SELECT card,revoked FROM devices WHERE id=?",
                [&card.id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)),
            )
            .optional()?
        {
            ensure!(
                !revoked,
                "revoked device keys cannot be enrolled again; create a new device identity"
            );
            ensure!(
                serde_json::from_str::<DeviceCard>(&old)? == card,
                "device card changed"
            );
        } else {
            self.db.execute(
                "INSERT INTO devices(id,card,created_at) VALUES(?,?,?)",
                params![card.id, serde_json::to_string(&card)?, now()],
            )?;
        }
        let mut pair = Pairing {
            version: DEVICE_VERSION,
            device: card,
            domain: self.contact_card()?,
            server: server.trim_end_matches('/').into(),
            ca_pem: ca_pem.into(),
            epoch: metadata(&self.db, "sync-epoch")?,
            signature: String::new(),
        };
        pair.signature = load_account(&self.db, &self.key)?
            .sign(pair.signing_bytes())
            .to_base64();
        Ok(pair)
    }
    pub fn revoke_device(&mut self, id: &str) -> Result<()> {
        ensure!(
            self.db
                .execute("UPDATE devices SET revoked=1 WHERE id=?", [id])?
                == 1,
            "unknown device"
        );
        Ok(())
    }
    pub fn active_device(&self, id: &str) -> Result<()> {
        ensure!(
            self.db.query_row(
                "SELECT EXISTS(SELECT 1 FROM devices WHERE id=? AND revoked=0)",
                [id],
                |r| r.get::<_, bool>(0)
            )?,
            "device is unknown or revoked"
        );
        Ok(())
    }
    pub fn authenticate_device(
        &mut self,
        auth: &DeviceAuth,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> Result<String> {
        let id = auth.validate(&self.contact_card()?.user_id, method, path, body)?;
        self.active_device(&id)?;
        let tx = self.db.transaction()?;
        tx.execute(
            "DELETE FROM device_nonces WHERE timestamp<?",
            [now() - AUTH_WINDOW],
        )?;
        tx.execute(
            "INSERT INTO device_nonces VALUES(?,?,?)",
            params![id, auth.nonce, auth.timestamp],
        )
        .map_err(|_| anyhow::anyhow!("device request replayed"))?;
        tx.execute(
            "UPDATE devices SET last_seen=? WHERE id=?",
            params![now(), id],
        )?;
        tx.commit()?;
        Ok(id)
    }
    pub fn sign_device_response<T: Serialize>(
        &self,
        device: &str,
        nonce: &str,
        data: T,
    ) -> Result<SignedResponse<T>> {
        self.active_device(device)?;
        let mut reply = SignedResponse {
            version: DEVICE_VERSION,
            domain_id: self.contact_card()?.user_id,
            device_id: device.into(),
            nonce: nonce.into(),
            data,
            signature: String::new(),
        };
        reply.signature = load_account(&self.db, &self.key)?
            .sign(reply.signing_bytes())
            .to_base64();
        Ok(reply)
    }
    pub fn device_page(
        &mut self,
        device: &str,
        epoch: &str,
        cursor: i64,
        limit: usize,
    ) -> Result<Page> {
        self.active_device(device)?;
        ensure!(
            epoch == metadata(&self.db, "sync-epoch")?,
            "sync epoch mismatch; do not reuse a replica for another center"
        );
        let high_water: i64 =
            self.db
                .query_row("SELECT COALESCE(MAX(seq),0) FROM sync_log", [], |r| {
                    r.get(0)
                })?;
        ensure!(
            cursor >= 0 && cursor <= high_water,
            "sync cursor is ahead of server; possible database rollback"
        );
        ensure!((1..=PAGE_LIMIT).contains(&limit), "invalid page limit");
        let mut stmt = self.db.prepare(
            "SELECT seq,kind,data FROM sync_log WHERE seq>? AND seq<=? ORDER BY seq LIMIT ?",
        )?;
        let mut changes = Vec::new();
        let mut encoded_bytes = 0;
        for r in stmt.query_map(params![cursor, high_water, limit as i64], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })? {
            let (seq, kind, data) = r?;
            // Count serialized snapshots, including JSON escaping. A count-only
            // limit can exceed the native client's response budget for 64 KiB bodies.
            let size = data.len() + kind.len() + 128;
            if !changes.is_empty() && encoded_bytes + size > 8 * 1024 * 1024 {
                break;
            }
            encoded_bytes += size;
            let entity = serde_json::from_value(
                serde_json::json!({"kind":kind,"data":serde_json::from_str::<serde_json::Value>(&data)?}),
            )?;
            changes.push(Change { seq, entity });
        }
        let next_cursor = changes.last().map_or(cursor, |c| c.seq);
        self.db.execute(
            "UPDATE devices SET served_cursor=MAX(served_cursor,?) WHERE id=?",
            params![next_cursor, device],
        )?;
        Ok(Page {
            epoch: epoch.into(),
            from_cursor: cursor,
            next_cursor,
            high_water,
            changes,
        })
    }
    pub fn device_ack(&mut self, device: &str, ack: &Ack) -> Result<Ack> {
        self.active_device(device)?;
        ensure!(
            ack.epoch == metadata(&self.db, "sync-epoch")?,
            "sync epoch mismatch"
        );
        let served: i64 = self.db.query_row(
            "SELECT served_cursor FROM devices WHERE id=?",
            [device],
            |r| r.get(0),
        )?;
        ensure!(
            ack.cursor >= 0 && ack.cursor <= served,
            "cannot acknowledge an unserved cursor"
        );
        self.db.execute(
            "UPDATE devices SET ack_cursor=MAX(ack_cursor,?) WHERE id=?",
            params![ack.cursor, device],
        )?;
        Ok(ack.clone())
    }
    fn validate_device_operation(&self, op: &Operation) -> Result<()> {
        match op {
            Operation::AddPeer { card } => {
                card.validate()?;
                ensure!(
                    card.user_id != self.contact_card()?.user_id,
                    "cannot add self as peer"
                );
                if let Some(old) = self.peer_optional(&card.user_id)? {
                    ensure!(
                        old.signing_key == card.signing_key && old.curve_key == card.curve_key,
                        "peer key replacement is forbidden"
                    );
                }
            }
            Operation::CreateTopic {
                topic_id,
                peer_id,
                title,
            } => {
                self.peer(peer_id)?;
                Event::TopicUpdate {
                    topic_id: topic_id.clone(),
                    title: title.clone(),
                    created_at: now(),
                    archived: false,
                }
                .validate()?;
                ensure!(
                    !self.db.query_row(
                        "SELECT EXISTS(SELECT 1 FROM topics WHERE id=?)",
                        [topic_id],
                        |r| r.get::<_, bool>(0)
                    )?,
                    "topic ID collision"
                );
            }
            Operation::UpdateTopic {
                topic_id,
                title,
                archived,
                base_title,
                base_archived,
            } => {
                let t = self.topic(topic_id)?;
                ensure!(
                    t.title == *base_title && t.archived == *base_archived,
                    "topic changed on another device; refresh and try again"
                );
                Event::TopicUpdate {
                    topic_id: topic_id.clone(),
                    title: title.clone(),
                    created_at: t.created_at,
                    archived: *archived,
                }
                .validate()?;
            }
            Operation::Send {
                message_id,
                topic_id,
                body,
                format,
                reply_to,
                timestamp,
            } => {
                let t = self.topic(topic_id)?;
                ensure!(!t.archived, "topic is archived");
                ensure!(*timestamp >= 0, "invalid timestamp");
                Event::Message {
                    message_id: message_id.clone(),
                    topic_id: topic_id.clone(),
                    topic_title: t.title,
                    created_at: t.created_at,
                    body: body.clone(),
                    format: format.clone(),
                    reply_to: reply_to.clone(),
                }
                .validate()?;
            }
            Operation::Control {
                operation_id,
                topic_id,
                body,
            } => {
                let t = self.topic(topic_id)?;
                ensure!(!t.archived, "topic is archived");
                Event::Control {
                    operation_id: operation_id.clone(),
                    topic_id: topic_id.clone(),
                    topic_title: t.title,
                    created_at: t.created_at,
                    body: body.clone(),
                }
                .validate()?;
                let s = cipherwhisper_protocol::special::Special::parse(body)?;
                chat::validate_action(&self.db, topic_id, &self.contact_card()?.user_id, &s)?;
            }
        };
        Ok(())
    }
    pub async fn device_command(&mut self, device: &str, command: Command) -> Result<CommandReply> {
        self.active_device(device)?;
        uuid(&command.id)?;
        let hash = digest(&serde_json::to_vec(&command)?);
        if let Some((old, result)) = self
            .db
            .query_row(
                "SELECT digest,result FROM device_commands WHERE device_id=? AND id=?",
                params![device, command.id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?
        {
            ensure!(
                old == hash,
                "command ID cannot be reused with different content"
            );
            return Ok(CommandReply {
                id: command.id,
                body_digest: hash,
                result: serde_json::from_str(&result)?,
            });
        }
        if let Err(e) = self.validate_device_operation(&command.operation) {
            return self.reject_command(device, &command, &hash, e.to_string());
        }
        match &command.operation {
            Operation::Send { topic_id, .. }
            | Operation::Control { topic_id, .. }
            | Operation::UpdateTopic { topic_id, .. } => {
                let t = self.topic(topic_id)?;
                self.ensure_session(&t.peer_id).await?;
            }
            _ => {}
        }
        let result = match &command.operation {
            Operation::AddPeer { card } => {
                let tx = self.db.transaction()?;
                tx.execute("INSERT INTO peers VALUES(?,?) ON CONFLICT(id) DO UPDATE SET card=excluded.card",params![card.user_id,serde_json::to_string(card)?])?;
                let result = accepted(&tx, Entity::Peer(card.clone()))?;
                record(&tx, device, &command.id, &hash, &result)?;
                tx.commit()?;
                result
            }
            Operation::CreateTopic {
                topic_id,
                peer_id,
                title,
            } => {
                let t = Topic {
                    id: topic_id.clone(),
                    peer_id: peer_id.clone(),
                    title: title.trim().into(),
                    created_at: now(),
                    updated_at: now(),
                    archived: false,
                    pinned: false,
                    tags: Vec::new(),
                    status: "open".into(),
                };

                let tx = self.db.transaction()?;
                tx.execute(
                    "INSERT INTO topics VALUES(?,?,?,?,?,?)",
                    params![
                        t.id,
                        t.peer_id,
                        t.title,
                        t.created_at,
                        t.updated_at,
                        t.archived
                    ],
                )?;
                let result = accepted(&tx, Entity::Topic(t.clone()))?;
                record(&tx, device, &command.id, &hash, &result)?;
                tx.commit()?;
                result
            }
            Operation::Send {
                message_id,
                topic_id,
                body,
                format,
                reply_to,
                ..
            } => {
                let t = self.topic(topic_id)?;
                let event = Event::Message {
                    message_id: message_id.clone(),
                    topic_id: topic_id.clone(),
                    topic_title: t.title,
                    created_at: t.created_at,
                    body: body.clone(),
                    format: format.clone(),
                    reply_to: reply_to.clone(),
                };
                let queued = self.queue_event_transaction(&t.peer_id, event, |tx, _| {
                    let entity = Entity::Message(tx.query_row(
                        "SELECT * FROM messages WHERE id=?",
                        [message_id],
                        message_row,
                    )?);
                    record(tx, device, &command.id, &hash, &accepted(tx, entity)?)
                });
                if let Err(e) = queued {
                    return self.reject_command(device, &command, &hash, e.to_string());
                }
                self.cached_command(device, &command.id)?
            }
            Operation::Control {
                operation_id,
                topic_id,
                body,
            } => {
                let t = self.topic(topic_id)?;
                let event = Event::Control {
                    operation_id: operation_id.clone(),
                    topic_id: topic_id.clone(),
                    topic_title: t.title,
                    created_at: t.created_at,
                    body: body.clone(),
                };
                let queued = self.queue_event_transaction(&t.peer_id, event, |tx, _| {
                    let entity = chat::control_result_entity(tx, operation_id, topic_id, body)?;
                    record(tx, device, &command.id, &hash, &accepted(tx, entity)?)
                });
                if let Err(e) = queued {
                    return self.reject_command(device, &command, &hash, e.to_string());
                }
                self.cached_command(device, &command.id)?
            }
            Operation::UpdateTopic {
                topic_id,
                title,
                archived,
                ..
            } => {
                let t = self.topic(topic_id)?;
                let event = Event::TopicUpdate {
                    topic_id: topic_id.clone(),
                    title: title.trim().into(),
                    created_at: t.created_at,
                    archived: *archived,
                };
                let queued = self.queue_event_transaction(&t.peer_id, event, |tx, _| {
                    let t =
                        tx.query_row("SELECT * FROM topics WHERE id=?", [topic_id], topic_row)?;
                    ensure!(t.title == title.trim() && t.archived == *archived,
                        "topic timestamp is ahead of the center clock; retry after correcting the clock");
                    record(
                        tx,
                        device,
                        &command.id,
                        &hash,
                        &accepted(tx, Entity::Topic(t))?,
                    )
                });
                if let Err(e) = queued {
                    return self.reject_command(device, &command, &hash, e.to_string());
                }
                self.cached_command(device, &command.id)?
            }
        };
        Ok(CommandReply {
            id: command.id,
            body_digest: hash,
            result,
        })
    }
    fn cached_command(&self, device: &str, id: &str) -> Result<CommandResult> {
        Ok(serde_json::from_str(&self.db.query_row(
            "SELECT result FROM device_commands WHERE device_id=? AND id=?",
            params![device, id],
            |r| r.get::<_, String>(0),
        )?)?)
    }
    fn reject_command(
        &mut self,
        device: &str,
        command: &Command,
        hash: &str,
        error: String,
    ) -> Result<CommandReply> {
        let current = match &command.operation {
            Operation::UpdateTopic { topic_id, .. } => self
                .topic(topic_id)
                .ok()
                .map(Entity::Topic)
                .map(|entity| -> Result<Change> {
                    Ok(Change {
                        seq: revision(&self.db, &entity)?,
                        entity,
                    })
                })
                .transpose()?,
            _ => None,
        };
        let result = CommandResult::Rejected { error, current };
        record(&self.db, device, &command.id, hash, &result)?;
        Ok(CommandReply {
            id: command.id.clone(),
            body_digest: hash.into(),
            result,
        })
    }
}
