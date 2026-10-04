//! A loopback setup UI supervising the existing center/device runtime.
//! Only public settings are persisted; the vault password stays in memory.
use anyhow::{Context, Result, bail, ensure};
use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{DefaultBodyLimit, Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use cipherwhisper_core::{Endpoint, device::Replica};
use cipherwhisper_domain::ui;
use cipherwhisper_protocol::{device::Pairing, digest};
use clap::Args as ClapArgs;
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, Command},
    sync::{Mutex, watch},
};
use zeroize::Zeroizing;

#[derive(ClapArgs)]
pub struct Args {
    /// Defaults to the user's application data directory.
    #[arg(long)]
    pub data: Option<PathBuf>,
    #[arg(long, default_value = "127.0.0.1:8790")]
    pub bind: SocketAddr,
    /// Run without opening the default browser (for supervised environments).
    #[arg(long)]
    pub no_open: bool,
}
impl Default for Args {
    fn default() -> Self {
        Self {
            data: None,
            bind: "127.0.0.1:8790".parse().unwrap(),
            no_open: false,
        }
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Role {
    Center,
    Device,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Network {
    host: String,
    peer_port: u16,
    devices: bool,
    device_port: u16,
}
impl Network {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.peer_port > 0 && self.device_port > 0,
            "端口必须在 1–65535 之间"
        );
        ensure!(
            !self.devices || self.peer_port != self.device_port,
            "聊天端口和设备端口不能相同"
        );
        let host = &self.host;
        let ip = host.parse::<IpAddr>().ok();
        let dns = !host.is_empty()
            && host.len() <= 253
            && host.split('.').all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'-')
            });
        ensure!(
            ip.is_some() || dns,
            "填写 IP 或域名，例如 192.168.1.10；不要填写网址或端口"
        );
        ensure!(
            !ip.is_some_and(|ip| ip.is_unspecified() || ip.is_multicast()),
            "连接地址不能是通配或组播地址"
        );
        Ok(())
    }
    fn origin(&self, port: u16) -> String {
        if self.host.parse::<std::net::Ipv6Addr>().is_ok() {
            format!("https://[{}]:{port}", self.host)
        } else {
            format!("https://{}:{port}", self.host)
        }
    }
    fn bind(&self, port: u16) -> String {
        let local = self.host.eq_ignore_ascii_case("localhost")
            || self.host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback());
        match (local, self.host.parse::<std::net::Ipv6Addr>().is_ok()) {
            (true, true) => format!("[::1]:{port}"),
            (false, true) => format!("[::]:{port}"),
            (true, false) => format!("127.0.0.1:{port}"),
            (false, false) => format!("0.0.0.0:{port}"),
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Config {
    version: u8,
    role: Role,
    name: String,
    network: Network,
    certificate: Option<String>,
    certificate_created: Option<u64>,
}
impl Config {
    fn validate(&self) -> Result<()> {
        ensure!(self.version == 1, "不支持此配置版本");
        ensure!(
            !self.name.trim().is_empty() && self.name.len() <= 256,
            "请输入名称（最多 256 字节）"
        );
        self.network.validate()?;
        if let Some(id) = &self.certificate {
            uuid::Uuid::parse_str(id)?;
        }
        Ok(())
    }
}
struct Running {
    child: Child,
    origin: String,
    token: Zeroizing<String>,
}
struct Runtime {
    config: Option<Config>,
    running: Option<Running>,
    password: Option<Zeroizing<String>>,
    next_unlock: Instant,
}
#[derive(Clone)]
struct Portal {
    data: Arc<PathBuf>,
    hosts: Arc<Vec<String>>,
    auth: Arc<Mutex<ui::BrowserAuth>>,
    token_digest: Arc<String>,
    runtime: Arc<Mutex<Runtime>>,
    http: reqwest::Client,
    shutdown: watch::Sender<bool>,
}
struct Error(String);
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": self.0})),
        )
            .into_response()
    }
}
impl From<anyhow::Error> for Error {
    fn from(error: anyhow::Error) -> Self {
        Self(error.to_string())
    }
}
type Api = std::result::Result<Json<serde_json::Value>, Error>;
fn require(valid: bool, message: &str) -> Result<()> {
    ensure!(valid, "{message}");
    Ok(())
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
fn save(data: &Path, config: &Config) -> Result<()> {
    config.validate()?;
    let path = data.join("ui-config.json");
    ensure!(!path.is_symlink(), "configuration cannot be a symlink");
    let temp = data.join(format!(".ui-config-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        write_private(&temp, &serde_json::to_vec_pretty(config)?)?;
        std::fs::rename(&temp, &path)?;
        std::fs::File::open(data)?.sync_all()?;
        Ok(())
    })();
    let _ = std::fs::remove_file(temp);
    result
}
fn default_data() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("找不到用户数据目录")?;
    #[cfg(target_os = "macos")]
    let suffix = "Library/Application Support/CipherWhisper";
    #[cfg(not(target_os = "macos"))]
    let suffix = ".local/share/CipherWhisper";
    Ok(PathBuf::from(home).join(suffix))
}
fn suggest_host() -> String {
    // UDP connect chooses a route without transmitting a packet.
    std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|socket| {
            socket.connect("192.0.2.1:9")?;
            socket.local_addr()
        })
        .map(|addr| addr.ip().to_string())
        .unwrap_or_else(|_| "localhost".into())
}
fn credentials(data: &Path, config: &mut Config) -> Result<()> {
    let id = uuid::Uuid::new_v4().to_string();
    crate::tls::generate(crate::tls::TlsArgs {
        host: vec![config.network.host.clone()],
        out: data.join("tls").join(&id),
    })?;
    config.certificate = Some(id);
    config.certificate_created = Some(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs());
    Ok(())
}
async fn stop(running: &mut Option<Running>) -> Result<()> {
    if let Some(mut running) = running.take() {
        #[cfg(unix)]
        if let Some(pid) = running.child.id() {
            let _ = Command::new("/bin/kill")
                .args(["-INT", &pid.to_string()])
                .status()
                .await;
        }
        match tokio::time::timeout(Duration::from_secs(12), running.child.wait()).await {
            Ok(result) => {
                result?;
            }
            Err(_) => {
                running.child.kill().await?;
                running.child.wait().await?;
            }
        }
    }
    Ok(())
}
async fn spawn_runtime(
    data: &Path,
    config: &Config,
    password: &str,
    http: &reqwest::Client,
) -> Result<Running> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg(if config.role == Role::Center {
            "serve"
        } else {
            "connect"
        })
        .arg("--data")
        .arg(data)
        .args(["--bind", "127.0.0.1:0", "--sync-seconds", "0.5"])
        .env("CIPHERWHISPER_PASSPHRASE", password)
        .env_remove("TOPICAIRN_PASSPHRASE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if config.role == Role::Center {
        let tls = data
            .join("tls")
            .join(config.certificate.as_ref().context("尚未生成证书")?);
        command
            .arg("--name")
            .arg(&config.name)
            .arg("--peer-bind")
            .arg(config.network.bind(config.network.peer_port))
            .arg("--peer-url")
            .arg(config.network.origin(config.network.peer_port))
            .arg("--peer-tls-cert")
            .arg(tls.join("server.pem"))
            .arg("--peer-tls-key")
            .arg(tls.join("server-key.pem"))
            .arg("--peer-ca")
            .arg(tls.join("ca.pem"));
        if config.network.devices {
            command
                .arg("--device-bind")
                .arg(config.network.bind(config.network.device_port))
                .arg("--device-url")
                .arg(config.network.origin(config.network.device_port))
                .arg("--device-tls-cert")
                .arg(tls.join("server.pem"))
                .arg("--device-tls-key")
                .arg(tls.join("server-key.pem"))
                .arg("--device-ca")
                .arg(tls.join("ca.pem"));
        }
    }
    let mut child = command.spawn()?;
    let (ready, mut address) = watch::channel(None::<String>);
    let stdout = child.stdout.take().unwrap();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(url) = line.strip_prefix("Local UI: ") {
                let _ = ready.send(Some(url.trim_end_matches('/').into()));
            }
        }
    });
    let errors = Arc::new(Mutex::new(String::new()));
    let stderr = child.stderr.take().unwrap();
    let captured = errors.clone();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let mut errors = captured.lock().await;
            if errors.len() < 4096 {
                errors.push_str(&line);
                errors.push('\n');
            }
        }
    });
    let startup = async {
        loop {
            let origin = address.borrow().clone();
            if let Some(origin) = origin {
                let token = Zeroizing::new(
                    std::fs::read_to_string(data.join("admin.token"))?
                        .trim()
                        .to_string(),
                );
                let response = http
                    .get(format!("{origin}/status"))
                    .bearer_auth(token.as_str())
                    .send()
                    .await?;
                ensure!(response.status().is_success(), "后台启动校验失败");
                return Ok::<_, anyhow::Error>((origin, token));
            }
            if address.changed().await.is_err() {
                child.wait().await?;
                bail!("后台启动失败：{}", errors.lock().await.trim());
            }
        }
    };
    match tokio::time::timeout(Duration::from_secs(30), startup).await {
        Ok(Ok((origin, token))) => Ok(Running {
            child,
            origin,
            token,
        }),
        result => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            match result {
                Ok(Err(error)) => Err(error),
                _ => bail!("后台启动超时，请检查端口和配置"),
            }
        }
    }
}
async fn snapshot(portal: &Portal, runtime: &mut Runtime) -> Result<serde_json::Value> {
    if let Some(running) = &mut runtime.running
        && running.child.try_wait()?.is_some()
    {
        runtime.running = None;
    }
    let card = if runtime.running.is_none()
        && let (Some(config), Some(password)) = (&runtime.config, &runtime.password)
        && config.role == Role::Device
    {
        Some(Replica::open(portal.data.as_ref(), password, None)?.device_card()?)
    } else {
        None
    };
    Ok(serde_json::json!({
        "config": runtime.config, "running": runtime.running.is_some(),
        "unlocked": runtime.password.is_some(), "deviceCard": card,
        "suggestedHost": suggest_host(), "dataDirectory": portal.data.as_ref(),
        "certificateExpires": runtime.config.as_ref().and_then(|c| c.certificate_created).map(|t| t + 365 * 86400),
    }))
}
async fn status(State(portal): State<Portal>) -> Api {
    Ok(Json(
        snapshot(&portal, &mut *portal.runtime.lock().await).await?,
    ))
}
async fn close_workspace(State(portal): State<Portal>) -> Api {
    let mut runtime = portal.runtime.lock().await;
    stop(&mut runtime.running).await?;
    runtime.password = None;
    Ok(Json(serde_json::json!({"ok": true})))
}
async fn quit(State(portal): State<Portal>) -> Api {
    let _ = portal.shutdown.send(true);
    Ok(Json(serde_json::json!({"ok": true})))
}
async fn open_window(State(portal): State<Portal>) -> Api {
    let code = portal.auth.lock().await.bootstrap()?;
    let url = Zeroizing::new(format!(
        "http://{}/#bootstrap={}",
        portal.hosts[0],
        code.as_str()
    ));
    ui::open(&url).map_err(anyhow::Error::from)?;
    Ok(Json(serde_json::json!({"ok": true})))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Start {
    passphrase: String,
    name: Option<String>,
    role: Option<Role>,
    network: Option<Network>,
}
async fn start_inner(portal: &Portal, runtime: &mut Runtime, input: Start) -> Result<()> {
    let password = Zeroizing::new(input.passphrase);
    ensure!(
        password.len() >= 12 && password.len() <= 1024,
        "口令至少需要 12 字节，最多 1024 字节"
    );
    if let Some(running) = &mut runtime.running
        && running.child.try_wait()?.is_some()
    {
        runtime.running = None;
    }
    if runtime.running.is_some() {
        ensure!(
            runtime
                .password
                .as_ref()
                .is_some_and(|old| bool::from(old.as_bytes().ct_eq(password.as_bytes()))),
            "口令不正确"
        );
        return Ok(());
    }
    let new = runtime.config.is_none();
    let mut config = runtime.config.clone().unwrap_or(Config {
        version: 1,
        role: input.role.unwrap_or(Role::Center),
        name: input.name.unwrap_or_default(),
        network: input.network.unwrap_or(Network {
            host: "localhost".into(),
            peer_port: 8800,
            devices: false,
            device_port: 8802,
        }),
        certificate: None,
        certificate_created: None,
    });
    config.validate()?;
    let paired = if config.role == Role::Device {
        let replica = Replica::open(portal.data.as_ref(), &password, Some(&config.name))
            .context("无法解锁设备，请检查口令及数据目录")?;
        replica.pairing().is_ok()
    } else {
        drop(
            Endpoint::open_direct(portal.data.as_ref(), &password, Some(&config.name))
                .context("无法解锁身份，请检查口令及数据目录")?,
        );
        if config.certificate.is_none() {
            credentials(&portal.data, &mut config)?;
        }
        true
    };
    let mut running = if paired {
        Some(spawn_runtime(&portal.data, &config, &password, &portal.http).await?)
    } else {
        None
    };
    if new && let Err(error) = save(&portal.data, &config) {
        stop(&mut running).await?;
        return Err(error);
    }
    runtime.config = Some(config);
    runtime.password = Some(password);
    runtime.running = running;
    Ok(())
}
async fn start(State(portal): State<Portal>, Json(input): Json<Start>) -> Api {
    let mut runtime = portal.runtime.lock().await;
    start_inner(&portal, &mut runtime, input).await?;
    Ok(Json(snapshot(&portal, &mut runtime).await?))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NetworkUpdate {
    network: Network,
    renew_certificate: bool,
}
async fn network(State(portal): State<Portal>, Json(input): Json<NetworkUpdate>) -> Api {
    input.network.validate()?;
    let mut runtime = portal.runtime.lock().await;
    let old = runtime.config.clone().context("请先创建身份")?;
    require(old.role == Role::Center, "设备的连接配置来自中心配对文件")?;
    let password = Zeroizing::new(
        runtime
            .password
            .as_ref()
            .context("请先解锁身份")?
            .to_string(),
    );
    let mut updated = old.clone();
    updated.network = input.network;
    if updated.network.host != old.network.host || input.renew_certificate {
        credentials(&portal.data, &mut updated)?;
    }
    stop(&mut runtime.running).await?;
    let change = async {
        let mut running =
            Some(spawn_runtime(&portal.data, &updated, &password, &portal.http).await?);
        if let Err(error) = save(&portal.data, &updated) {
            stop(&mut running).await?;
            return Err(error);
        }
        Ok::<_, anyhow::Error>(running)
    }
    .await;
    match change {
        Ok(running) => {
            runtime.running = running;
            runtime.config = Some(updated);
        }
        Err(error) => {
            runtime.running = spawn_runtime(&portal.data, &old, &password, &portal.http)
                .await
                .ok();
            return Err(Error(format!(
                "配置未保存；{}。{}",
                error,
                if runtime.running.is_some() {
                    "已恢复原配置"
                } else {
                    "请重新解锁以恢复原配置"
                }
            )));
        }
    }
    Ok(Json(snapshot(&portal, &mut runtime).await?))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Pair {
    pairing: Pairing,
    trusted_domain: String,
}
async fn pair(State(portal): State<Portal>, Json(input): Json<Pair>) -> Api {
    // Validate public signatures and the user-confirmed fingerprint before any restart.
    input.pairing.validate()?;
    require(
        input.pairing.domain.user_id == input.trusted_domain,
        "中心身份指纹不匹配",
    )?;
    let mut runtime = portal.runtime.lock().await;
    let config = runtime.config.clone().context("请先创建设备身份")?;
    require(config.role == Role::Device, "配对文件只能导入设备端")?;
    let password = Zeroizing::new(
        runtime
            .password
            .as_ref()
            .context("请先解锁设备")?
            .to_string(),
    );
    stop(&mut runtime.running).await?;
    let result = (|| {
        let mut replica = Replica::open(portal.data.as_ref(), &password, None)?;
        replica.pair(input.pairing, &input.trusted_domain)
    })();
    // Even an invalid replacement pairing must leave an already paired device usable.
    runtime.running = spawn_runtime(&portal.data, &config, &password, &portal.http)
        .await
        .ok();
    result?;
    require(
        runtime.running.is_some(),
        "配对已保存，但后台未启动；请重新解锁重试",
    )?;
    Ok(Json(snapshot(&portal, &mut runtime).await?))
}
async fn discovery(State(portal): State<Portal>) -> Json<serde_json::Value> {
    Json(
        serde_json::json!({"enabled": true, "configured": portal.runtime.lock().await.config.is_some()}),
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Code {
    code: String,
}
async fn session(State(portal): State<Portal>, Json(input): Json<Code>) -> Response {
    if input.code.len() != 64 {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match portal.auth.lock().await.exchange(&input.code) {
        Ok(Some(token)) => Json(serde_json::json!({"token": token.as_str()})).into_response(),
        _ => StatusCode::UNAUTHORIZED.into_response(),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Password {
    passphrase: String,
}
async fn unlock(State(portal): State<Portal>, Json(input): Json<Password>) -> Api {
    let mut runtime = portal.runtime.lock().await;
    require(
        runtime.config.is_some(),
        "首次配置请重新打开软件进入启动向导",
    )?;
    require(Instant::now() >= runtime.next_unlock, "请稍后再试")?;
    runtime.next_unlock = Instant::now() + Duration::from_secs(2);
    start_inner(
        &portal,
        &mut runtime,
        Start {
            passphrase: input.passphrase,
            name: None,
            role: None,
            network: None,
        },
    )
    .await?;
    runtime.next_unlock = Instant::now();
    let token = portal.auth.lock().await.new_session()?;
    Ok(Json(serde_json::json!({"token": token.as_str()})))
}
async fn authenticated(State(portal): State<Portal>, request: Request, next: Next) -> Response {
    let supplied = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "));
    let valid = if let Some(token) = supplied.filter(|s| s.len() == 64) {
        let hash = digest(token.as_bytes());
        bool::from(hash.as_bytes().ct_eq(portal.token_digest.as_bytes()))
            || portal.auth.lock().await.accepts(&hash)
    } else {
        false
    };
    if valid {
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "本机授权已失效"})),
        )
            .into_response()
    }
}
async fn guard(State(portal): State<Portal>, request: Request, next: Next) -> Response {
    let mut response = if ui::trusted_request(request.headers(), &portal.hosts) {
        next.run(request).await
    } else {
        (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "local same-origin requests only"})),
        )
            .into_response()
    };
    ui::protect_response(&mut response);
    response
}
fn api_path(path: &str) -> bool {
    matches!(
        path,
        "/identity"
            | "/peers"
            | "/topics"
            | "/unread"
            | "/search"
            | "/sync"
            | "/outbox"
            | "/status"
            | "/p2p/contact"
            | "/p2p/peers"
            | "/devices"
            | "/device-pending"
    ) || path.starts_with("/topics/")
        || path.starts_with("/outbox/")
        || path.starts_with("/p2p/peers/")
        || path.starts_with("/devices/")
        || path.starts_with("/device-pending/")
}
async fn proxy(
    State(portal): State<Portal>,
    request: Request,
) -> std::result::Result<Response, Error> {
    if !api_path(request.uri().path()) {
        return Ok(StatusCode::NOT_FOUND.into_response());
    }
    let (origin, token) = {
        let runtime = portal.runtime.lock().await;
        let running = runtime
            .running
            .as_ref()
            .context("请先完成启动向导并解锁身份")?;
        (
            running.origin.clone(),
            Zeroizing::new(running.token.to_string()),
        )
    };
    let url = format!("{}{}", origin, request.uri().path_and_query().unwrap());
    let mut builder = portal
        .http
        .request(request.method().clone(), url)
        .bearer_auth(token.as_str());
    if let Some(content_type) = request.headers().get(header::CONTENT_TYPE) {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }
    let body = to_bytes(request.into_body(), 512 * 1024)
        .await
        .map_err(|_| anyhow::anyhow!("请求过大"))?;
    let response = builder
        .body(body)
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("后台连接中断，请重新解锁或重试"))?;
    let status = response.status();
    let bytes = response.bytes().await.map_err(anyhow::Error::from)?;
    Ok((
        status,
        [(header::CONTENT_TYPE, "application/json")],
        Body::from(bytes),
    )
        .into_response())
}
pub async fn run(args: Args) -> Result<()> {
    ensure!(args.bind.ip().is_loopback(), "配置界面只允许本机访问");
    let data = args.data.unwrap_or(default_data()?);
    ensure!(!data.is_symlink(), "data directory cannot be a symlink");
    std::fs::create_dir_all(&data)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o700))?;
    }
    // A separate lock prevents two launchers from restarting the same workspace.
    let lock_path = data.join("ui.lockfile");
    ensure!(!lock_path.is_symlink(), "launcher lock cannot be a symlink");
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    if lock.try_lock().is_err() {
        if !args.no_open {
            let origin = std::fs::read_to_string(data.join("ui-address"))?;
            let url = reqwest::Url::parse(origin.trim())?;
            ensure!(
                url.scheme() == "http"
                    && url.host_str().is_some_and(|host| host
                        .parse::<IpAddr>()
                        .is_ok_and(|ip| ip.is_loopback())),
                "invalid launcher address"
            );
            let token = Zeroizing::new(std::fs::read_to_string(data.join("admin.token"))?);
            let response = reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(5))
                .build()?
                .post(url.join("/launcher/open")?)
                .bearer_auth(token.trim())
                .send()
                .await?;
            ensure!(response.status().is_success(), "无法打开已有窗口");
            return Ok(());
        }
        bail!("此工作区已经打开，请使用已有窗口");
    }
    let path = data.join("ui-config.json");
    ensure!(!path.is_symlink(), "configuration cannot be a symlink");
    let config = if path.exists() {
        let bytes = std::fs::read(path)?;
        ensure!(bytes.len() <= 16 * 1024, "configuration too large");
        let config: Config = serde_json::from_slice(&bytes)?;
        config.validate()?;
        Some(config)
    } else {
        None
    };
    let token_path = data.join("admin.token");
    ensure!(!token_path.is_symlink(), "admin token cannot be a symlink");
    if !token_path.exists() {
        let mut bytes = Zeroizing::new([0u8; 32]);
        getrandom::fill(bytes.as_mut())?;
        write_private(&token_path, hex::encode(bytes.as_slice()).as_bytes())?;
    }
    let token = Zeroizing::new(std::fs::read_to_string(token_path)?.trim().to_string());
    ensure!(token.len() == 64, "invalid admin token file");
    let listener = match tokio::net::TcpListener::bind(args.bind).await {
        Ok(listener) => listener,
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            tokio::net::TcpListener::bind(SocketAddr::new(args.bind.ip(), 0)).await?
        }
        Err(error) => return Err(error.into()),
    };
    let address = listener.local_addr()?;
    let address_path = data.join("ui-address");
    ensure!(
        !address_path.is_symlink(),
        "launcher address cannot be a symlink"
    );
    if address_path.exists() {
        std::fs::remove_file(&address_path)?;
    }
    write_private(&address_path, format!("http://{address}").as_bytes())?;
    let (shutdown, mut closing) = watch::channel(false);
    let portal = Portal {
        data: Arc::new(data),
        shutdown,
        hosts: Arc::new(vec![
            address.to_string(),
            format!("localhost:{}", address.port()),
        ]),
        auth: Arc::new(Mutex::new(ui::BrowserAuth::default())),
        token_digest: Arc::new(digest(token.as_bytes())),
        runtime: Arc::new(Mutex::new(Runtime {
            config,
            running: None,
            password: None,
            next_unlock: Instant::now(),
        })),
        http: reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(40))
            .build()?,
    };
    drop(token);
    let app = Router::new()
        .route("/launcher", get(status))
        .route("/launcher/start", post(start))
        .route("/launcher/stop", post(close_workspace))
        .route("/launcher/quit", post(quit))
        .route("/launcher/open", post(open_window))
        .route("/launcher/network", post(network))
        .route("/launcher/pair", post(pair));
    // Only these paths can reach the child's administration API.
    let app = [
        "/identity",
        "/peers",
        "/topics",
        "/unread",
        "/search",
        "/sync",
        "/outbox",
        "/status",
        "/p2p/contact",
        "/p2p/peers",
        "/devices",
        "/device-pending",
        "/topics/{*rest}",
        "/outbox/{*rest}",
        "/p2p/peers/{*rest}",
        "/devices/{*rest}",
        "/device-pending/{*rest}",
    ]
    .into_iter()
    .fold(app, |app, path| app.route(path, get(proxy).post(proxy)))
    .layer(middleware::from_fn_with_state(
        portal.clone(),
        authenticated,
    ))
    .route("/ui/launcher", get(discovery))
    .route("/ui/build", get(ui::build))
    .route("/ui/session", post(session))
    .route("/ui/unlock", post(unlock))
    .fallback(get(ui::assets))
    .layer(DefaultBodyLimit::max(512 * 1024))
    .layer(middleware::from_fn_with_state(portal.clone(), guard))
    .with_state(portal.clone());
    println!("Configuration UI: http://{address}/");
    if !args.no_open {
        let code = portal.auth.lock().await.bootstrap()?;
        let url = Zeroizing::new(format!("http://{address}/#bootstrap={}", code.as_str()));
        ui::open(&url).context("无法打开浏览器；请重新打开软件")?;
    }
    let served = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {},
                _ = closing.changed() => {},
            }
        })
        .await;
    stop(&mut portal.runtime.lock().await.running).await?;
    served?;
    drop(lock);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn network_rejects_urls_wildcards_and_duplicate_ports() {
        let mut network = Network {
            host: "192.168.1.10".into(),
            peer_port: 8800,
            device_port: 8802,
            devices: true,
        };
        assert!(network.validate().is_ok());
        for host in [
            "https://example.com",
            "a/b",
            "*.example.com",
            "0.0.0.0",
            "::",
            "-a.com",
            "a:8800",
        ] {
            network.host = host.into();
            assert!(network.validate().is_err(), "{host}");
        }
        network.host = "::1".into();
        assert!(network.validate().is_ok());
        assert_eq!(network.origin(8800), "https://[::1]:8800");
        network.device_port = 8800;
        assert!(network.validate().is_err());
    }
}
