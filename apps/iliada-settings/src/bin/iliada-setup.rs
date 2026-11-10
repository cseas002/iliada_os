//! The Iliada Setup Assistant: runs on first login. Restore apps and
//! settings from a GitHub backup, or start fresh and turn backups on.
//!
//!   iliada-setup              always opens
//!   iliada-setup --first-run  opens only if setup hasn't been completed

use std::path::PathBuf;

use cosmic::app::{Settings, Task};
use cosmic::iced::{Alignment, Length, Size};
use cosmic::widget;
use cosmic::{ApplicationExt, Core, Element, executor};
use futures_util::StreamExt;
use iliada_settings::{Progress, ago, open_url, run};
use iliada_system::client;
use iliada_system::protocol::{Backup, Event, Request, Status};

fn done_marker() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("iliada/setup-done"))
}

fn main() -> cosmic::iced::Result {
    if std::env::args().any(|a| a == "--first-run") && done_marker().is_some_and(|m| m.exists()) {
        return Ok(());
    }
    let settings = Settings::default().size(Size::new(680.0, 560.0));
    cosmic::app::run::<App>(settings, ())
}

#[derive(Debug, Clone, PartialEq)]
enum Step {
    Welcome,
    /// Restore a backup or start fresh.
    Choice,
    /// Waiting for the GitHub sign-in (then: `then`).
    SignIn {
        then: Box<Step>,
    },
    PickBackup,
    NameBackup,
    Working,
    Done,
}

#[derive(Debug, Clone)]
enum Message {
    Status(Result<Status, String>),
    Go(Step),
    SignIn(Step),
    OpenGitHub,
    Backups(Result<Vec<Backup>, String>),
    Restore(String),
    Name(String),
    EnableBackup,
    Progress(Progress),
    Finish,
}

struct App {
    core: Core,
    step: Step,
    status: Option<Status>,
    code: Option<(String, String)>,
    backups: Option<Result<Vec<Backup>, String>>,
    name: String,
    working: String,
    line: String,
    /// What happened, shown on the last page.
    summary: Vec<String>,
    error: Option<String>,
    /// The step to return to after an error.
    back: Step,
}

impl App {
    fn backup_possible(&self) -> bool {
        self.status.as_ref().is_some_and(|s| s.admin && s.backup.available)
    }

    fn start(&mut self, request: Request, what: &str, back: Step) -> Task<Message> {
        self.error = None;
        self.working = what.to_owned();
        self.line.clear();
        self.back = back;
        self.step = Step::Working;
        cosmic::task::stream(run(request).map(Message::Progress))
    }

    fn page<'a>(&'a self, title: &'a str, body: &'a str, content: Vec<Element<'a, Message>>) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();
        let mut col = widget::column::with_capacity(content.len() + 3)
            .push(widget::text::title2(title))
            .push(widget::text::body(body))
            .spacing(spacing.space_m)
            .align_x(Alignment::Center)
            .max_width(520);
        if let Some(e) = &self.error {
            col = col.push(
                widget::container(widget::text::body(e.as_str()))
                    .padding(spacing.space_s)
                    .class(cosmic::theme::Container::Card),
            );
        }
        for c in content {
            col = col.push(c);
        }
        widget::container(col).center(Length::Fill).padding(spacing.space_l).into()
    }
}

fn buttons<'a>(items: Vec<widget::button::TextButton<'a, Message>>) -> Element<'a, Message> {
    let mut row = widget::row::with_capacity(items.len()).spacing(cosmic::theme::spacing().space_s);
    for b in items {
        row = row.push(b);
    }
    row.into()
}

impl cosmic::Application for App {
    type Executor = executor::Default;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = "org.iliada.Setup";

