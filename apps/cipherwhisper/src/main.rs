use clap::{Parser, Subcommand};
mod local_test;
mod tls;
#[derive(Parser)]
#[command(
    name = "cipherwhisper",
    version,
    about = "CipherWhisper: end-to-end encrypted P2P chat with local topics and Trust Domain device sync"
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Start two temporary P2P chat instances, pair them, and optionally open both UIs.
    LocalTest(local_test::Args),
    /// Run an opaque ciphertext relay. Remote binds require TLS.
    Relay(cipherwhisper_relay::RelayArgs),
    /// Run this computer's Trust Domain center and embedded local UI (--open).
    Serve(Box<cipherwhisper_domain::DomainArgs>),
    /// Create an independent internal device identity; share only its public JSON card.
    DeviceInit(cipherwhisper_domain::DeviceInitArgs),
    /// Pull center history and send through an authorized encrypted device connection.
    Connect(cipherwhisper_domain::ClientArgs),
    /// Create identities, manage peers/topics and send/sync without running a daemon.
    Admin(cipherwhisper_cli::AdminArgs),
    /// Generate private test TLS credentials and a public CA for peer or device servers.
    TlsInit(tls::TlsArgs),
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Args::parse().command {
        Command::LocalTest(args) => local_test::run(args).await,
        Command::Relay(args) => cipherwhisper_relay::run(args).await,
        Command::Serve(args) => cipherwhisper_domain::run(*args).await,
        Command::DeviceInit(args) => cipherwhisper_domain::init_device(args),
        Command::Connect(args) => cipherwhisper_domain::run_client(args).await,
        Command::Admin(args) => cipherwhisper_cli::run(args).await,
        Command::TlsInit(args) => tls::generate(args),
    }
}
