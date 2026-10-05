use anyhow::Result;
use cipherwhisper_core::{Endpoint, device::Replica};
use cipherwhisper_protocol::device::*;
use cipherwhisper_protocol::*;
use rusqlite::Connection;
use tempfile::TempDir;
use uuid::Uuid;
const PASS: &str = "device-sync-test-passphrase";
const RELAY: &str = "http://127.0.0.1:8787";
struct Fixture {
    root: TempDir,
    center: Endpoint,
    bob: Endpoint,
    ca: String,
}
fn fixture() -> Result<Fixture> {
    let root = TempDir::new()?;
    let mut center = Endpoint::open(root.path().join("center"), PASS, Some("Alice"), RELAY)?;
    let mut bob = Endpoint::open(root.path().join("bob"), PASS, Some("Bob"), RELAY)?;
    center.add_peer(bob.contact_card()?)?;
    bob.add_peer(center.contact_card()?)?;
    let upload = bob.prekey_upload()?;
    center.establish(
        &upload.identity.user_id.clone(),
        PrekeyBundle {
            identity: upload.identity,
            signed_prekey: upload.signed_prekey,
            one_time_prekey: upload.one_time_prekeys.into_iter().next(),
        },
    )?;
    let mut params = rcgen::CertificateParams::new(Vec::<String>::new())?;
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca = params.self_signed(&rcgen::KeyPair::generate()?)?.pem();
    Ok(Fixture {
        root,
        center,
        bob,
        ca,
    })
}
fn client(f: &mut Fixture, name: &str) -> Result<Replica> {
    let mut r = Replica::open(f.root.path().join(name), PASS, Some(name))?;
    let pair = f
        .center
        .authorize_device(r.device_card()?, "https://localhost:8792", &f.ca)?;
    r.pair(pair, &f.center.contact_card()?.user_id)?;
    Ok(r)
}
fn pull(center: &mut Endpoint, r: &mut Replica, limit: usize) -> Result<()> {
    loop {
        let p = center.device_page(
            &r.device_card()?.id,
            &r.pairing()?.epoch,
            r.cursor()?,
            limit,
        )?;
        let high = p.high_water;
        let nonce = Uuid::new_v4().to_string();
        let signed = center.sign_device_response(&r.device_card()?.id, &nonce, p.clone())?;
        signed.validate(r.pairing()?, &nonce)?;
        r.apply_page(&p)?;
        center.device_ack(
            &r.device_card()?.id,
            &Ack {
                epoch: p.epoch,
                cursor: r.cursor()?,
            },
        )?;
        if r.cursor()? == high {
            break;
        }
    }
    Ok(())
}
fn event(t: &Topic, body: &str) -> Event {
    Event::Message {
        message_id: Uuid::new_v4().to_string(),
        topic_id: t.id.clone(),
        topic_title: t.title.clone(),
        created_at: t.created_at,
        body: body.into(),
        format: "markdown".into(),
        reply_to: None,
    }
}
fn command(op: Operation) -> Command {
    Command {
        id: Uuid::new_v4().to_string(),
        operation: op,
    }
}

