//! Assistant: ask Iliada to do things ("install Firefox and turn on SSH").
//!
//! A quick panel over `iliada-assistant`, opened from anywhere with
//! Alt+Space (or from Applications): a large "Ask Iliada…" field, the
//! conversation below it, Esc to close. In Ask mode each action appears as
//! a card with Allow / Don't Allow; in Auto mode actions run and are listed
//! as they happen. Only one panel runs: opening it again brings it forward.

use std::path::PathBuf;
use std::sync::Arc;

use cosmic::app::{Settings, Task};
use cosmic::iced::keyboard::{Key, key::Named};
use cosmic::iced::widget::scrollable::{RelativeOffset, snap_to};
use cosmic::iced::{Alignment, Length, Size, Subscription};
use cosmic::widget;
use cosmic::{ApplicationExt, Core, Element, executor};
use iliada_system::client;
use iliada_system::protocol::{AssistantMode, AssistantSettings, model_name};
use tokio::sync::{Mutex, mpsc, oneshot};

/// Things people commonly ask, shown before the first question.
const SUGGESTIONS: [&str; 5] = [
    "Switch to dark mode",
    "Install Firefox",
    "Set the volume to 40%",
    "What's taking up space on my disk?",
    "Turn on the SSH server",
];

/// The running panel listens here; a second start just asks it to come forward.
fn socket_path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join("iliada-assistant.sock"))
}

/// The socket this panel listens on, claimed before the window opens so a
/// quick second Alt+Space can't start a second panel.
static LISTENER: std::sync::OnceLock<std::os::unix::net::UnixListener> = std::sync::OnceLock::new();

/// `false`: another panel is open (and was asked to come forward).
fn claim_instance() -> bool {
    let Some(path) = socket_path() else { return true };
    for _ in 0..2 {
        match std::os::unix::net::UnixListener::bind(&path) {
            Ok(l) => {
                let _ = l.set_nonblocking(true);
                let _ = LISTENER.set(l);
                return true;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                // Open already: connecting asks it to come forward.
                if std::os::unix::net::UnixStream::connect(&path).is_ok() {
                    return false;
                }
                // Left over from a panel that was stopped: take it over.
                let _ = std::fs::remove_file(&path);
            }
            Err(_) => return true,
        }
    }
    true
}

fn main() -> cosmic::iced::Result {
    if !claim_instance() {
        return Ok(());
    }
    let settings = Settings::default().size(Size::new(760.0, 560.0));
    cosmic::app::run::<App>(settings, ())
}

#[derive(Debug, Clone)]
enum Line {
    You(String),
    Assistant(String),
    Note(String),
    Error(String),
}

/// The approval the assistant is waiting for.
type Pending = Arc<std::sync::Mutex<Option<oneshot::Sender<bool>>>>;

#[derive(Debug, Clone)]
enum Message {
    Ready(Result<AssistantSettings, String>),
    Input(String),
    Send,
    Suggest(&'static str),
    Note(String),
    /// A piece of the answer as it is written.
    Piece(String),
    /// The model stopped writing (it may act, then write again).
    PieceEnd,
    Approve(String, Pending),
    Answer(bool),
    /// The answer, and what the conversation has cost so far.
    Reply(Result<String, String>, f64),
    OpenSettings,
    /// Esc.
    Close,
    /// Opened again (Alt+Space): come forward.
    Show,
}

struct App {
    core: Core,
    settings: Option<Result<AssistantSettings, String>>,
    assistant: Option<Arc<Mutex<iliada_assistant::Assistant>>>,
    lines: Vec<Line>,
    input: String,
    busy: bool,
    approval: Option<(String, Pending)>,
    /// The answer being written right now (its line).
    writing: Option<usize>,
    /// The line the latest answer was written into.
    written: Option<usize>,
    /// What this conversation has cost (US dollars).
    spent: f64,
    scroll: widget::Id,
    field: widget::Id,
}

/// Bridges the assistant's callbacks to the window.
struct Window {
    tx: mpsc::UnboundedSender<Message>,
}

#[async_trait::async_trait]
impl iliada_assistant::Frontend for Window {
    async fn approve(&self, action: &str) -> bool {
        let (tx, rx) = oneshot::channel();
        let _ = self.tx.send(Message::Approve(action.to_owned(), Arc::new(std::sync::Mutex::new(Some(tx)))));
        rx.await.unwrap_or(false)
    }
    fn progress(&self, text: &str) {
        let _ = self.tx.send(Message::Note(text.to_owned()));
    }
    fn text(&self, piece: &str) {
        let _ = self.tx.send(Message::Piece(piece.to_owned()));
    }
    fn text_end(&self) {
        let _ = self.tx.send(Message::PieceEnd);
    }
}

/// Accepts "come forward" requests from later starts.
fn show_requests() -> impl futures_util::Stream<Item = Message> {
    futures_util::stream::unfold(None::<tokio::net::UnixListener>, |listener| async move {
        let listener = match listener {
            Some(l) => l,
            None => match LISTENER.get().and_then(|l| l.try_clone().ok()).map(tokio::net::UnixListener::from_std) {
                Some(Ok(l)) => l,
                // No socket: never yield (a second start opens a second panel).
                _ => std::future::pending().await,
            },
        };
        let _ = listener.accept().await;
        Some((Message::Show, Some(listener)))
    })
}

impl App {
    fn ask(&mut self, text: String) -> Task<Message> {
        let Some(assistant) = self.assistant.clone() else { return Task::none() };
        self.busy = true;
        self.written = None;
        self.lines.push(Line::You(text.clone()));
        let (tx, rx) = mpsc::unbounded_channel::<Message>();
        enum S {
            Start(String, mpsc::UnboundedReceiver<Message>),
            Run(mpsc::UnboundedReceiver<Message>),
        }
        let stream = futures_util::stream::unfold(S::Start(text, rx), move |s| {
            let (assistant, tx) = (assistant.clone(), tx.clone());
            async move {
                let mut rx = match s {
                    S::Run(rx) => rx,
                    S::Start(text, rx) => {
                        tokio::spawn(async move {
                            let ui = Window { tx: tx.clone() };
                            let mut a = assistant.lock().await;
                            let r = a.send(&text, &ui).await.map_err(|e| format!("{e:#}"));
                            let _ = tx.send(Message::Reply(r, a.spent()));
                        });
                        rx
                    }
                };
                let m = rx.recv().await?;
                let done = matches!(m, Message::Reply(..));
                // After the reply the stream ends (the sender side is dropped too).
                Some((m, if done { S::Run(mpsc::unbounded_channel().1) } else { S::Run(rx) }))
            }
        });
        Task::batch([cosmic::task::stream(stream), self.to_end()])
    }

