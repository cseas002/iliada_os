//! What Settings and the Setup Assistant share: running a request on the
//! Iliada system service as a stream of messages for the UI.

use futures_util::Stream;
use iliada_system::client;
use iliada_system::protocol::{Event, Request};

/// What a running request reports.
#[derive(Debug, Clone)]
pub enum Progress {
    Event(Event),
    Finished(Result<serde_json::Value, String>),
}

/// Run `request`, yielding its progress and then exactly one `Finished`.
pub fn run(request: Request) -> impl Stream<Item = Progress> + Send + 'static {
    enum State {
        Start(Request),
        Running(tokio::sync::mpsc::UnboundedReceiver<Progress>),
    }
    futures_util::stream::unfold(State::Start(request), |state| async move {
        let mut rx = match state {
            State::Running(rx) => rx,
            State::Start(request) => {
                let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
                tokio::spawn(async move {
                    let events = tx.clone();
                    let r = client::call(&request, &mut |e| {
                        let _ = events.send(Progress::Event(e.clone()));
                    })
                    .await
                    .map_err(|e| format!("{e:#}"));
                    let _ = tx.send(Progress::Finished(r));
                });
                rx
            }
        };
        rx.recv().await.map(|p| (p, State::Running(rx)))
    })
}

/// "5 minutes ago".
pub fn ago(t: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(t);
    match now.saturating_sub(t) {
        s if s < 90 => "just now".into(),
        s if s < 90 * 60 => format!("{} minutes ago", s / 60),
        s if s < 36 * 3600 => format!("{} hours ago", s / 3600),
        s => format!("{} days ago", s / 86400),
    }
}

/// Open a web page in the user's browser.
pub fn open_url(url: &str) {
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}
