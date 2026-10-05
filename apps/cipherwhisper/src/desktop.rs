//! Native windows live on the main thread; the launcher runs on Tokio workers.
#[cfg(any(target_os = "macos", target_os = "windows"))]
mod native;
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub use native::{Handle, run};

// Other platforms retain the browser/headless launcher without GUI dependencies.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
#[derive(Clone)]
pub struct Handle;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
impl Handle {
    pub fn open(&self, _: zeroize::Zeroizing<String>) -> anyhow::Result<()> {
        anyhow::bail!("native windows are supported on macOS and Windows")
    }
    pub fn reopen(&self) -> anyhow::Result<()> {
        anyhow::bail!("native windows are supported on macOS and Windows")
    }
    pub fn shutdown(&self) -> tokio::sync::watch::Sender<bool> {
        tokio::sync::watch::channel(false).0
    }
}

#[cfg(any(test, target_os = "macos", target_os = "windows"))]
fn navigation_allowed(origin: &reqwest::Url, destination: &str) -> bool {
    reqwest::Url::parse(destination).is_ok_and(|url| {
        url.origin() == origin.origin()
            && url.username().is_empty()
            && url.password().is_none()
            && matches!(url.path(), "/" | "/third-party-ui.txt")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_navigation_stays_in_this_workspace() {
        let origin = reqwest::Url::parse("http://127.0.0.1:8790/").unwrap();
        for url in [
            "http://127.0.0.1:8790/#about",
            "http://127.0.0.1:8790/third-party-ui.txt",
        ] {
            assert!(navigation_allowed(&origin, url));
        }
        for url in [
            "https://example.com/",
            "http://127.0.0.1:8791/",
            "http://127.0.0.1:8790.evil.test/",
            "http://user@127.0.0.1:8790/",
            "http://127.0.0.1:8790/launcher/quit",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,hello",
            "about:blank",
        ] {
            assert!(!navigation_allowed(&origin, url), "{url}");
        }
    }
}