#[tokio::test]
async fn device_retries_are_bounded_persisted_and_resend_with_new_ids() -> Result<()> {
    let mut f = fixture()?;
    let peer = f.bob.contact_card()?.user_id;
    let topic = f.center.create_topic(&peer, "周末计划")?;
    let mut r = client(&mut f, "retry-device")?;
    pull(&mut f.center, &mut r, 100)?;
    let original = r.send_message(&topic.id, "周六去散步", None).await?;
    let job = r.pending()?.remove(0);
    assert!(r.retry_outbox(&job.id).await.is_err());
    for attempts in 1..=cipherwhisper_core::MAX_DELIVERY_FAILURES {
        r.sync(true).await?;
        let pending = r.pending()?.remove(0);
        assert_eq!(pending.attempts, attempts);
        assert_eq!(pending.operation, job.operation);
        r.sync(false).await?;
        assert_eq!(r.pending()?[0].attempts, attempts);
    }
    assert_eq!(r.messages(&topic.id)?[0].delivery, "paused");
    r.sync(true).await?;
    assert_eq!(
        r.pending()?[0].attempts,
        cipherwhisper_core::MAX_DELIVERY_FAILURES
    );
    drop(r);
    let mut r = Replica::open(f.root.path().join("retry-device"), PASS, None)?;
    assert!(r.pending()?[0].retry_paused);
    let before = now();
    let resent = r.retry_outbox(&job.id).await?;
    assert_ne!(original.id, resent.id);
    assert!(resent.timestamp >= before);
    assert_eq!(resent.body, original.body);
    assert_eq!(r.messages(&topic.id)?.len(), 2);
    let new_job = r.pending()?.remove(1);
    assert_ne!(new_job.id, job.id);
    assert_eq!(new_job.attempts, 0);
    let command = Command {
        id: new_job.id,
        operation: new_job.operation,
    };
    let result = f
        .center
        .device_command(&r.device_card()?.id, command.clone())
        .await?;
    r.apply_command_reply(&command, &result)?;
    pull(&mut f.center, &mut r, 100)?;
    assert_eq!(f.center.messages(&topic.id)?.len(), 1);
    assert_eq!(f.center.messages(&topic.id)?[0].id, resent.id);
    assert!(
        r.messages(&topic.id)?
            .iter()
            .any(|m| m.id == original.id && m.delivery == "paused")
    );
    assert!(r.pending()?[0].retry_paused);
    assert!(r.update_topic(&topic.id, "周末散步", false).await.is_err());
    let metadata = r.pending()?.remove(1);
    let db = Connection::open(f.root.path().join("retry-device/domain.sqlite"))?;
    db.execute(
        "UPDATE device_pending SET attempts=? WHERE id=?",
        rusqlite::params![cipherwhisper_core::MAX_DELIVERY_FAILURES, metadata.id],
    )?;
    assert!(!r.pending()?[1].retry_paused);
    assert_eq!(r.pending()?[1].retry_limit, None);
    r.sync(true).await?;
    assert_eq!(
        r.pending()?[1].attempts,
        cipherwhisper_core::MAX_DELIVERY_FAILURES + 1
    );
    Ok(())
}

