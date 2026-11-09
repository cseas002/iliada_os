//! Ask the user for a file through the broker and print its first lines.
//!
//!     cargo run -p iliada-guest --example pick-file -- <broker socket>

use iliada_guest::Client;
use iliada_ipc::{Reply, Resource};
use iliada_types::{Capability, FileScope};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let socket = std::env::args().nth(1).ok_or_else(|| anyhow::anyhow!("usage: pick-file <broker socket>"))?;
    let mut client = Client::connect_unix(&socket).await?;
    match client.request(Capability::Files(FileScope::UserPicked), Some("Open a note")).await? {
        Reply::Granted(g) => {
            if let Resource::File { name, size } = &g.resource {
                println!("user shared {name:?} ({size} bytes); the app never learns its host path");
            }
            let bytes = client.read_all(g.handle).await?;
            for line in String::from_utf8_lossy(&bytes).lines().take(5) {
                println!("  | {line}");
            }
            client.close(g.handle).await?;
        }
        other => println!("not shared: {other:?}"),
    }
    Ok(())
}
