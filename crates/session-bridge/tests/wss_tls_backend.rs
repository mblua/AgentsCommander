use std::time::Duration;

use tokio::io::AsyncReadExt;

use tokio_tungstenite::connect_async;

#[tokio::test]
async fn wss_connection_starts_tls_handshake() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local TLS probe");
    let address = listener.local_addr().expect("local TLS probe address");
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await?;
        let mut record_header = [0_u8; 5];
        stream.read_exact(&mut record_header).await?;
        Ok::<_, std::io::Error>(record_header)
    });

    let error = match connect_async(format!("wss://{address}/")).await {
        Ok(_) => panic!("plain probe unexpectedly completed a WSS handshake"),
        Err(error) => error,
    };
    let record_header = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("TLS probe timed out")
        .expect("TLS probe task failed")
        .expect("read TLS record header");

    assert_eq!(record_header[0], 0x16, "expected a TLS handshake record");
    assert_eq!(record_header[1], 0x03, "expected a TLS record version");
    assert!(
        !format!("{error:?}").contains("TlsFeatureNotEnabled"),
        "WSS failed before reaching the TLS backend: {error:?}"
    );
    println!("WSS reached the TLS backend: {error:?}");
}