#[tokio::test]
async fn paginated_sync_has_independent_cursors_and_exact_topic_history() -> Result<()> {
    let mut f = fixture()?;
    let peer = f.bob.contact_card()?.user_id;
    let math = f.center.create_topic(&peer, "数学")?;
    let nas = f.center.create_topic(&peer, "NAS")?;
    let body = "# 源文\n\n$x^2$\n\n$$E=mc^2$$\n\n```rust\nfn main() {}\n```\n";
    for i in 0..55 {
        let t = if i % 2 == 0 { &math } else { &nas };
        let env = f.center.queue_event(&peer, event(t, body))?;
        f.bob.receive(&env)?;
    }
    let mut one = client(&mut f, "one")?;
    let mut two = client(&mut f, "two")?;
    pull(&mut f.center, &mut one, 7)?;
    assert_eq!(two.cursor()?, 0);
    assert_eq!(one.messages(&math.id)?.len(), 28);
    assert_eq!(one.messages(&nas.id)?.len(), 27);
    assert_eq!(one.messages(&math.id)?[0].body, body);
    assert_eq!(one.messages(&math.id)?, f.center.messages(&math.id)?);
    let statuses = f.center.devices()?;
    assert!(
        statuses
            .iter()
            .any(|d| d.card.id == two.device_card().unwrap().id && d.acknowledged_cursor == 0)
    );
    pull(&mut f.center, &mut two, 100)?;
    assert_eq!(one.cursor()?, two.cursor()?);
    let reply = f
        .bob
        .queue_event(&f.center.contact_card()?.user_id, event(&math, "回复"))?;
    f.center.receive(&reply)?;
    pull(&mut f.center, &mut one, 100)?;
    pull(&mut f.center, &mut two, 100)?;
    assert_eq!(two.messages(&math.id)?.len(), 29);
    assert_eq!(one.messages(&nas.id)?, two.messages(&nas.id)?);
    Ok(())
}
#[test]
fn bad_pages_are_atomic_and_duplicates_or_reordering_cannot_advance_cursor() -> Result<()> {
    let mut f = fixture()?;
    let mut r = client(&mut f, "one")?;
    let good = f
        .center
        .device_page(&r.device_card()?.id, &r.pairing()?.epoch, 0, 100)?;
    let mut bad = good.clone();
    let seq = bad.next_cursor + 1;
    bad.high_water = seq;
    bad.next_cursor = seq;
    bad.changes.push(Change {
        seq,
        entity: Entity::Message(Message {
            id: Uuid::new_v4().to_string(),
            topic_id: Uuid::new_v4().to_string(),
            sender_id: f.center.contact_card()?.user_id,
            timestamp: now(),
            body: "invalid parent".into(),
            format: "markdown".into(),
            reply_to: None,
            delivery: "queued".into(),
        }),
    });
    assert!(r.apply_page(&bad).is_err());
    assert_eq!(r.cursor()?, 0);
    assert!(r.peers()?.is_empty());
    let mut out_of_order = good.clone();
    out_of_order.from_cursor = 1;
    assert!(r.apply_page(&out_of_order).is_err());
    assert_eq!(r.cursor()?, 0);
    r.apply_page(&good)?;
    assert_eq!(r.apply_page(&good)?, 0);
    assert_eq!(r.peers()?.len(), 1);
    let mut rolled_back = good.clone();
    rolled_back.from_cursor = r.cursor()?;
    rolled_back.next_cursor = r.cursor()?;
    rolled_back.high_water = 0;
    rolled_back.changes.clear();
    assert!(r.apply_page(&rolled_back).is_err());
    assert!(
        f.center
            .device_ack(
                &r.device_card()?.id,
                &Ack {
                    epoch: good.epoch,
                    cursor: r.cursor()? + 1
                }
            )
            .is_err()
    );
    Ok(())
}
#[tokio::test]
async fn lost_response_and_both_restarts_keep_one_command_one_message_one_ratchet_advance()
-> Result<()> {
    let mut f = fixture()?;
    let t = f
        .center
        .create_topic(&f.bob.contact_card()?.user_id, "restart")?;
    let mut r = client(&mut f, "one")?;
    pull(&mut f.center, &mut r, 100)?;
    let m = r.send_message(&t.id, "durable draft $x$", None).await?;
    let p = r.pending()?.remove(0);
    let cmd = Command {
        id: p.id,
        operation: p.operation,
    };
    let receipt = f
        .center
        .device_command(&r.device_card()?.id, cmd.clone())
        .await?;
    let high = f
        .center
        .device_page(&r.device_card()?.id, &r.pairing()?.epoch, 0, 100)?
        .high_water;
    drop(r);
    drop(f.center);
    let mut center = Endpoint::open(f.root.path().join("center"), PASS, None, RELAY)?;
    let mut r = Replica::open(f.root.path().join("one"), PASS, None)?;
    let p = r.pending()?.remove(0);
    assert_eq!(p.id, cmd.id);
    let again = center
        .device_command(
            &r.device_card()?.id,
            Command {
                id: p.id,
                operation: p.operation,
            },
        )
        .await?;
    assert_eq!(
        serde_json::to_value(&receipt)?,
        serde_json::to_value(&again)?
    );
    assert_eq!(center.messages(&t.id)?.len(), 1);
    assert_eq!(center.pending_envelopes()?.len(), 1);
    assert_eq!(
        center
            .device_page(&r.device_card()?.id, &r.pairing()?.epoch, 0, 100)?
            .high_water,
        high
    );
    r.apply_command_reply(&cmd, &again)?;
    pull(&mut center, &mut r, 100)?;
    assert_eq!(r.messages(&t.id)?.len(), 1);
    assert_eq!(r.messages(&t.id)?[0].id, m.id);
    let mut altered = cmd.clone();
    if let Operation::Send { body, .. } = &mut altered.operation {
        body.push_str("changed");
    }
    assert!(
        center
            .device_command(&r.device_card()?.id, altered)
            .await
            .is_err()
    );
    assert_eq!(center.pending_envelopes()?.len(), 1);
    Ok(())
}
#[tokio::test]
async fn late_accepted_receipt_does_not_regress_newer_delivery_revision() -> Result<()> {
    let mut f = fixture()?;
    let t = f
        .center
        .create_topic(&f.bob.contact_card()?.user_id, "delivery")?;
    let mut r = client(&mut f, "one")?;
    pull(&mut f.center, &mut r, 100)?;
    let m = r.send_message(&t.id, "one message", None).await?;
    let p = r.pending()?.remove(0);
    let cmd = Command {
        id: p.id,
        operation: p.operation,
    };
    let old = f
        .center
        .device_command(&r.device_card()?.id, cmd.clone())
        .await?;
    Connection::open(f.root.path().join("center/domain.sqlite"))?.execute(
        "UPDATE messages SET delivery='delivered' WHERE id=?",
        [&m.id],
    )?;
    pull(&mut f.center, &mut r, 100)?;
    assert_eq!(r.messages(&t.id)?[0].delivery, "delivered");
    r.apply_command_reply(&cmd, &old)?;
    assert_eq!(r.messages(&t.id)?[0].delivery, "delivered");
    let nonce = Uuid::new_v4().to_string();
    let mut signed = f
        .center
        .sign_device_response(&r.device_card()?.id, &nonce, old)?;
    signed.validate(r.pairing()?, &nonce)?;
    assert!(signed.validate(r.pairing()?, "other request").is_err());
    signed.data.body_digest.push('0');
    assert!(signed.validate(r.pairing()?, &nonce).is_err());
    Ok(())
}
#[tokio::test]
async fn conflicting_rename_and_cross_topic_replies_are_rejected_without_losing_messages()
-> Result<()> {
    let mut f = fixture()?;
    let peer = f.bob.contact_card()?.user_id;
    let t = f.center.create_topic(&peer, "original")?;
    let other = f.center.create_topic(&peer, "other")?;
    let mut one = client(&mut f, "one")?;
    let two = client(&mut f, "two")?;
    pull(&mut f.center, &mut one, 100)?;
    let first = f
        .center
        .device_command(
            &one.device_card()?.id,
            command(Operation::UpdateTopic {
                topic_id: t.id.clone(),
                title: "first".into(),
                archived: false,
                base_title: "original".into(),
                base_archived: false,
            }),
        )
        .await?;
    assert!(matches!(first.result, CommandResult::Accepted { .. }));
    let second = f
        .center
        .device_command(
            &two.device_card()?.id,
            command(Operation::UpdateTopic {
                topic_id: t.id.clone(),
                title: "second".into(),
                archived: false,
                base_title: "original".into(),
                base_archived: false,
            }),
        )
        .await?;
    assert!(matches!(
        second.result,
        CommandResult::Rejected {
            current: Some(_),
            ..
        }
    ));
    assert_eq!(f.center.topic(&t.id)?.title, "first");
    let env = f
        .center
        .queue_event(&peer, event(&other, "belongs elsewhere"))?;
    let target = f.center.messages(&other.id)?[0].id.clone();
    let before = f.center.pending_envelopes()?.len();
    let rejected = f
        .center
        .device_command(
            &one.device_card()?.id,
            command(Operation::Send {
                format: "markdown".into(),
                message_id: Uuid::new_v4().to_string(),
                topic_id: t.id.clone(),
                body: "cross reply".into(),
                reply_to: Some(target),
                timestamp: now(),
            }),
        )
        .await?;
    assert!(matches!(rejected.result, CommandResult::Rejected { .. }));
    assert_eq!(f.center.pending_envelopes()?.len(), before);
    assert!(f.center.messages(&t.id)?.is_empty());
    assert!(f.bob.receive(&env)?);
    Ok(())
}
#[test]
fn authentication_replay_and_revocation_survive_center_restart() -> Result<()> {
    let mut f = fixture()?;
    let r = client(&mut f, "one")?;
    let auth = r.request_auth("GET", "/device/v1/changes", b"")?;
    assert!(
        f.center
            .authenticate_device(&auth, "GET", "/device/v1/changed", b"")
            .is_err()
    );
    f.center
        .authenticate_device(&auth, "GET", "/device/v1/changes", b"")?;
    assert!(
        f.center
            .authenticate_device(&auth, "GET", "/device/v1/changes", b"")
            .is_err()
    );
    let mut expired = auth.clone();
    expired.timestamp -= AUTH_WINDOW + 1;
    assert!(
        f.center
            .authenticate_device(&expired, "GET", "/device/v1/changes", b"")
            .is_err()
    );
    drop(f.center);
    let mut center = Endpoint::open(f.root.path().join("center"), PASS, None, RELAY)?;
    assert!(
        center
            .authenticate_device(&auth, "GET", "/device/v1/changes", b"")
            .is_err()
    );
    let next = r.request_auth("POST", "/device/v1/ack", b"{}");
    center.authenticate_device(&next?, "POST", "/device/v1/ack", b"{}")?;
    center.revoke_device(&r.device_card()?.id)?;
    assert!(
        center
            .authenticate_device(
                &r.request_auth("GET", "/device/v1/changes", b"")?,
                "GET",
                "/device/v1/changes",
                b""
            )
            .is_err()
    );
    assert!(
        center
            .device_page(&r.device_card()?.id, &r.pairing()?.epoch, 0, 100)
            .is_err()
    );
    assert!(
        center
            .authorize_device(r.device_card()?, "https://localhost:8792", &f.ca)
            .is_err()
    );
    drop(center);
    let mut center = Endpoint::open(f.root.path().join("center"), PASS, None, RELAY)?;
    assert!(center.active_device(&r.device_card()?.id).is_err());
    let account = vodozemac::olm::Account::new();
    let mut unknown = DeviceAuth {
        signing_key: account.ed25519_key().to_base64(),
        domain_id: center.contact_card()?.user_id,
        timestamp: now(),
        nonce: Uuid::new_v4().to_string(),
        signature: String::new(),
    };
    unknown.signature = account
        .sign(unknown.signing_bytes("GET", "/device/v1/changes", b""))
        .to_base64();
    assert!(
        center
            .authenticate_device(&unknown, "GET", "/device/v1/changes", b"")
            .is_err()
    );
    Ok(())
}
#[test]
fn device_keys_roles_and_encrypted_pairing_are_isolated() -> Result<()> {
    let mut f = fixture()?;
    let r = client(&mut f, "one")?;
    assert_ne!(
        r.device_card()?.signing_key,
        f.center.contact_card()?.signing_key
    );
    let pair = r.pairing()?.clone();
    drop(r);
    assert!(Replica::open(f.root.path().join("one"), "wrong-but-long-passphrase", None).is_err());
    assert!(Replica::open(f.root.path().join("center"), PASS, None).is_err());
    assert!(Endpoint::open(f.root.path().join("one"), PASS, None, RELAY).is_err());
    let mut second = Replica::open(f.root.path().join("two"), PASS, Some("two"))?;
    assert!(second.pair(pair.clone(), &pair.domain.user_id).is_err());
    let second_pair =
        f.center
            .authorize_device(second.device_card()?, "https://localhost:8792", &f.ca)?;
    assert!(second.pair(second_pair.clone(), "td_wrong").is_err());
    let mut altered = second_pair;
    altered.ca_pem.push('x');
    assert!(second.pair(altered, &pair.domain.user_id).is_err());
    let db = Connection::open(f.root.path().join("one/domain.sqlite"))?;
    let mut sealed: String = db.query_row(
        "SELECT value FROM metadata WHERE key='device-pair'",
        [],
        |r| r.get(0),
    )?;
    assert!(sealed.starts_with("xc1:"));
    assert!(!sealed.contains("https"));
    let last = sealed.pop().unwrap();
    sealed.push(if last == '0' { '1' } else { '0' });
    db.execute(
        "UPDATE metadata SET value=? WHERE key='device-pair'",
        [sealed],
    )?;
    drop(db);
    assert!(Replica::open(f.root.path().join("one"), PASS, None).is_err());
    Ok(())
}
#[test]
fn journal_upgrade_seeds_old_history_once_and_preserves_epoch_on_restart() -> Result<()> {
    let mut f = fixture()?;
    let peer = f.bob.contact_card()?.user_id;
    let t = f.center.create_topic(&peer, "existing")?;
    f.center.queue_event(&peer, event(&t, "old history $x$"))?;
    drop(f.center);
    let db = Connection::open(f.root.path().join("center/domain.sqlite"))?;
    db.execute_batch("DROP TRIGGER sync_peers_INSERT; DROP TRIGGER sync_peers_UPDATE; DROP TRIGGER sync_topics_INSERT; DROP TRIGGER sync_topics_UPDATE; DROP TRIGGER sync_messages_INSERT; DROP TRIGGER sync_messages_UPDATE; DROP TABLE sync_log; DELETE FROM metadata WHERE key IN ('device-sync-journal','sync-epoch');")?;
    drop(db);
    f.center = Endpoint::open(f.root.path().join("center"), PASS, None, RELAY)?;
    let mut r = client(&mut f, "one")?;
    pull(&mut f.center, &mut r, 1)?;
    assert_eq!(r.messages(&t.id)?[0].body, "old history $x$");
    let epoch = r.pairing()?.epoch.clone();
    let cursor = r.cursor()?;
    drop(f.center);
    let mut center = Endpoint::open(f.root.path().join("center"), PASS, None, RELAY)?;
    let p = center.device_page(&r.device_card()?.id, &epoch, cursor, 100)?;
    assert!(p.changes.is_empty());
    assert_eq!(p.high_water, cursor);
    Ok(())
}

