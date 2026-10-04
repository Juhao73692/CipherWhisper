use anyhow::{Result, ensure};
use cipherwhisper_core::Endpoint;
use cipherwhisper_protocol::ContactCard;
use clap::{Parser, Subcommand};
use std::{io::Read, path::PathBuf};
use zeroize::Zeroizing;

#[derive(Parser)]
#[command(about = "CipherWhisper headless Trust Domain administration", version)]
pub struct AdminArgs {
    #[arg(long, default_value = "domain-data")]
    pub data: PathBuf,
    /// Optional legacy relay; the default is direct P2P.
    #[arg(long)]
    pub relay: Option<String>,
    /// Trust this PEM CA for the relay HTTPS connection.
    #[arg(long, requires = "relay")]
    pub relay_ca: Option<PathBuf>,
    #[arg(
        long,
        env = cipherwhisper_core::passphrase_env(),
        hide_env_values = true,
        hide = true
    )]
    pub passphrase: String,
    #[command(subcommand)]
    pub command: Command,
}
#[derive(Subcommand)]
pub enum Command {
    /// Create a stable local identity and its prekeys. Publish separately.
    Init {
        #[arg(long)]
        name: String,
    },
    /// Export the public signed contact card (safe to share).
    Identity,
    /// Publish authenticated public prekeys for offline first contact.
    Publish,
    /// Import a public contact card verified through a trusted channel.
    AddPeer {
        card: PathBuf,
    },
    Peers,
    NewTopic {
        #[arg(long)]
        peer: String,
        #[arg(long)]
        title: String,
        /// Print only the new topic UUID for use in shell variables.
        #[arg(long)]
        id_only: bool,
    },
    Topics {
        #[arg(long)]
        peer: Option<String>,
    },
    Send {
        #[arg(long)]
        topic: String,
        #[arg(long)]
        body: Option<String>,
        #[arg(long)]
        file: Option<PathBuf>,
        #[arg(long)]
        reply_to: Option<String>,
    },
    TopicUpdate {
        #[arg(long)]
        topic: String,
        #[arg(long)]
        title: String,
        #[arg(long)]
        archived: bool,
    },
    History {
        #[arg(long)]
        topic: String,
    },
    Search {
        query: String,
    },
    /// Resend queued ciphertext, receive messages, and acknowledge durable commits.
    Sync,
    Outbox,
}
fn print(value: impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
pub async fn run(args: AdminArgs) -> Result<()> {
    let passphrase = Zeroizing::new(args.passphrase);
    let label = match &args.command {
        Command::Init { name } => Some(name.as_str()),
        _ => None,
    };
    let mut domain = if let Some(relay) = &args.relay {
        Endpoint::open_with_ca(
            &args.data,
            &passphrase,
            label,
            relay,
            args.relay_ca.as_deref(),
        )?
    } else {
        Endpoint::open_direct(&args.data, &passphrase, label)?
    };
    match args.command {
        Command::Init { .. } | Command::Identity => print(domain.contact_card()?)?,
        Command::Publish => {
            domain.publish().await?;
            print(serde_json::json!({"published":true}))?;
        }
        Command::AddPeer { card } => {
            let data: serde_json::Value = serde_json::from_slice(&std::fs::read(card)?)?;
            let id = if data.get("identity").is_some() {
                let profile: cipherwhisper_protocol::p2p::PeerProfile =
                    serde_json::from_value(data)?;
                let id = profile.identity.user_id.clone();
                domain.add_direct_peer(profile)?;
                id
            } else {
                let card: ContactCard = serde_json::from_value(data)?;
                let id = card.user_id.clone();
                domain.add_peer(card)?;
                id
            };
            print(serde_json::json!({"added":id}))?;
        }
        Command::Peers => print(domain.peers()?)?,
        Command::NewTopic {
            peer,
            title,
            id_only,
        } => {
            let topic = domain.create_topic(&peer, &title)?;
            if id_only {
                println!("{}", topic.id);
            } else {
                print(topic)?;
            }
        }
        Command::Topics { peer } => print(domain.topics(peer.as_deref())?)?,
        Command::Send {
            topic,
            body,
            file,
            reply_to,
        } => {
            ensure!(
                !(body.is_some() && file.is_some()),
                "use either --body or --file"
            );
            let text = if let Some(body) = body {
                body
            } else if let Some(file) = file {
                std::fs::read_to_string(file)?
            } else {
                let mut text = String::new();
                std::io::stdin()
                    .take((cipherwhisper_protocol::MAX_BODY + 1) as u64)
                    .read_to_string(&mut text)?;
                text
            };
            print(domain.send_message(&topic, &text, reply_to).await?)?;
        }
        Command::TopicUpdate {
            topic,
            title,
            archived,
        } => {
            domain.update_topic(&topic, &title, archived).await?;
            print(domain.topic(&topic)?)?;
        }
        Command::History { topic } => print(domain.messages(&topic)?)?,
        Command::Search { query } => print(domain.search(&query)?)?,
        Command::Sync => {
            let report = domain.sync(true).await?;
            let successful = report.errors.is_empty();
            print(report)?;
            ensure!(
                successful,
                "sync incomplete; queued data preserved for retry"
            );
        }
        Command::Outbox => print(domain.outbox()?)?,
    }
    Ok(())
}