    /// Keep the newest message in view.
    fn to_end(&self) -> Task<Message> {
        snap_to(self.scroll.clone(), RelativeOffset::END.into())
    }

    fn bubble<'a>(&'a self, line: &'a Line) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();
        let (text, mine, class): (String, bool, cosmic::theme::Container) = match line {
            Line::You(t) => (t.clone(), true, cosmic::theme::Container::custom(yours)),
            Line::Assistant(t) => (plain(t), false, cosmic::theme::Container::Primary),
            Line::Note(t) => {
                return widget::row::with_capacity(2)
                    .push(widget::icon::from_name("emblem-system-symbolic").size(14).icon())
                    .push(widget::text::caption(t.as_str()))
                    .spacing(spacing.space_xxs)
                    .align_y(Alignment::Center)
                    .into();
            }
            Line::Error(t) => (t.clone(), false, cosmic::theme::Container::Primary),
        };
        let b = widget::container(widget::text::body(text)).padding(spacing.space_s).class(class).max_width(560);
        let row = widget::row::with_capacity(2);
        if mine {
            row.push(widget::space::horizontal()).push(b).into()
        } else {
            row.push(b).push(widget::space::horizontal()).into()
        }
    }

    fn status_line(&self) -> String {
        match self.settings.as_ref().and_then(|s| s.as_ref().ok()) {
            // Off or not set up: the message above says so.
            Some(_) if self.assistant.is_none() => String::new(),
            Some(s) => {
                let mode = match s.effective_mode() {
                    AssistantMode::Ask => "asks first",
                    AssistantMode::Auto => "acts on its own",
                };
                let full = if s.full_access { " · full access" } else { "" };
                let cost = if self.spent > 0.0 { format!(" · ${:.4}", self.spent) } else { String::new() };
                format!("{} · {mode}{full}{cost}", model_name(&s.model))
            }
            None => String::new(),
        }
    }
}

impl cosmic::Application for App {
    type Executor = executor::Default;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = "org.iliada.Assistant";

