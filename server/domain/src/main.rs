use clap::Parser;
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    cipherwhisper_domain::run(cipherwhisper_domain::DomainArgs::parse()).await
}
