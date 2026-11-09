//! Install Iliada: the graphical installer on the live system.
//!
//! Choose a disk, say who you are, confirm, wait. The work is done by
//! `iliada-install` (the same script the automated install test runs),
//! through the live system's passwordless sudo; the password goes to it on
//! stdin, never on a command line.

use std::process::Stdio;

use cosmic::app::{Settings, Task};
use cosmic::iced::{Alignment, Length, Size};
use cosmic::widget;
use cosmic::{ApplicationExt, Core, Element, executor};
use iliada_system::init::{valid_hostname, valid_timezone, valid_user};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const MIN_DISK: u64 = 20_000_000_000;

fn main() -> cosmic::iced::Result {
    let settings = Settings::default().size(Size::new(760.0, 600.0));
    cosmic::app::run::<App>(settings, ())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Disk {
    path: String,
    size: u64,
    model: String,
    removable: bool,
}

impl Disk {
    fn label(&self) -> String {
        let model = if self.model.is_empty() { "Disk".to_owned() } else { self.model.clone() };
        let kind = if self.removable { " (removable)" } else { "" };
        format!("{model}{kind} · {:.0} GB · {}", self.size as f64 / 1e9, self.path)
    }
}

/// The disks Iliada can be installed on: whole disks of at least 20 GB,
/// except the one the installer itself is running from.
fn disks() -> Result<Vec<Disk>, String> {
    let out = std::process::Command::new("lsblk")
        .args(["-J", "-b", "-o", "NAME,PATH,SIZE,MODEL,TYPE,RM,LABEL"])
        .output()
        .map_err(|e| format!("lsblk: {e}"))?;
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("lsblk: {e}"))?;
    fn text(v: &serde_json::Value, k: &str) -> String {
        match &v[k] {
            serde_json::Value::String(s) => s.trim().to_owned(),
            serde_json::Value::Number(n) => n.to_string(),
            _ => String::new(),
        }
    }
    fn labels(v: &serde_json::Value, out: &mut Vec<String>) {
        out.push(text(v, "label"));
        for c in v["children"].as_array().into_iter().flatten() {
            labels(c, out);
        }
    }
    let mut disks = Vec::new();
    for d in v["blockdevices"].as_array().into_iter().flatten() {
        let name = text(d, "name");
        if text(d, "type") != "disk" || name.starts_with("zram") || name.starts_with("loop") {
            continue;
        }
        let mut l = Vec::new();
        labels(d, &mut l);
        if l.iter().any(|l| l.starts_with("ILIADA_")) {
            continue; // the installation medium
        }
        let size: u64 = text(d, "size").parse().unwrap_or(0);
        if size < MIN_DISK {
            continue;
        }
        disks.push(Disk {
            path: text(d, "path"),
            size,
            model: text(d, "model"),
            removable: matches!(&d["rm"], serde_json::Value::Bool(true)) || text(d, "rm") == "1",
        });
    }
    Ok(disks)
}

fn timezones() -> Vec<String> {
    let out = std::process::Command::new("timedatectl").arg("list-timezones").output();
    let mut tz: Vec<String> = out
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().map(str::to_owned).collect())
        .unwrap_or_default();
    if tz.is_empty() {
        tz.push("UTC".into());
    }
    tz
}

/// "Alex Papadopoulos" -> "alex".
fn user_from_name(name: &str) -> String {
    let first = name.split_whitespace().next().unwrap_or("").to_lowercase();
    let u: String = first.chars().filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit()).take(31).collect();
    if u.chars().next().is_some_and(|c| c.is_ascii_lowercase()) { u } else { String::new() }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Welcome,
    Disk,
    You,
    Confirm,
    Installing,
    Done,
    Failed,
}

#[derive(Debug, Clone)]
enum Message {
    Go(Step),
    Disks(Result<Vec<Disk>, String>),
    PickDisk(usize),
    FullName(String),
    User(String),
    Hostname(String),
    Password(String),
    Again(String),
    TogglePassword,
    Timezone(String),
    PickTimezone(String),
    Install,
    Line(String),
    Finished(Result<(), String>),
    Restart,
    Quit,
}

struct App {
    core: Core,
    step: Step,
    disks: Option<Result<Vec<Disk>, String>>,
    disk: Option<usize>,
    full_name: String,
    user: String,
    user_edited: bool,
    hostname: String,
    hostname_edited: bool,
    password: String,
    again: String,
    show_password: bool,
    timezone: String,
    zones: Vec<String>,
    stage: String,
    log: Vec<String>,
    error: Option<String>,
}

