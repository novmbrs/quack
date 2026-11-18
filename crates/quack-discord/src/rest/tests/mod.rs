//! The REST client against local HTTP servers: request shapes and every
//! outcome classification, without Discord.

mod lookups;
mod outcomes;
mod shapes;

use std::time::Duration;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;

use super::{ClientConfig, DiscordClient, DownloadPolicy};

/// A client pointed at `base` with short deadlines.
pub(crate) fn client(base: &str) -> DiscordClient {
    let mut config = ClientConfig::new("TOKEN", 42);
    config.api_base = format!("{base}/api/v10");
    config.request_timeout = Duration::from_millis(400);
    config.file_timeout = Duration::from_millis(400);
    config.rate_limit_budget = Duration::from_millis(500);
    config.downloads = DownloadPolicy {
        hosts: &["127.0.0.1"],
        https_only: false,
        max_redirects: 5,
    };
    DiscordClient::new(&config).unwrap()
}

/// A server that reads each request and answers with `response` verbatim
/// (empty: closes the connection without answering).
pub(super) async fn raw_server(response: &'static [u8]) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let mut buffer = vec![0_u8; 64 * 1024];
            let _ = socket.read(&mut buffer).await;
            let _ = socket.write_all(response).await;
            let _ = socket.shutdown().await;
        }
    });
    format!("http://{address}")
}

/// An address nothing listens on.
pub(super) async fn closed_port() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    format!("http://{address}")
}
