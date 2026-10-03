use anyhow::Result;
use rusqlite::Connection;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tempfile::TempDir;
use topicairn_core::{Endpoint, transport::RelayClient};
use topicairn_protocol::*;
use uuid::Uuid;
use vodozemac::olm::{Account, OlmMessage, SessionConfig};

const PASS: &str = "test-only-long-passphrase";
const RELAY: &str = "http://127.0.0.1:8787";
const BODY: &str = "# 证明\n\n$e^{i\\pi}+1=0$\n\n$$E=mc^2$$\n\n```rust\nfn main() {}\n```\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n<script>alert('stored source only')</script>\n";
struct Pair {
    root: TempDir,
    alice: Endpoint,
    bob: Endpoint,
}
fn bundle(domain: &mut Endpoint) -> Result<PrekeyBundle> {
    let p = domain.prekey_upload()?;
    Ok(PrekeyBundle {
        identity: p.identity,
        signed_prekey: p.signed_prekey,
        one_time_prekey: p.one_time_prekeys.into_iter().next(),
    })
}
fn pair() -> Result<Pair> {
    let root = TempDir::new()?;
    let mut alice = Endpoint::open(root.path().join("alice"), PASS, Some("Alice"), RELAY)?;
    let mut bob = Endpoint::open(root.path().join("bob"), PASS, Some("Bob"), RELAY)?;
    alice.add_peer(bob.contact_card()?)?;
    bob.add_peer(alice.contact_card()?)?;
    let id = bob.contact_card()?.user_id;
    alice.establish(&id, bundle(&mut bob)?)?;
    Ok(Pair { root, alice, bob })
}
fn event(topic: &Topic, body: &str) -> Event {
    Event::Message {
        message_id: Uuid::new_v4().to_string(),
        topic_id: topic.id.clone(),
        topic_title: topic.title.clone(),
        created_at: topic.created_at,
        body: body.into(),
        format: "markdown".into(),
        reply_to: None,
    }
}
#[test]
fn encrypted_roundtrip_topics_and_markdown_source() -> Result<()> {
    let Pair {
        mut alice, mut bob, ..
    } = pair()?;
    let peer = bob.contact_card()?.user_id;
    let math = alice.create_topic(&peer, "数学")?;
    let nas = alice.create_topic(&peer, "NAS")?;
    let env = alice.queue_event(&peer, event(&math, BODY))?;
    let json = serde_json::to_string(&env)?;
    assert!(!json.contains("证明"));
    assert!(!json.contains("数学"));
    assert!(!json.contains("topic_id"));
    assert!(bob.receive(&env)?);
    let other = alice.queue_event(&peer, event(&nas, "separate topic"))?;
    bob.receive(&other)?;
    assert_eq!(bob.messages(&math.id)?[0].body, BODY);
    assert_eq!(bob.messages(&nas.id)?.len(), 1);
    assert_eq!(bob.search("证明")?[0].topic_id, math.id);
    let reply = bob.queue_event(
        &alice.contact_card()?.user_id,
        event(&bob.topic(&math.id)?, "收到"),
    )?;
    assert!(alice.receive(&reply)?);
    assert_eq!(alice.messages(&math.id)?.len(), 2);
    Ok(())
}
#[test]
fn wrong_private_key_and_modified_ciphertext_cannot_decrypt() -> Result<()> {
    let alice = Account::new();
    let mut bob = Account::new();
    bob.generate_one_time_keys(1);
    let otk = *bob.one_time_keys().values().next().unwrap();
    let mut sender =
        alice.create_outbound_session(SessionConfig::version_1(), bob.curve25519_key(), otk)?;
    let first = sender.encrypt("first")?;
    let OlmMessage::PreKey(prekey) = &first else {
        panic!("prekey expected")
    };
    let mut wrong = Account::new();
    assert!(
        wrong
            .create_inbound_session(SessionConfig::version_1(), alice.curve25519_key(), prekey)
            .is_err()
    );
    let mut receiver = bob
        .create_inbound_session(SessionConfig::version_1(), alice.curve25519_key(), prekey)?
        .session;
    let reply = receiver.encrypt("reply")?;
    assert_eq!(sender.decrypt(&reply)?, b"reply");
    let message = sender.encrypt("secret")?;
    let (kind, bytes) = message.to_parts();
    let mut altered = bytes.to_vec();
    *altered.last_mut().unwrap() ^= 1;
    let altered = OlmMessage::from_parts(kind, &altered)?;
    assert!(receiver.decrypt(&altered).is_err());
    assert_eq!(receiver.decrypt(&message)?, b"secret");
    Ok(())
}
#[test]
fn duplicate_and_out_of_order_envelopes_are_safe() -> Result<()> {
    let Pair {
        mut alice, mut bob, ..
    } = pair()?;
    let peer = bob.contact_card()?.user_id;
    let topic = alice.create_topic(&peer, "ordering")?;
    let a = alice.queue_event(&peer, event(&topic, "one"))?;
    let b = alice.queue_event(&peer, event(&topic, "two"))?;
    let c = alice.queue_event(&peer, event(&topic, "three"))?;
    assert!(bob.receive(&c)?);
    assert!(bob.receive(&a)?);
    assert!(bob.receive(&b)?);
    assert!(!bob.receive(&b)?);
    assert_eq!(bob.messages(&topic.id)?.len(), 3);
    Ok(())
}
#[test]
fn tampering_does_not_poison_state_or_deduplication() -> Result<()> {
    let Pair {
        mut alice, mut bob, ..
    } = pair()?;
    let peer = bob.contact_card()?.user_id;
    let topic = alice.create_topic(&peer, "tamper")?;
    let good = alice.queue_event(&peer, event(&topic, "unchanged"))?;
    let mut bad = good.clone();
    bad.ciphertext.push('X');
    assert!(bob.receive(&bad).is_err());
    bad = good.clone();
    bad.id = Uuid::new_v4().to_string();
    assert!(bob.receive(&bad).is_err());
    bad = good.clone();
    bad.timestamp += 1;
    assert!(bob.receive(&bad).is_err());
    assert!(bob.receive(&good)?);
    assert!(!bob.receive(&good)?);
    Ok(())
}
#[test]
fn restart_preserves_both_ratchets_identity_and_pending_ciphertext() -> Result<()> {
    let Pair {
        root,
        mut alice,
        mut bob,
    } = pair()?;
    let alice_card = alice.contact_card()?;
    let bob_card = bob.contact_card()?;
    let topic = alice.create_topic(&bob_card.user_id, "persistent")?;
    let first = alice.queue_event(&bob_card.user_id, event(&topic, "before restart"))?;
    bob.receive(&first)?;
    let reply = bob.queue_event(
        &alice_card.user_id,
        event(&bob.topic(&topic.id)?, "ratchet step"),
    )?;
    alice.receive(&reply)?;
    let pending = alice.pending_envelopes()?;
    drop(alice);
    drop(bob);
    let mut alice = Endpoint::open(root.path().join("alice"), PASS, None, RELAY)?;
    let mut bob = Endpoint::open(root.path().join("bob"), PASS, None, RELAY)?;
    assert_eq!(alice.contact_card()?, alice_card);
    assert_eq!(alice.pending_envelopes()?, pending);
    assert!(!bob.receive(&first)?);
    let after = alice.queue_event(
        &bob_card.user_id,
        event(&alice.topic(&topic.id)?, "after restart"),
    )?;
    bob.receive(&after)?;
    assert_eq!(bob.messages(&topic.id)?.len(), 3);
    drop(alice);
    assert!(
        Endpoint::open(
            root.path().join("alice"),
            "wrong-long-passphrase",
            None,
            RELAY
        )
        .is_err()
    );
    // No private account or ratchet JSON is stored in cleartext.
    let db = Connection::open(root.path().join("bob/domain.sqlite"))?;
    let pickle: String = db.query_row("SELECT pickle FROM identity", [], |r| r.get(0))?;
    assert!(serde_json::from_str::<serde_json::Value>(&pickle).is_err());
    Ok(())
}
#[test]
fn identity_pin_and_prekey_signature_are_enforced() -> Result<()> {
    let root = TempDir::new()?;
    let mut alice = Endpoint::open(root.path().join("a"), PASS, Some("A"), RELAY)?;
    let mut bob = Endpoint::open(root.path().join("b"), PASS, Some("B"), RELAY)?;
    let mut eve = Endpoint::open(root.path().join("e"), PASS, Some("E"), RELAY)?;
    let card = bob.contact_card()?;
    alice.add_peer(card.clone())?;
    assert!(alice.establish(&card.user_id, bundle(&mut eve)?).is_err());
    let mut broken = bundle(&mut bob)?;
    broken.signed_prekey.key = eve.contact_card()?.curve_key;
    assert!(alice.establish(&card.user_id, broken).is_err());
    let mut bad_card = card;
    bad_card.label = "forged".into();
    assert!(alice.add_peer(bad_card).is_err());
    Ok(())
}
#[test]
fn simultaneous_first_contact_keeps_both_sessions() -> Result<()> {
    let Pair {
        mut alice, mut bob, ..
    } = pair()?;
    let a = alice.contact_card()?.user_id;
    let b = bob.contact_card()?.user_id;
    bob.establish(&a, bundle(&mut alice)?)?;
    let t1 = alice.create_topic(&b, "Alice initiated")?;
    let t2 = bob.create_topic(&a, "Bob initiated")?;
    let m1 = alice.queue_event(&b, event(&t1, "first a"))?;
    let m2 = bob.queue_event(&a, event(&t2, "first b"))?;
    alice.receive(&m2)?;
    bob.receive(&m1)?;
    let m3 = alice.queue_event(&b, event(&t1, "next a"))?;
    let m4 = bob.queue_event(&a, event(&t2, "next b"))?;
    bob.receive(&m3)?;
    alice.receive(&m4)?;
    assert_eq!(alice.messages(&t1.id)?.len(), 2);
    assert_eq!(bob.messages(&t2.id)?.len(), 2);
    Ok(())
}
#[test]
fn topics_cannot_be_injected_across_peers_and_validation_is_atomic() -> Result<()> {
    let Pair {
        root,
        mut alice,
        mut bob,
    } = pair()?;
    let charlie = Endpoint::open(root.path().join("c"), PASS, Some("Charlie"), RELAY)?;
    let b = bob.contact_card()?.user_id;
    bob.add_peer(charlie.contact_card()?)?;
    let foreign = bob.create_topic(&charlie.contact_card()?.user_id, "private with Charlie")?;
    let forged = alice.queue_event(&b, event(&foreign, "intrusion"))?;
    assert!(bob.receive(&forged).is_err());
    assert!(bob.messages(&foreign.id)?.is_empty());
    let valid = alice.create_topic(&b, "valid")?;
    let env = alice.queue_event(&b, event(&valid, "allowed"))?;
    assert!(bob.receive(&env)?);
    assert_eq!(bob.messages(&valid.id)?.len(), 1);
    Ok(())
}
#[test]
fn single_writer_and_remote_https_are_required() -> Result<()> {
    let root = TempDir::new()?;
    let _endpoint = Endpoint::open(root.path(), PASS, Some("A"), RELAY)?;
    assert!(Endpoint::open(root.path(), PASS, None, RELAY).is_err());
    assert!(RelayClient::new("http://192.168.3.5:8787").is_err());
    assert!(RelayClient::new("https://relay.example.com").is_ok());
    assert!(RelayClient::new("https://user:password@relay.example.com").is_err());
    Ok(())
}