impl App {
    fn chosen(&self) -> Option<&Disk> {
        match (&self.disks, self.disk) {
            (Some(Ok(d)), Some(i)) => d.get(i),
            _ => None,
        }
    }

    fn you_ok(&self) -> bool {
        !self.full_name.trim().is_empty()
            && valid_user(&self.user)
            && valid_hostname(&self.hostname)
            && !self.password.is_empty()
            && self.password == self.again
            && valid_timezone(&self.timezone)
            && self.zones.iter().any(|z| z == &self.timezone)
    }

    fn install(&self) -> Task<Message> {
        let disk = self.chosen().map(|d| d.path.clone()).unwrap_or_default();
        let args = vec![
            "-n".to_owned(),
            "iliada-install".into(),
            "--disk".into(),
            disk,
            "--user".into(),
            self.user.clone(),
            "--fullname".into(),
            self.full_name.trim().to_owned(),
            "--hostname".into(),
            self.hostname.clone(),
            "--timezone".into(),
            self.timezone.clone(),
            "--yes".into(),
        ];
        let password = self.password.clone();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Message>();
        let started = async move {
            let run = async {
                let mut child = tokio::process::Command::new("sudo")
                    .args(&args)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .map_err(|e| format!("could not start the installer: {e}"))?;
                let mut stdin = child.stdin.take().expect("piped");
                stdin.write_all(format!("{password}\n{password}\n").as_bytes()).await.map_err(|e| e.to_string())?;
                drop(stdin);
                let (out, err) = (child.stdout.take().expect("piped"), child.stderr.take().expect("piped"));
                let (t1, t2) = (tx.clone(), tx.clone());
                let a = tokio::spawn(async move {
                    let mut l = BufReader::new(out).lines();
                    while let Ok(Some(line)) = l.next_line().await {
                        let _ = t1.send(Message::Line(line));
                    }
                });
                let b = tokio::spawn(async move {
                    let mut l = BufReader::new(err).lines();
                    while let Ok(Some(line)) = l.next_line().await {
                        let _ = t2.send(Message::Line(line));
                    }
                });
                let status = child.wait().await.map_err(|e| e.to_string())?;
                let _ = (a.await, b.await);
                if status.success() { Ok(()) } else { Err("the installation failed".to_owned()) }
            };
            let r = run.await;
            let _ = tx.send(Message::Finished(r));
        };
        enum State<F> {
            Start(F, tokio::sync::mpsc::UnboundedReceiver<Message>),
            Running(tokio::sync::mpsc::UnboundedReceiver<Message>),
        }
        let stream = futures_util::stream::unfold(State::Start(Box::pin(started), rx), |s| async move {
            let mut rx = match s {
                State::Start(f, rx) => {
                    tokio::spawn(f);
                    rx
                }
                State::Running(rx) => rx,
            };
            rx.recv().await.map(|m| (m, State::Running(rx)))
        });
        cosmic::task::stream(stream)
    }

    fn page<'a>(&'a self, title: &'a str, body: &'a str, content: Vec<Element<'a, Message>>, actions: Vec<widget::button::TextButton<'a, Message>>) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();
        let mut col = widget::column::with_capacity(content.len() + 3)
            .push(widget::text::title2(title))
            .push(widget::text::body(body))
            .spacing(spacing.space_m)
            .max_width(600);
        for c in content {
            col = col.push(c);
        }
        let mut row = widget::row::with_capacity(actions.len() + 1).push(widget::space::horizontal()).spacing(spacing.space_s);
        for a in actions {
            row = row.push(a);
        }
        col = col.push(row);
        widget::container(col).center(Length::Fill).padding(spacing.space_l).into()
    }
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

