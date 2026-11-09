//! Guest-side client for the Iliada broker.
//!
//! ```no_run
//! # async fn demo() -> Result<(), iliada_guest::ClientError> {
//! use iliada_guest::Client;
//! use iliada_types::Capability;
//!
//! let mut client = Client::connect_unix("/run/iliada/broker.sock").await?;
//! match client.request(Capability::Microphone, Some("Record a voice memo")).await? {
//!     iliada_ipc::Reply::Granted(g) => println!("granted: {:?}", g.scope),
//!     other => println!("not granted: {other:?}"),
//! }
//! # Ok(()) }
//! ```
//!
//! Inside a micro-VM the stream is AF_VSOCK to `HOST_CID:BROKER_VSOCK_PORT`;
//! [`Client::from_stream`] accepts any byte stream so the vsock connector can
//! live in the guest image without this crate depending on it.

use std::collections::VecDeque;
use std::path::Path;

use iliada_ipc::{
    Channel, Event, FrameError, GrantInfo, GuestMessage, Handle, HostMessage, MAX_READ_CHUNK, PROTOCOL_VERSION, Reply,
    Request,
};
use iliada_types::{AppId, Capability};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::UnixStream;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("transport: {0}")]
    Frame(#[from] FrameError),
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("broker closed the session: {0}")]
    Goodbye(String),
    #[error("protocol violation: {0}")]
    Protocol(String),
    #[error("broker error: {0}")]
    Broker(String),
}

#[derive(Debug)]
pub struct Client<S = UnixStream> {
    ch: Channel<S>,
    next_id: u32,
    app_id: AppId,
    session: u64,
    events: VecDeque<Event>,
}

impl Client<UnixStream> {
    pub async fn connect_unix(path: impl AsRef<Path>) -> Result<Self, ClientError> {
        Self::from_stream(UnixStream::connect(path).await?).await
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> Client<S> {
    pub async fn from_stream(stream: S) -> Result<Self, ClientError> {
        let mut ch = Channel::new(stream);
        ch.send(&GuestMessage::Hello {
            version: PROTOCOL_VERSION,
            client: concat!("iliada-guest/", env!("CARGO_PKG_VERSION")).into(),
        })
        .await?;
        match ch.recv::<HostMessage>().await? {
            Some(HostMessage::Welcome { app_id, session, .. }) => {
                Ok(Self { ch, next_id: 1, app_id, session, events: VecDeque::new() })
            }
            Some(HostMessage::Goodbye { reason }) => Err(ClientError::Goodbye(reason)),
            other => Err(ClientError::Protocol(format!("expected Welcome, got {other:?}"))),
        }
    }

    /// The identity the *host* assigned to this sandbox.
    pub fn app_id(&self) -> &AppId {
        &self.app_id
    }
    pub fn session(&self) -> u64 {
        self.session
    }
    /// Events (e.g. revocations) received while waiting for replies.
    pub fn take_events(&mut self) -> Vec<Event> {
        self.events.drain(..).collect()
    }

    async fn call(&mut self, body: Request) -> Result<Reply, ClientError> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.ch.send(&GuestMessage::Request { id, body }).await?;
        loop {
            match self.ch.recv::<HostMessage>().await? {
                Some(HostMessage::Reply { id: rid, body }) if rid == id => return Ok(body),
                Some(HostMessage::Reply { id: rid, .. }) => {
                    return Err(ClientError::Protocol(format!("reply for {rid}, expected {id}")));
                }
                Some(HostMessage::Event(e)) => self.events.push_back(e),
                Some(HostMessage::Goodbye { reason }) => return Err(ClientError::Goodbye(reason)),
                Some(HostMessage::Welcome { .. }) => return Err(ClientError::Protocol("unexpected Welcome".into())),
                None => return Err(ClientError::Goodbye("connection closed".into())),
            }
        }
    }

    pub async fn request(&mut self, capability: Capability, reason: Option<&str>) -> Result<Reply, ClientError> {
        self.call(Request::Capability { capability, reason: reason.map(str::to_owned) }).await
    }

    pub async fn revoke(&mut self, capability: Capability) -> Result<(), ClientError> {
        match self.call(Request::Revoke { capability }).await? {
            Reply::Ok => Ok(()),
            Reply::Error { message } => Err(ClientError::Broker(message)),
            other => Err(ClientError::Protocol(format!("unexpected {other:?}"))),
        }
    }

    pub async fn list_grants(&mut self) -> Result<Vec<GrantInfo>, ClientError> {
        match self.call(Request::ListGrants).await? {
            Reply::Grants(g) => Ok(g),
            other => Err(ClientError::Protocol(format!("unexpected {other:?}"))),
        }
    }

    /// Read a whole broker-mediated file handle.
    pub async fn read_all(&mut self, handle: Handle) -> Result<Vec<u8>, ClientError> {
        let mut out = Vec::new();
        loop {
            match self.call(Request::Read { handle, offset: out.len() as u64, len: MAX_READ_CHUNK }).await? {
                Reply::Data { bytes, eof } => {
                    out.extend_from_slice(&bytes);
                    if eof {
                        return Ok(out);
                    }
                }
                Reply::Error { message } => return Err(ClientError::Broker(message)),
                other => return Err(ClientError::Protocol(format!("unexpected {other:?}"))),
            }
        }
    }

    pub async fn close(&mut self, handle: Handle) -> Result<(), ClientError> {
        self.call(Request::Close { handle }).await.map(|_| ())
    }
}
