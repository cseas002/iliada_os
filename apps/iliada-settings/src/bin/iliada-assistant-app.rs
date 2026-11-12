//! Assistant: ask Iliada to do things ("install Firefox and turn on SSH").
//!
//! Two faces of one program:
//! * **The app** (`iliada-assistant-app`, "Assistant" in Applications): a
//!   window with your conversations in the sidebar (each one saved, and
//!   continued where you left it) and **Activity**: everything the assistant
//!   did, with Undo where an action can be reversed, and every change made
//!   to this computer from anywhere.
//! * **The quick panel** (`--panel`, Alt+Space anywhere): a large "Ask
//!   Iliada…" field and the answer below it; Esc closes. Its conversations
//!   are saved too, and "Open in Assistant" continues one in the app.
//!
//! In Ask mode each action appears as a card with Allow / Don't Allow; in
//! Auto mode actions run and are listed as they happen. One window of each
//! kind runs: starting it again brings it forward.

use std::path::PathBuf;
use std::sync::Arc;

use cosmic::app::{Settings, Task};
use cosmic::iced::keyboard::{Key, key::Named};
use cosmic::iced::widget::scrollable::{RelativeOffset, snap_to};
use cosmic::iced::{Alignment, Length, Size, Subscription};
use cosmic::widget::{self, nav_bar, settings};
use cosmic::{ApplicationExt, Core, Element, executor};
use iliada_assistant::history::{Conversation, Line, LineKind, Summary};
use iliada_assistant::{Activity, Assistant};
use iliada_system::client;
use iliada_system::protocol::{AssistantMode, AssistantSettings, model_name};
use serde_json::Value;
use tokio::sync::{Mutex, mpsc, oneshot};

/// Things people commonly ask, shown before the first question.
const SUGGESTIONS: [&str; 6] = [
    "Switch to dark mode",
    "Install Firefox",
    "Set the volume to 40%",
    "What's taking up space on my disk?",
    "Turn on the SSH server",
    "What changed on this computer this week?",
];

#[derive(Debug, Clone, Default)]
struct Flags {
    panel: bool,
    open: Option<String>,
}

fn parse_flags() -> Flags {
    let mut f = Flags::default();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--panel" => f.panel = true,
            "--open" => f.open = args.next(),
            _ => {}
        }
    }
    f
}

/// The running window listens here; a later start asks it to come forward
/// (and to open a conversation).
fn socket_path(panel: bool) -> Option<PathBuf> {
    let name = if panel { "iliada-assistant.sock" } else { "iliada-assistant-app.sock" };
    Some(PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join(name))
}

/// The socket this window listens on, claimed before the window opens so a
/// quick second start can't open a second one.
static LISTENER: std::sync::OnceLock<std::os::unix::net::UnixListener> = std::sync::OnceLock::new();

/// `false`: one is open already (and was asked to come forward).
fn claim_instance(flags: &Flags) -> bool {
    use std::io::Write;
    let Some(path) = socket_path(flags.panel) else { return true };
    for _ in 0..2 {
        match std::os::unix::net::UnixListener::bind(&path) {
            Ok(l) => {
                let _ = l.set_nonblocking(true);
                let _ = LISTENER.set(l);
                return true;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                if let Ok(mut s) = std::os::unix::net::UnixStream::connect(&path) {
                    let _ = writeln!(s, "{}", flags.open.as_deref().unwrap_or(""));
                    return false;
                }
                // Left over from one that was stopped: take it over.
                let _ = std::fs::remove_file(&path);
            }
            Err(_) => return true,
        }
    }
    true
}

fn main() -> cosmic::iced::Result {
    let flags = parse_flags();
    if !claim_instance(&flags) {
        return Ok(());
    }
    let size = if flags.panel { Size::new(760.0, 560.0) } else { Size::new(1040.0, 720.0) };
    cosmic::app::run::<App>(Settings::default().size(size), flags)
}

/// The approval the assistant is waiting for.
type Pending = Arc<std::sync::Mutex<Option<oneshot::Sender<bool>>>>;

