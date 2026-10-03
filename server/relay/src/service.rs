use anyhow::{Result, ensure};
use clap::Parser;
use std::{path::PathBuf, time::Duration};
#[derive(Parser)]
#[command(about = "Topicairn opaque ciphertext relay", version)]
pub struct RelayArgs {
    #[arg(long, default_value = "127.0.0.1:8787")]
    pub bind: std::net::SocketAddr,
    #[arg(long, default_value = "relay.sqlite")]
    pub database: PathBuf,
    /// PEM server certificate/chain; must be paired with --tls-key.
    #[arg(long, requires = "tls_key")]
    pub tls_cert: Option<PathBuf>,
    /// PEM private key kept only on the Relay computer.
    #[arg(long, requires = "tls_cert")]
    pub tls_key: Option<PathBuf>,
}
pub async fn run(args: RelayArgs) -> Result<()> {
    let tls = match (&args.tls_cert, &args.tls_key) {
        (Some(cert), Some(key)) => {
            Some(axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key).await?)
        }
        (None, None) => None,
        _ => anyhow::bail!("provide both --tls-cert and --tls-key"),
    };
    ensure!(
        args.bind.ip().is_loopback() || tls.is_some(),
        "remote Relay bind requires --tls-cert and --tls-key"
    );
    let app = crate::router(args.database)?;
    if let Some(config) = tls {
        let listener = std::net::TcpListener::bind(args.bind)?;
        listener.set_nonblocking(true)?;
        println!(
            "Topicairn HTTPS relay listening on {}",
            listener.local_addr()?
        );
        let handle = axum_server::Handle::new();
        let shutdown = handle.clone();
        let signal = tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                shutdown.graceful_shutdown(Some(Duration::from_secs(10)));
            }
        });
        let result = axum_server::tls_rustls::from_tcp_rustls(listener, config)?
            .handle(handle)
            .serve(app.into_make_service())
            .await;
        signal.abort();
        result?;
    } else {
        let listener = tokio::net::TcpListener::bind(args.bind).await?;
        println!(
            "Topicairn HTTP loopback relay listening on {}",
            listener.local_addr()?
        );
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await?;
    }
    Ok(())
}