#[tokio::test]
async fn future_topic_clock_cannot_silently_accept_a_rename_or_advance_ratchet() -> Result<()> {
    let mut f = fixture()?;
    let t = f
        .center
        .create_topic(&f.bob.contact_card()?.user_id, "original")?;
    let r = client(&mut f, "one")?;
    Connection::open(f.root.path().join("center/domain.sqlite"))?.execute(
        "UPDATE topics SET updated_at=? WHERE id=?",
        rusqlite::params![now() + 60, t.id],
    )?;
    let result = f
        .center
        .device_command(
            &r.device_card()?.id,
            command(Operation::UpdateTopic {
                topic_id: t.id.clone(),
                title: "new".into(),
                archived: false,
                base_title: "original".into(),
                base_archived: false,
            }),
        )
        .await?;
    assert!(matches!(result.result, CommandResult::Rejected { .. }));
    assert_eq!(f.center.topic(&t.id)?.title, "original");
    assert!(f.center.pending_envelopes()?.is_empty());
    let env = f
        .center
        .queue_event(&f.bob.contact_card()?.user_id, event(&t, "still valid"))?;
    assert!(f.bob.receive(&env)?);
    Ok(())
}

#[test]
fn encoded_page_budget_handles_many_escape_heavy_bodies_without_stalling_sync() -> Result<()> {
    let mut f = fixture()?;
    let t = f
        .center
        .create_topic(&f.bob.contact_card()?.user_id, "large source")?;
    let body = "\0".repeat(MAX_BODY);
    let mut db = Connection::open(f.root.path().join("center/domain.sqlite"))?;
    let tx = db.transaction()?;
    for _ in 0..50 {
        tx.execute(
            "INSERT INTO messages VALUES(?,?,?,?,?,'markdown',NULL,'received')",
            rusqlite::params![Uuid::new_v4().to_string(), t.id, t.peer_id, now(), body],
        )?;
    }
    tx.commit()?;
    let mut r = client(&mut f, "one")?;
    let first = f
        .center
        .device_page(&r.device_card()?.id, &r.pairing()?.epoch, 0, 100)?;
    assert!(first.next_cursor < first.high_water);
    assert!(serde_json::to_vec(&first)?.len() < 9 * 1024 * 1024);
    pull(&mut f.center, &mut r, 100)?;
    assert_eq!(r.messages(&t.id)?.len(), 50);
    assert_eq!(r.messages(&t.id)?[0].body, body);
    Ok(())
}

