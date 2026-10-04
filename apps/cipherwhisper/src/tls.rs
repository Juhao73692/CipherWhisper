use anyhow::{Result, ensure};
use clap::Args;
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose,
};
use std::{
    io::Write,
    path::{Path, PathBuf},
};
use time::{Duration, OffsetDateTime};
use zeroize::Zeroizing;

#[derive(Args)]
pub struct TlsArgs {
    /// IP or DNS name that the other computer will use to reach this relay (repeatable).
    #[arg(long, required = true)]
    pub host: Vec<String>,
    /// A new directory; existing credentials are never overwritten.
    #[arg(long, default_value = "relay-tls")]
    pub out: PathBuf,
}
fn write(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(mode);
    }
    #[cfg(not(unix))]
    let _ = mode;
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
pub fn generate(args: TlsArgs) -> Result<()> {
    ensure!(
        !args.out.exists(),
        "TLS output directory already exists; choose a new directory"
    );
    ensure!(
        args.host.len() <= 16
            && args.host.iter().all(|h| !h.is_empty()
                && h.len() <= 253
                && !h.contains('*')
                && !h.contains('/')
                && !h.contains(':')
                || h.parse::<std::net::IpAddr>().is_ok()),
        "provide an IP or DNS host, not a URL/wildcard"
    );
    let now = OffsetDateTime::now_utc();
    let mut ca_params = CertificateParams::new(Vec::<String>::new())?;
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    ca_params
        .distinguished_name
        .push(DnType::CommonName, "CipherWhisper test TLS CA");
    ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    ca_params.not_before = now - Duration::days(1);
    ca_params.not_after = now + Duration::days(365);
    let ca = CertifiedIssuer::self_signed(ca_params, KeyPair::generate()?)?;
    let mut hosts = args.host.clone();
    hosts.extend(["localhost".into(), "127.0.0.1".into(), "::1".into()]);
    hosts.sort();
    hosts.dedup();
    let mut params = CertificateParams::new(hosts)?;
    params
        .distinguished_name
        .push(DnType::CommonName, "CipherWhisper test server");
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.not_before = now - Duration::days(1);
    params.not_after = now + Duration::days(365);
    let key = KeyPair::generate()?;
    let certificate = params.signed_by(&key, &ca)?;
    let ca_pem = ca.pem();
    let server_pem = format!("{}{}", certificate.pem(), ca_pem);
    let key_pem = Zeroizing::new(key.serialize_pem());
    if let Some(parent) = args.out.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::create_dir(&args.out)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&args.out, std::fs::Permissions::from_mode(0o700))?;
    }
    write(&args.out.join("ca.pem"), ca_pem.as_bytes(), 0o644)?;
    write(&args.out.join("server.pem"), server_pem.as_bytes(), 0o644)?;
    write(&args.out.join("server-key.pem"), key_pem.as_bytes(), 0o600)?;
    // The CA signing private key is never written or shared.
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "ca":args.out.join("ca.pem"),"certificate":args.out.join("server.pem"),"privateKey":args.out.join("server-key.pem"),
            "caFileSha256":cipherwhisper_protocol::digest(ca_pem.as_bytes()),"hosts":args.host,"validDays":365,
        }))?
    );
    Ok(())
}
