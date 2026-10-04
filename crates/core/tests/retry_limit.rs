use anyhow::Result;
use cipherwhisper_core::{Endpoint, MAX_DELIVERY_FAILURES};
use cipherwhisper_protocol::*;
use tempfile::TempDir;

const PASS: &str = "retry-limit-test-passphrase";

async fn bounded_delivery(direct: bool) -> Result<()> {
    let root = TempDir::new()?;
    let open = |name: &str| {
        if direct {
            Endpoint::open_direct(root.path().join(name), PASS, Some(name))
        } else {
            Endpoint::open(
                root.path().join(name),
                PASS,
                Some(name),
                "http://127.0.0.1:9",
            )
        }
    };
    let mut a = open("alice")?;
    let mut b = open("bob")?;
    let peer = b.contact_card()?.user_id;
    if direct {
        a.set_direct_address("http://127.0.0.1:9", None)?;
        b.set_direct_address("http://127.0.0.1:9", None)?;
        a.add_direct_peer(b.direct_profile()?)?;
        b.add_direct_peer(a.direct_profile()?)?;
    } else {
        a.add_peer(b.contact_card()?)?;
        b.add_peer(a.contact_card()?)?;
    }
    let upload = b.prekey_upload()?;
    a.establish(
        &peer,
        PrekeyBundle {
            identity: upload.identity,
            signed_prekey: upload.signed_prekey,
            one_time_prekey: upload.one_time_prekeys.into_iter().next(),
        },
    )?;
    let topic = a.create_topic(&peer, "周末计划")?;
    let original = a.send_message(&topic.id, "周六一起散步？", None).await?;
    let envelope = a.pending_envelopes()?.remove(0);
    assert!(a.retry_outbox(&envelope.id).await.is_err());
    for attempts in 1..=MAX_DELIVERY_FAILURES {
        a.sync(true).await?;
        let job = a.outbox()?.remove(0);
        assert_eq!(job.attempts, attempts);
        assert_eq!(job.retry_paused, attempts == MAX_DELIVERY_FAILURES);
        assert_eq!(a.pending_envelopes()?[0], envelope);
        // Automatic refresh respects backoff, even when the chat polls rapidly.
        a.sync(false).await?;
        assert_eq!(a.outbox()?[0].attempts, attempts);
    }
    assert_eq!(a.messages(&topic.id)?[0].delivery, "paused");
    for _ in 0..3 {
        a.sync(true).await?;
    }
    assert_eq!(a.outbox()?[0].attempts, MAX_DELIVERY_FAILURES);
    assert!(b.topics(None)?.is_empty());
    drop(a);
    let mut a = open("alice")?;
    assert!(a.outbox()?[0].retry_paused);
    assert_eq!(a.messages(&topic.id)?[0].delivery, "paused");
    let before = now();
    let resent = a.retry_outbox(&envelope.id).await?;
    assert_ne!(resent.id, original.id);
    assert!(resent.timestamp >= before);
    assert_eq!(resent.body, original.body);
    assert_eq!(resent.reply_to, original.reply_to);
    assert_eq!(resent.delivery, "queued");
    assert_eq!(a.messages(&topic.id)?.len(), 2);
    let envelopes = a.pending_envelopes()?;
    assert_eq!(envelopes[0], envelope);
    assert_ne!(envelopes[1].id, envelope.id);
    assert_ne!(envelopes[1].ciphertext, envelope.ciphertext);
    assert!(b.receive(&envelopes[1])?);
    assert_eq!(b.messages(&topic.id)?.len(), 1);
    assert_eq!(b.messages(&topic.id)?[0].id, resent.id);
    a.sync(true).await?;
    assert_eq!(a.outbox()?[0].attempts, MAX_DELIVERY_FAILURES);
    assert_eq!(a.outbox()?[1].attempts, 1);
    // Topic metadata must still converge after outages; only chat messages pause.
    a.update_topic(&topic.id, "周末散步", false).await?;
    let db = rusqlite::Connection::open(root.path().join("alice/domain.sqlite"))?;
    db.execute(
        "UPDATE outbox SET attempts=? WHERE message_id=? OR message_id IS NULL",
        rusqlite::params![MAX_DELIVERY_FAILURES, resent.id],
    )?;
    let metadata = a.outbox()?.remove(2);
    assert!(!metadata.retry_paused);
    assert_eq!(metadata.retry_limit, None);
    a.sync(true).await?;
    assert_eq!(a.outbox()?[2].attempts, MAX_DELIVERY_FAILURES + 1);
    Ok(())
}

#[tokio::test]
async fn direct_failures_pause_durably_and_manual_resend_creates_new_message() -> Result<()> {
    bounded_delivery(true).await
}

#[tokio::test]
async fn relay_failures_pause_durably_and_manual_resend_creates_new_message() -> Result<()> {
    bounded_delivery(false).await
}
