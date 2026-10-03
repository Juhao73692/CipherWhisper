use anyhow::{Result, ensure};
use clap::{Parser, Subcommand};
use std::{io::Read, path::PathBuf};
use topicairn_core::Endpoint;
use topicairn_protocol::ContactCard;
use zeroize::Zeroizing;

#[derive(Parser)]
#[command(about = "Topicairn headless Trust Domain administration", version)]
struct Args {
    #[arg(long, default_value = "domain-data")]
    data: PathBuf,
    #[arg(long, default_value = "http://127.0.0.1:8787")]
    relay: String,
    #[arg(
        long,
        env = "TOPICAIRN_PASSPHRASE",
        hide_env_values = true,
        hide = true
    )]
    passphrase: String,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
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
#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let passphrase = Zeroizing::new(args.passphrase);
    let label = match &args.command {
        Command::Init { name } => Some(name.as_str()),
        _ => None,
    };
    let mut domain = Endpoint::open(&args.data, &passphrase, label, &args.relay)?;
    match args.command {
        Command::Init { .. } | Command::Identity => print(domain.contact_card()?)?,
        Command::Publish => {
            domain.publish().await?;
            print(serde_json::json!({"published":true}))?;
        }
        Command::AddPeer { card } => {
            let card: ContactCard = serde_json::from_slice(&std::fs::read(card)?)?;
            let id = card.user_id.clone();
            domain.add_peer(card)?;
            print(serde_json::json!({"added":id}))?;
        }
        Command::Peers => print(domain.peers()?)?,
        Command::NewTopic { peer, title } => print(domain.create_topic(&peer, &title)?)?,
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
                    .take((topicairn_protocol::MAX_BODY + 1) as u64)
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