struct TestRelay {
    url: String,
    path: PathBuf,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl TestRelay {
    async fn start(path: &Path) -> Result<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}", listener.local_addr()?);
        let app = topicairn_relay::router(path)?;
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Ok(Self {
            url,
            path: path.to_owned(),
            task: Some(task),
        })
    }
    async fn stop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = task.await;
        }
    }
    async fn restart(&mut self) -> Result<()> {
        self.stop().await;
        let addr = self.url.strip_prefix("http://").unwrap();
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let app = topicairn_relay::router(&self.path)?;
        self.task = Some(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        }));
        Ok(())
    }
}
impl Drop for TestRelay {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}
async fn request<T: serde::de::DeserializeOwned>(
    relay: &TestRelay,
    domain: &Endpoint,
    method: &str,
    path: &str,
    body: Vec<u8>,
) -> Result<T> {
    let auth = domain.request_auth(method, path, &body)?;
    RelayClient::new(&relay.url)?
        .request(method, path, body, auth)
        .await
}
#[tokio::test]
async fn offline_first_contact_ack_and_all_three_restarts() -> Result<()> {
    let root = TempDir::new()?;
    let mut relay = TestRelay::start(&root.path().join("relay.sqlite")).await?;
    let mut alice = Endpoint::open(root.path().join("a"), PASS, Some("Alice"), &relay.url)?;
    let mut bob = Endpoint::open(root.path().join("b"), PASS, Some("Bob"), &relay.url)?;
    let a = alice.contact_card()?;
    let b = bob.contact_card()?;
    alice.add_peer(b.clone())?;
    bob.add_peer(a.clone())?;
    alice.publish().await?;
    bob.publish().await?;
    drop(bob); // Bob is offline before Alice has any session.
    let topic = alice.create_topic(&b.user_id, "offline mathematics")?;
    alice.send_message(&topic.id, BODY, None).await?;
    let report = alice.sync(true).await?;
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.sent, 1);
    let envelope = alice.pending_envelopes()?[0].clone();
    // Resending exactly the same envelope is idempotent.
    let twice: Delivery = request(
        &relay,
        &alice,
        "POST",
        "/messages",
        serde_json::to_vec(&envelope)?,
    )
    .await?;
    assert!(!twice.acknowledged);
    drop(alice);
    relay.restart().await?;
    let db = Connection::open(&relay.path)?;
    let raw: String = db.query_row("SELECT envelope FROM envelopes", [], |r| r.get(0))?;
    assert!(!raw.contains("mathematics"));
    assert!(!raw.contains("证明"));
    drop(db);
    let mut bob = Endpoint::open(root.path().join("b"), PASS, None, &relay.url)?;
    let mut alice = Endpoint::open(root.path().join("a"), PASS, None, &relay.url)?;
    let report = bob.sync(true).await?;
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.received, 1);
    assert_eq!(report.acknowledged, 1);
    assert_eq!(bob.messages(&topic.id)?[0].body, BODY);
    let page: InboxPage = request(&relay, &bob, "GET", "/messages?cursor=0", vec![]).await?;
    assert!(page.items.is_empty());
    let ack: Delivery = request(
        &relay,
        &bob,
        "POST",
        &format!("/messages/{}/ack", envelope.id),
        vec![],
    )
    .await?;
    assert!(ack.acknowledged);
    let report = alice.sync(true).await?;
    assert!(report.errors.is_empty());
    assert_eq!(report.delivered, 1);
    assert_eq!(alice.messages(&topic.id)?[0].delivery, "delivered");
    assert!(alice.outbox()?.is_empty());
    bob.send_message(&topic.id, "reply after restart", None)
        .await?;
    bob.sync(true).await?;
    alice.sync(true).await?;
    assert_eq!(alice.messages(&topic.id)?.len(), 2);
    // ACKed envelopes keep a hash tombstone, never requeue after retry.
    let replay: Delivery = request(
        &relay,
        &alice,
        "POST",
        "/messages",
        serde_json::to_vec(&envelope)?,
    )
    .await?;
    assert!(replay.acknowledged);
    Ok(())
}
#[tokio::test]
async fn relay_authorization_replay_and_one_time_claims() -> Result<()> {
    let root = TempDir::new()?;
    let relay = TestRelay::start(&root.path().join("relay.sqlite")).await?;
    let mut alice = Endpoint::open(root.path().join("a"), PASS, Some("A"), &relay.url)?;
    let mut bob = Endpoint::open(root.path().join("b"), PASS, Some("B"), &relay.url)?;
    alice.add_peer(bob.contact_card()?)?;
    bob.add_peer(alice.contact_card()?)?;
    bob.publish().await?;
    let b = bob.contact_card()?.user_id;
    let path = format!("/prekeys/{b}/claim");
    let first: PrekeyBundle = request(&relay, &alice, "POST", &path, vec![]).await?;
    let key = first.one_time_prekey.as_ref().unwrap().key.clone();
    bob.publish().await?; // Re-upload cannot resurrect consumed one-time keys.
    let second: PrekeyBundle = request(&relay, &alice, "POST", &path, vec![]).await?;
    assert_ne!(key, second.one_time_prekey.unwrap().key);
    alice.establish(&b, first)?;
    let topic = alice.create_topic(&b, "auth")?;
    let env = alice.queue_event(&b, event(&topic, "private"))?;
    let _: Delivery = request(
        &relay,
        &alice,
        "POST",
        "/messages",
        serde_json::to_vec(&env)?,
    )
    .await?;
    let my_page: InboxPage = request(&relay, &alice, "GET", "/messages?cursor=0", vec![]).await?;
    assert!(my_page.items.is_empty());
    assert!(
        request::<Delivery>(
            &relay,
            &alice,
            "POST",
            &format!("/messages/{}/ack", env.id),
            vec![]
        )
        .await
        .is_err()
    );
    let client = RelayClient::new(&relay.url)?;
    let auth = bob.request_auth("GET", "/messages?cursor=0", &[])?;
    let page: InboxPage = client
        .request("GET", "/messages?cursor=0", vec![], auth.clone())
        .await?;
    assert_eq!(page.items.len(), 1);
    assert!(
        client
            .request::<InboxPage>("GET", "/messages?cursor=0", vec![], auth)
            .await
            .is_err()
    );
    let wrong_path = bob.request_auth("GET", "/messages?cursor=0", &[])?;
    assert!(
        client
            .request::<InboxPage>("GET", "/messages?cursor=1", vec![], wrong_path)
            .await
            .is_err()
    );
    let auth = bob.request_auth("GET", "/messages?cursor=0", &[])?;
    let mut expired = auth.clone();
    expired.timestamp = 0;
    assert!(
        client
            .request::<InboxPage>("GET", "/messages?cursor=0", vec![], expired)
            .await
            .is_err()
    );
    let unauthenticated = reqwest::get(format!("{}/messages?cursor=0", relay.url)).await?;
    assert_eq!(unauthenticated.status(), reqwest::StatusCode::UNAUTHORIZED);
    Ok(())
}
#[tokio::test]
async fn network_failure_preserves_exact_ciphertext_for_retry() -> Result<()> {
    let root = TempDir::new()?;
    let mut relay = TestRelay::start(&root.path().join("relay.sqlite")).await?;
    let mut alice = Endpoint::open(root.path().join("a"), PASS, Some("A"), &relay.url)?;
    let mut bob = Endpoint::open(root.path().join("b"), PASS, Some("B"), &relay.url)?;
    alice.add_peer(bob.contact_card()?)?;
    bob.add_peer(alice.contact_card()?)?;
    bob.publish().await?;
    let topic = alice.create_topic(&bob.contact_card()?.user_id, "retry")?;
    alice.send_message(&topic.id, "one", None).await?;
    let original = alice.pending_envelopes()?;
    relay.stop().await;
    let report = tokio::time::timeout(Duration::from_secs(5), alice.sync(true)).await??;
    assert!(!report.errors.is_empty());
    assert_eq!(alice.pending_envelopes()?, original);
    assert_eq!(alice.outbox()?[0].attempts, 1);
    relay.restart().await?;
    let report = alice.sync(true).await?;
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    bob.sync(true).await?;
    assert_eq!(bob.messages(&topic.id)?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn tls_ca_and_hostname_verification_are_enforced() -> Result<()> {
    use rcgen::{
        BasicConstraints, CertificateParams, CertifiedIssuer, IsCa, KeyPair, KeyUsagePurpose,
    };
    let root = TempDir::new()?;
    let mut ca_params = CertificateParams::new(Vec::<String>::new())?;
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign];
    let ca = CertifiedIssuer::self_signed(ca_params, KeyPair::generate()?)?;
    let server_key = KeyPair::generate()?;
    // A DNS SAN for localhost only, deliberately without the numeric loopback IP.
    let cert = CertificateParams::new(vec!["localhost".into()])?.signed_by(&server_key, &ca)?;
    let ca_path = root.path().join("ca.pem");
    std::fs::write(&ca_path, ca.pem())?;
    let config = axum_server::tls_rustls::RustlsConfig::from_pem(
        format!("{}{}", cert.pem(), ca.pem()).into_bytes(),
        server_key.serialize_pem().into_bytes(),
    )
    .await?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    listener.set_nonblocking(true)?;
    let app = topicairn_relay::router(root.path().join("relay.sqlite"))?;
    let task = tokio::spawn(async move {
        axum_server::tls_rustls::from_tcp_rustls(listener, config)
            .unwrap()
            .serve(app.into_make_service())
            .await
            .unwrap();
    });
    let domain = Endpoint::open(root.path().join("domain"), PASS, Some("TLS"), RELAY)?;
    let auth = domain.request_auth("GET", "/health", &[])?;
    let url = format!("https://localhost:{port}");
    let good = RelayClient::with_ca(&url, Some(&ca_path))?;
    let result: serde_json::Value = good.request("GET", "/health", vec![], auth.clone()).await?;
    assert_eq!(result["status"], "ok");
    assert!(
        RelayClient::new(&url)?
            .request::<serde_json::Value>("GET", "/health", vec![], auth.clone())
            .await
            .is_err()
    );
    let wrong_host = RelayClient::with_ca(&format!("https://127.0.0.1:{port}"), Some(&ca_path))?;
    assert!(
        wrong_host
            .request::<serde_json::Value>("GET", "/health", vec![], auth)
            .await
            .is_err()
    );
    assert!(RelayClient::with_ca(RELAY, Some(&ca_path)).is_err());
    task.abort();
    let _ = task.await;
    Ok(())
}
