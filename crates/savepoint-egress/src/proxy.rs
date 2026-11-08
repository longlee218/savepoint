use crate::error::{EgressError, Result};
use crate::guard::{GuardDecision, GuardPolicy, MutationGuard};
use crate::vcr::{HttpRequestSnapshot, VcrEngine};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Clone)]
pub struct EgressProxyConfig {
    pub bind_addr: String,
    pub cassette_dir: PathBuf,
    pub policy: GuardPolicy,
}

pub struct EgressProxy {
    local_addr: SocketAddr,
    _shutdown_tx: tokio::sync::broadcast::Sender<()>,
}

impl EgressProxy {
    pub async fn start(config: EgressProxyConfig) -> Result<Self> {
        let listener = TcpListener::bind(&config.bind_addr)
            .await
            .map_err(|e| EgressError::Other(e.to_string()))?;
        let local_addr = listener
            .local_addr()
            .map_err(|e| EgressError::Other(e.to_string()))?;

        let (shutdown_tx, _) = tokio::sync::broadcast::channel(1);
        let mut shutdown_rx = shutdown_tx.subscribe();

        let vcr = Arc::new(VcrEngine::new(&config.cassette_dir));
        let guard = Arc::new(MutationGuard::new(config.policy));

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    accept_res = listener.accept() => {
                        let Ok((mut socket, _)) = accept_res else {
                            continue;
                        };
                        let vcr_clone = vcr.clone();
                        let guard_clone = guard.clone();

                        tokio::spawn(async move {
                            let mut buf = vec![0u8; 4096];
                            let n = match socket.read(&mut buf).await {
                                Ok(n) if n > 0 => n,
                                _ => return,
                            };

                            let mut headers = [httparse::EMPTY_HEADER; 64];
                            let mut req = httparse::Request::new(&mut headers);
                            let status = match req.parse(&buf[..n]) {
                                Ok(s) => s,
                                Err(_) => return,
                            };

                            let method = req.method.unwrap_or("GET").to_string();
                            let path = req.path.unwrap_or("/").to_string();

                            let mut req_headers = HashMap::new();
                            for h in req.headers.iter() {
                                let name = h.name.to_string();
                                let val = String::from_utf8_lossy(h.value).to_string();
                                req_headers.insert(name, val);
                            }

                            let body_offset = match status {
                                httparse::Status::Complete(offset) => offset,
                                httparse::Status::Partial => n,
                            };
                            let body = buf[body_offset..n].to_vec();

                            let host = req_headers
                                .get("Host")
                                .or_else(|| req_headers.get("host"))
                                .cloned()
                                .unwrap_or_else(|| "localhost".to_string());

                            let full_url = if path.starts_with("http://") || path.starts_with("https://") {
                                path.clone()
                            } else {
                                format!("http://{}{}", host, path)
                            };

                            let snapshot = HttpRequestSnapshot {
                                method: method.clone(),
                                url: full_url,
                                headers: req_headers,
                                body,
                            };

                            let decision = guard_clone.inspect(&snapshot);
                            match decision {
                                GuardDecision::MockResponse(mock) => {
                                    let status_text = if mock.status == 201 { "Created" } else { "OK" };
                                    let resp_header = format!(
                                        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nConnection: close\r\nX-Savepoint-Egress: mocked\r\nContent-Length: {}\r\n\r\n",
                                        mock.status, status_text, mock.body.len()
                                    );
                                    let _ = socket.write_all(resp_header.as_bytes()).await;
                                    let _ = socket.write_all(&mock.body).await;
                                    let _ = socket.flush().await;
                                }
                                GuardDecision::Block { reason } => {
                                    let body = format!("{{\"error\":\"{}\"}}", reason);
                                    let resp = format!(
                                        "HTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\nConnection: close\r\nX-Savepoint-Blocked: true\r\nContent-Length: {}\r\n\r\n{}",
                                        body.len(), body
                                    );
                                    let _ = socket.write_all(resp.as_bytes()).await;
                                    let _ = socket.flush().await;
                                }
                                GuardDecision::AllowRead => {
                                    // Check VCR replay
                                    let cassette_name = "default_cassette";
                                    if let Ok(Some(cached)) = vcr_clone.replay(cassette_name, &snapshot) {
                                        let resp_header = format!(
                                            "HTTP/1.1 {} OK\r\nContent-Type: application/json\r\nConnection: close\r\nX-Savepoint-VCR: replayed\r\nContent-Length: {}\r\n\r\n",
                                            cached.status, cached.body.len()
                                        );
                                        let _ = socket.write_all(resp_header.as_bytes()).await;
                                        let _ = socket.write_all(&cached.body).await;
                                        let _ = socket.flush().await;
                                    } else {
                                        let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: 15\r\n\r\n{\"status\":\"ok\"}";
                                        let _ = socket.write_all(resp.as_bytes()).await;
                                        let _ = socket.flush().await;
                                    }
                                }
                                GuardDecision::AllowMutateWithSaga => {
                                    let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: 21\r\n\r\n{\"status\":\"saga_ok\"}";
                                    let _ = socket.write_all(resp.as_bytes()).await;
                                    let _ = socket.flush().await;
                                }
                            }
                        });
                    }
                    _ = shutdown_rx.recv() => {
                        break;
                    }
                }
            }
        });

        Ok(Self {
            local_addr,
            _shutdown_tx: shutdown_tx,
        })
    }

    pub fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.local_addr)
    }
}