/// What a sidebar entry opens.
#[derive(Debug, Clone, PartialEq)]
enum Nav {
    New,
    Activity,
    Conversation(String),
}

#[derive(Debug, Clone, PartialEq)]
enum Page {
    Chat,
    Activity,
}

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
    /// The answer, what this session has cost, and the conversation so far.
    Reply(Result<String, String>, f64, Vec<Value>),
    OpenSettings,
    NewConversation,
    Open(String),
    DeleteConversation,
    OpenInApp,
    ShowActivity,
    Loaded(Vec<Activity>, Result<Vec<iliada_system::Change>, String>),
    Undo(usize),
    /// The undo's outcome, and the conversation the action came from.
    Undone(Result<String, String>, Option<String>),
    Dismiss,
    /// Esc (the panel).
    Close,
    /// Started again: come forward (and open this conversation).
    Show(Option<String>),
}

struct App {
    core: Core,
    panel: bool,
    settings: Option<Result<AssistantSettings, String>>,
    assistant: Option<Arc<Mutex<Assistant>>>,
    conversation: Conversation,
    /// What the conversation cost before this session.
    spent_before: f64,
    input: String,
    busy: bool,
    approval: Option<(String, Pending)>,
    /// The answer being written right now (its line).
    writing: Option<usize>,
    /// The line the latest answer was written into.
    written: Option<usize>,
    page: Page,
    nav: nav_bar::Model,
    conversations: Vec<Summary>,
    activity: Vec<Activity>,
    changes: Option<Result<Vec<iliada_system::Change>, String>>,
    undoing: Option<usize>,
    notice: Option<String>,
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

/// Accepts "come forward" requests from later starts (with a conversation
/// to open, or an empty line).
fn show_requests() -> impl futures_util::Stream<Item = Message> {
    use tokio::io::AsyncBufReadExt;
    futures_util::stream::unfold(None::<tokio::net::UnixListener>, |listener| async move {
        let listener = match listener {
            Some(l) => l,
            None => match LISTENER.get().and_then(|l| l.try_clone().ok()).map(tokio::net::UnixListener::from_std) {
                Some(Ok(l)) => l,
                // No socket: never yield (a second start opens a second window).
                _ => std::future::pending().await,
            },
        };
        let mut open = None;
        if let Ok((stream, _)) = listener.accept().await {
            let mut line = String::new();
            let mut reader = tokio::io::BufReader::new(stream);
            let read = reader.read_line(&mut line);
            let _ = tokio::time::timeout(std::time::Duration::from_secs(1), read).await;
            open = Some(line.trim().to_owned()).filter(|l| !l.is_empty());
        }
        Some((Message::Show(open), Some(listener)))
    })
}

fn ago(t: u64) -> String {
    match iliada_assistant::history::now().saturating_sub(t) {
        s if s < 90 => "just now".into(),
        s if s < 90 * 60 => format!("{} minutes ago", s / 60),
        s if s < 36 * 3600 => format!("{} hours ago", s / 3600),
        s => format!("{} days ago", s / 86400),
    }
}

impl App {
    fn settings_ok(&self) -> Option<&AssistantSettings> {
        self.settings.as_ref().and_then(|s| s.as_ref().ok())
    }

    /// A fresh assistant for the current conversation (continuing it).
    fn connect(&mut self) {
        self.assistant = None;
        let Some(settings) = self.settings_ok().filter(|s| s.enabled).cloned() else { return };
        match Assistant::new(&settings) {
            Ok(mut a) => {
                a.set_conversation(&self.conversation.id);
                if !self.conversation.messages.is_empty() {
                    a.restore(self.conversation.messages.clone(), &self.conversation.model);
                }
                self.assistant = Some(Arc::new(Mutex::new(a)));
            }
            Err(e) => self.notice = Some(format!("{e:#}")),
        }
    }

    fn model(&self) -> String {
        self.settings_ok().map(|s| s.model.clone()).unwrap_or_default()
    }

