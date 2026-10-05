//! Known application operations mutate history; only unknown payloads become raw history entries.
use super::*;
use cipherwhisper_protocol::device::{Entity, FilePart};
use cipherwhisper_protocol::file::FileOffer;
use cipherwhisper_protocol::special::{CHUNK_BYTES, MAX_FILE_BYTES, Special};
use serde_json::{Value, json};

pub(crate) fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS replica_message_order(id TEXT PRIMARY KEY,seq INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS chat_journal(seq INTEGER PRIMARY KEY AUTOINCREMENT,id TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS chat_journal_id ON chat_journal(id,seq);
        CREATE TRIGGER IF NOT EXISTS chat_order_update AFTER UPDATE ON replica_message_order WHEN new.seq<old.seq BEGIN INSERT INTO chat_journal(id) VALUES(new.id); END;
        CREATE TABLE IF NOT EXISTS chat_drafts(topic_id TEXT PRIMARY KEY REFERENCES topics(id),sealed TEXT NOT NULL,revision INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE IF NOT EXISTS chat_topic_meta(topic_id TEXT PRIMARY KEY REFERENCES topics(id),data TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS chat_clocks(target TEXT NOT NULL,kind TEXT NOT NULL,revision INTEGER NOT NULL,operation_id TEXT NOT NULL,PRIMARY KEY(target,kind));
        CREATE TABLE IF NOT EXISTS chat_file_sources(id TEXT PRIMARY KEY,topic_id TEXT NOT NULL,name TEXT NOT NULL,mime TEXT NOT NULL,size INTEGER NOT NULL,sha256 TEXT NOT NULL,next_part INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE IF NOT EXISTS chat_file_source_chunks(file_id TEXT NOT NULL,part INTEGER NOT NULL,sealed TEXT NOT NULL,PRIMARY KEY(file_id,part));
        CREATE TABLE IF NOT EXISTS chat_file_parts(offer_id TEXT NOT NULL REFERENCES messages(id),part INTEGER NOT NULL,hex TEXT NOT NULL,PRIMARY KEY(offer_id,part));
        CREATE TRIGGER IF NOT EXISTS chat_message_insert AFTER INSERT ON messages BEGIN INSERT INTO chat_journal(id) VALUES(new.id); END;
        CREATE TRIGGER IF NOT EXISTS chat_message_update AFTER UPDATE ON messages WHEN old.delivery<>new.delivery OR old.body<>new.body OR old.format<>new.format BEGIN INSERT INTO chat_journal(id) VALUES(new.id); END;")?;
    if !db.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('chat_drafts') WHERE name='revision')",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        db.execute_batch("ALTER TABLE chat_drafts ADD COLUMN revision INTEGER NOT NULL DEFAULT 0")?;
    }
    Ok(())
}
fn message(db: &Connection, id: &str) -> Result<Message> {
    Ok(db.query_row("SELECT * FROM messages WHERE id=?", [id], message_row)?)
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Draft {
    pub body: String,
    pub reply_to: Option<String>,
    #[serde(default)]
    pub revision: i64,
}
pub(crate) fn load_draft(db: &Connection, key: &[u8; 32], topic: &str) -> Result<Draft> {
    let raw: Option<String> = db
        .query_row(
            "SELECT sealed FROM chat_drafts WHERE topic_id=?",
            [topic],
            |r| r.get(0),
        )
        .optional()?;
    raw.map(|v| {
        vault::unseal(
            &v,
            key,
            format!("cipherwhisper.draft.v1:{topic}").as_bytes(),
        )
    })
    .transpose()
    .map(|d| d.unwrap_or_default())
}
pub(crate) fn save_draft(
    db: &Connection,
    key: &[u8; 32],
    topic: &str,
    draft: &Draft,
) -> Result<()> {
    ensure!(
        draft.body.len() <= MAX_BODY && draft.revision >= 0,
        "invalid draft"
    );
    if let Some(id) = &draft.reply_to {
        uuid(id)?;
    }
    // Keep an encrypted empty draft too, so a delayed save cannot resurrect sent text.
    db.execute("INSERT INTO chat_drafts(topic_id,sealed,revision) VALUES(?,?,?) ON CONFLICT(topic_id) DO UPDATE SET sealed=excluded.sealed,revision=excluded.revision WHERE excluded.revision>=chat_drafts.revision",params![topic,vault::seal(draft,key,format!("cipherwhisper.draft.v1:{topic}").as_bytes())?,draft.revision])?;
    Ok(())
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TopicMeta {
    pub pinned: bool,
    pub tags: Vec<String>,
    pub status: String,
}
pub(crate) fn topic_meta(db: &Connection, id: &str) -> Result<TopicMeta> {
    let raw: Option<String> = db
        .query_row(
            "SELECT data FROM chat_topic_meta WHERE topic_id=?",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    raw.map(|s| serde_json::from_str(&s).map_err(Into::into))
        .unwrap_or_else(|| {
            Ok(TopicMeta {
                status: "open".into(),
                ..Default::default()
            })
        })
}
pub(crate) fn decorate_topic(db: &Connection, mut t: Topic) -> Result<Topic> {
    let meta = topic_meta(db, &t.id)?;
    t.pinned = meta.pinned;
    t.tags = meta.tags;
    t.status = meta.status;
    Ok(t)
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileView {
    pub file_id: String,
    pub name: String,
    pub mime: String,
    pub size: usize,
    pub sha256: String,
    pub chunk_size: usize,
    pub state: String,
    pub received: usize,
    pub chunks: usize,
    pub accept_id: Option<String>,
    pub error: Option<String>,
}
fn file_message(db: &Connection, id: &str) -> Result<(Message, FileView)> {
    let m = message(db, id)?;
    ensure!(m.format == "file", "target is not a file");
    let f = serde_json::from_str(&m.body)?;
    Ok((m, f))
}
fn update_file(db: &Connection, id: &str, f: &FileView) -> Result<()> {
    db.execute(
        "UPDATE messages SET body=? WHERE id=?",
        params![serde_json::to_string(f)?, id],
    )?;
    Ok(())
}
fn author_target(db: &Connection, topic: &str, sender: &str, s: &Special) -> Result<Message> {
    let t = message(db, s.text("messageId")?)?;
    ensure!(
        t.topic_id == topic && t.sender_id == sender,
        "only the author may change a message in the same topic"
    );
    ensure!(
        ["markdown", "markdown.edited", "withdrawn"].contains(&t.format.as_str()),
        "cannot edit special/file messages"
    );
    Ok(t)
}
fn consent_target(
    db: &Connection,
    topic: &str,
    sender: &str,
    s: &Special,
) -> Result<(Message, FileView)> {
    let (offer, f) = file_message(db, s.text("offerId")?)?;
    ensure!(
        offer.topic_id == topic && offer.sender_id != sender && f.file_id == s.target(),
        "invalid file acceptance"
    );
    Ok((offer, f))
}
fn chunk_target(
    db: &Connection,
    topic: &str,
    sender: &str,
    s: &Special,
) -> Result<(Message, FileView)> {
    let (offer, f) = file_message(db, s.text("offerId")?)?;
    ensure!(
        offer.topic_id == topic
            && offer.sender_id == sender
            && f.file_id == s.target()
            && f.accept_id.as_deref() == Some(s.text("acceptId")?),
        "unrequested file chunk"
    );
    let part = s.number("part")? as usize;
    ensure!(
        part < f.chunks
            && s.text("hex")?.len() == (f.size - part * CHUNK_BYTES).min(CHUNK_BYTES) * 2,
        "chunk differs from file metadata"
    );
    Ok((offer, f))
}
pub(crate) fn validate_action(
    db: &Connection,
    topic: &str,
    sender: &str,
    s: &Special,
) -> Result<()> {
    s.validate()?;
    match s.kind.as_str() {
        "message.edit" => {
            let m = author_target(db, topic, sender, s)?;
            ensure!(
                m.format != "withdrawn",
                "withdrawn message cannot be edited"
            );
        }
        "message.withdraw" => {
            author_target(db, topic, sender, s)?;
        }
        "file.accept" => {
            consent_target(db, topic, sender, s)?;
        }
        "file.chunk" => {
            chunk_target(db, topic, sender, s)?;
        }
        _ => {}
    }
    Ok(())
}
fn clock(db: &Connection, target: &str, s: &Special, op: &str, timestamp: i64) -> Result<bool> {
    let revision = s
        .data
        .get("revision")
        .and_then(Value::as_i64)
        .unwrap_or(timestamp.saturating_mul(1000));
    let old: Option<(i64, String)> = db
        .query_row(
            "SELECT revision,operation_id FROM chat_clocks WHERE target=? AND kind=?",
            params![target, s.kind],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if old
        .as_ref()
        .is_some_and(|(r, id)| (*r, id.as_str()) >= (revision, op))
    {
        return Ok(false);
    }
    db.execute("INSERT INTO chat_clocks VALUES(?,?,?,?) ON CONFLICT(target,kind) DO UPDATE SET revision=excluded.revision,operation_id=excluded.operation_id",params![target,s.kind,revision,op])?;
    Ok(true)
}
fn stamp(db: &Connection, topic: &str, s: &mut Special) -> Result<()> {
    if matches!(
        s.kind.as_str(),
        "message.edit" | "message.withdraw" | "topic.meta"
    ) {
        let target = if s.kind == "topic.meta" {
            topic
        } else {
            s.target()
        };
        let old: i64 = db.query_row(
            "SELECT COALESCE(MAX(revision),0) FROM chat_clocks WHERE target=?",
            [target],
            |r| r.get(0),
        )?;
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis()
            .min(i64::MAX as u128) as i64;
        s.data["revision"] = json!(ms.max(old.saturating_add(1)));
    }
    Ok(())
}
// Called inside the ratchet/history transaction. Known operations are never inserted
// as messages. Missing dependencies cause retry without advancing the ratchet.
pub(crate) fn apply(
    db: &Connection,
    env: &Envelope,
    message_id: &str,
    topic: &str,
    s: &Special,
    outgoing: bool,
) -> Result<()> {
    match s.kind.as_str() {
        "message.edit" | "message.withdraw" => {
            let m = author_target(db, topic, &env.from, s)?;
            if s.kind == "message.edit" && m.format == "withdrawn" {
                return Ok(());
            }
            if clock(db, &m.id, s, message_id, env.timestamp)? {
                let (body, format) = if s.kind == "message.withdraw" {
                    ("", "withdrawn")
                } else {
                    (s.text("body")?, "markdown.edited")
                };
                db.execute(
                    "UPDATE messages SET body=?,format=? WHERE id=?",
                    params![body, format, m.id],
                )?;
            }
        }
        "topic.meta" => {
            if clock(db, topic, s, message_id, env.timestamp)? {
                let meta = TopicMeta {
                    pinned: s.data["pinned"].as_bool().unwrap(),
                    tags: serde_json::from_value(s.data["tags"].clone())?,
                    status: s.text("status")?.into(),
                };
                db.execute("INSERT INTO chat_topic_meta VALUES(?,?) ON CONFLICT(topic_id) DO UPDATE SET data=excluded.data",params![topic,serde_json::to_string(&meta)?])?;
                db.execute(
                    "UPDATE topics SET updated_at=MAX(updated_at,?) WHERE id=?",
                    params![env.timestamp, topic],
                )?;
            }
        }
        "file.accept" => {
            let (offer, mut f) = consent_target(db, topic, &env.from, s)?;
            if f.accept_id.is_none() {
                let hash = hex::decode(digest(
                    serde_json::to_vec(&("cipherwhisper.file.consent.v1", &offer.id, &env.from))?
                        .as_slice(),
                ))?;
                let mut id: [u8; 16] = hash[..16].try_into()?;
                id[6] = (id[6] & 0x0f) | 0x40;
                id[8] = (id[8] & 0x3f) | 0x80;
                f.accept_id = Some(Uuid::from_bytes(id).to_string());
                f.state = "accepted".into();
                if f.size == 0 {
                    f.state = if outgoing { "complete" } else { "sent" }.into();
                    if digest(b"") != f.sha256 {
                        f.state = "failed".into();
                        f.error = Some("文件 SHA-256 校验失败".into());
                    }
                }
                update_file(db, &offer.id, &f)?;
            }
        }
        "file.chunk" => {
            let (offer, mut f) = chunk_target(db, topic, &env.from, s)?;
            let part = s.number("part")? as usize;
            if outgoing {
                f.received = f.received.max(part + 1);
                f.state = if f.received == f.chunks {
                    "sent"
                } else {
                    "transferring"
                }
                .into();
            } else {
                store_part(
                    db,
                    &FilePart {
                        id: format!("{}:{part}", offer.id),
                        offer_id: offer.id.clone(),
                        part,
                        hex: s.text("hex")?.into(),
                    },
                )?;
                let parts = file_parts(db, &offer.id)?;
                f.received = parts.len();
                f.state = "transferring".into();
                if f.received == f.chunks {
                    let bytes = assemble(&parts, &f)?;
                    if digest(&bytes) == f.sha256 {
                        f.state = "complete".into();
                    } else {
                        f.state = "failed".into();
                        f.error = Some("文件 SHA-256 校验失败".into());
                    }
                }
            }
            update_file(db, &offer.id, &f)?;
        }
        _ => anyhow::bail!("unknown special operation"),
    }
    Ok(())
}
fn invitation_view(f: FileOffer) -> FileView {
    FileView {
        file_id: f.file_id,
        name: f.name,
        mime: f.mime,
        size: f.size,
        sha256: f.sha256.to_lowercase(),
        chunk_size: f.chunk_size,
        state: "offered".into(),
        received: 0,
        chunks: f.size.div_ceil(CHUNK_BYTES),
        accept_id: None,
        error: None,
    }
}
pub(crate) fn apply_file(
    db: &Connection,
    env: &Envelope,
    id: &str,
    topic: &str,
    body: &str,
    outgoing: bool,
) -> Result<()> {
    let f: FileOffer = serde_json::from_str(body)?;
    f.validate()?;
    let count: i64 = db.query_row(
        "SELECT COUNT(*) FROM messages WHERE format='file' AND json_extract(body,'$.fileId')=?",
        [&f.file_id],
        |r| r.get(0),
    )?;
    ensure!(count == 0, "file ID collision");
    db.execute(
        "INSERT INTO messages VALUES(?,?,?,?,?,'file',NULL,?)",
        params![
            id,
            topic,
            env.from,
            env.timestamp,
            serde_json::to_string(&invitation_view(f))?,
            if outgoing { "queued" } else { "received" }
        ],
    )?;
    db.execute(
        "UPDATE topics SET updated_at=MAX(updated_at,?) WHERE id=?",
        params![env.timestamp, topic],
    )?;
    Ok(())
}
pub(crate) fn store_part(db: &Connection, p: &FilePart) -> Result<()> {
    ensure!(
        p.id == format!("{}:{}", p.offer_id, p.part),
        "invalid file part identity"
    );
    let (_, f) = file_message(db, &p.offer_id)?;
    ensure!(
        p.part < f.chunks && p.hex.len() == (f.size - p.part * CHUNK_BYTES).min(CHUNK_BYTES) * 2,
        "invalid synchronized file chunk"
    );
    hex::decode(&p.hex)?;
    if let Some(old) = db
        .query_row(
            "SELECT hex FROM chat_file_parts WHERE offer_id=? AND part=?",
            params![p.offer_id, p.part as i64],
            |r| r.get::<_, String>(0),
        )
        .optional()?
    {
        ensure!(old.eq_ignore_ascii_case(&p.hex), "conflicting file chunk");
        return Ok(());
    }
    db.execute(
        "INSERT INTO chat_file_parts VALUES(?,?,?)",
        params![p.offer_id, p.part as i64, p.hex.to_ascii_lowercase()],
    )?;
    Ok(())
}
fn file_parts(db: &Connection, offer: &str) -> Result<Vec<(usize, Vec<u8>)>> {
    let mut stmt =
        db.prepare("SELECT part,hex FROM chat_file_parts WHERE offer_id=? ORDER BY part")?;
    let rows = stmt
        .query_map([offer], |r| {
            Ok((r.get::<_, i64>(0)? as usize, r.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(|(p, s)| Ok((p, hex::decode(s)?)))
        .collect()
}
fn assemble(parts: &[(usize, Vec<u8>)], f: &FileView) -> Result<Vec<u8>> {
    ensure!(
        parts.len() == f.chunks
            && parts.iter().enumerate().all(
                |(i, (p, b))| i == *p && b.len() == (f.size - i * CHUNK_BYTES).min(CHUNK_BYTES)
            ),
        "file incomplete"
    );
    Ok(parts.iter().flat_map(|(_, b)| b.iter().copied()).collect())
}
pub(crate) fn control_result_entity(
    db: &Connection,
    id: &str,
    topic: &str,
    body: &str,
) -> Result<Entity> {
    if let Some(m) = db
        .query_row("SELECT * FROM messages WHERE id=?", [id], message_row)
        .optional()?
    {
        return Ok(Entity::Message(m));
    }
    if let Ok(s) = Special::parse(body) {
        match s.kind.as_str() {
            "topic.meta" => {
                return Ok(Entity::Topic(decorate_topic(
                    db,
                    db.query_row("SELECT * FROM topics WHERE id=?", [topic], topic_row)?,
                )?));
            }
            "message.edit" | "message.withdraw" => {
                return Ok(Entity::Message(message(db, s.target())?));
            }
            "file.accept" | "file.chunk" => {
                return Ok(Entity::Message(message(db, s.text("offerId")?)?));
            }
            _ => {}
        }
    }
    Ok(Entity::Message(message(db, id)?))
}
pub(crate) fn control_result_message(
    db: &Connection,
    id: &str,
    topic: &str,
    body: &str,
    sender: &str,
) -> Result<Message> {
    match control_result_entity(db, id, topic, body)? {
        Entity::Message(m) => Ok(m),
        _ => Ok(Message {
            id: id.into(),
            topic_id: topic.into(),
            sender_id: sender.into(),
            timestamp: now(),
            body: String::new(),
            format: "control".into(),
            reply_to: None,
            delivery: "queued".into(),
        }),
    }
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    #[serde(flatten)]
    pub message: Message,
    pub sequence: i64,
    pub edited: bool,
    pub withdrawn: bool,
    pub special: Option<Value>,
    pub special_kind: Option<String>,
    pub special_error: Option<String>,
    pub file: Option<FileView>,
}
pub(crate) fn project(db: &Connection, m: Message) -> Result<ChatMessage> {
    let sequence = db
        .query_row("SELECT COALESCE(o.seq,m.rowid) FROM messages m LEFT JOIN replica_message_order o ON o.id=m.id WHERE m.id=?", [&m.id], |r| {
            r.get::<_, i64>(0)
        })
        .optional()?
        .unwrap_or(i64::MAX);
    let edited = m.format == "markdown.edited";
    let withdrawn = m.format == "withdrawn";
    let file = if m.format == "file" {
        Some(
            serde_json::from_str(&m.body)
                .or_else(|_| serde_json::from_str::<FileOffer>(&m.body).map(invitation_view))?,
        )
    } else {
        None
    };
    let special = if m.format == "control.unknown" {
        serde_json::from_str(&m.body).ok()
    } else {
        None
    };
    let special_kind = if m.format == "control.unknown" {
        Some("unknown".into())
    } else {
        None
    };
    let special_error = (m.format == "control.unknown").then(|| {
        Special::parse(&m.body)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_else(|| "操作目标或权限无效".into())
    });
    Ok(ChatMessage {
        message: m,
        sequence,
        edited,
        withdrawn,
        special,
        special_kind,
        special_error,
        file,
    })
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessagePage {
    pub items: Vec<ChatMessage>,
    pub older_cursor: Option<String>,
    pub has_more: bool,
    pub revision: i64,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageChanges {
    pub items: Vec<ChatMessage>,
    pub revision: i64,
    pub has_more: bool,
}
fn revision(db: &Connection) -> Result<i64> {
    Ok(
        db.query_row("SELECT COALESCE(MAX(seq),0) FROM chat_journal", [], |r| {
            r.get(0)
        })?,
    )
}
pub(crate) fn page(
    db: &Connection,
    topic: &str,
    before: Option<&str>,
    around: Option<&str>,
    limit: usize,
) -> Result<MessagePage> {
    ensure!((1..=100).contains(&limit), "invalid page limit");
    let bound = if let Some(id) = before {
        db.query_row(
            "SELECT COALESCE(o.seq,m.rowid) FROM messages m LEFT JOIN replica_message_order o ON o.id=m.id WHERE m.id=? AND m.topic_id=?",
            params![id, topic],
            |r| r.get::<_, i64>(0),
        )?
    } else {
        i64::MAX
    };
    let mut rows = if let Some(id) = around {
        let center: i64 = db.query_row(
            "SELECT COALESCE(o.seq,m.rowid) FROM messages m LEFT JOIN replica_message_order o ON o.id=m.id WHERE m.id=? AND m.topic_id=?",
            params![id, topic],
            |r| r.get(0),
        )?;
        let mut stmt = db.prepare(
            "SELECT m.* FROM messages m LEFT JOIN replica_message_order o ON o.id=m.id WHERE m.topic_id=? AND COALESCE(o.seq,m.rowid)<=? ORDER BY COALESCE(o.seq,m.rowid) DESC LIMIT ?",
        )?;
        let mut older = stmt
            .query_map(
                params![topic, center, limit.div_ceil(2) as i64],
                message_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        older.reverse();
        let mut stmt = db.prepare(
            "SELECT m.* FROM messages m LEFT JOIN replica_message_order o ON o.id=m.id WHERE m.topic_id=? AND COALESCE(o.seq,m.rowid)>? ORDER BY COALESCE(o.seq,m.rowid) LIMIT ?",
        )?;
        older.extend(
            stmt.query_map(
                params![topic, center, (limit - limit.div_ceil(2)) as i64],
                message_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?,
        );
        older
    } else {
        let mut stmt = db.prepare(
            "SELECT m.* FROM messages m LEFT JOIN replica_message_order o ON o.id=m.id WHERE m.topic_id=? AND COALESCE(o.seq,m.rowid)<? ORDER BY COALESCE(o.seq,m.rowid) DESC LIMIT ?",
        )?;
        let mut rows = stmt
            .query_map(params![topic, bound, limit as i64], message_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.reverse();
        rows
    };
    let older_cursor = rows.first().map(|m| m.id.clone());
    let has_more = if let Some(id) = &older_cursor {
        db.query_row("SELECT EXISTS(SELECT 1 FROM messages m LEFT JOIN replica_message_order o ON o.id=m.id WHERE m.topic_id=? AND COALESCE(o.seq,m.rowid)<(SELECT COALESCE(o2.seq,m2.rowid) FROM messages m2 LEFT JOIN replica_message_order o2 ON o2.id=m2.id WHERE m2.id=?))",params![topic,id],|r|r.get(0))?
    } else {
        false
    };
    Ok(MessagePage {
        items: rows
            .drain(..)
            .map(|m| project(db, m))
            .collect::<Result<_>>()?,
        older_cursor,
        has_more,
        revision: revision(db)?,
    })
}
pub(crate) fn changes(db: &Connection, topic: &str, since: i64) -> Result<MessageChanges> {
    ensure!(
        since >= 0 && since <= revision(db)?,
        "invalid history revision"
    );
    let mut stmt=db.prepare("SELECT j.id,MAX(j.seq) AS seq FROM chat_journal j JOIN messages m ON m.id=j.id WHERE j.seq>? AND m.topic_id=? GROUP BY j.id ORDER BY seq LIMIT 101")?;
    let mut ids = stmt
        .query_map(params![since, topic], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let has_more = ids.len() > 100;
    ids.truncate(100);
    let next = if has_more {
        ids.last().map(|v| v.1).unwrap_or(since)
    } else {
        revision(db)?
    };
    Ok(MessageChanges {
        items: ids
            .into_iter()
            .map(|(id, _)| project(db, message(db, &id)?))
            .collect::<Result<_>>()?,
        revision: next,
        has_more,
    })
}
pub(crate) fn stage_file(
    db: &mut Connection,
    key: &[u8; 32],
    topic: &str,
    name: &str,
    mime: &str,
    bytes: &[u8],
) -> Result<FileOffer> {
    let id = Uuid::new_v4().to_string();
    let s = FileOffer {
        version: 1,
        file_id: id.clone(),
        name: name.into(),
        mime: mime.into(),
        size: bytes.len(),
        sha256: digest(bytes),
        chunk_size: CHUNK_BYTES,
    };
    s.validate()?;
    ensure!(bytes.len() <= MAX_FILE_BYTES, "file exceeds 16 MiB");
    let tx = db.transaction()?;
    tx.execute(
        "INSERT INTO chat_file_sources(id,topic_id,name,mime,size,sha256) VALUES(?,?,?,?,?,?)",
        params![id, topic, name, mime, bytes.len() as i64, digest(bytes)],
    )?;
    for (part, chunk) in bytes.chunks(CHUNK_BYTES).enumerate() {
        tx.execute(
            "INSERT INTO chat_file_source_chunks VALUES(?,?,?)",
            params![
                id,
                part as i64,
                vault::seal(
                    &hex::encode(chunk),
                    key,
                    format!("cipherwhisper.file.v1:{id}:{part}").as_bytes()
                )?
            ],
        )?;
    }
    tx.commit()?;
    Ok(s)
}
pub(crate) fn next_chunk(
    db: &Connection,
    key: &[u8; 32],
    self_id: &str,
) -> Result<Option<(String, String, Special)>> {
    let mut stmt=db.prepare("SELECT f.id,f.topic_id,f.next_part FROM chat_file_sources f JOIN topics t ON t.id=f.topic_id WHERE f.next_part*? < f.size AND t.archived=0 ORDER BY f.rowid")?;
    let sources = stmt
        .query_map([CHUNK_BYTES as i64], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (id, topic, part) in sources {
        let mut stmt=db.prepare("SELECT * FROM messages WHERE format='file' AND topic_id=? AND sender_id=? AND json_extract(body,'$.fileId')=?")?;
        let offers = stmt
            .query_map(params![topic, self_id, id], message_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for offer in offers {
            let f: FileView = serde_json::from_str(&offer.body)?;
            let Some(accept_id) = f.accept_id else {
                continue;
            };
            if f.state == "failed" {
                continue;
            }
            let sealed: String = db.query_row(
                "SELECT sealed FROM chat_file_source_chunks WHERE file_id=? AND part=?",
                params![id, part],
                |r| r.get(0),
            )?;
            let raw: String = vault::unseal(
                &sealed,
                key,
                format!("cipherwhisper.file.v1:{id}:{part}").as_bytes(),
            )?;
            return Ok(Some((
                id.clone(),
                topic.clone(),
                Special::new(
                    "file.chunk",
                    json!({"fileId":id,"offerId":offer.id,"acceptId":accept_id,"part":part,"hex":raw}),
                ),
            )));
        }
    }
    Ok(None)
}
pub(crate) fn download(db: &Connection, offer: &str, self_id: &str) -> Result<(String, Vec<u8>)> {
    let (m, f) = file_message(db, offer)?;
    ensure!(
        m.sender_id != self_id,
        "download belongs to the receiving side"
    );
    ensure!(
        f.state == "complete",
        "file not complete or integrity validation failed"
    );
    let bytes = assemble(&file_parts(db, offer)?, &f)?;
    ensure!(digest(&bytes) == f.sha256, "file SHA-256 mismatch");
    Ok((f.name, bytes))
}
impl Endpoint {
    pub fn chat_page(
        &self,
        topic: &str,
        before: Option<&str>,
        around: Option<&str>,
        limit: usize,
    ) -> Result<MessagePage> {
        self.topic(topic)?;
        page(&self.db, topic, before, around, limit)
    }
    pub fn chat_changes(&self, topic: &str, since: i64) -> Result<MessageChanges> {
        self.topic(topic)?;
        changes(&self.db, topic, since)
    }
    pub fn draft(&self, topic: &str) -> Result<Draft> {
        self.topic(topic)?;
        load_draft(&self.db, &self.key, topic)
    }
    pub fn save_draft(&self, topic: &str, draft: &Draft) -> Result<()> {
        self.topic(topic)?;
        save_draft(&self.db, &self.key, topic, draft)
    }
    pub async fn send_special(&mut self, topic: &str, mut s: Special) -> Result<Message> {
        validate_action(&self.db, topic, &self.contact_card()?.user_id, &s)?;
        stamp(&self.db, topic, &mut s)?;
        let t = self.topic(topic)?;
        ensure!(!t.archived, "topic is archived");
        let id = Uuid::new_v4().to_string();
        let body = s.body()?;
        self.ensure_session(&t.peer_id).await?;
        self.queue_event(
            &t.peer_id,
            Event::Control {
                operation_id: id.clone(),
                topic_id: topic.into(),
                topic_title: t.title,
                created_at: t.created_at,
                body: body.clone(),
            },
        )?;
        control_result_message(&self.db, &id, topic, &body, &self.contact_card()?.user_id)
    }
    pub async fn offer_file(
        &mut self,
        topic: &str,
        name: &str,
        mime: &str,
        bytes: &[u8],
    ) -> Result<Message> {
        self.topic(topic)?;
        let s = stage_file(&mut self.db, &self.key, topic, name, mime, bytes)?;
        self.send_formatted(topic, &serde_json::to_string(&s)?, None, "file")
            .await
    }
    pub fn download_file(&self, offer: &str) -> Result<(String, Vec<u8>)> {
        download(&self.db, offer, &self.contact_card()?.user_id)
    }
    pub(crate) async fn pump_files(&mut self) -> Result<()> {
        let pending: i64 = self
            .db
            .query_row("SELECT COUNT(*) FROM outbox", [], |r| r.get(0))?;
        if pending >= 8 {
            return Ok(());
        }
        for _ in 0..2 {
            let Some((id, topic, s)) =
                next_chunk(&self.db, &self.key, &self.contact_card()?.user_id)?
            else {
                break;
            };
            let t = self.topic(&topic)?;
            if t.archived {
                break;
            }
            self.ensure_session(&t.peer_id).await?;
            let event = Event::Control {
                operation_id: Uuid::new_v4().to_string(),
                topic_id: topic,
                topic_title: t.title,
                created_at: t.created_at,
                body: s.body()?,
            };
            self.queue_event_transaction(&t.peer_id, event, |tx, _| {
                tx.execute(
                    "UPDATE chat_file_sources SET next_part=next_part+1 WHERE id=?",
                    [&id],
                )?;
                Ok(())
            })?;
        }
        Ok(())
    }
}
impl device::Replica {
    fn append_queued(&self, topic: &str, items: &mut Vec<ChatMessage>) -> Result<()> {
        for m in self
            .queued_messages()?
            .into_iter()
            .filter(|m| m.topic_id == topic)
            .take(100)
        {
            if items.iter().any(|item| item.message.id == m.id) {
                continue;
            }
            items.push(project(&self.db, m)?);
        }
        Ok(())
    }
    pub fn chat_page(
        &self,
        topic: &str,
        before: Option<&str>,
        around: Option<&str>,
        limit: usize,
    ) -> Result<MessagePage> {
        self.topic(topic)?;
        let mut p = page(&self.db, topic, before, around, limit)?;
        if before.is_none() && around.is_none() {
            self.append_queued(topic, &mut p.items)?;
        }
        Ok(p)
    }
    pub fn chat_changes(&self, topic: &str, since: i64) -> Result<MessageChanges> {
        self.topic(topic)?;
        let mut c = changes(&self.db, topic, since)?;
        self.append_queued(topic, &mut c.items)?;
        Ok(c)
    }
    pub fn draft(&self, topic: &str) -> Result<Draft> {
        self.topic(topic)?;
        load_draft(&self.db, &self.key, topic)
    }
    pub fn save_draft(&self, topic: &str, draft: &Draft) -> Result<()> {
        self.topic(topic)?;
        save_draft(&self.db, &self.key, topic, draft)
    }
    pub async fn send_special(&mut self, topic: &str, mut s: Special) -> Result<Message> {
        validate_action(&self.db, topic, &self.contact_card()?.user_id, &s)?;
        stamp(&self.db, topic, &mut s)?;
        ensure!(!self.topic(topic)?.archived, "topic is archived");
        let id = Uuid::new_v4().to_string();
        self.enqueue(cipherwhisper_protocol::device::Operation::Control {
            operation_id: id.clone(),
            topic_id: topic.into(),
            body: s.body()?,
        })?;
        Ok(Message {
            id,
            topic_id: topic.into(),
            sender_id: self.contact_card()?.user_id,
            timestamp: now(),
            body: String::new(),
            format: "control".into(),
            reply_to: None,
            delivery: "queued".into(),
        })
    }
    pub async fn offer_file(
        &mut self,
        topic: &str,
        name: &str,
        mime: &str,
        bytes: &[u8],
    ) -> Result<Message> {
        self.topic(topic)?;
        let s = stage_file(&mut self.db, &self.key, topic, name, mime, bytes)?;
        self.send_formatted(topic, &serde_json::to_string(&s)?, None, "file")
            .await
    }
    pub fn download_file(&self, offer: &str) -> Result<(String, Vec<u8>)> {
        download(&self.db, offer, &self.contact_card()?.user_id)
    }
    pub(crate) async fn pump_files(&mut self) -> Result<()> {
        if self
            .pending()?
            .iter()
            .filter(|p| p.state == "pending")
            .count()
            >= 8
        {
            return Ok(());
        }
        for _ in 0..2 {
            let Some((id, topic, s)) =
                next_chunk(&self.db, &self.key, &self.contact_card()?.user_id)?
            else {
                break;
            };
            if self.topic(&topic)?.archived {
                break;
            }
            self.send_special(&topic, s).await?;
            self.db.execute(
                "UPDATE chat_file_sources SET next_part=next_part+1 WHERE id=?",
                [id],
            )?;
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    const PASS: &str = "chat-feature-test-passphrase";
    fn pair() -> Result<(TempDir, Endpoint, Endpoint, Topic)> {
        let root = TempDir::new()?;
        let mut a = Endpoint::open(root.path().join("a"), PASS, Some("A"), "http://127.0.0.1:9")?;
        let mut b = Endpoint::open(root.path().join("b"), PASS, Some("B"), "http://127.0.0.1:9")?;
        a.add_peer(b.contact_card()?)?;
        b.add_peer(a.contact_card()?)?;
        let upload = b.prekey_upload()?;
        a.establish(
            &b.contact_card()?.user_id,
            PrekeyBundle {
                identity: upload.identity,
                signed_prekey: upload.signed_prekey,
                one_time_prekey: upload.one_time_prekeys.into_iter().next(),
            },
        )?;
        let topic = a.create_topic(&b.contact_card()?.user_id, "Features")?;
        Ok((root, a, b, topic))
    }
    fn last(a: &Endpoint) -> Result<Envelope> {
        a.pending_envelopes()?
            .last()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no envelope"))
    }
    fn event(t: &Topic, body: String) -> Event {
        Event::Control {
            operation_id: Uuid::new_v4().to_string(),
            topic_id: t.id.clone(),
            topic_title: t.title.clone(),
            created_at: t.created_at,
            body,
        }
    }
    #[tokio::test]
    async fn edits_and_withdrawals_mutate_history_without_control_rows() -> Result<()> {
        let (_root, mut a, mut b, t) = pair()?;
        let original = a.send_message(&t.id, "original secret", None).await?;
        b.receive(&last(&a)?)?;
        let revision = b.chat_page(&t.id, None, None, 50)?.revision;
        a.send_special(
            &t.id,
            Special::new(
                "message.edit",
                json!({"messageId":original.id,"body":"updated text"}),
            ),
        )
        .await?;
        let edit = last(&a)?;
        b.receive(&edit)?;
        assert!(!b.receive(&edit)?);
        assert_eq!(b.messages(&t.id)?.len(), 1);
        assert_eq!(b.messages(&t.id)?[0].body, "updated text");
        assert!(b.chat_changes(&t.id, revision)?.items[0].edited);
        assert!(b.search("original secret")?.is_empty());
        a.send_special(
            &t.id,
            Special::new("message.withdraw", json!({"messageId":original.id})),
        )
        .await?;
        b.receive(&last(&a)?)?;
        assert_eq!(b.messages(&t.id)?.len(), 1);
        assert!(b.messages(&t.id)?[0].body.is_empty());
        assert!(b.chat_page(&t.id, None, None, 50)?.items[0].withdrawn);
        assert!(
            a.send_special(
                &t.id,
                Special::new(
                    "message.edit",
                    json!({"messageId":original.id,"body":"cannot resurrect"})
                )
            )
            .await
            .is_err()
        );
        assert_eq!(a.messages(&t.id)?.len(), 1);
        assert_eq!(
            a.outbox()?
                .iter()
                .filter(|job| job.message_id.is_none())
                .count(),
            2
        );
        Ok(())
    }
    #[tokio::test]
    async fn missing_targets_retry_and_foreign_authors_cannot_mutate() -> Result<()> {
        let (_root, mut a, mut b, t) = pair()?;
        let original = a.send_message(&t.id, "original", None).await?;
        let first = last(&a)?;
        a.send_special(
            &t.id,
            Special::new(
                "message.edit",
                json!({"messageId":original.id,"body":"edited"}),
            ),
        )
        .await?;
        let edit = last(&a)?;
        assert!(b.receive(&edit).is_err());
        b.receive(&first)?;
        b.receive(&edit)?;
        assert_eq!(b.messages(&t.id)?[0].body, "edited");
        let forged = b.queue_event(
            &a.contact_card()?.user_id,
            event(
                &t,
                Special::new("message.withdraw", json!({"messageId":original.id})).body()?,
            ),
        )?;
        a.receive(&forged)?;
        assert_eq!(a.messages(&t.id)?[0].body, "edited");
        assert_eq!(a.messages(&t.id)?[1].format, "control.unknown");
        Ok(())
    }
    #[tokio::test]
    async fn future_unknown_and_malformed_messages_are_visible_and_do_not_block() -> Result<()> {
        let (_root, mut a, mut b, t) = pair()?;
        for raw in [
            "{",
            r#"{"version":3,"kind":"future","data":{"x":1}}"#,
            r#"{"version":1,"kind":"future.type","data":{}}"#,
        ] {
            let body = raw.to_string();
            a.queue_event(&b.contact_card()?.user_id, event(&t, body.clone()))?;
            b.receive(&last(&a)?)?;
            let p = b.chat_page(&t.id, None, None, 50)?;
            let m = p.items.last().unwrap();
            assert_eq!(m.message.body, body);
            assert_eq!(m.special_kind.as_deref(), Some("unknown"));
        }
        a.send_message(&t.id, "still works", None).await?;
        b.receive(&last(&a)?)?;
        assert_eq!(b.messages(&t.id)?.len(), 4);
        Ok(())
    }
    #[tokio::test]
    async fn ordinary_text_never_executes_controls_or_authorizes_files() -> Result<()> {
        let (_root, mut a, mut b, t) = pair()?;
        let original = a.send_message(&t.id, "original", None).await?;
        b.receive(&last(&a)?)?;
        let offer = a
            .offer_file(&t.id, "inert.txt", "text/plain", b"consent required")
            .await?;
        b.receive(&last(&a)?)?;
        let file: FileView = serde_json::from_str(&b.messages(&t.id)?[1].body)?;
        for s in [
            Special::new(
                "message.edit",
                json!({"messageId":original.id,"body":"injected"}),
            ),
            Special::new("message.withdraw", json!({"messageId":original.id})),
            Special::new(
                "topic.meta",
                json!({"pinned":true,"tags":["injected"],"status":"resolved"}),
            ),
        ] {
            for body in [s.body()?, format!("cipherwhisper.special\n{}", s.body()?)] {
                let sent = a.send_message(&t.id, &body, None).await?;
                b.receive(&last(&a)?)?;
                assert_ne!(sent.id, original.id);
                assert_eq!(sent.body, body);
                let page = b.chat_page(&t.id, None, None, 50)?;
                let received = page.items.last().unwrap();
                assert_eq!(received.message.body, body);
                assert_eq!(received.message.format, "markdown");
                assert!(received.special.is_none() && received.special_kind.is_none());
                assert_eq!(a.messages(&t.id)?[0].body, "original");
                assert_eq!(b.messages(&t.id)?[0].body, "original");
                assert!(!a.topic(&t.id)?.pinned && !b.topic(&t.id)?.pinned);
            }
        }
        let accept = Special::new(
            "file.accept",
            json!({"fileId":file.file_id,"offerId":offer.id}),
        );
        for body in [
            accept.body()?,
            format!("cipherwhisper.special\n{}", accept.body()?),
        ] {
            let sent = b.send_message(&t.id, &body, None).await?;
            a.receive(&last(&b)?)?;
            assert_eq!(sent.body, body);
            assert!(next_chunk(&a.db, &a.key, &a.contact_card()?.user_id)?.is_none());
            assert!(file_message(&a.db, &offer.id)?.1.accept_id.is_none());
            assert!(file_message(&b.db, &offer.id)?.1.accept_id.is_none());
            assert!(b.download_file(&offer.id).is_err());
        }
        // Explicitly typed control still works; even its replacement text can
        // contain the former marker without being executed recursively.
        let literal = format!("cipherwhisper.special\n{}", accept.body()?);
        a.send_special(
            &t.id,
            Special::new(
                "message.edit",
                json!({"messageId":original.id,"body":literal}),
            ),
        )
        .await?;
        b.receive(&last(&a)?)?;
        let page = b.chat_page(&t.id, None, None, 50)?;
        let edited = &page.items[0];
        assert_eq!(edited.message.body, literal);
        assert!(edited.special_kind.is_none());
        Ok(())
    }
    #[tokio::test]
    async fn file_bytes_require_consent_and_chunks_never_become_history() -> Result<()> {
        let (_root, mut a, mut b, t) = pair()?;
        let bytes: Vec<u8> = (0..CHUNK_BYTES * 2 + 127)
            .map(|i| (i % 251) as u8)
            .collect();
        let offer = a
            .offer_file(&t.id, "报告.txt", "text/plain", &bytes)
            .await?;
        b.receive(&last(&a)?)?;
        assert!(next_chunk(&a.db, &a.key, &a.contact_card()?.user_id)?.is_none());
        assert!(b.download_file(&offer.id).is_err());
        a.pump_files().await?;
        assert_eq!(a.pending_envelopes()?.len(), 1);
        let f: FileView = serde_json::from_str(&b.messages(&t.id)?[0].body)?;
        b.send_special(
            &t.id,
            Special::new(
                "file.accept",
                json!({"fileId":f.file_id,"offerId":offer.id}),
            ),
        )
        .await?;
        a.receive(&last(&b)?)?;
        a.pump_files().await?;
        let first_pass = a.pending_envelopes()?;
        for env in &first_pass {
            b.receive(env)?;
        }
        assert!(b.download_file(&offer.id).is_err());
        a.pump_files().await?;
        let chunks = a.pending_envelopes()?;
        for env in chunks.iter().rev() {
            b.receive(env)?;
        }
        let (name, received) = b.download_file(&offer.id)?;
        assert_eq!(name, "报告.txt");
        assert_eq!(received, bytes);
        assert_eq!(a.messages(&t.id)?.len(), 1);
        assert_eq!(b.messages(&t.id)?.len(), 1);
        assert_eq!(
            b.chat_page(&t.id, None, None, 50)?.items[0]
                .file
                .as_ref()
                .unwrap()
                .state,
            "complete"
        );
        Ok(())
    }
    #[tokio::test]
    async fn bad_file_hash_and_unrequested_chunks_are_not_downloadable() -> Result<()> {
        let (_root, mut a, mut b, t) = pair()?;
        let id = Uuid::new_v4().to_string();
        let offer = a
            .send_formatted(
                &t.id,
                &serde_json::to_string(&FileOffer {
                    version: 1,
                    file_id: id.clone(),
                    name: "bad.bin".into(),
                    mime: "application/octet-stream".into(),
                    size: 1,
                    sha256: "00".repeat(32),
                    chunk_size: CHUNK_BYTES,
                })?,
                None,
                "file",
            )
            .await?;
        b.receive(&last(&a)?)?;
        let bad = Special::new(
            "file.chunk",
            json!({"fileId":id,"offerId":offer.id,"acceptId":Uuid::new_v4().to_string(),"part":0,"hex":"ff"}),
        );
        assert!(a.send_special(&t.id, bad).await.is_err());
        b.send_special(
            &t.id,
            Special::new("file.accept", json!({"fileId":id,"offerId":offer.id})),
        )
        .await?;
        let accepted = last(&b)?;
        a.receive(&accepted)?;
        let f: FileView = serde_json::from_str(&a.messages(&t.id)?[0].body)?;
        a.send_special(
            &t.id,
            Special::new(
                "file.chunk",
                json!({"fileId":id,"offerId":offer.id,"acceptId":f.accept_id,"part":0,"hex":"ff"}),
            ),
        )
        .await?;
        b.receive(&last(&a)?)?;
        assert!(b.download_file(&offer.id).is_err());
        assert_eq!(
            b.chat_page(&t.id, None, None, 50)?.items[0]
                .file
                .as_ref()
                .unwrap()
                .state,
            "failed"
        );
        Ok(())
    }
    #[tokio::test]
    async fn pagination_drafts_and_shared_topic_metadata_survive_restart() -> Result<()> {
        let (root, mut a, mut b, t) = pair()?;
        for i in 0..123 {
            a.send_message(&t.id, &format!("message {i}"), None).await?;
            b.receive(&last(&a)?)?;
        }
        let mut ids = Vec::new();
        let mut before = None;
        loop {
            let p = b.chat_page(&t.id, before.as_deref(), None, 50)?;
            assert!(p.items.len() <= 50);
            ids.extend(p.items.iter().map(|m| m.message.id.clone()));
            if !p.has_more {
                break;
            }
            before = p.older_cursor;
        }
        assert_eq!(ids.len(), 123);
        assert_eq!(b.chat_page(&t.id, None, Some(&ids[0]), 1)?.items.len(), 1);
        assert!(b.chat_page(&t.id, None, Some(&ids[60]), 50)?.items.len() <= 50);
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 123);
        a.save_draft(
            &t.id,
            &Draft {
                body: "private draft".into(),
                reply_to: None,
                revision: 2,
            },
        )?;
        a.save_draft(
            &t.id,
            &Draft {
                body: "stale".into(),
                reply_to: None,
                revision: 1,
            },
        )?;
        assert_eq!(a.draft(&t.id)?.body, "private draft");
        let sealed: String = a.db.query_row(
            "SELECT sealed FROM chat_drafts WHERE topic_id=?",
            [&t.id],
            |r| r.get(0),
        )?;
        assert!(!sealed.contains("private draft"));
        a.send_special(
            &t.id,
            Special::new(
                "topic.meta",
                json!({"pinned":true,"tags":["工作","代码"],"status":"resolved"}),
            ),
        )
        .await?;
        b.receive(&last(&a)?)?;
        assert!(b.topic(&t.id)?.pinned);
        assert_eq!(b.topic(&t.id)?.status, "resolved");
        assert_eq!(b.messages(&t.id)?.len(), 123);
        drop(a);
        let a = Endpoint::open(root.path().join("a"), PASS, None, "http://127.0.0.1:9")?;
        assert_eq!(a.draft(&t.id)?.body, "private draft");
        assert_eq!(a.topic(&t.id)?.tags, vec!["工作", "代码"]);
        Ok(())
    }
}
