use clap::Parser;
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    cipherwhisper_cli::run(cipherwhisper_cli::AdminArgs::parse()).await
}
