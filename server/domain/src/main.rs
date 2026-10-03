use clap::Parser;
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    topicairn_domain::run(topicairn_domain::DomainArgs::parse()).await
}
