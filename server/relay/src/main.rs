use clap::Parser;
#[derive(Parser)]
#[command(about = "Topicairn opaque ciphertext relay")]
struct Args {
    #[arg(long, default_value = "127.0.0.1:8787")]
    bind: std::net::SocketAddr,
    #[arg(long, default_value = "relay.sqlite")]
    database: std::path::PathBuf,
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    topicairn_relay::validate_bind(args.bind)?;
    let app = topicairn_relay::router(args.database)?;
    let listener = tokio::net::TcpListener::bind(args.bind).await?;
    println!("Topicairn relay listening on {}", listener.local_addr()?);
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