    fn core(&self) -> &Core {
        &self.core
    }
    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: ()) -> (Self, Task<Message>) {
        let mut app = App {
            core,
            settings: None,
            assistant: None,
            lines: Vec::new(),
            input: String::new(),
            busy: false,
            approval: None,
            writing: None,
            written: None,
            spent: 0.0,
            scroll: widget::Id::unique(),
            field: widget::Id::unique(),
        };
        // A panel, not a document window: no title bar.
        app.core.window.show_headerbar = false;
        // The panel draws its own background, see-through when the desktop
        // blurs what is behind windows (frosted glass, on by default).
        app.core.window.content_container = false;
        let title = match app.core.main_window_id() {
            Some(id) => app.set_window_title("Assistant".into(), id),
            None => Task::none(),
        };
        let load = cosmic::task::future(async {
            Message::Ready(client::status().await.map(|s| s.assistant).map_err(|e| format!("{e:#}")))
        });
        let focus = widget::text_input::focus(app.field.clone());
        (app, Task::batch([title, load, focus]))
    }

    fn subscription(&self) -> Subscription<Message> {
        let keys = cosmic::iced::event::listen_with(|event, _status, _window| match event {
            cosmic::iced::Event::Keyboard(cosmic::iced::keyboard::Event::KeyPressed {
                key: Key::Named(Named::Escape),
                ..
            }) => Some(Message::Close),
            _ => None,
        });
        Subscription::batch([keys, Subscription::run(show_requests)])
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Ready(s) => {
                if let Ok(settings) = &s
                    && settings.enabled
                {
                    match iliada_assistant::Assistant::new(settings) {
                        Ok(a) => self.assistant = Some(Arc::new(Mutex::new(a))),
                        Err(e) => self.lines.push(Line::Error(format!("{e:#}"))),
                    }
                }
                self.settings = Some(s);
            }
            Message::Input(t) => self.input = t,
            Message::Send => {
                let text = self.input.trim().to_owned();
                if !text.is_empty() && !self.busy && self.assistant.is_some() {
                    self.input.clear();
                    return self.ask(text);
                }
            }
            Message::Suggest(text) => {
                if !self.busy && self.assistant.is_some() {
                    return self.ask(text.to_owned());
                }
            }
            Message::Note(n) => {
                // "✓ Install Firefox" replaces "Install Firefox…".
                if let Some(Line::Note(last)) = self.lines.last()
                    && let Some(action) = last.strip_suffix('…')
                    && (n.strip_prefix("✓ ") == Some(action)
                        || n.strip_prefix("✗ ").is_some_and(|rest| rest.starts_with(action)))
                {
                    self.lines.pop();
                }
                self.lines.push(Line::Note(n));
                return self.to_end();
            }
            Message::Piece(piece) => {
                let i = match self.writing {
                    Some(i) => i,
                    None => {
                        self.lines.push(Line::Assistant(String::new()));
                        self.writing = Some(self.lines.len() - 1);
                        self.lines.len() - 1
                    }
                };
                if let Some(Line::Assistant(t)) = self.lines.get_mut(i) {
                    t.push_str(if t.is_empty() { piece.trim_start() } else { &piece });
                }
                return self.to_end();
            }
            Message::PieceEnd => {
                if let Some(i) = self.writing.take() {
                    if i + 1 == self.lines.len() && matches!(&self.lines[i], Line::Assistant(t) if t.trim().is_empty())
                    {
                        self.lines.pop();
                    } else {
                        self.written = Some(i);
                    }
                }
            }
            Message::Approve(action, pending) => {
                self.approval = Some((action, pending));
                return self.to_end();
            }
            Message::Answer(allow) => {
                if let Some((action, pending)) = self.approval.take() {
                    if let Some(tx) = pending.lock().expect("approval lock").take() {
                        let _ = tx.send(allow);
                    }
                    if !allow {
                        self.lines.push(Line::Note(format!("You didn't allow: {action}")));
                    }
                }
                return self.to_end();
            }
            Message::Reply(r, spent) => {
                self.busy = false;
                self.writing = None;
                match r {
                    // Already written out as it came: the whole answer replaces it.
                    Ok(t) if self.written.is_some_and(|i| i + 1 == self.lines.len()) => {
                        if let Some(last) = self.lines.last_mut() {
                            *last = Line::Assistant(t);
                        }
                    }
                    Ok(t) if t.is_empty() => {}
                    Ok(t) => self.lines.push(Line::Assistant(t)),
                    Err(e) => self.lines.push(Line::Error(e)),
                }
                self.spent = spent;
                return Task::batch([self.to_end(), widget::text_input::focus(self.field.clone())]);
            }
            Message::OpenSettings => {
                let _ = std::process::Command::new("iliada-settings").spawn();
            }
            Message::Close => {
                // Never in the middle of an action: it would be cut off.
                if !self.busy {
                    if let Some(p) = socket_path() {
                        let _ = std::fs::remove_file(p);
                    }
                    std::process::exit(0);
                }
            }
            Message::Show => {
                if let Some(id) = self.core.main_window_id() {
                    return Task::batch([
                        cosmic::iced::window::gain_focus(id),
                        widget::text_input::focus(self.field.clone()),
                    ]);
                }
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();
        // The ask field: the Iliada helmet, then a large input.
        let field: Element<_> = if self.assistant.is_some() {
            widget::text_input("Ask Iliada…", &self.input)
                .id(self.field.clone())
                .on_input(Message::Input)
                .on_submit(|_| Message::Send)
                .size(20)
                .padding([spacing.space_s, spacing.space_m])
                .width(Length::Fill)
                .into()
        } else {
            let off = matches!(&self.settings, Some(Ok(s)) if !s.enabled);
            widget::row::with_capacity(2)
                .push(widget::text::body(if off { "The assistant is off." } else { "The assistant isn't ready yet." }))
                .push(widget::button::suggested("Set it up in Iliada Settings").on_press(Message::OpenSettings))
                .spacing(spacing.space_s)
                .align_y(Alignment::Center)
                .into()
        };
        let top = widget::row::with_capacity(2)
            .push(widget::icon::from_name("iliada").size(40).icon())
            .push(field)
            .spacing(spacing.space_s)
            .align_y(Alignment::Center);

        let mut col = widget::column::with_capacity(self.lines.len() + 3).spacing(spacing.space_s);
        if self.lines.is_empty() && self.assistant.is_some() {
            col = col.push(widget::text::body(
                "Ask in your own words. I can install and remove apps, change settings, turn services on, \
                 send email and more.",
            ));
            let chips: Vec<Element<_>> =
                SUGGESTIONS.iter().map(|s| widget::button::standard(*s).on_press(Message::Suggest(s)).into()).collect();
            col = col.push(widget::flex_row(chips).row_spacing(spacing.space_xs).column_spacing(spacing.space_xs));
        }
        for l in &self.lines {
            col = col.push(self.bubble(l));
        }
        if let Some((action, _)) = &self.approval {
            col = col.push(
                widget::container(
                    widget::column::with_capacity(2)
                        .push(widget::text::heading(format!("May I? {action}")))
                        .push(
                            widget::row::with_capacity(2)
                                .push(widget::button::standard("Don't Allow").on_press(Message::Answer(false)))
                                .push(widget::button::suggested("Allow").on_press(Message::Answer(true)))
                                .spacing(spacing.space_s),
                        )
                        .spacing(spacing.space_s),
                )
                .padding(spacing.space_s)
                .class(cosmic::theme::Container::Card)
                .width(Length::Fill),
            );
        } else if self.busy && self.writing.is_none() {
            col = col.push(
                widget::row::with_capacity(2)
                    .push(widget::progress_bar::circular::Circular::new().size(18.0))
                    .push(widget::text::caption("Working on it…"))
                    .spacing(spacing.space_xs)
                    .align_y(Alignment::Center),
            );
        }

        let footer = widget::row::with_capacity(3)
            .push(widget::text::caption(self.status_line()))
            .push(widget::space::horizontal())
            .push(widget::text::caption(if self.busy { "Working…" } else { "Enter to ask · Esc to close" }));

        let body = widget::column::with_capacity(3)
            .push(top)
            .push(widget::scrollable(col).id(self.scroll.clone()).height(Length::Fill))
            .push(footer)
            .spacing(spacing.space_m)
            .padding(spacing.space_l);
        widget::container(body).class(cosmic::theme::Container::custom(frosted)).into()
    }
}

