use clap::Parser;
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    cipherwhisper_relay::run(cipherwhisper_relay::RelayArgs::parse()).await
}
