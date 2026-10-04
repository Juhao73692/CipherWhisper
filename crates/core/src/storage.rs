use anyhow::{Result, ensure};
use fs2::FileExt;
use rusqlite::Connection;
use std::{
    fs::{File, OpenOptions},
    path::Path,
};

pub fn open(dir: &Path) -> Result<(Connection, File)> {
    ensure!(!dir.is_symlink(), "domain directory cannot be a symlink");
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let lockpath = dir.join("domain.lockfile");
    ensure!(!lockpath.is_symlink(), "lock file cannot be a symlink");
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lock = options.open(lockpath)?;
    lock.try_lock_exclusive()
        .map_err(|_| anyhow::anyhow!("domain is already open; use its management API"))?;
    let path = dir.join("domain.sqlite");
    ensure!(!path.is_symlink(), "database cannot be a symlink");
    options.open(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    let mut db = Connection::open(path)?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;
    CREATE TABLE IF NOT EXISTS metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS identity(id INTEGER PRIMARY KEY CHECK(id=1),pickle TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS peers(id TEXT PRIMARY KEY,card TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS sessions(peer_id TEXT NOT NULL REFERENCES peers(id),id TEXT NOT NULL,pickle TEXT NOT NULL,PRIMARY KEY(peer_id,id));
    CREATE TABLE IF NOT EXISTS topics(id TEXT PRIMARY KEY,peer_id TEXT NOT NULL REFERENCES peers(id),title TEXT NOT NULL,created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL,archived INTEGER NOT NULL DEFAULT 0);
    CREATE TABLE IF NOT EXISTS messages(id TEXT PRIMARY KEY,topic_id TEXT NOT NULL REFERENCES topics(id),sender_id TEXT NOT NULL,timestamp INTEGER NOT NULL,body TEXT NOT NULL,format TEXT NOT NULL,reply_to TEXT,delivery TEXT NOT NULL);
    CREATE INDEX IF NOT EXISTS topic_messages ON messages(topic_id,timestamp,id);
    CREATE TABLE IF NOT EXISTS received_envelopes(id TEXT PRIMARY KEY,digest TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS outbox(id TEXT PRIMARY KEY,envelope TEXT NOT NULL,message_id TEXT,accepted INTEGER NOT NULL DEFAULT 0,attempts INTEGER NOT NULL DEFAULT 0,next_attempt INTEGER NOT NULL DEFAULT 0,last_error TEXT);
    CREATE TABLE IF NOT EXISTS pending_acks(id TEXT PRIMARY KEY);
    CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(body,content='messages',content_rowid='rowid');
    CREATE TRIGGER IF NOT EXISTS messages_insert AFTER INSERT ON messages BEGIN INSERT INTO messages_fts(rowid,body) VALUES(new.rowid,new.body); END;
    CREATE TRIGGER IF NOT EXISTS messages_delete AFTER DELETE ON messages BEGIN INSERT INTO messages_fts(messages_fts,rowid,body) VALUES('delete',old.rowid,old.body); END;
    CREATE TRIGGER IF NOT EXISTS messages_update AFTER UPDATE OF body ON messages BEGIN INSERT INTO messages_fts(messages_fts,rowid,body) VALUES('delete',old.rowid,old.body); INSERT INTO messages_fts(rowid,body) VALUES(new.rowid,new.body); END;")?;
    let has_read_positions: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='ui_topic_reads')",
        [],
        |row| row.get(0),
    )?;
    if !has_read_positions {
        let tx = db.transaction()?;
        // Existing history predates unread tracking. Start with it read; new
        // messages in either centers or replicas will advance beyond this baseline.
        tx.execute_batch("CREATE TABLE ui_topic_reads(topic_id TEXT PRIMARY KEY REFERENCES topics(id),last_rowid INTEGER NOT NULL);
            INSERT INTO ui_topic_reads SELECT topic_id,MAX(rowid) FROM messages GROUP BY topic_id;")?;
        tx.commit()?;
    }
    let indexed: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM metadata WHERE key='search-index' AND value='trigram-v1')",
        [],
        |row| row.get(0),
    )?;
    if !indexed {
        // Rebuild the plaintext index transactionally, including existing histories.
        // Trigrams allow literal substrings within Chinese text without word boundaries.
        let tx = db.transaction()?;
        tx.execute_batch("DROP TRIGGER messages_insert; DROP TRIGGER messages_delete; DROP TRIGGER messages_update;
        DROP TABLE messages_fts;
        CREATE VIRTUAL TABLE messages_fts USING fts5(body,content='messages',content_rowid='rowid',tokenize='trigram');
        CREATE TRIGGER messages_insert AFTER INSERT ON messages BEGIN INSERT INTO messages_fts(rowid,body) VALUES(new.rowid,new.body); END;
        CREATE TRIGGER messages_delete AFTER DELETE ON messages BEGIN INSERT INTO messages_fts(messages_fts,rowid,body) VALUES('delete',old.rowid,old.body); END;
        CREATE TRIGGER messages_update AFTER UPDATE OF body ON messages BEGIN INSERT INTO messages_fts(messages_fts,rowid,body) VALUES('delete',old.rowid,old.body); INSERT INTO messages_fts(rowid,body) VALUES(new.rowid,new.body); END;
        INSERT INTO messages_fts(messages_fts) VALUES('rebuild');
        INSERT INTO metadata(key,value) VALUES('search-index','trigram-v1') ON CONFLICT(key) DO UPDATE SET value=excluded.value;")?;
        tx.commit()?;
    }
    Ok((db, lock))
}
