//! What Settings and the Setup Assistant share: running a request on the
//! Iliada system service as a stream of messages for the UI.

pub mod account;
pub mod panel;

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

/// "5 minutes ago", "1 hour ago".
pub use iliada_system::ago;

/// Open a web page in the user's browser.
pub fn open_url(url: &str) {
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}
