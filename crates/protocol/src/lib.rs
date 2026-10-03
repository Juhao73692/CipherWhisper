//! Versioned public routing and signed identity types. Conversation data is ciphertext only.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};
use vodozemac::{Curve25519PublicKey, Ed25519PublicKey, Ed25519Signature};

pub const VERSION: u32 = 1;
pub const MAX_BODY: usize = 64 * 1024;
pub const MAX_ENVELOPE: usize = 128 * 1024;
pub const AUTH_WINDOW: i64 = 300;
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
pub fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
pub fn user_id(signing_key: &str) -> Result<String> {
    let key = Ed25519PublicKey::from_base64(signing_key)?;
    Ok(format!("td_{}", digest(key.as_bytes())))
}
pub fn verify(key: &str, bytes: &[u8], signature: &str) -> Result<()> {
    Ed25519PublicKey::from_base64(key)?
        .verify(bytes, &Ed25519Signature::from_base64(signature)?)?;
    Ok(())
}
pub fn uuid(id: &str) -> Result<()> {
    uuid::Uuid::parse_str(id)?;
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ContactCard {
    pub version: u32,
    pub user_id: String,
    pub signing_key: String,
    pub curve_key: String,
    pub label: String,
    pub signature: String,
}
impl ContactCard {
    pub fn signing_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&(
            "topicairn.contact.v1",
            self.version,
            &self.user_id,
            &self.signing_key,
            &self.curve_key,
            &self.label,
        ))
        .expect("serializable")
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(self.version == VERSION, "unsupported identity version");
        ensure!(
            self.user_id == user_id(&self.signing_key)?,
            "identity fingerprint mismatch"
        );
        ensure!(
            !self.label.trim().is_empty() && self.label.len() <= 128,
            "invalid identity label"
        );
        Curve25519PublicKey::from_base64(&self.curve_key)?;
        verify(&self.signing_key, &self.signing_bytes(), &self.signature)
    }
}