#[tokio::test]
async fn ahead_receipt_keeps_same_second_messages_in_center_journal_order() -> Result<()> {
    let mut f = fixture()?;
    let t = f
        .center
        .create_topic(&f.bob.contact_card()?.user_id, "order")?;
    let mut r = client(&mut f, "one")?;
    pull(&mut f.center, &mut r, 100)?;
    let queued = r.send_message(&t.id, "second", None).await?;
    let p = r.pending()?.remove(0);
    let command = Command {
        id: p.id,
        operation: p.operation,
    };
    let first = Message {
        id: Uuid::new_v4().to_string(),
        topic_id: t.id.clone(),
        sender_id: t.peer_id.clone(),
        timestamp: 123,
        body: "first".into(),
        format: "markdown".into(),
        reply_to: None,
        delivery: "received".into(),
    };
    let second = Message {
        id: queued.id,
        sender_id: f.center.contact_card()?.user_id,
        body: "second".into(),
        delivery: "queued".into(),
        ..first.clone()
    };
    let cursor = r.cursor()?;
    let reply = CommandReply {
        id: command.id.clone(),
        body_digest: digest(&serde_json::to_vec(&command)?),
        result: CommandResult::Accepted {
            entity: Entity::Message(second.clone()),
            revision: cursor + 2,
        },
    };
    r.apply_command_reply(&command, &reply)?;
    r.apply_page(&Page {
        epoch: r.pairing()?.epoch.clone(),
        from_cursor: cursor,
        next_cursor: cursor + 2,
        high_water: cursor + 2,
        changes: vec![
            Change {
                seq: cursor + 1,
                entity: Entity::Message(first.clone()),
            },
            Change {
                seq: cursor + 2,
                entity: Entity::Message(second.clone()),
            },
        ],
    })?;
    assert_eq!(r.messages(&t.id)?, vec![first, second]);
    drop(r);
    let r = Replica::open(f.root.path().join("one"), PASS, None)?;
    assert_eq!(r.messages(&t.id)?[0].body, "first");
    Ok(())
}
#[tokio::test]
async fn device_edits_and_topic_metadata_apply_without_special_history_rows() -> Result<()> {
    use cipherwhisper_protocol::special::Special;
    let mut f = fixture()?;
    let mut r = client(&mut f, "chat-device")?;
    let t = f
        .center
        .create_topic(&f.bob.contact_card()?.user_id, "device editing")?;
    let original = f.center.send_message(&t.id, "before", None).await?;
    f.bob
        .receive(f.center.pending_envelopes()?.last().unwrap())?;
    pull(&mut f.center, &mut r, 1)?;
    r.send_special(
        &t.id,
        Special::new(
            "message.edit",
            serde_json::json!({"messageId":original.id,"body":"after"}),
        ),
    )
    .await?;
    let pending = r.pending()?.remove(0);
    assert!(matches!(pending.operation, Operation::Control { .. }));
    let cmd = Command {
        id: pending.id,
        operation: pending.operation,
    };
    let reply = f
        .center
        .device_command(&r.device_card()?.id, cmd.clone())
        .await?;
    r.apply_command_reply(&cmd, &reply)?;
    pull(&mut f.center, &mut r, 1)?;
    assert_eq!(r.chat_page(&t.id, None, None, 50)?.items.len(), 1);
    assert_eq!(r.messages(&t.id)?[0].body, "after");
    r.send_special(
        &t.id,
        Special::new(
            "topic.meta",
            serde_json::json!({"pinned":true,"tags":["测试"],"status":"active"}),
        ),
    )
    .await?;
    let pending = r.pending()?.remove(0);
    let cmd = Command {
        id: pending.id,
        operation: pending.operation,
    };
    let reply = f
        .center
        .device_command(&r.device_card()?.id, cmd.clone())
        .await?;
    r.apply_command_reply(&cmd, &reply)?;
    pull(&mut f.center, &mut r, 1)?;
    assert!(r.topics(None)?[0].pinned);
    assert_eq!(r.topic(&t.id)?.tags, vec!["测试"]);
    r.send_special(
        &t.id,
        Special::new(
            "message.withdraw",
            serde_json::json!({"messageId":original.id}),
        ),
    )
    .await?;
    let pending = r.pending()?.remove(0);
    let cmd = Command {
        id: pending.id,
        operation: pending.operation,
    };
    let reply = f
        .center
        .device_command(&r.device_card()?.id, cmd.clone())
        .await?;
    r.apply_command_reply(&cmd, &reply)?;
    pull(&mut f.center, &mut r, 1)?;
    assert_eq!(r.messages(&t.id)?.len(), 1);
    assert!(r.messages(&t.id)?[0].body.is_empty());
    Ok(())
}
#[tokio::test]
async fn received_file_parts_sync_to_devices_after_consent() -> Result<()> {
    use cipherwhisper_protocol::special::Special;
    let mut f = fixture()?;
    let mut r = client(&mut f, "file-device")?;
    let t = f
        .center
        .create_topic(&f.bob.contact_card()?.user_id, "device files")?;
    f.center.send_message(&t.id, "ready", None).await?;
    f.bob
        .receive(f.center.pending_envelopes()?.last().unwrap())?;
    let bytes = b"file bytes through center and replica";
    let offer = f
        .bob
        .offer_file(&t.id, "sample.txt", "text/plain", bytes)
        .await?;
    f.center
        .receive(f.bob.pending_envelopes()?.last().unwrap())?;
    pull(&mut f.center, &mut r, 1)?;
    let file = r
        .chat_page(&t.id, None, None, 50)?
        .items
        .last()
        .unwrap()
        .file
        .clone()
        .unwrap();
    r.send_special(
        &t.id,
        Special::new(
            "file.accept",
            serde_json::json!({"fileId":file.file_id,"offerId":offer.id}),
        ),
    )
    .await?;
    let pending = r.pending()?.remove(0);
    let cmd = Command {
        id: pending.id,
        operation: pending.operation,
    };
    let reply = f
        .center
        .device_command(&r.device_card()?.id, cmd.clone())
        .await?;
    r.apply_command_reply(&cmd, &reply)?;
    f.bob
        .receive(f.center.pending_envelopes()?.last().unwrap())?;
    // The fixture's relay is deliberately absent. Pumping still durably queues
    // authorized file chunks before the network request fails.
    f.bob.sync(true).await?;
    for envelope in f.bob.pending_envelopes()? {
        f.center.receive(&envelope)?;
    }
    pull(&mut f.center, &mut r, 1)?;
    assert_eq!(r.download_file(&offer.id)?.1, bytes);
    assert_eq!(r.messages(&t.id)?.len(), 2);
    Ok(())
}