    fn start_conversation(&mut self, c: Conversation) -> Task<Message> {
        self.spent_before = c.spent;
        self.conversation = c;
        self.approval = None;
        self.writing = None;
        self.written = None;
        self.page = Page::Chat;
        self.connect();
        self.rebuild_nav();
        Task::batch([self.to_end(), widget::text_input::focus(self.field.clone())])
    }

    fn save(&mut self) {
        if self.conversation.lines.is_empty() {
            return;
        }
        if let Err(e) = self.conversation.save() {
            self.notice = Some(format!("This conversation could not be saved: {e:#}"));
        }
        if !self.panel {
            self.conversations = Conversation::list();
            self.rebuild_nav();
        }
    }

    fn rebuild_nav(&mut self) {
        if self.panel {
            return;
        }
        self.nav.clear();
        let new = self
            .nav
            .insert()
            .text("New conversation")
            .icon(widget::icon::from_name("list-add-symbolic"))
            .data(Nav::New);
        if self.page == Page::Chat && self.conversation.lines.is_empty() {
            new.activate();
        }
        let act = self
            .nav
            .insert()
            .text("Activity")
            .icon(widget::icon::from_name("document-open-recent-symbolic"))
            .data(Nav::Activity);
        if self.page == Page::Activity {
            act.activate();
        }
        for (i, c) in self.conversations.iter().enumerate() {
            let e = self.nav.insert().text(c.title.clone()).data(Nav::Conversation(c.id.clone())).divider_above(i == 0);
            if self.page == Page::Chat && c.id == self.conversation.id {
                e.activate();
            }
        }
    }

    fn ask(&mut self, text: String) -> Task<Message> {
        let Some(assistant) = self.assistant.clone() else { return Task::none() };
        self.busy = true;
        self.written = None;
        self.notice = None;
        self.conversation.lines.push(Line::new(LineKind::You, text.clone()));
        // What happened outside the conversation (an undo) goes to the model
        // with the question; the transcript already shows it as a note.
        let pending = std::mem::take(&mut self.conversation.pending);
        let text = if pending.is_empty() {
            text
        } else {
            format!("(Since your last answer: {}.)\n\n{text}", pending.join("; "))
        };
        self.save();
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
                            let _ = tx.send(Message::Reply(r, a.spent(), a.messages().to_vec()));
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

    fn load_activity(&self) -> Task<Message> {
        cosmic::task::future(async {
            let changes = tokio::task::spawn_blocking(|| iliada_system::changes(30).map_err(|e| format!("{e:#}")))
                .await
                .unwrap_or_else(|e| Err(e.to_string()));
            Message::Loaded(iliada_assistant::activity(50), changes)
        })
    }

    fn bubble<'a>(&'a self, line: &'a Line) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();
        let (text, mine, class): (String, bool, cosmic::theme::Container) = match line.kind {
            LineKind::You => (line.text.clone(), true, cosmic::theme::Container::custom(yours)),
            LineKind::Assistant => (plain(&line.text), false, cosmic::theme::Container::Primary),
            LineKind::Note => {
                return widget::row::with_capacity(2)
                    .push(widget::icon::from_name("emblem-system-symbolic").size(14).icon())
                    .push(widget::text::caption(line.text.as_str()))
                    .spacing(spacing.space_xxs)
                    .align_y(Alignment::Center)
                    .into();
            }
            LineKind::Error => (line.text.clone(), false, cosmic::theme::Container::Primary),
        };
        let b = widget::container(widget::text::body(text)).padding(spacing.space_s).class(class).max_width(620);
        let row = widget::row::with_capacity(2);
        if mine {
            row.push(widget::space::horizontal()).push(b).into()
        } else {
            row.push(b).push(widget::space::horizontal()).into()
        }
    }

    fn status_line(&self) -> String {
        match self.settings_ok() {
            // Off or not set up: the message above says so.
            Some(_) if self.assistant.is_none() => String::new(),
            Some(s) => {
                let mode = match s.effective_mode() {
                    AssistantMode::Ask => "asks first",
                    AssistantMode::Auto => "acts on its own",
                };
                let full = if s.full_access { " · full access" } else { "" };
                let spent = self.conversation.spent;
                let cost = if spent > 0.0 { format!(" · ${spent:.4}") } else { String::new() };
                format!("{} · {mode}{full}{cost}", model_name(&s.model))
            }
            None => String::new(),
        }
    }

