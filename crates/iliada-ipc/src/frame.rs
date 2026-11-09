use serde::{Serialize, de::DeserializeOwned};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::MAX_FRAME_LEN;

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("frame of {0} bytes exceeds the {MAX_FRAME_LEN} byte limit")]
    TooLarge(usize),
    #[error("malformed message: {0}")]
    Codec(#[from] postcard::Error),
}

pub async fn write_frame<W, T>(w: &mut W, msg: &T) -> Result<(), FrameError>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let bytes = postcard::to_stdvec(msg)?;
    if bytes.len() > MAX_FRAME_LEN as usize {
        return Err(FrameError::TooLarge(bytes.len()));
    }
    w.write_all(&(bytes.len() as u32).to_be_bytes()).await?;
    w.write_all(&bytes).await?;
    w.flush().await?;
    Ok(())
}

/// Read one frame. `Ok(None)` on a clean end-of-stream before a new frame.
pub async fn read_frame<R, T>(r: &mut R) -> Result<Option<T>, FrameError>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let mut len = [0u8; 4];
    match r.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_FRAME_LEN as usize {
        return Err(FrameError::TooLarge(len));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    Ok(Some(postcard::from_bytes(&buf)?))
}

/// A framed, typed duplex channel over any byte stream.
#[derive(Debug)]
pub struct Channel<S> {
    stream: S,
}

impl<S: AsyncRead + AsyncWrite + Unpin> Channel<S> {
    pub fn new(stream: S) -> Self {
        Self { stream }
    }
    pub async fn send<T: Serialize>(&mut self, msg: &T) -> Result<(), FrameError> {
        write_frame(&mut self.stream, msg).await
    }
    pub async fn recv<T: DeserializeOwned>(&mut self) -> Result<Option<T>, FrameError> {
        read_frame(&mut self.stream).await
    }
    pub fn into_inner(self) -> S {
        self.stream
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;
    use iliada_types::{AppId, Capability, FileScope};

    #[tokio::test]
    async fn round_trip_all_shapes() {
        let (a, b) = tokio::io::duplex(4096);
        let (mut a, mut b) = (Channel::new(a), Channel::new(b));
        let msgs = vec![
            GuestMessage::Hello { version: PROTOCOL_VERSION, client: "test".into() },
            GuestMessage::Request {
                id: 7,
                body: Request::Capability {
                    capability: Capability::Files(FileScope::UserPicked),
                    reason: Some("open".into()),
                },
            },
        ];
        for m in &msgs {
            a.send(m).await.unwrap();
        }
        for m in &msgs {
            assert_eq!(&b.recv::<GuestMessage>().await.unwrap().unwrap(), m);
        }
        let reply = HostMessage::Welcome { version: 1, app_id: AppId::new("org.example.x").unwrap(), session: 3 };
        b.send(&reply).await.unwrap();
        assert_eq!(a.recv::<HostMessage>().await.unwrap().unwrap(), reply);
        drop(b);
        assert!(a.recv::<HostMessage>().await.unwrap().is_none(), "clean EOF");
    }

    #[tokio::test]
    async fn rejects_oversized_length_prefix() {
        let (mut a, b) = tokio::io::duplex(64);
        a.write_all(&(MAX_FRAME_LEN + 1).to_be_bytes()).await.unwrap();
        let mut b = Channel::new(b);
        assert!(matches!(b.recv::<GuestMessage>().await, Err(FrameError::TooLarge(_))));
    }

    #[tokio::test]
    async fn rejects_garbage() {
        let (mut a, b) = tokio::io::duplex(64);
        a.write_all(&3u32.to_be_bytes()).await.unwrap();
        a.write_all(&[0xff, 0xff, 0xff]).await.unwrap();
        let mut b = Channel::new(b);
        assert!(matches!(b.recv::<GuestMessage>().await, Err(FrameError::Codec(_))));
    }
}
