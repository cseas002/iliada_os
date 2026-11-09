//! End-to-end: real broker sockets + the real guest client.

use std::path::PathBuf;
use std::sync::Arc;

use iliada_broker::prompt::Scripted;
use iliada_broker::{AppContext, Broker, Engine, GrantStore, PromptAnswer};
use iliada_bundle::{PermissionRequest, Permissions};
use iliada_guest::Client;
use iliada_ipc::{DenyReason, GrantScope, Reply, Resource, transport};
use iliada_types::{AppId, Capability, FileScope};

fn app() -> AppContext {
    let req = |capability, purpose: &str| PermissionRequest { capability, purpose: purpose.into() };
    AppContext {
        id: AppId::new("org.example.memo").unwrap(),
        name: "Memo".into(),
        permissions: Permissions {
            requests: vec![
                req(Capability::Microphone, "Record voice memos"),
                req(Capability::Files(FileScope::UserPicked), "Open notes"),
                req(Capability::Notifications, "Remind you"),
            ],
        },
    }
}

async fn start(state: &std::path::Path, sockets: &std::path::Path, frontend: Arc<Scripted>) -> PathBuf {
    let engine = Engine::new(GrantStore::open(state).unwrap(), frontend);
    let broker = Arc::new(Broker::new(engine, state));
    let mut l = broker.listen(sockets, vec![app()]).unwrap();
    l.remove(0).0
}

#[tokio::test]
async fn full_permission_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let (state, sockets) = (dir.path().join("state"), dir.path().join("sock"));
    let shared = dir.path().join("note.txt");
    std::fs::write(&shared, "line one\nline two\n").unwrap();

    let fe = Arc::new(Scripted::new(
        [
            PromptAnswer::Allow(GrantScope::Always), // microphone
            PromptAnswer::Allow(GrantScope::Once),   // file
            PromptAnswer::Allow(GrantScope::Once),   // notifications #1
            PromptAnswer::Deny,                      // notifications #2
        ],
        [shared.clone()],
    ));
    let sock = start(&state, &sockets, fe.clone()).await;
    assert_eq!(sock, transport::broker_socket(&sockets, &app().id));

    let mut c = Client::connect_unix(&sock).await.unwrap();
    assert_eq!(c.app_id().as_str(), "org.example.memo", "identity is assigned by the host");

    // Undeclared → denied without bothering the user.
    let r = c.request(Capability::Camera, None).await.unwrap();
    assert_eq!(r, Reply::Denied { reason: DenyReason::NotDeclared });
    assert!(fe.seen.lock().await.is_empty());

    // Microphone: prompt once, "Always".
    let Reply::Granted(g) = c.request(Capability::Microphone, Some("\x1b[31mtrust me")).await.unwrap() else {
        panic!()
    };
    assert_eq!(g.scope, GrantScope::Always);
    assert_eq!(fe.seen.lock().await[0].app_reason.as_deref(), Some("[31mtrust me"), "escape codes stripped");

    // File: user picks; guest reads bytes through the handle, never the path.
    let Reply::Granted(g) = c.request(Capability::Files(FileScope::UserPicked), None).await.unwrap() else { panic!() };
    assert_eq!(g.resource, Resource::File { name: "note.txt".into(), size: 18 });
    assert_eq!(c.read_all(g.handle).await.unwrap(), b"line one\nline two\n");

    // "Once" does not stick: second request prompts again (and is denied).
    assert!(matches!(c.request(Capability::Notifications, None).await.unwrap(), Reply::Granted(_)));
    assert_eq!(
        c.request(Capability::Notifications, None).await.unwrap(),
        Reply::Denied { reason: DenyReason::UserDenied }
    );
    assert_eq!(fe.seen.lock().await.len(), 4);
    drop(c);

    // New session: the "Always" microphone grant is honoured without a prompt.
    let mut c = Client::connect_unix(&sock).await.unwrap();
    let Reply::Granted(g) = c.request(Capability::Microphone, None).await.unwrap() else { panic!() };
    assert_eq!(g.scope, GrantScope::Always);
    assert_eq!(fe.seen.lock().await.len(), 4, "no new prompt");

    // Audit log has one line per decision.
    let audit = std::fs::read_to_string(state.join("audit.jsonl")).unwrap();
    assert_eq!(audit.lines().count(), 6);
    assert!(audit.lines().next().unwrap().contains("NotDeclared"));
}

