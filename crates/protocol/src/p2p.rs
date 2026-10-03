//! Direct center-to-center transport. Identity remains independent of addresses.
use crate::*;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerProfile {
    pub version: u32,
    pub identity: ContactCard,
    pub endpoint: String,
    pub ca_pem: Option<String>,
    pub signature: String,
}
impl PeerProfile {
    pub fn signing_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&(
            "topicairn.peer.profile.v1",
            self.version,
            &self.identity,
            &self.endpoint,
            &self.ca_pem,
        ))
        .unwrap()
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(self.version == VERSION, "unsupported peer profile");
        self.identity.validate()?;
        ensure!(
            self.endpoint.len() <= 2048
                && self.ca_pem.as_ref().is_none_or(|ca| ca.len() <= 256 * 1024),
            "peer profile too large"
        );
        verify(
            &self.identity.signing_key,
            &self.signing_bytes(),
            &self.signature,
        )
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerAuth {
    pub signing_key: String,
    pub target: String,
    pub timestamp: i64,
    pub nonce: String,
    pub signature: String,
}
impl PeerAuth {
    pub fn signing_bytes(&self, method: &str, path: &str, body: &[u8]) -> Vec<u8> {
        serde_json::to_vec(&(
            "topicairn.peer.http.v1",
            &self.target,
            method,
            path,
            digest(body),
            self.timestamp,
            &self.nonce,
        ))
        .unwrap()
    }
    pub fn validate(&self, target: &str, method: &str, path: &str, body: &[u8]) -> Result<String> {
        ensure!(
            self.target == target && self.timestamp.abs_diff(now()) <= AUTH_WINDOW as u64,
            "wrong target or expired peer request"
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
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerResponse<T> {
    pub version: u32,
    pub from: String,
    pub to: String,
    pub nonce: String,
    pub data: T,
    pub signature: String,
}
impl<T: Serialize> PeerResponse<T> {
    pub fn signing_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&(
            "topicairn.peer.response.v1",
            self.version,
            &self.from,
            &self.to,
            &self.nonce,
            &self.data,
        ))
        .unwrap()
    }
    pub fn validate(&self, peer: &ContactCard, me: &str, nonce: &str) -> Result<()> {
        ensure!(
            self.version == VERSION
                && self.from == peer.user_id
                && self.to == me
                && self.nonce == nonce,
            "peer response binding mismatch"
        );
        verify(&peer.signing_key, &self.signing_bytes(), &self.signature)
    }
}
