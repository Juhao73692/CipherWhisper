use clap::Parser;
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    topicairn_relay::run(topicairn_relay::RelayArgs::parse()).await
}
