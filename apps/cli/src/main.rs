use clap::Parser;
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    topicairn_cli::run(topicairn_cli::AdminArgs::parse()).await
}
