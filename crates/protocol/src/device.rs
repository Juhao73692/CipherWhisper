//! Internal Trust Domain device protocol; never used by the opaque Relay.
use crate::*;
pub const DEVICE_VERSION: u32 = 1;
pub const PAGE_LIMIT: usize = 100;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DeviceCard {
    pub version: u32,
    pub id: String,
    pub label: String,
    pub signing_key: String,
    pub signature: String,
}
pub fn device_id(key: &str) -> Result<String> {
    Ok(format!(
        "dev_{}",
        digest(Ed25519PublicKey::from_base64(key)?.as_bytes())
    ))
}
impl DeviceCard {
    pub fn signing_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&(
            "topicairn.device.card.v1",
            self.version,
            &self.id,
            &self.label,
            &self.signing_key,
        ))
        .unwrap()
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == DEVICE_VERSION && self.id == device_id(&self.signing_key)?,
            "invalid device identity"
        );
        ensure!(
            !self.label.trim().is_empty() && self.label.len() <= 128,
            "invalid device label"
        );
        verify(&self.signing_key, &self.signing_bytes(), &self.signature)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Pairing {
    pub version: u32,
    pub device: DeviceCard,
    pub domain: ContactCard,
    pub server: String,
    pub ca_pem: String,
    pub epoch: String,
    pub signature: String,
}
impl Pairing {
    pub fn signing_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&(
            "topicairn.device.pair.v1",
            self.version,
            &self.device,
            &self.domain,
            &self.server,
            &self.ca_pem,
            &self.epoch,
        ))
        .unwrap()
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == DEVICE_VERSION,
            "unsupported device protocol"
        );
        self.device.validate()?;
        self.domain.validate()?;
        uuid(&self.epoch)?;
        ensure!(self.ca_pem.len() <= 256 * 1024, "CA too large");
        verify(
            &self.domain.signing_key,
            &self.signing_bytes(),
            &self.signature,
        )
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceAuth {
    pub signing_key: String,
    pub domain_id: String,
    pub timestamp: i64,
    pub nonce: String,
    pub signature: String,
}
impl DeviceAuth {
    pub fn signing_bytes(&self, method: &str, path: &str, body: &[u8]) -> Vec<u8> {
        serde_json::to_vec(&(
            "topicairn.device.http.v1",
            &self.domain_id,
            method,
            path,
            digest(body),
            self.timestamp,
            &self.nonce,
        ))
        .unwrap()
    }
    pub fn validate(&self, domain: &str, method: &str, path: &str, body: &[u8]) -> Result<String> {
        ensure!(self.domain_id == domain, "wrong trust domain");
        ensure!(
            self.timestamp.abs_diff(now()) <= AUTH_WINDOW as u64,
            "device request expired"
        );
        uuid(&self.nonce)?;
        verify(
            &self.signing_key,
            &self.signing_bytes(method, path, body),
            &self.signature,
        )?;
        device_id(&self.signing_key)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Entity {
    Peer(ContactCard),
    Topic(Topic),
    Message(Message),
}
impl Entity {
    pub fn key(&self) -> (&'static str, &str) {
        match self {
            Self::Peer(v) => ("peer", &v.user_id),
            Self::Topic(v) => ("topic", &v.id),
            Self::Message(v) => ("message", &v.id),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub seq: i64,
    pub entity: Entity,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Page {
    pub epoch: String,
    pub from_cursor: i64,
    pub next_cursor: i64,
    pub high_water: i64,
    pub changes: Vec<Change>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    AddPeer {
        card: ContactCard,
    },
    CreateTopic {
        topic_id: String,
        peer_id: String,
        title: String,
    },
    UpdateTopic {
        topic_id: String,
        title: String,
        archived: bool,
        base_title: String,
        base_archived: bool,
    },
    Send {
        message_id: String,
        topic_id: String,
        body: String,
        reply_to: Option<String>,
        timestamp: i64,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    pub id: String,
    pub operation: Operation,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum CommandResult {
    Accepted {
        entity: Entity,
        revision: i64,
    },
    Rejected {
        error: String,
        current: Option<Change>,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandReply {
    pub id: String,
    pub body_digest: String,
    pub result: CommandResult,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ack {
    pub epoch: String,
    pub cursor: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedResponse<T> {
    pub version: u32,
    pub domain_id: String,
    pub device_id: String,
    pub nonce: String,
    pub data: T,
    pub signature: String,
}
impl<T: Serialize> SignedResponse<T> {
    pub fn signing_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&(
            "topicairn.device.response.v1",
            self.version,
            &self.domain_id,
            &self.device_id,
            &self.nonce,
            &self.data,
        ))
        .unwrap()
    }
    pub fn validate(&self, pair: &Pairing, nonce: &str) -> Result<()> {
        ensure!(
            self.version == DEVICE_VERSION
                && self.domain_id == pair.domain.user_id
                && self.device_id == pair.device.id
                && self.nonce == nonce,
            "device response binding mismatch"
        );
        verify(
            &pair.domain.signing_key,
            &self.signing_bytes(),
            &self.signature,
        )
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceStatus {
    pub card: DeviceCard,
    pub revoked: bool,
    pub created_at: i64,
    pub last_seen: i64,
    pub acknowledged_cursor: i64,
}
