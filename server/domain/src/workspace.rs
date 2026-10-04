//! One local API/UI backed by either the center or a separately keyed replica.
use anyhow::{Result, bail};
use cipherwhisper_core::{
    Endpoint, OutboxStatus, SyncReport,
    device::{Pending, Replica},
};
use cipherwhisper_protocol::device::*;
use cipherwhisper_protocol::*;
pub(crate) enum Workspace {
    Center(Endpoint),
    Client(Box<Replica>),
}
impl Workspace {
    pub(crate) fn transport_kind(&self) -> &'static str {
        match self {
            Self::Center(e) => {
                if e.is_direct() {
                    "direct"
                } else {
                    "relay"
                }
            }
            Self::Client(_) => "device",
        }
    }
    pub(crate) fn center(&mut self) -> Result<&mut Endpoint> {
        match self {
            Self::Center(e) => Ok(e),
            Self::Client(_) => bail!("device administration belongs to the center"),
        }
    }
    pub(crate) fn contact_card(&self) -> Result<ContactCard> {
        match self {
            Self::Center(e) => e.contact_card(),
            Self::Client(e) => e.contact_card(),
        }
    }
    pub(crate) fn peers(&self) -> Result<Vec<ContactCard>> {
        match self {
            Self::Center(e) => e.peers(),
            Self::Client(e) => e.peers(),
        }
    }
    pub(crate) async fn add_peer(&mut self, card: ContactCard) -> Result<()> {
        match self {
            Self::Center(e) => e.add_peer(card),
            Self::Client(e) => e.add_peer(card).await,
        }
    }
    pub(crate) fn topic(&self, id: &str) -> Result<Topic> {
        match self {
            Self::Center(e) => e.topic(id),
            Self::Client(e) => e.topic(id),
        }
    }
    pub(crate) fn topics(&self, peer: Option<&str>) -> Result<Vec<Topic>> {
        match self {
            Self::Center(e) => e.topics(peer),
            Self::Client(e) => e.topics(peer),
        }
    }
    pub(crate) async fn create_topic(&mut self, peer: &str, title: &str) -> Result<Topic> {
        match self {
            Self::Center(e) => e.create_topic(peer, title),
            Self::Client(e) => e.create_topic(peer, title).await,
        }
    }
    pub(crate) async fn update_topic(
        &mut self,
        id: &str,
        title: &str,
        archived: bool,
    ) -> Result<()> {
        match self {
            Self::Center(e) => e.update_topic(id, title, archived).await,
            Self::Client(e) => e.update_topic(id, title, archived).await,
        }
    }
    pub(crate) fn messages(&self, id: &str) -> Result<Vec<Message>> {
        match self {
            Self::Center(e) => e.messages(id),
            Self::Client(e) => e.messages(id),
        }
    }
    pub(crate) fn unread(&self) -> Result<Vec<cipherwhisper_core::unread::UnreadTopic>> {
        match self {
            Self::Center(e) => e.unread(),
            Self::Client(e) => e.unread(),
        }
    }
    pub(crate) fn mark_read(&self, id: &str, through: &str) -> Result<()> {
        match self {
            Self::Center(e) => e.mark_read(id, through),
            Self::Client(e) => e.mark_read(id, through),
        }
    }
    pub(crate) fn search(&self, q: &str) -> Result<Vec<Message>> {
        match self {
            Self::Center(e) => e.search(q),
            Self::Client(e) => e.search(q),
        }
    }
    pub(crate) async fn send_message(
        &mut self,
        id: &str,
        body: &str,
        reply: Option<String>,
    ) -> Result<Message> {
        match self {
            Self::Center(e) => e.send_message(id, body, reply).await,
            Self::Client(e) => e.send_message(id, body, reply).await,
        }
    }
    pub(crate) async fn sync(&mut self, force: bool) -> Result<SyncReport> {
        match self {
            Self::Center(e) => e.sync(force).await,
            Self::Client(e) => e.sync(force).await,
        }
    }
    pub(crate) fn outbox(&self) -> Result<Vec<OutboxStatus>> {
        match self {
            Self::Center(e) => e.outbox(),
            Self::Client(e) => e.outbox(),
        }
    }
    pub(crate) async fn retry_outbox(&mut self, id: &str) -> Result<Message> {
        match self {
            Self::Center(e) => e.retry_outbox(id).await,
            Self::Client(e) => e.retry_outbox(id).await,
        }
    }
    pub(crate) fn mode(&self) -> &'static str {
        match self {
            Self::Center(_) => "server",
            Self::Client(_) => "client",
        }
    }
    pub(crate) fn device_info(&self) -> Result<Option<serde_json::Value>> {
        match self {
            Self::Center(_) => Ok(None),
            Self::Client(e) => Ok(Some(e.info()?)),
        }
    }
    pub(crate) fn pending(&self) -> Result<Vec<Pending>> {
        match self {
            Self::Center(_) => Ok(vec![]),
            Self::Client(e) => e.pending(),
        }
    }
    pub(crate) fn discard(&mut self, id: &str) -> Result<()> {
        match self {
            Self::Center(_) => bail!("only device clients have a device operation queue"),
            Self::Client(e) => e.discard_failed(id),
        }
    }
    pub(crate) fn devices(&mut self) -> Result<Vec<DeviceStatus>> {
        self.center()?.devices()
    }
}
