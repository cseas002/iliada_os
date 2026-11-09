//! Socket naming and connection helpers shared by the broker, the sandbox
//! controller and guest tooling.

use std::path::{Path, PathBuf};

use iliada_types::AppId;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use crate::BROKER_VSOCK_PORT;

/// The Unix socket cloud-hypervisor exposes as the VM's vsock device.
pub fn vm_vsock_socket(socket_dir: &Path, app: &AppId) -> PathBuf {
    socket_dir.join(format!("{app}.vsock"))
}

/// Where the broker listens for one app. For micro-VMs this is the path
/// cloud-hypervisor connects to when the guest dials `CID 2 : 7000`
/// (`<vsock socket>_<port>`); Tier-1 sandboxes get it bind-mounted.
pub fn broker_socket(socket_dir: &Path, app: &AppId) -> PathBuf {
    socket_dir.join(format!("{app}.vsock_{BROKER_VSOCK_PORT}"))
}

/// Host → guest connection through cloud-hypervisor's hybrid vsock: connect
/// to the VM's vsock socket and send `CONNECT <port>\n`; the VMM answers
/// `OK <host port>\n` and the stream is then spliced to the guest listener.
pub async fn connect_guest_port(vm_vsock_socket: &Path, port: u32) -> std::io::Result<UnixStream> {
    let mut stream = UnixStream::connect(vm_vsock_socket).await?;
    stream.write_all(format!("CONNECT {port}\n").as_bytes()).await?;
    // Read the reply byte by byte: anything after the newline already belongs
    // to the guest's stream and must not be swallowed by a buffer.
    let mut line = Vec::with_capacity(32);
    loop {
        let b = stream.read_u8().await?;
        if b == b'\n' {
            break;
        }
        if line.len() >= 64 {
            return Err(std::io::Error::other("vsock handshake line too long"));
        }
        line.push(b);
    }
    let line = String::from_utf8_lossy(&line);
    if !line.starts_with("OK ") {
        return Err(std::io::Error::other(format!("vsock handshake refused: {:?}", line.trim())));
    }
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_names_follow_cloud_hypervisor_convention() {
        let app = AppId::new("org.debian.jq").unwrap();
        let dir = Path::new("/run/iliada");
        assert_eq!(vm_vsock_socket(dir, &app), Path::new("/run/iliada/org.debian.jq.vsock"));
        assert_eq!(broker_socket(dir, &app), Path::new("/run/iliada/org.debian.jq.vsock_7000"));
    }

    #[tokio::test]
    async fn hybrid_vsock_handshake() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vm.vsock");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 13];
            s.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"CONNECT 7000\n");
            s.write_all(b"OK 1073741824\nping").await.unwrap();
        });
        let mut s = connect_guest_port(&path, 7000).await.unwrap();
        let mut buf = [0u8; 4];
        s.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ping");
    }
}