impl cosmic::Application for App {
    type Executor = executor::Default;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = "org.iliada.Installer";

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
            disks: None,
            disk: None,
            full_name: String::new(),
            user: String::new(),
            user_edited: false,
            hostname: String::new(),
            hostname_edited: false,
            password: String::new(),
            again: String::new(),
            show_password: false,
            timezone: "UTC".into(),
            zones: timezones(),
            stage: String::new(),
            log: Vec::new(),
            error: None,
        };
        app.set_header_title("Install Iliada".into());
        let title = match app.core.main_window_id() {
            Some(id) => app.set_window_title("Install Iliada".into(), id),
            None => Task::none(),
        };
        (app, title)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Go(Step::Disk) => {
                self.step = Step::Disk;
                self.disks = None;
                return cosmic::task::future(async { Message::Disks(tokio::task::spawn_blocking(disks).await.unwrap_or_else(|e| Err(e.to_string()))) });
            }
            Message::Go(step) => self.step = step,
            Message::Disks(d) => {
                self.disk = match &d {
                    Ok(list) if list.len() == 1 => Some(0),
                    _ => None,
                };
                self.disks = Some(d);
            }
            Message::PickDisk(i) => self.disk = Some(i),
            Message::FullName(n) => {
                if !self.user_edited {
                    self.user = user_from_name(&n);
                }
                if !self.hostname_edited {
                    let u = user_from_name(&n);
                    self.hostname = if u.is_empty() { String::new() } else { format!("{u}-computer") };
                }
                self.full_name = n;
            }
            Message::User(u) => {
                self.user_edited = true;
                self.user = u;
            }
            Message::Hostname(h) => {
                self.hostname_edited = true;
                self.hostname = h;
            }
            Message::Password(p) => self.password = p,
            Message::Again(p) => self.again = p,
            Message::TogglePassword => self.show_password = !self.show_password,
            Message::Timezone(t) => self.timezone = t,
            Message::PickTimezone(t) => self.timezone = t,
            Message::Install => {
                self.step = Step::Installing;
                self.log.clear();
                self.stage = "Preparing".into();
                return self.install();
            }
            Message::Line(l) => {
                let l = strip_ansi(&l);
                if let Some(stage) = l.strip_prefix("==> ") {
                    self.stage = stage.trim().to_owned();
                }
                if !l.trim().is_empty() {
                    if self.log.len() == 200 {
                        self.log.remove(0);
                    }
                    self.log.push(l);
                }
            }
            Message::Finished(Ok(())) => self.step = Step::Done,
            Message::Finished(Err(e)) => {
                self.error = Some(e);
                self.step = Step::Failed;
            }
            Message::Restart => {
                let _ = std::process::Command::new("systemctl").arg("reboot").spawn();
            }
            Message::Quit => std::process::exit(0),
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();
        match self.step {
            Step::Welcome => self.page(
                "Install Iliada",
                "Iliada will be installed on this computer's disk. You can also keep trying it: \
                 nothing is changed until you confirm, a few steps from now.",
                vec![],
                vec![
                    widget::button::standard("Keep trying Iliada").on_press(Message::Quit),
                    widget::button::suggested("Continue").on_press(Message::Go(Step::Disk)),
                ],
            ),
            Step::Disk => {
                let content: Element<_> = match &self.disks {
                    None => widget::progress_bar::circular::Circular::new().size(24.0).into(),
                    Some(Err(e)) => widget::text::body(format!("Could not list the disks: {e}")).into(),
                    Some(Ok(list)) if list.is_empty() => widget::text::body(
                        "There is no disk of at least 20 GB to install on. Connect one, then go back and try again.",
                    )
                    .into(),
                    Some(Ok(list)) => {
                        let mut col = widget::column::with_capacity(list.len()).spacing(spacing.space_xs);
                        for (i, d) in list.iter().enumerate() {
                            col = col.push(
                                widget::container(widget::radio(widget::text::body(d.label()), i, self.disk, Message::PickDisk))
                                    .padding(spacing.space_s)
                                    .class(cosmic::theme::Container::Card)
                                    .width(Length::Fill),
                            );
                        }
                        col.into()
                    }
                };
                self.page(
                    "Where should Iliada go?",
                    "Everything on the disk you choose will be erased.",
                    vec![content],
                    vec![
                        widget::button::standard("Back").on_press(Message::Go(Step::Welcome)),
                        widget::button::suggested("Continue").on_press_maybe(self.chosen().is_some().then_some(Message::Go(Step::You))),
                    ],
                )
            }
            Step::You => {
                let field = |label: &'static str, input: widget::TextInput<'static, Message>| -> Element<'static, Message> {
                    widget::column::with_capacity(2)
                        .push(widget::text::caption_heading(label))
                        .push(input.width(Length::Fill))
                        .spacing(spacing.space_xxs)
                        .into()
                };
                let mut content = vec![
                    field("Your name", widget::text_input("Alex Papadopoulos", self.full_name.clone()).on_input(Message::FullName)),
                    field("Account name", widget::text_input("alex", self.user.clone()).on_input(Message::User)),
                    field("Computer name", widget::text_input("alex-computer", self.hostname.clone()).on_input(Message::Hostname)),
                    field(
                        "Password",
                        widget::secure_input("Password", self.password.clone(), Some(Message::TogglePassword), !self.show_password)
                            .on_input(Message::Password),
                    ),
                    field(
                        "Repeat the password",
                        widget::secure_input("Password", self.again.clone(), Some(Message::TogglePassword), !self.show_password)
                            .on_input(Message::Again),
                    ),
                    field("Time zone", widget::text_input("Europe/Athens", self.timezone.clone()).on_input(Message::Timezone)),
                ];
                let needle = self.timezone.to_lowercase();
                if !self.zones.iter().any(|z| *z == self.timezone) && needle.len() >= 2 {
                    let mut row = widget::row::with_capacity(6).spacing(spacing.space_xxs);
                    for z in self.zones.iter().filter(|z| z.to_lowercase().contains(&needle)).take(5) {
                        row = row.push(widget::button::standard(z.clone()).on_press(Message::PickTimezone(z.clone())));
                    }
                    content.push(row.into());
                }
                let mut hints = Vec::new();
                if !self.user.is_empty() && !valid_user(&self.user) {
                    hints.push("The account name can use lowercase letters, digits, - and _.");
                }
                if !self.hostname.is_empty() && !valid_hostname(&self.hostname) {
                    hints.push("The computer name can use letters, digits and -.");
                }
                if !self.again.is_empty() && self.password != self.again {
                    hints.push("The passwords don't match.");
                }
                for h in hints {
                    content.push(widget::text::caption(h).into());
                }
                self.page(
                    "About you",
                    "Your account is an administrator: it can install apps and change settings.",
                    vec![widget::scrollable(widget::column::with_children(content).spacing(spacing.space_s)).height(Length::Fixed(330.0)).into()],
                    vec![
                        widget::button::standard("Back").on_press(Message::Go(Step::Disk)),
                        widget::button::suggested("Continue").on_press_maybe(self.you_ok().then_some(Message::Go(Step::Confirm))),
                    ],
                )
            }
            Step::Confirm => {
                let disk = self.chosen().map(Disk::label).unwrap_or_default();
                self.page(
                    "Erase the disk and install Iliada?",
                    "Everything on this disk will be erased. This can't be undone.",
                    vec![
                        widget::text::heading(disk).into(),
                        widget::text::body(format!(
                            "{} ({}) on \"{}\", time zone {}.",
                            self.full_name.trim(),
                            self.user,
                            self.hostname,
                            self.timezone
                        ))
                        .into(),
                    ],
                    vec![
                        widget::button::standard("Back").on_press(Message::Go(Step::You)),
                        widget::button::destructive("Erase and install").on_press(Message::Install),
                    ],
                )
            }
            Step::Installing => {
                let last = self.log.last().map(String::as_str).unwrap_or("");
                self.page(
                    "Installing Iliada",
                    "This takes a while: Iliada is downloaded and set up for this computer. Keep it connected to the internet and to power.",
                    vec![
                        widget::row::with_capacity(2)
                            .push(widget::progress_bar::circular::Circular::new().size(24.0))
                            .push(widget::text::heading(format!("{}…", self.stage)))
                            .spacing(spacing.space_s)
                            .align_y(Alignment::Center)
                            .into(),
                        widget::text::caption(last.chars().take(110).collect::<String>()).into(),
                    ],
                    vec![],
                )
            }
            Step::Done => self.page(
                "Iliada is installed",
                "Remove the installation USB stick (or disc), then restart. Log in with the password you chose.",
                vec![],
                vec![widget::button::suggested("Restart now").on_press(Message::Restart)],
            ),
            Step::Failed => {
                let tail: Vec<&str> = self.log.iter().rev().take(12).rev().map(String::as_str).collect();
                self.page(
                    "The installation did not finish",
                    "Nothing on this computer is usable yet. Check the internet connection and try again.",
                    vec![
                        widget::text::body(self.error.clone().unwrap_or_default()).into(),
                        widget::container(widget::text::monotext(tail.join("\n")))
                            .padding(spacing.space_s)
                            .class(cosmic::theme::Container::Card)
                            .width(Length::Fill)
                            .into(),
                    ],
                    vec![
                        widget::button::standard("Quit").on_press(Message::Quit),
                        widget::button::suggested("Try again").on_press(Message::Go(Step::Confirm)),
                    ],
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_ansi() {
        assert_eq!(user_from_name("Alex Papadopoulos"), "alex");
        assert_eq!(user_from_name("Ζωή"), "");
        assert_eq!(user_from_name("  9lives"), "");
        assert_eq!(strip_ansi("\x1b[1m==> Partitioning\x1b[0m"), "==> Partitioning");
    }
}
