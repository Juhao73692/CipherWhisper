//! Ordinary file-invitation messages. No file bytes are included in the invitation.
use crate::special::{CHUNK_BYTES, MAX_FILE_BYTES};
use crate::*;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileOffer {
    pub version: u32,
    pub file_id: String,
    pub name: String,
    pub mime: String,
    pub size: usize,
    pub sha256: String,
    pub chunk_size: usize,
}
impl FileOffer {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.version == 1, "unsupported file invitation version");
        uuid(&self.file_id)?;
        let name = &self.name;
        ensure!(
            !name.trim().is_empty()
                && name.len() <= 255
                && !name.contains(['/', '\\', '\0'])
                && !name.chars().any(char::is_control)
                && name != "."
                && name != "..",
            "invalid filename"
        );
        ensure!(self.size <= MAX_FILE_BYTES, "file exceeds 16 MiB");
        ensure!(self.chunk_size == CHUNK_BYTES, "unsupported chunk size");
        ensure!(self.mime.len() <= 128, "invalid MIME type");
        ensure!(hex::decode(&self.sha256)?.len() == 32, "invalid SHA-256");
        Ok(())
    }
}
