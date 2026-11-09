//! Ask the broker for the microphone, as an app inside a sandbox would.
//!
//!     cargo run -p iliada-guest --example request-mic -- <broker socket>

use iliada_guest::Client;
use iliada_ipc::Reply;
use iliada_types::Capability;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let socket = std::env::args().nth(1).ok_or_else(|| anyhow::anyhow!("usage: request-mic <broker socket>"))?;
    let mut client = Client::connect_unix(&socket).await?;
    println!("connected as {} (session {})", client.app_id(), client.session());
    match client.request(Capability::Microphone, Some("Record a voice memo")).await? {
        Reply::Granted(g) => println!("microphone granted ({:?}); resource: {:?}", g.scope, g.resource),
        Reply::Denied { reason } => println!("microphone denied: {reason:?}"),
        other => println!("unexpected reply: {other:?}"),
    }
    match client.request(Capability::Camera, None).await? {
        Reply::Denied { reason } => println!("camera denied without a prompt: {reason:?}"),
        other => println!("camera: {other:?}"),
    }
    println!("current grants: {:?}", client.list_grants().await?);
    Ok(())
}
