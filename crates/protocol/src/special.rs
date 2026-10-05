//! Forward-compatible application operations carried only inside Event::Control.
//! Unknown versions/types and malformed payloads remain inert, inspectable text.
use crate::*;
use serde_json::Value;

pub const SPECIAL_VERSION: u32 = 1;
pub const CHUNK_BYTES: usize = 24 * 1024;
pub const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Special {
    pub version: u32,
    pub kind: String,
    pub data: Value,
}
impl Special {
    pub fn new(kind: &str, data: Value) -> Self {
        Self {
            version: SPECIAL_VERSION,
            kind: kind.into(),
            data,
        }
    }
    pub fn body(&self) -> Result<String> {
        self.validate()?;
        let body = serde_json::to_string(self)?;
        ensure!(body.len() <= MAX_BODY, "special message too large");
        Ok(body)
    }
    pub fn parse(body: &str) -> Result<Self> {
        let s: Self = serde_json::from_str(body)?;
        s.validate()?;
        Ok(s)
    }
    pub fn text(&self, key: &str) -> Result<&str> {
        self.data
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("missing/invalid {key}"))
    }
    pub fn number(&self, key: &str) -> Result<u64> {
        self.data
            .get(key)
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow::anyhow!("missing/invalid {key}"))
    }
    pub fn target(&self) -> &str {
        self.data
            .get(if self.kind.starts_with("message.") {
                "messageId"
            } else {
                "fileId"
            })
            .and_then(Value::as_str)
            .unwrap_or("")
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == SPECIAL_VERSION,
            "unsupported special message version {}",
            self.version
        );
        ensure!(self.data.is_object(), "special data must be an object");
        if let Some(revision) = self.data.get("revision") {
            ensure!(
                revision.as_i64().is_some_and(|r| r >= 0),
                "invalid operation revision"
            );
        }
        match self.kind.as_str() {
            "message.edit" => {
                uuid(self.text("messageId")?)?;
                let body = self.text("body")?;
                ensure!(
                    !body.trim().is_empty() && body.len() <= MAX_BODY,
                    "invalid edited text"
                );
            }
            "message.withdraw" => uuid(self.text("messageId")?)?,
            "topic.meta" => {
                ensure!(
                    self.data.get("pinned").is_some_and(Value::is_boolean),
                    "missing pinned"
                );
                ensure!(
                    ["open", "active", "resolved"].contains(&self.text("status")?),
                    "invalid topic status"
                );
                let tags = self
                    .data
                    .get("tags")
                    .and_then(Value::as_array)
                    .ok_or_else(|| anyhow::anyhow!("missing tags"))?;
                ensure!(
                    tags.len() <= 12
                        && tags.iter().all(|v| v
                            .as_str()
                            .is_some_and(|s| !s.trim().is_empty() && s.len() <= 64)),
                    "invalid tags"
                );
            }
            "file.accept" => {
                uuid(self.text("fileId")?)?;
                uuid(self.text("offerId")?)?;
            }
            "file.chunk" => {
                uuid(self.text("fileId")?)?;
                uuid(self.text("offerId")?)?;
                uuid(self.text("acceptId")?)?;
                ensure!(
                    self.number("part")? < (MAX_FILE_BYTES / CHUNK_BYTES + 1) as u64,
                    "invalid chunk index"
                );
                let raw = self.text("hex")?;
                ensure!(
                    raw.len() <= CHUNK_BYTES * 2 && raw.len().is_multiple_of(2),
                    "invalid chunk size"
                );
                hex::decode(raw)?;
            }
            _ => anyhow::bail!("unknown special message type {}", self.kind),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_and_future_are_inert() {
        assert!(Special::parse("ordinary text").is_err());
        for raw in [
            "{",
            r#"{"version":2,"kind":"message.withdraw","data":{}}"#,
            r#"{"version":1,"kind":"future.type","data":{}}"#,
        ] {
            assert!(Special::parse(raw).is_err());
        }
    }
}
