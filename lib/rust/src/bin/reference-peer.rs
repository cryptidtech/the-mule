use tokio_stream::StreamExt;

#[tokio::main]
async fn main() {
    let mut client = the_mule::MuleClientBuilder::new()
        .build()
        .await
        .expect("failed to build mule client");

    client
        .send_status("started")
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