/// Olm's signed fallback prekey + optional atomically claimed one-time prekey.
/// This is Olm 3DH, not wire-compatible X3DH/PQXDH.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedPrekey {
    pub key: String,
    pub expires_at: i64,
    pub signature: String,
}
impl SignedPrekey {
    pub fn signing_bytes(&self, card: &ContactCard, fallback: bool) -> Vec<u8> {
        serde_json::to_vec(&(
            "topicairn.prekey.v1",
            &card.user_id,
            &card.curve_key,
            &self.key,
            self.expires_at,
            fallback,
        ))
        .expect("serializable")
    }
    pub fn validate(&self, card: &ContactCard, fallback: bool) -> Result<()> {
        ensure!(
            self.expires_at > now() && self.expires_at <= now() + 32 * 86400,
            "expired or invalid prekey"
        );
        Curve25519PublicKey::from_base64(&self.key)?;
        verify(
            &card.signing_key,
            &self.signing_bytes(card, fallback),
            &self.signature,
        )
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrekeyBundle {
    pub identity: ContactCard,
    pub signed_prekey: SignedPrekey,
    pub one_time_prekey: Option<SignedPrekey>,
}
impl PrekeyBundle {
    pub fn validate(&self) -> Result<()> {
        self.identity.validate()?;
        self.signed_prekey.validate(&self.identity, true)?;
        if let Some(key) = &self.one_time_prekey {
            key.validate(&self.identity, false)?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrekeyUpload {
    pub identity: ContactCard,
    pub signed_prekey: SignedPrekey,
    pub one_time_prekeys: Vec<SignedPrekey>,
}
impl PrekeyUpload {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.one_time_prekeys.len() <= 64, "too many prekeys");
        self.identity.validate()?;
        self.signed_prekey.validate(&self.identity, true)?;
        for key in &self.one_time_prekeys {
            key.validate(&self.identity, false)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub version: u32,
    pub id: String,
    pub from: String,
    pub to: String,
    /// Serialized Olm message: includes the library's opaque ratchet header.
    pub ciphertext: String,
    pub timestamp: i64,
    pub signature: String,
}
impl Envelope {
    pub fn signing_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&(
            "topicairn.envelope.v1",
            self.version,
            &self.id,
            &self.from,
            &self.to,
            &self.ciphertext,
            self.timestamp,
        ))
        .expect("serializable")
    }
    pub fn validate(&self, signing_key: &str) -> Result<()> {
        ensure!(self.version == VERSION, "unsupported envelope version");
        uuid(&self.id)?;
        ensure!(
            self.from == user_id(signing_key)?,
            "sender identity mismatch"
        );
        ensure!(
            self.to.starts_with("td_") && self.to.len() == 67,
            "invalid recipient"
        );
        ensure!(
            self.ciphertext.len() <= MAX_ENVELOPE && self.timestamp >= 0,
            "invalid envelope"
        );
        verify(signing_key, &self.signing_bytes(), &self.signature)
    }
    pub fn digest(&self) -> String {
        digest(&self.signing_bytes())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueuedEnvelope {
    pub cursor: i64,
    pub envelope: Envelope,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InboxPage {
    pub items: Vec<QueuedEnvelope>,
    pub next_cursor: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Delivery {
    pub id: String,
    pub acknowledged: bool,
}

/// Request signature binds method, full path/query, exact body, timestamp and unique nonce.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RequestAuth {
    pub signing_key: String,
    pub timestamp: i64,
    pub nonce: String,
    pub signature: String,
}
impl RequestAuth {
    pub fn signing_bytes(&self, method: &str, path: &str, body: &[u8]) -> Vec<u8> {
        serde_json::to_vec(&(
            "topicairn.http.v1",
            method,
            path,
            digest(body),
            self.timestamp,
            &self.nonce,
        ))
        .expect("serializable")
    }
    pub fn validate(&self, method: &str, path: &str, body: &[u8]) -> Result<String> {
        ensure!(
            self.timestamp.abs_diff(now()) <= AUTH_WINDOW as u64,
            "request expired"
        );
        uuid(&self.nonce)?;
        verify(
            &self.signing_key,
            &self.signing_bytes(method, path, body),
            &self.signature,
        )?;
        user_id(&self.signing_key)
    }
}

// Everything below belongs to the client Conversation Layer and is never a Relay field.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Topic {
    pub id: String,
    pub peer_id: String,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub archived: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    pub topic_id: String,
    pub sender_id: String,
    pub timestamp: i64,
    pub body: String,
    pub format: String,
    pub reply_to: Option<String>,
    pub delivery: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Event {
    Message {
        message_id: String,
        topic_id: String,
        topic_title: String,
        created_at: i64,
        body: String,
        format: String,
        reply_to: Option<String>,
    },
    #[serde(rename = "topic.update")]
    TopicUpdate {
        topic_id: String,
        title: String,
        created_at: i64,
        archived: bool,
    },
}
impl Event {
    pub fn validate(&self) -> Result<()> {
        let (topic_id, title, created) = match self {
            Self::Message {
                message_id,
                topic_id,
                topic_title,
                created_at,
                body,
                format,
                reply_to,
            } => {
                uuid(message_id)?;
                ensure!(
                    format == "markdown" && !body.trim().is_empty() && body.len() <= MAX_BODY,
                    "invalid Markdown body"
                );
                if let Some(id) = reply_to {
                    uuid(id)?;
                }
                (topic_id, topic_title, created_at)
            }
            Self::TopicUpdate {
                topic_id,
                title,
                created_at,
                ..
            } => (topic_id, title, created_at),
        };
        uuid(topic_id)?;
        ensure!(
            !title.trim().is_empty() && title.len() <= 256 && *created >= 0,
            "invalid topic"
        );
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Payload {
    pub version: u32,
    pub envelope_id: String,
    pub sender: String,
    pub recipient: String,
    pub timestamp: i64,
    pub event: Event,
}
impl Payload {
    pub fn validate_for(&self, env: &Envelope) -> Result<()> {
        ensure!(
            self.version == env.version
                && self.envelope_id == env.id
                && self.sender == env.from
                && self.recipient == env.to
                && self.timestamp == env.timestamp,
            "encrypted routing binding mismatch"
        );
        self.event.validate()
    }
}