/// The panel's background: the window color, see-through over the blurred
/// desktop when frosted glass is on, solid when it is off.
fn frosted(theme: &cosmic::Theme) -> widget::container::Style {
    let c = theme.cosmic();
    let bg: cosmic::iced::Color = c.background(theme.transparent).base.into();
    widget::container::Style {
        background: Some(cosmic::iced::Background::Color(bg)),
        border: cosmic::iced::Border { radius: c.radius_m().into(), width: 1.0, color: c.bg_divider().into() },
        text_color: Some(c.on_bg_color().into()),
        icon_color: Some(c.on_bg_color().into()),
        ..Default::default()
    }
}

/// What you asked: tinted with the accent color.
fn yours(theme: &cosmic::Theme) -> widget::container::Style {
    let c = theme.cosmic();
    let mut bg: cosmic::iced::Color = c.accent_color().into();
    bg.a = 0.3;
    widget::container::Style {
        background: Some(cosmic::iced::Background::Color(bg)),
        border: cosmic::iced::Border { radius: c.radius_m().into(), ..Default::default() },
        text_color: Some(c.on_bg_color().into()),
        ..Default::default()
    }
}

/// Models sometimes format answers anyway: "**Install apps**" shows as
/// "Install apps", "## Steps" as "Steps".
fn plain(text: &str) -> String {
    text.lines()
        .map(|l| {
            let trimmed = l.trim_start();
            let l = if trimmed.starts_with('#') { trimmed.trim_start_matches('#').trim_start() } else { l };
            l.replace("**", "").replace("__", "")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    #[test]
    fn formatting_marks_are_dropped() {
        assert_eq!(
            super::plain("## Steps\n1. **Install apps** from the Store\n__done__"),
            "Steps\n1. Install apps from the Store\ndone"
        );
        assert_eq!(super::plain("2 * 3 = 6"), "2 * 3 = 6");
    }
}
