//! No relay. A small local test launcher owning two independent center processes.
use anyhow::{Result, ensure};
use clap::Parser;
use std::{
    path::PathBuf,
    process::{Child, Command},
    time::Duration,
};
use zeroize::Zeroizing;
#[derive(Parser)]
pub struct Args {
    #[arg(long)]
    pub open: bool,
    #[arg(long, default_value = "8790")]
    pub alice_port: u16,
    #[arg(long, default_value = "8791")]
    pub bob_port: u16,
    #[arg(long, default_value = "8800")]
    pub alice_peer_port: u16,
    #[arg(long, default_value = "8801")]
    pub bob_peer_port: u16,
}
struct TestRun {
    dir: PathBuf,
    children: Vec<Child>,
}
impl Drop for TestRun {
    fn drop(&mut self) {
        for child in &mut self.children {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
async fn call(
    client: &reqwest::Client,
    base: &str,
    path: &str,
    token: &str,
    body: Option<serde_json::Value>,
) -> Result<serde_json::Value> {
    let request = if let Some(body) = body {
        client.post(format!("{base}{path}")).json(&body)
    } else {
        client.get(format!("{base}{path}"))
    };
    let response = request.bearer_auth(token).send().await?;
    ensure!(
        response.status().is_success(),
        "test setup failed: {}",
        response.status()
    );
    Ok(response.json().await?)
}
pub async fn run(args: Args) -> Result<()> {
    let ports = [
        args.alice_port,
        args.bob_port,
        args.alice_peer_port,
        args.bob_peer_port,
    ];
    let mut listeners = Vec::new();
    for port in ports {
        ensure!(port > 0, "test ports must be nonzero");
        listeners.push(std::net::TcpListener::bind((
            std::net::Ipv4Addr::LOCALHOST,
            port,
        ))?);
    }
    drop(listeners);
    let dir = std::env::temp_dir().join(format!("topicairn-local-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let mut run = TestRun {
        dir,
        children: vec![],
    };
    let mut random = Zeroizing::new([0u8; 32]);
    getrandom::fill(random.as_mut())?;
    let pass = Zeroizing::new(hex::encode(random.as_slice()));
    let executable = std::env::current_exe()?;
    for (name, ui, peer) in [
        ("Alice", args.alice_port, args.alice_peer_port),
        ("Bob", args.bob_port, args.bob_peer_port),
    ] {
        let mut command = Command::new(&executable);
        command
            .args(["serve", "--name", name, "--data"])
            .arg(run.dir.join(name))
            .args([
                "--bind",
                &format!("127.0.0.1:{ui}"),
                "--peer-bind",
                &format!("127.0.0.1:{peer}"),
                "--sync-seconds",
                "1",
            ])
            .env("TOPICAIRN_PASSPHRASE", pass.as_str());
        if args.open {
            command.arg("--open");
        }
        run.children.push(command.spawn()?);
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()?;
    let mut tokens = Vec::new();
    for (name, port) in [("Alice", args.alice_port), ("Bob", args.bob_port)] {
        let base = format!("http://127.0.0.1:{port}");
        let mut ready = None;
        for _ in 0..150 {
            ensure!(
                run.children
                    .iter_mut()
                    .all(|child| child.try_wait().ok().flatten().is_none()),
                "a test instance exited"
            );
            if let Ok(token) = std::fs::read_to_string(run.dir.join(name).join("admin.token"))
                && call(&client, &base, "/status", token.trim(), None)
                    .await
                    .is_ok()
            {
                ready = Some(Zeroizing::new(token.trim().to_owned()));
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        tokens.push(ready.ok_or_else(|| anyhow::anyhow!("test instance did not start"))?);
    }
    let a = format!("http://127.0.0.1:{}", args.alice_port);
    let b = format!("http://127.0.0.1:{}", args.bob_port);
    let ac = call(&client, &a, "/p2p/contact", &tokens[0], None).await?;
    let bc = call(&client, &b, "/p2p/contact", &tokens[1], None).await?;
    call(&client, &a, "/p2p/peers", &tokens[0], Some(bc.clone())).await?;
    call(&client, &b, "/p2p/peers", &tokens[1], Some(ac.clone())).await?;
    call(
        &client,
        &a,
        &format!(
            "/p2p/peers/{}/check",
            bc["identity"]["user_id"].as_str().unwrap()
        ),
        &tokens[0],
        Some(serde_json::json!({})),
    )
    .await?;
    call(
        &client,
        &b,
        &format!(
            "/p2p/peers/{}/check",
            ac["identity"]["user_id"].as_str().unwrap()
        ),
        &tokens[1],
        Some(serde_json::json!({})),
    )
    .await?;
    call(
        &client,
        &a,
        "/topics",
        &tokens[0],
        Some(serde_json::json!({"peer_id":bc["identity"]["user_id"],"title":"本机 P2P 测试"})),
    )
    .await?;
    println!("Ready: Alice {a}/ | Bob {b}/ — two directly connected identities; no relay.");
    println!(
        "Temporary test data: {}. Ctrl-C stops both instances and deletes this test data. Use serve with your own passphrase for permanent accounts.",
        run.dir.display()
    );
    tokio::signal::ctrl_c().await?;
    Ok(())
}
