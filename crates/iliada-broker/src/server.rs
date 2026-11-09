//! One Unix listener per app; one session per connection.

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use iliada_ipc::{
    Channel, DenyReason, Event, Grant, GrantInfo, GrantScope, GuestMessage, Handle, HostMessage, MAX_READ_CHUNK,
    MediaKind, PROTOCOL_VERSION, Reply, Request, Resource, transport,
};
use iliada_types::{Capability, FileScope};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::task::JoinHandle;

use crate::audit::AuditLog;
use crate::engine::{AppContext, Decision, Engine};

#[derive(Debug)]
pub struct Broker {
    engine: Engine,
    audit: AuditLog,
    sessions: AtomicU64,
}

#[derive(Debug)]
struct OpenFile {
    file: tokio::fs::File,
    capability: Capability,
}

/// Per-connection state. "While running" grants live here and die with it.
#[derive(Debug, Default)]
struct Session {
    grants: BTreeSet<Capability>,
    handles: BTreeMap<Handle, Option<OpenFile>>,
    next_handle: u64,
}

impl Broker {
    pub fn new(engine: Engine, state_dir: &Path) -> Self {
        Self { engine, audit: AuditLog::new(state_dir), sessions: AtomicU64::new(1) }
    }

    pub fn audit_path(&self) -> &Path {
        self.audit.path()
    }