    /// Off or not set up: say so, with the way to fix it.
    fn not_ready(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();
        let off = matches!(&self.settings, Some(Ok(s)) if !s.enabled);
        let text = match (&self.notice, off) {
            (_, true) => "The assistant is off.".to_owned(),
            (Some(n), false) => n.clone(),
            (None, false) => "The assistant isn't ready yet.".to_owned(),
        };
        widget::row::with_capacity(2)
            .push(widget::text::body(text))
            .push(widget::button::suggested("Set it up in Iliada Settings").on_press(Message::OpenSettings))
            .spacing(spacing.space_s)
            .align_y(Alignment::Center)
            .into()
    }

    /// The conversation: welcome and suggestions when empty, the lines, an
    /// approval card or "Working on it…".
    fn transcript(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();
        let lines = &self.conversation.lines;
        let mut col = widget::column::with_capacity(lines.len() + 3).spacing(spacing.space_s);
        if lines.is_empty() && self.assistant.is_some() {
            if !self.panel {
                col = col.push(
                    widget::column::with_capacity(2)
                        .push(widget::icon::from_name("iliada").size(72).icon())
                        .push(widget::text::title3("What can I do for you?"))
                        .spacing(spacing.space_s)
                        .align_x(Alignment::Center)
                        .width(Length::Fill),
                );
            }
            col = col.push(widget::text::body(
                "Ask in your own words. I can install and remove apps and software, change settings, turn \
                 services on, send email and more. Everything I do is listed in Activity, and most of it \
                 can be undone.",
            ));
            let chips: Vec<Element<_>> =
                SUGGESTIONS.iter().map(|s| widget::button::standard(*s).on_press(Message::Suggest(s)).into()).collect();
            col = col.push(widget::flex_row(chips).row_spacing(spacing.space_xs).column_spacing(spacing.space_xs));
        }
        for l in lines {
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
        widget::scrollable(col).id(self.scroll.clone()).height(Length::Fill).into()
    }

    fn ask_field(&self, size: u16) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();
        let placeholder = if self.conversation.lines.is_empty() { "Ask Iliada…" } else { "Reply…" };
        widget::text_input(placeholder, &self.input)
            .id(self.field.clone())
            .on_input(Message::Input)
            .on_submit(|_| Message::Send)
            .size(size)
            .padding([spacing.space_s, spacing.space_m])
            .width(Length::Fill)
            .into()
    }

    fn panel_view(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();
        let field = if self.assistant.is_some() { self.ask_field(20) } else { self.not_ready() };
        let top = widget::row::with_capacity(2)
            .push(widget::icon::from_name("iliada").size(40).icon())
            .push(field)
            .spacing(spacing.space_s)
            .align_y(Alignment::Center);
        let mut footer = widget::row::with_capacity(4)
            .push(widget::text::caption(self.status_line()))
            .push(widget::space::horizontal())
            .spacing(spacing.space_s)
            .align_y(Alignment::Center);
        if !self.conversation.lines.is_empty() && !self.busy {
            footer = footer.push(widget::button::text("Open in Assistant").on_press(Message::OpenInApp));
        }
        footer =
            footer.push(widget::text::caption(if self.busy { "Working…" } else { "Enter to ask · Esc to close" }));
        let body = widget::column::with_capacity(3)
            .push(top)
            .push(self.transcript())
            .push(footer)
            .spacing(spacing.space_m)
            .padding(spacing.space_l);
        widget::container(body).class(cosmic::theme::Container::custom(frosted)).into()
    }

