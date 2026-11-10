//! Assistant: ask Iliada to do things ("install Firefox and turn on SSH").
//!
//! A chat window over `iliada-assistant`. In Ask mode each action appears
//! as a card with Allow / Don't Allow; in Auto mode actions run and are
//! listed as they happen.

use std::sync::Arc;

use cosmic::app::{Settings, Task};
use cosmic::iced::{Alignment, Length, Size};
use cosmic::widget;
use cosmic::{ApplicationExt, Core, Element, executor};
use iliada_system::client;
use iliada_system::protocol::{AssistantMode, AssistantSettings};
use tokio::sync::{Mutex, mpsc, oneshot};

fn main() -> cosmic::iced::Result {
    let settings = Settings::default().size(Size::new(720.0, 640.0));
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
    Note(String),
    Approve(String, Pending),
    Answer(bool),
    Reply(Result<String, String>),
    OpenSettings,
}

struct App {
    core: Core,
    settings: Option<Result<AssistantSettings, String>>,
    assistant: Option<Arc<Mutex<iliada_assistant::Assistant>>>,
    lines: Vec<Line>,
    input: String,
    busy: bool,
    approval: Option<(String, Pending)>,
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
}

impl App {
    fn ask(&mut self, text: String) -> Task<Message> {
        let Some(assistant) = self.assistant.clone() else { return Task::none() };
        self.busy = true;
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
                            let r = assistant.lock().await.send(&text, &ui).await.map_err(|e| format!("{e:#}"));
                            let _ = tx.send(Message::Reply(r));
                        });
                        rx
                    }
                };
                let m = rx.recv().await?;
                let done = matches!(m, Message::Reply(_));
                // After the reply the stream ends (the sender side is dropped too).
                Some((m, if done { S::Run(mpsc::unbounded_channel().1) } else { S::Run(rx) }))
            }
        });
        cosmic::task::stream(stream)
    }

    fn bubble<'a>(&'a self, line: &'a Line) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();
        let (text, mine, class): (&str, bool, cosmic::theme::Container) = match line {
            Line::You(t) => (t, true, cosmic::theme::Container::Primary),
            Line::Assistant(t) => (t, false, cosmic::theme::Container::Card),
            Line::Note(t) => return widget::text::caption(t.as_str()).into(),
            Line::Error(t) => (t, false, cosmic::theme::Container::Card),
        };
        let b = widget::container(widget::text::body(text)).padding(spacing.space_s).class(class).max_width(520);
        let row = widget::row::with_capacity(2);
        if mine {
            row.push(widget::space::horizontal()).push(b).into()
        } else {
            row.push(b).push(widget::space::horizontal()).into()
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
        let mut app =
            App { core, settings: None, assistant: None, lines: Vec::new(), input: String::new(), busy: false, approval: None };
        app.set_header_title("Assistant".into());
        let title = match app.core.main_window_id() {
            Some(id) => app.set_window_title("Assistant".into(), id),
            None => Task::none(),
        };
        let load = cosmic::task::future(async {
            Message::Ready(client::status().await.map(|s| s.assistant).map_err(|e| format!("{e:#}")))
        });
        (app, Task::batch([title, load]))
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Ready(s) => {
                if let Ok(settings) = &s {
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
            Message::Note(n) => self.lines.push(Line::Note(n)),
            Message::Approve(action, pending) => self.approval = Some((action, pending)),
            Message::Answer(allow) => {
                if let Some((action, pending)) = self.approval.take() {
                    if let Some(tx) = pending.lock().expect("approval lock").take() {
                        let _ = tx.send(allow);
                    }
                    if !allow {
                        self.lines.push(Line::Note(format!("You didn't allow: {action}")));
                    }
                }
            }
            Message::Reply(r) => {
                self.busy = false;
                self.lines.push(match r {
                    Ok(t) => Line::Assistant(t),
                    Err(e) => Line::Error(e),
                });
            }
            Message::OpenSettings => {
                let _ = std::process::Command::new("iliada-settings").spawn();
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();
        let mut col = widget::column::with_capacity(self.lines.len() + 2).spacing(spacing.space_s);
        let mode = match self.settings.as_ref().and_then(|s| s.as_ref().ok()) {
            Some(s) => match s.effective_mode() {
                AssistantMode::Ask => format!("{} · asks before every action", s.model),
                AssistantMode::Auto => format!("{} · acts on its own", s.model),
            },
            None => String::new(),
        };
        col = col.push(widget::text::caption(mode));
        if self.lines.is_empty() && self.assistant.is_some() {
            col = col.push(widget::text::body(
                "Ask me to install or remove apps, add command-line tools, turn on the SSH server or printing, \
                 update apps or Iliada, back up, or change what an app may use.",
            ));
        }
        for l in &self.lines {
            col = col.push(self.bubble(l));
        }
        if let Some((action, _)) = &self.approval {
            col = col.push(
                widget::container(
                    widget::column::with_capacity(2)
                        .push(widget::text::heading(format!("The assistant wants to: {action}")))
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
        } else if self.busy {
            col = col.push(widget::progress_bar::circular::Circular::new().size(20.0));
        }
        let input: Element<_> = if self.assistant.is_some() {
            widget::row::with_capacity(2)
                .push(
                    widget::text_input("What should I do?", &self.input)
                        .on_input(Message::Input)
                        .on_submit(|_| Message::Send)
                        .width(Length::Fill),
                )
                .push(widget::button::suggested("Send").on_press_maybe((!self.busy).then_some(Message::Send)))
                .spacing(spacing.space_s)
                .align_y(Alignment::Center)
                .into()
        } else {
            widget::button::standard("Open Iliada Settings → Assistant").on_press(Message::OpenSettings).into()
        };
        widget::column::with_capacity(2)
            .push(widget::scrollable(col).height(Length::Fill))
            .push(input)
            .spacing(spacing.space_m)
            .padding(spacing.space_m)
            .into()
    }
}
