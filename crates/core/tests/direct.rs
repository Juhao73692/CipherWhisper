use anyhow::Result;
use tempfile::TempDir;
use topicairn_core::{Endpoint, direct::http_client};
use topicairn_protocol::*;
use uuid::Uuid;
const PASS: &str = "direct-test-passphrase-123";
fn endpoints() -> Result<(TempDir, Endpoint, Endpoint)> {
    let root = TempDir::new()?;
    let mut a = Endpoint::open_direct(root.path().join("a"), PASS, Some("Alice"))?;
    let mut b = Endpoint::open_direct(root.path().join("b"), PASS, Some("Bob"))?;
    a.set_direct_address("http://127.0.0.1:9", None)?;
    b.set_direct_address("http://127.0.0.1:9", None)?;
    a.add_direct_peer(b.direct_profile()?)?;
    b.add_direct_peer(a.direct_profile()?)?;
    Ok((root, a, b))
}
#[test]
fn profile_and_response_signatures_bind_identity_address_and_request() -> Result<()> {
    let (_root, mut a, b) = endpoints()?;
    let mut profile = b.direct_profile()?;
    profile.endpoint.push('x');
    assert!(a.add_direct_peer(profile).is_err());
    assert!(http_client("http://192.168.1.10:8800", None).is_err());
    assert!(http_client("https://example.com/path", None).is_err());
    let ingress = b.direct_ingress()?;
    let nonce = Uuid::new_v4().to_string();
    let mut response = ingress.response(&a.contact_card()?.user_id, &nonce, b.contact_card()?)?;
    response.validate(&b.contact_card()?, &a.contact_card()?.user_id, &nonce)?;
    assert!(
        response
            .validate(
                &b.contact_card()?,
                &a.contact_card()?.user_id,
                "wrong nonce"
            )
            .is_err()
    );
    response.data.label.push('x');
    assert!(
        response
            .validate(&b.contact_card()?, &a.contact_card()?.user_id, &nonce)
            .is_err()
    );
    Ok(())
}
#[test]
fn pinned_authentication_replay_and_claim_consumption_survive_restart() -> Result<()> {
    let (root, a, b) = endpoints()?;
    let mut ingress = b.direct_ingress()?;
    let auth = a.direct_auth(&b.contact_card()?.user_id, "POST", "/p2p/v1/ping", b"")?;
    assert!(
        ingress
            .authenticate(&auth, "POST", "/p2p/v1/prekeys/claim", b"")
            .is_err()
    );
    assert!(
        ingress
            .authenticate(&auth, "POST", "/p2p/v1/ping", b"changed")
            .is_err()
    );
    ingress.authenticate(&auth, "POST", "/p2p/v1/ping", b"")?;
    assert!(
        ingress
            .authenticate(&auth, "POST", "/p2p/v1/ping", b"")
            .is_err()
    );
    let first = ingress.claim()?.one_time_prekey.unwrap().key;
    for _ in 0..31 {
        assert_ne!(ingress.claim()?.one_time_prekey.unwrap().key, first);
    }
    assert!(ingress.claim()?.one_time_prekey.is_none());
    let c = Endpoint::open_direct(root.path().join("c"), PASS, Some("unknown"))?;
    assert!(
        ingress
            .authenticate(
                &c.direct_auth(&b.contact_card()?.user_id, "POST", "/p2p/v1/ping", b"")?,
                "POST",
                "/p2p/v1/ping",
                b""
            )
            .is_err()
    );
    drop(ingress);
    drop(b);
    let b = Endpoint::open_direct(root.path().join("b"), PASS, None)?;
    let mut ingress = b.direct_ingress()?;
    assert!(
        ingress
            .authenticate(&auth, "POST", "/p2p/v1/ping", b"")
            .is_err()
    );
    assert!(ingress.claim()?.one_time_prekey.is_none());
    Ok(())
}
#[tokio::test]
async fn direct_ack_requires_decrypted_commit_and_duplicate_ciphertext_is_idempotent() -> Result<()>
{
    let (root, mut a, b) = endpoints()?;
    let peer = b.contact_card()?.user_id;
    let mut inbox = b.direct_ingress()?;
    a.establish(&peer, inbox.claim()?)?;
    let t = a.create_topic(&peer, "数学")?;
    let body = "# 原始源文\n\n$x^2$\n\n```rust\nfn main() {}\n```";
    let env = a.queue_event(
        &peer,
        Event::Message {
            message_id: Uuid::new_v4().to_string(),
            topic_id: t.id.clone(),
            topic_title: t.title.clone(),
            created_at: t.created_at,
            body: body.into(),
            format: "markdown".into(),
            reply_to: None,
        },
    )?;
    assert!(
        !inbox
            .enqueue(&a.contact_card()?.user_id, env.clone())?
            .acknowledged
    );
    assert!(
        !inbox
            .enqueue(&a.contact_card()?.user_id, env.clone())?
            .acknowledged
    );
    assert!(b.messages(&t.id).is_err());
    drop(inbox);
    drop(b);
    let mut b = Endpoint::open_direct(root.path().join("b"), PASS, None)?;
    assert_eq!(b.sync(true).await?.received, 1);
    assert_eq!(b.messages(&t.id)?[0].body, body);
    let mut inbox = b.direct_ingress()?;
    assert!(
        inbox
            .delivery(&a.contact_card()?.user_id, &env.id)?
            .acknowledged
    );
    assert!(
        inbox
            .enqueue(&a.contact_card()?.user_id, env.clone())?
            .acknowledged
    );
    let mut modified = env.clone();
    modified.ciphertext.push('x');
    assert!(inbox.enqueue(&a.contact_card()?.user_id, modified).is_err());
    assert_eq!(b.sync(true).await?.received, 0);
    assert_eq!(b.messages(&t.id)?.len(), 1);
    let saved = a.pending_envelopes()?;
    assert!(!a.sync(true).await?.errors.is_empty());
    assert_eq!(a.pending_envelopes()?, saved);
    drop(a);
    let a = Endpoint::open_direct(root.path().join("a"), PASS, None)?;
    assert_eq!(a.pending_envelopes()?, saved);
    assert_eq!(a.direct_peers()?.len(), 1);
    Ok(())
}