    fn chat_view(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();
        let bottom: Element<_> = if self.assistant.is_some() {
            widget::row::with_capacity(2)
                .push(self.ask_field(16))
                .push(
                    widget::button::icon(widget::icon::from_name("mail-send-symbolic"))
                        .on_press_maybe((!self.busy && !self.input.trim().is_empty()).then_some(Message::Send)),
                )
                .spacing(spacing.space_xs)
                .align_y(Alignment::Center)
                .into()
        } else {
            self.not_ready()
        };
        let mut col = widget::column::with_capacity(4).spacing(spacing.space_s);
        if let Some(n) = self.notice.as_ref().filter(|_| self.assistant.is_some()) {
            col = col.push(self.banner(n));
        }
        col.push(self.transcript())
            .push(bottom)
            .push(widget::text::caption(self.status_line()))
            .padding([spacing.space_s, spacing.space_l])
            .max_width(900)
            .into()
    }

    fn banner<'a>(&self, text: &'a str) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();
        widget::container(
            widget::row::with_capacity(2)
                .push(widget::text::body(text).width(Length::Fill))
                .push(widget::button::icon(widget::icon::from_name("window-close-symbolic")).on_press(Message::Dismiss))
                .align_y(Alignment::Center),
        )
        .padding([spacing.space_xxs, spacing.space_s])
        .class(cosmic::theme::Container::Card)
        .width(Length::Fill)
        .into()
    }

    fn activity_view(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();
        let mut items: Vec<Element<_>> = Vec::new();
        if let Some(n) = &self.notice {
            items.push(self.banner(n));
        }
        let mut done = settings::section().title("What the assistant did");
        if self.activity.is_empty() {
            done = done.add(settings::item_row(vec![
                widget::text::body("Nothing yet. Actions the assistant takes appear here.").into(),
            ]));
        }
        for (i, a) in self.activity.iter().enumerate() {
            let outcome = match a.outcome.as_str() {
                "done" => "Done",
                "failed" => "Failed",
                "declined" => "You didn't allow it",
                "undone" => "Undone",
                other => other,
            };
            let first = a.action.lines().next().unwrap_or("").to_owned();
            let mut controls = widget::row::with_capacity(2).spacing(spacing.space_xs).align_y(Alignment::Center);
            if let Some(id) = a.conversation.clone().filter(|id| self.conversations.iter().any(|c| &c.id == id)) {
                controls = controls.push(widget::button::text("Conversation").on_press(Message::Open(id)));
            }
            if a.can_undo {
                let free = self.undoing.is_none() && !self.busy;
                let label = if self.undoing == Some(i) { "Undoing…" } else { "Undo" };
                controls =
                    controls.push(widget::button::standard(label).on_press_maybe(free.then_some(Message::Undo(i))));
            }
            done = done.add(
                settings::item::builder(first).description(format!("{} · {outcome}", ago(a.time))).control(controls),
            );
        }
        items.push(done.into());
        let mut changes = settings::section().title("Changes to this computer");
        match &self.changes {
            None => changes = changes.add(settings::item_row(vec![widget::text::body("Loading…").into()])),
            Some(Err(e)) => changes = changes.add(settings::item_row(vec![widget::text::body(e.clone()).into()])),
            Some(Ok(list)) => {
                for c in list {
                    changes = changes.add(
                        settings::item::builder(c.what.clone())
                            .description(ago(c.time))
                            .control(widget::text::body("")),
                    );
                }
            }
        }
        items.push(
            widget::column::with_capacity(2)
                .push(widget::text::caption(
                    "Everything that changed the system, made in Settings, the Store, the terminal or by the assistant.",
                ))
                .push(changes)
                .spacing(spacing.space_xxs)
                .into(),
        );
        widget::scrollable(settings::view_column(items).padding([spacing.space_s, spacing.space_l])).into()
    }
}

impl cosmic::Application for App {
    type Executor = executor::Default;
    type Flags = Flags;
    type Message = Message;
    const APP_ID: &'static str = "org.iliada.Assistant";