#[tokio::test]
async fn device_plaintext_commands_stay_visible_and_cannot_mutate_history() -> Result<()> {
    use cipherwhisper_protocol::special::Special;
    let mut f = fixture()?;
    let mut r = client(&mut f, "injection-device")?;
    let t = f
        .center
        .create_topic(&f.bob.contact_card()?.user_id, "inert control text")?;
    let original = f.center.send_message(&t.id, "original", None).await?;
    f.bob
        .receive(f.center.pending_envelopes()?.last().unwrap())?;
    pull(&mut f.center, &mut r, 1)?;
    let control = Special::new(
        "message.withdraw",
        serde_json::json!({"messageId":original.id}),
    );
    for body in [
        control.body()?,
        format!("cipherwhisper.special\n{}", control.body()?),
    ] {
        let sent = r.send_message(&t.id, &body, None).await?;
        let page = r.chat_page(&t.id, None, None, 50)?;
        assert!(page.items.iter().any(|m| m.message.id == sent.id
            && m.message.body == body
            && m.special_kind.is_none()));
        let pending = r.pending()?.remove(0);
        assert!(pending.retry_limit.is_some());
        assert!(matches!(pending.operation, Operation::Send { .. }));
        let cmd = Command {
            id: pending.id,
            operation: pending.operation,
        };
        let reply = f
            .center
            .device_command(&r.device_card()?.id, cmd.clone())
            .await?;
        assert!(
            matches!(&reply.result, CommandResult::Accepted { entity: Entity::Message(m), .. } if m.id == sent.id && m.body == body)
        );
        r.apply_command_reply(&cmd, &reply)?;
        f.bob
            .receive(f.center.pending_envelopes()?.last().unwrap())?;
        pull(&mut f.center, &mut r, 1)?;
        assert_eq!(f.center.messages(&t.id)?[0].body, "original");
        assert_eq!(f.bob.messages(&t.id)?[0].body, "original");
        assert_eq!(r.messages(&t.id)?[0].body, "original");
    }
    // Even a caller-supplied display format cannot promote Send to Control.
    let cmd = command(Operation::Send {
        format: "control".into(),
        message_id: Uuid::new_v4().to_string(),
        topic_id: t.id.clone(),
        body: control.body()?,
        reply_to: None,
        timestamp: now(),
    });
    let reply = f.center.device_command(&r.device_card()?.id, cmd).await?;
    assert!(matches!(reply.result, CommandResult::Accepted { .. }));
    assert_eq!(f.center.messages(&t.id)?[0].body, "original");
    assert_eq!(f.center.messages(&t.id)?.last().unwrap().format, "unknown");
    f.bob
        .receive(f.center.pending_envelopes()?.last().unwrap())?;
    assert_eq!(f.bob.messages(&t.id)?[0].body, "original");
    assert_eq!(f.bob.messages(&t.id)?.last().unwrap().format, "unknown");
    let raw = r#"{"version":99,"kind":"future.control","data":{"html":"<script>bad()</script>"}}"#;
    let unknown = f.center.queue_event(
        &f.bob.contact_card()?.user_id,
        Event::Control {
            operation_id: Uuid::new_v4().to_string(),
            topic_id: t.id.clone(),
            topic_title: t.title.clone(),
            created_at: t.created_at,
            body: raw.into(),
        },
    )?;
    f.bob.receive(&unknown)?;
    pull(&mut f.center, &mut r, 1)?;
    let page = r.chat_page(&t.id, None, None, 50)?;
    let received = page.items.last().unwrap();
    assert_eq!(received.message.body, raw);
    assert_eq!(received.special_kind.as_deref(), Some("unknown"));
    assert!(received.special_error.as_deref().unwrap().contains("99"));
    Ok(())
}