    /// Bind `<socket_dir>/<app-id>.vsock_7000` for each app and serve forever.
    pub fn listen(
        self: &Arc<Self>,
        socket_dir: &Path,
        apps: Vec<AppContext>,
    ) -> Result<Vec<(PathBuf, JoinHandle<()>)>> {
        std::fs::create_dir_all(socket_dir)?;
        let mut out = Vec::new();
        for app in apps {
            let path = transport::broker_socket(socket_dir, &app.id);
            if path.exists() {
                std::fs::remove_file(&path)?;
            }
            let listener = UnixListener::bind(&path).with_context(|| format!("binding {}", path.display()))?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o660))?;
            let (me, app) = (self.clone(), Arc::new(app));
            let task = tokio::spawn(async move {
                loop {
                    match listener.accept().await {
                        Ok((stream, _)) => {
                            let (me, app) = (me.clone(), app.clone());
                            tokio::spawn(async move {
                                if let Err(e) = me.run_session(app.clone(), stream).await {
                                    tracing::warn!(app = %app.id, error = %format!("{e:#}"), "session ended with error");
                                }
                            });
                        }
                        Err(e) => tracing::error!(error = %e, "accept failed"),
                    }
                }
            });
            out.push((path, task));
        }
        Ok(out)
    }

    async fn run_session(&self, app: Arc<AppContext>, stream: UnixStream) -> Result<()> {
        let mut ch = Channel::new(stream);
        let id = self.sessions.fetch_add(1, Ordering::Relaxed);
        match ch.recv::<GuestMessage>().await? {
            Some(GuestMessage::Hello { version, client }) if version == PROTOCOL_VERSION => {
                tracing::info!(app = %app.id, session = id, client = %client, "session started");
            }
            Some(GuestMessage::Hello { version, .. }) => {
                ch.send(&HostMessage::Goodbye { reason: format!("unsupported protocol version {version}") }).await?;
                return Ok(());
            }
            _ => {
                ch.send(&HostMessage::Goodbye { reason: "expected Hello".into() }).await?;
                return Ok(());
            }
        }
        // Identity comes from the socket, not from the guest.
        ch.send(&HostMessage::Welcome { version: PROTOCOL_VERSION, app_id: app.id.clone(), session: id }).await?;

        let mut session = Session::default();
        while let Some(msg) = ch.recv::<GuestMessage>().await? {
            let GuestMessage::Request { id: req_id, body } = msg else {
                ch.send(&HostMessage::Goodbye { reason: "duplicate Hello".into() }).await?;
                break;
            };
            let (reply, event) = self.handle(&app, id, &mut session, body).await;
            ch.send(&HostMessage::Reply { id: req_id, body: reply }).await?;
            if let Some(ev) = event {
                ch.send(&HostMessage::Event(ev)).await?;
            }
        }
        tracing::info!(app = %app.id, session = id, "session closed");
        Ok(())
    }

    async fn handle(&self, app: &AppContext, sid: u64, s: &mut Session, req: Request) -> (Reply, Option<Event>) {
        match req {
            Request::Capability { capability, reason } => {
                (self.request_capability(app, sid, s, capability, reason).await, None)
            }
            Request::Revoke { capability } => {
                s.grants.remove(&capability);
                let handles: Vec<Handle> = s
                    .handles
                    .iter()
                    .filter(|(_, f)| f.as_ref().is_some_and(|f| f.capability == capability))
                    .map(|(h, _)| *h)
                    .collect();
                for h in &handles {
                    s.handles.remove(h);
                }
                if let Err(e) = self.engine.revoke(&app.id, &capability).await {
                    return (Reply::Error { message: format!("{e:#}") }, None);
                }
                self.audit.record(&app.id, sid, &capability, "revoked", "by app".into()).await;
                (Reply::Ok, Some(Event::Revoked { capability, handles }))
            }
            Request::ListGrants => {
                let mut v: Vec<GrantInfo> = self
                    .engine
                    .stored(&app.id)
                    .await
                    .into_iter()
                    .map(|capability| GrantInfo { capability, scope: GrantScope::Always })
                    .collect();
                v.extend(s.grants.iter().map(|c| GrantInfo { capability: *c, scope: GrantScope::WhileRunning }));
                (Reply::Grants(v), None)
            }
            Request::Read { handle, offset, len } => (read_handle(s, handle, offset, len).await, None),
            Request::Close { handle } => {
                s.handles.remove(&handle);
                (Reply::Ok, None)
            }
        }
    }

    async fn request_capability(
        &self,
        app: &AppContext,
        sid: u64,
        s: &mut Session,
        cap: Capability,
        reason: Option<String>,
    ) -> Reply {
        let decision = match self.engine.decide(app, &s.grants, &cap, reason.as_deref()).await {
            Ok(d) => d,
            Err(e) => return Reply::Error { message: format!("{e:#}") },
        };
        let (scope, source) = match decision {
            Decision::Deny { reason, source } => {
                self.audit.record(&app.id, sid, &cap, "deny", format!("{reason:?} via {source:?}")).await;
                return Reply::Denied { reason };
            }
            Decision::Allow { scope, source } => (scope, source),
        };
        if scope == GrantScope::WhileRunning {
            s.grants.insert(cap);
        }

        // Attach the resource. The guest only ever receives a handle.
        let (resource, file) = match cap {
            Capability::Files(FileScope::UserPicked) => {
                let Some(path) = self.engine.frontend().pick_file(&app.name).await else {
                    self.audit.record(&app.id, sid, &cap, "deny", "file chooser cancelled".into()).await;
                    return Reply::Denied { reason: DenyReason::UserDenied };
                };
                match open_user_file(&path).await {
                    Ok((file, name, size)) => (Resource::File { name, size }, Some(OpenFile { file, capability: cap })),
                    Err(e) => return Reply::Error { message: format!("cannot open chosen file: {e}") },
                }
            }
            Capability::Microphone => (Resource::Media { kind: MediaKind::AudioCapture, endpoint: None }, None),
            Capability::Camera => (Resource::Media { kind: MediaKind::VideoCapture, endpoint: None }, None),
            Capability::ScreenCapture => (Resource::Media { kind: MediaKind::ScreenCapture, endpoint: None }, None),
            _ => (Resource::None, None),
        };
        s.next_handle += 1;
        let handle = Handle(s.next_handle);
        s.handles.insert(handle, file);
        self.audit.record(&app.id, sid, &cap, "allow", format!("{scope:?} via {source:?}")).await;
        Reply::Granted(Grant { handle, capability: cap, scope, resource })
    }
}

async fn open_user_file(path: &Path) -> std::io::Result<(tokio::fs::File, String, u64)> {
    let file = tokio::fs::File::open(path).await?;
    let meta = file.metadata().await?;
    if !meta.is_file() {
        return Err(std::io::Error::other("not a regular file"));
    }
    // Only the base name crosses the boundary — never the host path.
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Ok((file, name, meta.len()))
}

async fn read_handle(s: &mut Session, handle: Handle, offset: u64, len: u32) -> Reply {
    let Some(Some(f)) = s.handles.get_mut(&handle) else {
        return Reply::Error { message: "unknown or non-readable handle".into() };
    };
    let len = len.min(MAX_READ_CHUNK) as usize;
    let mut buf = vec![0u8; len];
    let res = async {
        f.file.seek(std::io::SeekFrom::Start(offset)).await?;
        let mut n = 0;
        while n < len {
            let r = f.file.read(&mut buf[n..]).await?;
            if r == 0 {
                break;
            }
            n += r;
        }
        Ok::<_, std::io::Error>(n)
    }
    .await;
    match res {
        Ok(n) => {
            buf.truncate(n);
            Reply::Data { eof: n < len, bytes: buf }
        }
        Err(e) => Reply::Error { message: e.to_string() },
    }
}
