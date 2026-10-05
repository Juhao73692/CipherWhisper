use anyhow::{Result, ensure};
use rusqlite::{Connection, params};
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnreadTopic {
    pub topic_id: String,
    pub peer_id: String,
    pub count: i64,
    pub last_message_id: String,
    pub first_message_id: String,
}

pub(crate) fn summary(db: &Connection, self_id: &str) -> Result<Vec<UnreadTopic>> {
    let mut stmt = db.prepare(
        "WITH unread AS (
            SELECT m.topic_id,COUNT(*) AS count,MAX(m.rowid) AS latest,MIN(m.rowid) AS first
            FROM messages m LEFT JOIN ui_topic_reads r ON r.topic_id=m.topic_id
            WHERE m.sender_id<>? AND m.rowid>COALESCE(r.last_rowid,0)
            GROUP BY m.topic_id
        ) SELECT u.topic_id,t.peer_id,u.count,m.id,first.id FROM unread u
          JOIN topics t ON t.id=u.topic_id JOIN messages m ON m.rowid=u.latest JOIN messages first ON first.rowid=u.first",
    )?;
    Ok(stmt
        .query_map([self_id], |row| {
            Ok(UnreadTopic {
                topic_id: row.get(0)?,
                peer_id: row.get(1)?,
                count: row.get(2)?,
                last_message_id: row.get(3)?,
                first_message_id: row.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
}

pub(crate) fn mark_read(db: &Connection, self_id: &str, topic: &str, through: &str) -> Result<()> {
    // A message from the displayed snapshot bounds the read position. Messages
    // received after fetching that snapshot must remain unread, even in the same second.
    let changed = db.execute(
        "INSERT INTO ui_topic_reads(topic_id,last_rowid)
         SELECT topic_id,rowid FROM messages WHERE id=? AND topic_id=? AND sender_id<>?
         ON CONFLICT(topic_id) DO UPDATE SET last_rowid=MAX(last_rowid,excluded.last_rowid)",
        params![through, topic, self_id],
    )?;
    ensure!(
        changed == 1,
        "read position must be an incoming message in this topic"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_snapshot_does_not_clear_later_or_other_topic_messages() -> Result<()> {
        let db = Connection::open_in_memory()?;
        db.execute_batch("CREATE TABLE topics(id TEXT PRIMARY KEY,peer_id TEXT);
            CREATE TABLE messages(id TEXT PRIMARY KEY,topic_id TEXT,sender_id TEXT,timestamp INTEGER);
            CREATE TABLE ui_topic_reads(topic_id TEXT PRIMARY KEY,last_rowid INTEGER NOT NULL);
            INSERT INTO topics VALUES('plans','alice'),('dinner','alice');
            INSERT INTO messages VALUES('first','plans','alice',100),('own','plans','me',100),('other','dinner','alice',100);")?;
        let snapshot = summary(&db, "me")?;
        assert_eq!(snapshot.len(), 2);
        assert_eq!(
            snapshot
                .iter()
                .find(|t| t.topic_id == "plans")
                .unwrap()
                .count,
            1
        );
        // A message arrives after the history snapshot, in the same second.
        db.execute(
            "INSERT INTO messages VALUES('later','plans','alice',100)",
            [],
        )?;
        mark_read(&db, "me", "plans", "first")?;
        let unread = summary(&db, "me")?;
        let plans = unread.iter().find(|t| t.topic_id == "plans").unwrap();
        assert_eq!(plans.count, 1);
        assert_eq!(plans.last_message_id, "later");
        assert_eq!(unread.len(), 2);
        assert!(mark_read(&db, "me", "plans", "other").is_err());
        assert!(mark_read(&db, "me", "plans", "own").is_err());
        mark_read(&db, "me", "plans", "later")?;
        mark_read(&db, "me", "plans", "first")?; // A stale read cannot rewind the frontier.
        let unread = summary(&db, "me")?;
        assert_eq!(unread.len(), 1);
        assert_eq!(unread[0].topic_id, "dinner");
        Ok(())
    }

    #[test]
    fn upgrade_baselines_old_history_and_new_reads_survive_restart() -> Result<()> {
        let root = tempfile::tempdir()?;
        let (db, lock) = crate::storage::open(root.path())?;
        db.execute_batch(
            "INSERT INTO peers VALUES('alice','{}');
            INSERT INTO topics VALUES('plans','alice','周末计划',100,100,0);
            INSERT INTO messages VALUES('old','plans','alice',100,'old','markdown',NULL,'received');
            DROP TABLE ui_topic_reads;",
        )?;
        drop(db);
        drop(lock);
        let (db, lock) = crate::storage::open(root.path())?;
        assert!(summary(&db, "me")?.is_empty());
        db.execute("INSERT INTO messages VALUES('new','plans','alice',100,'new','markdown',NULL,'received')", [])?;
        assert_eq!(summary(&db, "me")?[0].count, 1);
        drop(db);
        drop(lock);
        let (db, lock) = crate::storage::open(root.path())?;
        assert_eq!(summary(&db, "me")?[0].last_message_id, "new");
        mark_read(&db, "me", "plans", "new")?;
        drop(db);
        drop(lock);
        let (db, _lock) = crate::storage::open(root.path())?;
        assert!(summary(&db, "me")?.is_empty());
        Ok(())
    }
}
