use anyhow::{Result, ensure};
use reqwest::{Client, Method, Url};
use serde::de::DeserializeOwned;
use std::{path::Path, time::Duration};
use topicairn_protocol::RequestAuth;

#[derive(Clone)]
pub struct RelayClient {
    base: String,
    http: Client,
}
impl RelayClient {
    pub fn new(base: &str) -> Result<Self> {
        Self::with_ca(base, None)
    }
    pub fn with_ca(base: &str, ca: Option<&Path>) -> Result<Self> {
        let url = Url::parse(base)?;
        ensure!(
            url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
                && url.path() == "/",
            "relay URL must be an origin without credentials/path/query"
        );
        let loopback = url.host_str().is_some_and(|host| {
            host == "localhost"
                || host
                    .trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        });
        ensure!(
            url.scheme() == "https" || (url.scheme() == "http" && loopback),
            "remote relay requires HTTPS"
        );
        let mut builder = Client::builder()
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none());
        if let Some(path) = ca {
            ensure!(
                url.scheme() == "https",
                "--relay-ca requires an HTTPS relay URL"
            );
            let pem = std::fs::read(path)?;
            ensure!(pem.len() <= 256 * 1024, "relay CA file too large");
            builder = builder.add_root_certificate(reqwest::Certificate::from_pem(&pem)?);
        }
        let http = builder.build()?;
        Ok(Self {
            base: base.trim_end_matches('/').into(),
            http,
        })
    }
    pub async fn request<T: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: Vec<u8>,
        auth: RequestAuth,
    ) -> Result<T> {
        let mut response = self
            .http
            .request(
                Method::from_bytes(method.as_bytes())?,
                format!("{}{path}", self.base),
            )
            .header("content-type", "application/json")
            .header("x-td-key", auth.signing_key)
            .header("x-td-time", auth.timestamp.to_string())
            .header("x-td-nonce", auth.nonce)
            .header("x-td-signature", auth.signature)
            .body(body)
            .send()
            .await?;
        let status = response.status();
        ensure!(
            response.content_length().unwrap_or(0) <= 16 * 1024 * 1024,
            "relay response too large"
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len() + chunk.len() <= 16 * 1024 * 1024,
                "relay response too large"
            );
            bytes.extend_from_slice(&chunk);
        }
        ensure!(
            status.is_success(),
            "relay returned {status}: {}",
            String::from_utf8_lossy(&bytes)
        );
        Ok(serde_json::from_slice(&bytes)?)
    }
}
