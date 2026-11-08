use savepoint_egress::guard::GuardPolicy;
use savepoint_egress::proxy::{EgressProxy, EgressProxyConfig};
use tempfile::tempdir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[tokio::test]
async fn test_egress_proxy_serves_mock_for_mutations() {
    let dir = tempdir().unwrap();
    let cassette_dir = dir.path().join("cassettes");

    let config = EgressProxyConfig {
        bind_addr: "127.0.0.1:0".to_string(),
        cassette_dir,
        policy: GuardPolicy::MockAlways,
    };

    let proxy = EgressProxy::start(config).await.unwrap();
    let proxy_addr = proxy.local_addr().unwrap();

    // Send HTTP POST request to proxy simulating agent making a Stripe charge
    let mut stream = TcpStream::connect(proxy_addr).await.unwrap();
    let request_data = "POST http://api.stripe.com/v1/charges HTTP/1.1\r\nHost: api.stripe.com\r\nContent-Length: 17\r\n\r\n{\"amount\":1000}\r\n";
    stream.write_all(request_data.as_bytes()).await.unwrap();

    let mut response = String::new();
    let mut buf = [0u8; 1024];
    let n = stream.read(&mut buf).await.unwrap();
    response.push_str(&String::from_utf8_lossy(&buf[..n]));

    assert!(response.starts_with("HTTP/1.1 201 Created") || response.starts_with("HTTP/1.1 200 OK"));
    assert!(response.contains("ch_mock_"));
    assert!(response.contains("X-Savepoint-Egress: mocked"));
}