#[tokio::test]
async fn always_grant_survives_broker_restart_and_revoke_clears_it() {
    let dir = tempfile::tempdir().unwrap();
    let (state, sockets) = (dir.path().join("state"), dir.path().join("sock"));
    let sock = start(&state, &sockets, Arc::new(Scripted::new([PromptAnswer::Allow(GrantScope::Always)], []))).await;
    let mut c = Client::connect_unix(&sock).await.unwrap();
    assert!(matches!(c.request(Capability::Microphone, None).await.unwrap(), Reply::Granted(_)));
    drop(c);

    // "Restart": a fresh broker over the same state dir, with a frontend that would deny.
    let sockets2 = dir.path().join("sock2");
    let fe = Arc::new(Scripted::new([], []));
    let sock = start(&state, &sockets2, fe.clone()).await;
    let mut c = Client::connect_unix(&sock).await.unwrap();
    assert!(matches!(c.request(Capability::Microphone, None).await.unwrap(), Reply::Granted(_)));
    assert!(fe.seen.lock().await.is_empty());

    c.revoke(Capability::Microphone).await.unwrap();
    assert_eq!(
        c.request(Capability::Microphone, None).await.unwrap(),
        Reply::Denied { reason: DenyReason::UserDenied }
    );
    assert!(
        matches!(c.take_events().as_slice(), [iliada_ipc::Event::Revoked { capability: Capability::Microphone, .. }]),
        "revocation event delivered"
    );
}

#[tokio::test]
async fn while_running_grant_dies_with_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let (state, sockets) = (dir.path().join("state"), dir.path().join("sock"));
    let fe = Arc::new(Scripted::new([PromptAnswer::Allow(GrantScope::WhileRunning), PromptAnswer::Deny], []));
    let sock = start(&state, &sockets, fe.clone()).await;

    let mut c = Client::connect_unix(&sock).await.unwrap();
    assert!(matches!(c.request(Capability::Microphone, None).await.unwrap(), Reply::Granted(_)));
    assert!(matches!(c.request(Capability::Microphone, None).await.unwrap(), Reply::Granted(_)));
    assert_eq!(fe.seen.lock().await.len(), 1, "second request served from the session grant");
    drop(c);

    let mut c = Client::connect_unix(&sock).await.unwrap();
    assert!(matches!(c.request(Capability::Microphone, None).await.unwrap(), Reply::Denied { .. }));
}

/// Bundles installed after start-up are picked up; removed ones are dropped.
#[tokio::test]
async fn follows_installs_and_removals() {
    use iliada_bundle::{Bundle, BundleLayout, Info, IsolationTier};
    let dir = tempfile::tempdir().unwrap();
    let (apps, sockets, state) = (dir.path().join("Applications"), dir.path().join("sock"), dir.path().join("state"));
    std::fs::create_dir_all(&apps).unwrap();
    let engine = Engine::new(GrantStore::open(&state).unwrap(), Arc::new(Scripted::new([], [])));
    let broker = Arc::new(Broker::new(engine, &state));
    let watcher = tokio::spawn(broker.clone().watch_applications(
        apps.clone(),
        sockets.clone(),
        std::time::Duration::from_millis(50),
    ));

    let wait_for = |p: PathBuf, exists: bool| async move {
        for _ in 0..100 {
            if p.exists() == exists {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        false
    };

    // Install a bundle after the broker is already running.
    let layout = BundleLayout::in_dir(&apps, "Memo");
    layout.create_dirs().unwrap();
    std::fs::write(layout.executables().join("memo"), "#!/bin/sh\n").unwrap();
    let info = Info {
        id: app().id,
        name: "Memo".into(),
        version: "1".into(),
        executable: "memo".into(),
        entry: None,
        icon: None,
        runtime: "iliada-base-1".into(),
        isolation: IsolationTier::Namespace,
        categories: vec![],
        mime_types: vec![],
    };
    Bundle::write_manifests(&layout, &info, &app().permissions, None).unwrap();
    let sock = transport::broker_socket(&sockets, &app().id);
    assert!(wait_for(sock.clone(), true).await, "socket appears after install");
    let mut c = Client::connect_unix(&sock).await.unwrap();
    assert_eq!(c.request(Capability::Camera, None).await.unwrap(), Reply::Denied { reason: DenyReason::NotDeclared });
    drop(c);

    // Uninstall: the socket goes away.
    std::fs::remove_dir_all(layout.root()).unwrap();
    assert!(wait_for(sock, false).await, "socket removed after uninstall");
    watcher.abort();
}
