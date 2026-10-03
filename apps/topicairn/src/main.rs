use clap::{Parser, Subcommand};
mod tls;
#[derive(Parser)]
#[command(
    name = "topicairn",
    version,
    about = "Topicairn: one executable for Trust Domain centers, device clients, local topic chat and opaque relays"
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Run an opaque ciphertext relay. Remote binds require TLS.
    Relay(topicairn_relay::RelayArgs),
    /// Run this computer's Trust Domain center and embedded local UI (--open).
    Serve(topicairn_domain::DomainArgs),
    /// Create an independent internal device identity; share only its public JSON card.
    DeviceInit(topicairn_domain::DeviceInitArgs),
    /// Pull center history and send through an authorized encrypted device connection.
    Connect(topicairn_domain::ClientArgs),
    /// Create identities, manage peers/topics and send/sync without running a daemon.
    Admin(topicairn_cli::AdminArgs),
    /// Generate private test TLS credentials and a public CA for a relay or device server.
    TlsInit(tls::TlsArgs),
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Args::parse().command {
        Command::Relay(args) => topicairn_relay::run(args).await,
        Command::Serve(args) => topicairn_domain::run(args).await,
        Command::DeviceInit(args) => topicairn_domain::init_device(args),
        Command::Connect(args) => topicairn_domain::run_client(args).await,
        Command::Admin(args) => topicairn_cli::run(args).await,
        Command::TlsInit(args) => tls::generate(args),
    }
}
