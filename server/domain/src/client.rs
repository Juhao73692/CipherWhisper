use anyhow::{Result, ensure};
use clap::Parser;
use std::{net::SocketAddr, path::PathBuf};
use topicairn_core::device::Replica;
use topicairn_protocol::device::Pairing;
use zeroize::Zeroizing;
#[derive(Parser)]
pub struct DeviceInitArgs {
    #[arg(long, default_value = "device-data")]
    pub data: PathBuf,
    #[arg(long)]
    pub name: String,
    #[arg(
        long,
        env = "TOPICAIRN_PASSPHRASE",
        hide_env_values = true,
        hide = true
    )]
    pub passphrase: String,
}
pub fn init_device(args: DeviceInitArgs) -> Result<()> {
    let pass = Zeroizing::new(args.passphrase);
    let replica = Replica::open(args.data, &pass, Some(&args.name))?;
    println!("{}", serde_json::to_string_pretty(&replica.device_card()?)?);
    Ok(())
}
#[derive(Parser)]
#[command(
    about = "Internal Trust Domain client with independent device keys, local history and shared UI",
    version
)]
pub struct ClientArgs {
    #[arg(long, default_value = "device-data")]
    pub data: PathBuf,
    /// Public, center-signed pairing file exported after device authorization.
    #[arg(long, requires = "trust_domain")]
    pub pairing: Option<PathBuf>,
    /// Verify this complete center user_id through a trusted channel before pairing.
    #[arg(long)]
    pub trust_domain: Option<String>,
    #[arg(long, default_value = "127.0.0.1:8790")]
    pub bind: SocketAddr,
    #[arg(long, default_value = "5")]
    pub sync_seconds: u64,
    #[arg(long)]
    pub open: bool,
    #[arg(
        long,
        env = "TOPICAIRN_PASSPHRASE",
        hide_env_values = true,
        hide = true
    )]
    pub passphrase: String,
}
pub async fn run_client(args: ClientArgs) -> Result<()> {
    let pass = Zeroizing::new(args.passphrase);
    let mut replica = Replica::open(&args.data, &pass, None)?;
    drop(pass);
    if let Some(path) = args.pairing {
        let bytes = std::fs::read(path)?;
        ensure!(bytes.len() <= 300 * 1024, "pairing file too large");
        let pairing: Pairing = serde_json::from_slice(&bytes)?;
        replica.pair(pairing, args.trust_domain.as_deref().unwrap())?;
    }
    replica.pairing()?;
    super::serve_workspace(
        args.data,
        args.bind,
        args.sync_seconds,
        args.open,
        super::workspace::Workspace::Client(Box::new(replica)),
        None,
        None,
    )
    .await
}