    fn core(&self) -> &Core {
        &self.core
    }
    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: ()) -> (Self, Task<Message>) {
        let mut app = App {
            core,
            step: Step::Welcome,
            status: None,
            code: None,
            backups: None,
            name: String::new(),
            working: String::new(),
            line: String::new(),
            summary: Vec::new(),
            error: None,
            back: Step::Choice,
        };
        app.set_header_title("Welcome".into());
        let title = match app.core.main_window_id() {
            Some(id) => app.set_window_title("Welcome to Iliada".into(), id),
            None => Task::none(),
        };
        let status =
            cosmic::task::future(async { Message::Status(client::status().await.map_err(|e| format!("{e:#}"))) });
        (app, Task::batch([title, status]))
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Status(s) => {
                if let Ok(s) = &s {
                    if self.name.is_empty() {
                        self.name = s.hostname.clone();
                    }
                }
                self.status = s.ok();
            }
            Message::Go(step) => {
                self.error = None;
                if step == Step::Choice && !self.backup_possible() {
                    // Nothing to restore from or back up to: straight to the end.
                    self.step = Step::Done;
                } else {
                    self.step = step;
                }
            }
            Message::SignIn(then) => {
                // Already signed in: go straight on.
                if self.status.as_ref().is_some_and(|s| s.backup.account.is_some()) {
                    return self.update(Message::Go(then));
                }
                self.code = None;
                self.error = None;
                let back = self.step.clone();
                self.step = Step::SignIn { then: Box::new(then) };
                self.back = back;
                return cosmic::task::stream(run(Request::BackupSignIn).map(Message::Progress));
            }
            Message::OpenGitHub => {
                if let Some((_, url)) = &self.code {
                    open_url(url);
                }
            }
            Message::Backups(b) => self.backups = Some(b),
            Message::Restore(repo) => {
                let name = repo.split_once('/').map(|(_, r)| r).unwrap_or(&repo).to_owned();
                self.summary.push(format!("Restored your apps and settings from {name}."));
                return self.start(Request::Restore { repo }, "Restoring your apps and settings", Step::PickBackup);
            }
            Message::Name(n) => self.name = n,
            Message::EnableBackup => {
                let name = self.name.trim().to_owned();
                if iliada_system::github::valid_name(&name) {
                    self.summary.push(format!("Your apps and settings are backed up to iliada_{name} on GitHub."));
                    return self.start(Request::BackupEnable { name }, "Setting up your backup", Step::NameBackup);
                }
            }
            Message::Progress(Progress::Event(Event::SignInCode { code, url })) => {
                open_url(&url);
                self.code = Some((code, url));
            }
            Message::Progress(Progress::Event(Event::Log { line })) => self.line = line.chars().take(100).collect(),
            Message::Progress(Progress::Event(_)) => {}
            Message::Progress(Progress::Finished(r)) => match (&self.step, r) {
                (Step::SignIn { then }, Ok(_)) => {
                    let then = (**then).clone();
                    self.step = then.clone();
                    let status = cosmic::task::future(async {
                        Message::Status(client::status().await.map_err(|e| format!("{e:#}")))
                    });
                    if then == Step::PickBackup {
                        self.backups = None;
                        let list = cosmic::task::future(async {
                            Message::Backups(
                                client::call_as(&Request::BackupList, &mut |_| {}).await.map_err(|e| format!("{e:#}")),
                            )
                        });
                        return Task::batch([status, list]);
                    }
                    return status;
                }
                (Step::Working, Ok(_)) => self.step = Step::Done,
                (_, Err(e)) => {
                    if matches!(self.step, Step::Working) {
                        self.summary.pop();
                    }
                    self.error = Some(e);
                    self.step = self.back.clone();
                }
                _ => {}
            },
            Message::Finish => {
                if let Some(m) = done_marker() {
                    if let Some(dir) = m.parent() {
                        let _ = std::fs::create_dir_all(dir);
                    }
                    let _ = std::fs::write(&m, "");
                }
                std::process::exit(0);
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();
        match &self.step {
            Step::Welcome => self.page(
                "Welcome to Iliada",
                "Apps are in Applications, each in its own sandbox. Get more from the Store; \
                 install one by dragging it into Applications, remove it by dragging it to the Trash.",
                vec![widget::button::suggested("Continue").on_press(Message::Go(Step::Choice)).into()],
            ),
            Step::Choice => self.page(
                "Have you used Iliada before?",
                "If you backed up another computer to GitHub, bring its apps and settings here.",
                vec![buttons(vec![
                    widget::button::suggested("Restore from a backup").on_press(Message::SignIn(Step::PickBackup)),
                    widget::button::standard("Set up as new").on_press(Message::Go(Step::NameBackup)),
                ])],
            ),
            Step::SignIn { .. } => {
                let mut content: Vec<Element<_>> = Vec::new();
                match &self.code {
                    Some((code, _)) => {
                        content.push(widget::text::body("Enter this code on GitHub:").into());
                        content.push(widget::text::title1(code.as_str()).into());
                        content.push(widget::button::suggested("Open GitHub").on_press(Message::OpenGitHub).into());
                        content.push(widget::text::caption("Waiting for you to approve it…").into());
                    }
                    None => content.push(widget::progress_bar::circular::Circular::new().size(24.0).into()),
                }
                content.push(widget::button::text("Back").on_press(Message::Go(self.back.clone())).into());
                self.page(
                    "Sign in to GitHub",
                    "Iliada keeps your backup in a private repository on your account.",
                    content,
                )
            }
            Step::PickBackup => {
                let mut content: Vec<Element<_>> = Vec::new();
                match &self.backups {
                    None => content.push(widget::progress_bar::circular::Circular::new().size(24.0).into()),
                    Some(Err(e)) => {
                        content.push(widget::text::body(format!("Could not list your backups: {e}")).into())
                    }
                    Some(Ok(list)) if list.is_empty() => {
                        content.push(widget::text::body("There are no Iliada backups on this account yet.").into())
                    }
                    Some(Ok(list)) => {
                        let mut col = widget::column::with_capacity(list.len()).spacing(spacing.space_xs);
                        for b in list {
                            let date = b.updated.split('T').next().unwrap_or("").to_owned();
                            let row = widget::row::with_capacity(2)
                                .push(
                                    widget::column::with_capacity(2)
                                        .push(widget::text::heading(b.name.clone()))
                                        .push(widget::text::caption(format!("Last backup {date}")))
                                        .width(Length::Fill),
                                )
                                .push(widget::button::suggested("Restore").on_press(Message::Restore(b.repo.clone())))
                                .align_y(Alignment::Center);
                            col = col.push(
                                widget::container(row)
                                    .padding(spacing.space_s)
                                    .class(cosmic::theme::Container::Card)
                                    .width(Length::Fill),
                            );
                        }
                        content.push(col.into());
                    }
                }
                content
                    .push(widget::button::text("Set up as new instead").on_press(Message::Go(Step::NameBackup)).into());
                self.page("Choose a backup", "Its apps and settings replace this computer's.", content)
            }
            Step::NameBackup => {
                let signed_in = self.status.as_ref().and_then(|s| s.backup.account.clone());
                let mut content: Vec<Element<_>> = Vec::new();
                match signed_in {
                    None => content.push(buttons(vec![
                        widget::button::suggested("Sign in with GitHub").on_press(Message::SignIn(Step::NameBackup)),
                        widget::button::standard("Not now").on_press(Message::Go(Step::Done)),
                    ])),
                    Some(account) => {
                        let valid = iliada_system::github::valid_name(self.name.trim());
                        content.push(
                            widget::row::with_capacity(2)
                                .push(widget::text::body(format!("github.com/{account}/iliada_")))
                                .push(
                                    widget::text_input("laptop", &self.name)
                                        .on_input(Message::Name)
                                        .on_submit(|_| Message::EnableBackup)
                                        .width(Length::Fixed(200.0)),
                                )
                                .align_y(Alignment::Center)
                                .into(),
                        );
                        content.push(buttons(vec![
                            widget::button::suggested("Back up").on_press_maybe(valid.then_some(Message::EnableBackup)),
                            widget::button::standard("Not now").on_press(Message::Go(Step::Done)),
                        ]));
                    }
                }
                self.page(
                    "Back up this computer",
                    "Iliada can keep your apps and settings in a private GitHub repository, updated after every change. \
                     A new computer is then ready in minutes.",
                    content,
                )
            }
            Step::Working => self.page(
                &self.working,
                "This can take a few minutes.",
                vec![
                    widget::progress_bar::circular::Circular::new().size(32.0).into(),
                    widget::text::caption(self.line.as_str()).into(),
                ],
            ),
            Step::Done => {
                let mut content: Vec<Element<_>> =
                    self.summary.iter().map(|s| widget::text::body(s.as_str()).into()).collect();
                if let Some(t) = self.status.as_ref().and_then(|s| s.backup.last_backup) {
                    content.push(widget::text::caption(format!("Last backup {}", ago(t))).into());
                }
                content.push(widget::text::caption("Change these any time in Iliada Settings.").into());
                content.push(widget::button::suggested("Start using Iliada").on_press(Message::Finish).into());
                self.page("You're all set", "Find apps in Applications and the Store, both in the dock.", content)
            }
        }
    }
}
