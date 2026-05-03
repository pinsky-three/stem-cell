use crate::error::{Error, Result};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthStatus {
    Healthy,
    Timeout,
}

pub async fn wait_for_http_ok(
    url: &str,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<HealthStatus> {
    let client = reqwest_client();
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if tokio::time::Instant::now() > deadline {
            return Err(Error::HealthTimeout {
                url: url.to_string(),
                timeout_secs: timeout.as_secs(),
            });
        }
        if let Ok(resp) = client.get(url).send().await
            && resp.status().is_success()
        {
            return Ok(HealthStatus::Healthy);
        }
        tokio::time::sleep(poll_interval).await;
    }
}

pub async fn wait_until_port_released(port: u16, timeout: Duration) {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if tokio::time::Instant::now() > deadline {
            tracing::warn!(%port, "timeout waiting for port to release");
            return;
        }
        match tokio::net::TcpStream::connect(("localhost", port)).await {
            Ok(_stream) => tokio::time::sleep(Duration::from_millis(200)).await,
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => return,
            Err(_) => tokio::time::sleep(Duration::from_millis(200)).await,
        }
    }
}

fn reqwest_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .expect("build sandbox health client")
    })
}
