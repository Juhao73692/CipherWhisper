use clap::{Parser, Subcommand};
mod tls;
#[derive(Parser)]
#[command(
    name = "topicairn",
    version,
    about = "Topicairn: one headless executable for Trust Domain endpoints and opaque relays"
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Run an opaque ciphertext relay. Remote binds require TLS.
    Relay(topicairn_relay::RelayArgs),
    /// Run this computer's Trust Domain center (loopback administration API).
    Serve(topicairn_domain::DomainArgs),
    /// Create identities, manage peers/topics and send/sync without running a daemon.
    Admin(topicairn_cli::AdminArgs),
    /// Generate private test-relay TLS credentials and a public CA to exchange.
    TlsInit(tls::TlsArgs),
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Args::parse().command {
        Command::Relay(args) => topicairn_relay::run(args).await,
        Command::Serve(args) => topicairn_domain::run(args).await,
        Command::Admin(args) => topicairn_cli::run(args).await,
        Command::TlsInit(args) => tls::generate(args),
    }
}