    fn core(&self) -> &Core {
        &self.core
    }
    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, flags: Flags) -> (Self, Task<Message>) {
        let conversation =
            flags.open.as_deref().and_then(|id| Conversation::load(id).ok()).unwrap_or_else(|| Conversation::new(""));
        let mut app = App {
            core,
            panel: flags.panel,
            settings: None,
            assistant: None,
            spent_before: conversation.spent,
            conversation,
            input: String::new(),
            busy: false,
            approval: None,
            writing: None,
            written: None,
            page: Page::Chat,
            nav: nav_bar::Model::default(),
            conversations: if flags.panel { Vec::new() } else { Conversation::list() },
            activity: Vec::new(),
            changes: None,
            undoing: None,
            notice: None,
            scroll: widget::Id::unique(),
            field: widget::Id::unique(),
        };
        if app.panel {
            // A panel, not a document window: no title bar, and its own
            // see-through background (frosted glass, on by default).
            app.core.window.show_headerbar = false;
            app.core.window.content_container = false;
        }
        app.rebuild_nav();
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

    fn nav_model(&self) -> Option<&nav_bar::Model> {
        (!self.panel).then_some(&self.nav)
    }

    fn on_nav_select(&mut self, id: nav_bar::Id) -> Task<Message> {
        match self.nav.data::<Nav>(id).cloned() {
            Some(Nav::New) => self.update(Message::NewConversation),
            Some(Nav::Activity) => self.update(Message::ShowActivity),
            Some(Nav::Conversation(c)) => self.update(Message::Open(c)),
            None => Task::none(),
        }
    }

    fn header_end(&self) -> Vec<Element<'_, Message>> {
        if self.panel || self.page != Page::Chat || self.conversation.lines.is_empty() {
            return Vec::new();
        }
        vec![
            widget::tooltip(
                widget::button::icon(widget::icon::from_name("edit-delete-symbolic"))
                    .on_press_maybe((!self.busy).then_some(Message::DeleteConversation)),
                widget::text::body("Delete this conversation"),
                widget::tooltip::Position::Bottom,
            )
            .into(),
        ]
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![Subscription::run(show_requests)];
        if self.panel {
            subs.push(cosmic::iced::event::listen_with(|event, _status, _window| match event {
                cosmic::iced::Event::Keyboard(cosmic::iced::keyboard::Event::KeyPressed {
                    key: Key::Named(Named::Escape),
                    ..
                }) => Some(Message::Close),
                _ => None,
            }));
        }
        Subscription::batch(subs)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Ready(s) => {
                self.settings = Some(s);
                if self.conversation.model.is_empty() {
                    self.conversation.model = self.model();
                }
                self.connect();
                return self.to_end();
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
                let lines = &mut self.conversation.lines;
                // "✓ Install Firefox" replaces "Install Firefox…".
                if let Some(last) = lines.last().filter(|l| l.kind == LineKind::Note)
                    && let Some(action) = last.text.strip_suffix('…')
                    && (n.strip_prefix("✓ ") == Some(action)
                        || n.strip_prefix("✗ ").is_some_and(|rest| rest.starts_with(action)))
                {
                    lines.pop();
                }
                lines.push(Line::new(LineKind::Note, n));
                return self.to_end();
            }
            Message::Piece(piece) => {
                let lines = &mut self.conversation.lines;
                let i = match self.writing {
                    Some(i) => i,
                    None => {
                        lines.push(Line::new(LineKind::Assistant, ""));
                        self.writing = Some(lines.len() - 1);
                        lines.len() - 1
                    }
                };
                if let Some(l) = lines.get_mut(i) {
                    l.text.push_str(if l.text.is_empty() { piece.trim_start() } else { &piece });
                }
                return self.to_end();
            }
            Message::PieceEnd => {
                let lines = &mut self.conversation.lines;
                if let Some(i) = self.writing.take() {
                    if i + 1 == lines.len() && lines[i].kind == LineKind::Assistant && lines[i].text.trim().is_empty() {
                        lines.pop();
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
                        self.conversation.lines.push(Line::new(LineKind::Note, format!("You didn't allow: {action}")));
                    }
                }
                return self.to_end();
            }
            Message::Reply(r, spent, messages) => {
                self.busy = false;
                self.writing = None;
                let lines = &mut self.conversation.lines;
                match r {
                    // Already written out as it came: the whole answer replaces it.
                    Ok(t) if self.written.is_some_and(|i| i + 1 == lines.len()) => {
                        if let Some(last) = lines.last_mut() {
                            *last = Line::new(LineKind::Assistant, t);
                        }
                    }
                    Ok(t) if t.is_empty() => {}
                    Ok(t) => lines.push(Line::new(LineKind::Assistant, t)),
                    Err(e) => lines.push(Line::new(LineKind::Error, e)),
                }
                self.conversation.messages = messages;
                self.conversation.spent = self.spent_before + spent;
                self.conversation.model = self.model();
                self.save();
                return Task::batch([self.to_end(), widget::text_input::focus(self.field.clone())]);
            }
            Message::OpenSettings => {
                let _ = std::process::Command::new("iliada-settings").spawn();
            }
            Message::NewConversation => {
                if self.busy {
                    self.rebuild_nav();
                    return Task::none();
                }
                return self.start_conversation(Conversation::new(&self.model()));
            }
            Message::Open(id) => {
                if self.busy {
                    self.notice = Some("Wait for this answer to finish first.".into());
                    self.rebuild_nav();
                    return Task::none();
                }
                if id == self.conversation.id && self.page == Page::Chat {
                    return Task::none();
                }
                match Conversation::load(&id) {
                    Ok(c) => return self.start_conversation(c),
                    Err(e) => self.notice = Some(format!("{e:#}")),
                }
            }
            Message::DeleteConversation => {
                if !self.busy {
                    let _ = Conversation::delete(&self.conversation.id);
                    self.conversations = Conversation::list();
                    return self.start_conversation(Conversation::new(&self.model()));
                }
            }
            Message::OpenInApp => {
                self.save();
                let _ =
                    std::process::Command::new("iliada-assistant-app").args(["--open", &self.conversation.id]).spawn();
                return self.update(Message::Close);
            }
            Message::ShowActivity => {
                self.page = Page::Activity;
                self.notice = None;
                self.rebuild_nav();
                return self.load_activity();
            }
            Message::Loaded(activity, changes) => {
                self.activity = activity;
                self.changes = Some(changes);
            }
            Message::Undo(i) => {
                if let Some(a) = self.activity.get(i).cloned() {
                    self.undoing = Some(i);
                    return cosmic::task::future(async move {
                        let r = iliada_assistant::undo(&a).await.map_err(|e| format!("{e:#}"));
                        Message::Undone(r, a.conversation.clone())
                    });
                }
            }
            Message::Undone(r, from) => {
                // The open conversation got a note about it: show it, and
                // keep it when this conversation is saved next.
                if from.as_deref() == Some(self.conversation.id.as_str())
                    && let Ok(c) = Conversation::load(&self.conversation.id)
                {
                    self.conversation.lines = c.lines;
                    self.conversation.pending = c.pending;
                }
                self.undoing = None;
                self.notice = Some(match r {
                    Ok(what) => format!("Undone: {what}."),
                    Err(e) => format!("Could not undo it: {e}"),
                });
                return self.load_activity();
            }
            Message::Dismiss => self.notice = None,
            Message::Close => {
                // Never in the middle of an action: it would be cut off.
                if !self.busy {
                    if let Some(p) = socket_path(self.panel) {
                        let _ = std::fs::remove_file(p);
                    }
                    std::process::exit(0);
                }
            }
            Message::Show(open) => {
                let mut tasks = Vec::new();
                if let Some(id) = open {
                    tasks.push(self.update(Message::Open(id)));
                }
                if let Some(id) = self.core.main_window_id() {
                    tasks.push(cosmic::iced::window::gain_focus(id));
                    tasks.push(widget::text_input::focus(self.field.clone()));
                }
                return Task::batch(tasks);
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        if self.panel {
            return self.panel_view();
        }
        let content = match self.page {
            Page::Chat => self.chat_view(),
            Page::Activity => self.activity_view(),
        };
        widget::container(content).center_x(Length::Fill).height(Length::Fill).into()
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
