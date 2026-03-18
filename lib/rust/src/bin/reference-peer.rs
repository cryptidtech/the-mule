use tokio_stream::StreamExt;

fn local_ip() -> String {
    let socket = match std::net::UdpSocket::bind("0.0.0.0:0") {
        Ok(s) => s,
        Err(_) => return "0.0.0.0".to_string(),
    };
    if socket.connect("8.8.8.8:80").is_err() {
        return "0.0.0.0".to_string();
    }
    socket
        .local_addr()
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|_| "0.0.0.0".to_string())
}

fn extract_port(listen_addr: &str) -> String {
    let parts: Vec<&str> = listen_addr.split('/').collect();
    for (i, p) in parts.iter().enumerate() {
        if *p == "udp" {
            if let Some(port) = parts.get(i + 1) {
                return port.to_string();
            }
        }
    }
    "0".to_string()
}

fn detect_multiaddr() -> String {
    let port = std::env::var("LISTEN_ADDR")
        .map(|a| extract_port(&a))
        .unwrap_or_else(|_| "0".to_string());
    let ip = local_ip();
    format!("/ip4/{ip}/udp/{port}/quic-v1")
}

#[tokio::main]
async fn main() {
    let mut client = the_mule::MuleClientBuilder::new()
        .build()
        .await
        .expect("failed to build mule client");

    let multiaddr = detect_multiaddr();
    client
        .send_status(&format!("started|{multiaddr}"))
        .await
        .expect("failed to send started");

    loop {
        let cmd = {
            use std::pin::Pin;
            let mut pinned = Pin::new(&mut client);
            pinned.next().await
        };
        match cmd {
            Some(the_mule::Command::Shutdown) => {
                let _ = client.send_status("stopped").await;
                break;
            }
            Some(the_mule::Command::Restart { delay_secs }) => {
                let _ = client.send_status("restarting").await;
                let _ = std::fs::write("/tmp/delay", delay_secs.to_string());
                std::process::exit(42);
            }
            Some(cmd) => {
                tracing::info!("received command: {:?}", cmd);
            }
            None => break,
        }
    }
}
