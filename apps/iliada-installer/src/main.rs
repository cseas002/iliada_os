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
    let settings = Settings::default().size(Size::new(780.0, 720.0));
    cosmic::app::run::<App>(settings, ())
}

/// The least free space Iliada goes into next to another system.
const MIN_FREE: u64 = 25 << 30;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Disk {
    path: String,
    size: u64,
    model: String,
    removable: bool,
    /// What is on it now, in people's words ("ChromeOS Flex", "Windows").
    holds: Option<String>,
    /// The largest stretch of unpartitioned space (GPT disks only).
    free: u64,
}

impl Disk {
    fn label(&self) -> String {
        let model = if self.model.is_empty() { "Disk".to_owned() } else { self.model.clone() };
        let kind = if self.removable { " (removable)" } else { "" };
        format!("{model}{kind} · {} · {}", gb(self.size), self.path)
    }

    /// Iliada fits next to what is there.
    fn can_keep(&self) -> bool {
        self.holds.is_some() && self.free >= MIN_FREE
    }
}

fn gb(bytes: u64) -> String {
    format!("{:.0} GB", bytes as f64 / 1e9)
}

/// The disks Iliada can be installed on: whole disks of at least 20 GB,
/// except the one the installer itself is running from.
fn disks() -> Result<Vec<Disk>, String> {
    let out = std::process::Command::new("lsblk")
        .args(["-J", "-b", "-o", "NAME,PATH,SIZE,MODEL,TYPE,RM,LABEL,PARTLABEL,FSTYPE,PTTYPE,START"])
        .output()
        .map_err(|e| format!("lsblk: {e}"))?;
    parse_disks(&out.stdout)
}

fn parse_disks(json: &[u8]) -> Result<Vec<Disk>, String> {
    let v: serde_json::Value = serde_json::from_slice(json).map_err(|e| format!("lsblk: {e}"))?;
    fn text(v: &serde_json::Value, k: &str) -> String {
        match &v[k] {
            serde_json::Value::String(s) => s.trim().to_owned(),
            serde_json::Value::Number(n) => n.to_string(),
            _ => String::new(),
        }
    }
    let mut disks = Vec::new();
    for d in v["blockdevices"].as_array().into_iter().flatten() {
        let name = text(d, "name");
        if text(d, "type") != "disk" || name.starts_with("zram") || name.starts_with("loop") {
            continue;
        }
        let parts: Vec<&serde_json::Value> = d["children"].as_array().into_iter().flatten().collect();
        let has = |key: &str, f: &dyn Fn(&str) -> bool| parts.iter().any(|p| f(&text(p, key)));
        if text(d, "label").starts_with("ILIADA_") || has("label", &|l| l.starts_with("ILIADA_")) {
            continue; // the installation medium
        }
        let size: u64 = text(d, "size").parse().unwrap_or(0);
        if size < MIN_DISK {
            continue;
        }
        let holds = if parts.is_empty() {
            None
        } else if has("partlabel", &|l| l == "STATE") && has("partlabel", &|l| l == "ROOT-A") {
            Some("ChromeOS Flex")
        } else if has("fstype", &|f| f == "ntfs") || has("partlabel", &|l| l == "Microsoft reserved partition") {
            Some("Windows")
        } else if has("fstype", &|f| f == "apfs" || f == "hfsplus") {
            Some("macOS")
        } else if has("label", &|l| l == "iliada") {
            Some("Iliada")
        } else if has("fstype", &|f| ["ext4", "btrfs", "xfs", "f2fs"].contains(&f)) {
            Some("Linux")
        } else {
            Some("other files")
        };
        // Free space: the gaps between partitions (lsblk START is in
        // 512-byte sectors), within the usable part of a GPT disk.
        let free = if text(d, "pttype") == "gpt" {
            let mut spans: Vec<(u64, u64)> = parts
                .iter()
                .filter_map(|p| {
                    let start: u64 = text(p, "start").parse().ok()?;
                    let len: u64 = text(p, "size").parse().ok()?;
                    Some((start * 512, start * 512 + len))
                })
                .collect();
            spans.sort();
            let (mut at, end, mut best) = (1u64 << 20, size.saturating_sub(34 * 512), 0u64);
            for (s, e) in spans {
                best = best.max(s.saturating_sub(at));
                at = at.max(e);
            }
            best.max(end.saturating_sub(at))
        } else {
            0
        };
        disks.push(Disk {
            path: text(d, "path"),
            size,
            model: text(d, "model"),
            removable: matches!(&d["rm"], serde_json::Value::Bool(true)) || text(d, "rm") == "1",
            holds: holds.map(str::to_owned),
            free,
        });
    }
    Ok(disks)
}

fn timezones() -> Vec<String> {
    let out = std::process::Command::new("timedatectl").arg("list-timezones").output();
    let mut tz: Vec<String> =
        out.map(|o| String::from_utf8_lossy(&o.stdout).lines().map(str::to_owned).collect()).unwrap_or_default();
    if tz.is_empty() {
        tz.push("UTC".into());
    }
    tz
}

/// The parts of the world time zones are grouped in (the first part of
/// "Europe/Athens"); everything else ("UTC", "Etc/GMT+2") is "Other".
const REGIONS: &[&str] = &[
    "Africa",
    "America",
    "Antarctica",
    "Arctic",
    "Asia",
    "Atlantic",
    "Australia",
    "Europe",
    "Indian",
    "Pacific",
    "Other",
];

fn region_of(zone: &str) -> &str {
    match zone.split_once('/') {
        Some((r, _)) if REGIONS.contains(&r) => r,
        _ => "Other",
    }
}

/// The zones of a region, with names for people: "Buenos Aires (Argentina)".
fn cities(zones: &[String], region: &str) -> (Vec<String>, Vec<String>) {
    let mut v: Vec<(String, String)> = zones
        .iter()
        .filter(|z| region_of(z) == region)
        .map(|z| {
            let rest = if region == "Other" { z.as_str() } else { z.split_once('/').map_or(z.as_str(), |(_, r)| r) };
            let name = match rest.rsplit_once('/') {
                Some((within, city)) => format!("{} ({})", city.replace('_', " "), within.replace('_', " ")),
                None => rest.replace('_', " "),
            };
            (name, z.clone())
        })
        .collect();
    v.sort();
    v.into_iter().unzip()
}

/// The time zone of this internet connection (Fedora's location service,
/// which its installer uses too), or None.
async fn detect_timezone() -> Option<String> {
    let out = tokio::process::Command::new("curl")
        .args(["-sf", "--max-time", "6", "https://geoip.fedoraproject.org/city"])
        .output()
        .await
        .ok()?;
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    v["time_zone"].as_str().map(str::to_owned)
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
    /// A disk, and whether to keep what is on it.
    PickDisk((usize, bool)),
    FullName(String),
    User(String),
    Hostname(String),
    Password(String),
    Again(String),
    TogglePassword,
    Region(usize),
    City(usize),
    Detected(Option<String>),
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
    /// The disk, and whether Iliada goes next to what is on it.
    choice: Option<(usize, bool)>,
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
    /// The chosen region's zones: names for people, and the zones.
    cities: Vec<String>,
    city_zones: Vec<String>,
    /// Guessed from the internet connection; chosen by the person.
    tz_guessed: bool,
    tz_chosen: bool,
    stage: String,
    log: Vec<String>,
    error: Option<String>,
}

impl App {
    fn chosen(&self) -> Option<&Disk> {
        match (&self.disks, self.choice) {
            (Some(Ok(d)), Some((i, _))) => d.get(i),
            _ => None,
        }
    }

    fn set_timezone(&mut self, zone: String) {
        (self.cities, self.city_zones) = cities(&self.zones, region_of(&zone));
        self.timezone = zone;
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

    /// Iliada goes next to what is on the disk (nothing is erased).
    fn keep(&self) -> bool {
        self.choice.is_some_and(|(_, keep)| keep)
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
        let mut args = args;
        if self.keep() {
            args.push("--free-space".into());
        }
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

    fn page<'a>(
        &'a self,
        title: impl Into<std::borrow::Cow<'a, str>> + 'a,
        body: impl Into<std::borrow::Cow<'a, str>> + 'a,
        content: Vec<Element<'a, Message>>,
        actions: Vec<widget::button::TextButton<'a, Message>>,
    ) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();
        let mut col = widget::column::with_capacity(content.len() + 3)
            .push(widget::text::title2(title))
            .push(widget::text::body(body))
            .spacing(spacing.space_m)
            .max_width(600);
        for c in content {
            col = col.push(c);
        }
        let mut row =
            widget::row::with_capacity(actions.len() + 1).push(widget::space::horizontal()).spacing(spacing.space_s);
        for a in actions {
            row = row.push(a);
        }
        // The page scrolls when it doesn't fit; the buttons stay below it at
        // their normal size.
        let body = widget::container(widget::scrollable(widget::container(col).center_x(Length::Fill)))
            .center_y(Length::Fill)
            .width(Length::Fill);
        let bar = widget::container(widget::column::with_capacity(1).push(row).max_width(600)).center_x(Length::Fill);
        widget::column::with_capacity(2)
            .push(body)
            .push(bar)
            .spacing(spacing.space_m)
            .padding(spacing.space_l)
            .into()
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
            choice: None,
            full_name: String::new(),
            user: String::new(),
            user_edited: false,
            hostname: String::new(),
            hostname_edited: false,
            password: String::new(),
            again: String::new(),
            show_password: false,
            timezone: String::new(),
            zones: timezones(),
            cities: Vec::new(),
            city_zones: Vec::new(),
            tz_guessed: false,
            tz_chosen: false,
            stage: String::new(),
            log: Vec::new(),
            error: None,
        };
        app.set_timezone("UTC".into());
        app.set_header_title("Install Iliada".into());
        let title = match app.core.main_window_id() {
            Some(id) => app.set_window_title("Install Iliada".into(), id),
            None => Task::none(),
        };
        let detect = cosmic::task::future(async { Message::Detected(detect_timezone().await) });
        (app, Task::batch([title, detect]))
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Go(Step::Disk) => {
                self.step = Step::Disk;
                self.disks = None;
                return cosmic::task::future(async {
                    Message::Disks(tokio::task::spawn_blocking(disks).await.unwrap_or_else(|e| Err(e.to_string())))
                });
            }
            Message::Go(step) => self.step = step,
            Message::Disks(d) => {
                self.choice = match &d {
                    // One disk: keep what is on it if Iliada fits next to it.
                    Ok(list) if list.len() == 1 => Some((0, list[0].can_keep())),
                    _ => None,
                };
                self.disks = Some(d);
            }
            Message::PickDisk(c) => self.choice = Some(c),
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
            Message::Region(i) => {
                let region = REGIONS.get(i).copied().unwrap_or("Other");
                if region_of(&self.timezone) != region {
                    let (_, zones) = cities(&self.zones, region);
                    if let Some(z) = zones.into_iter().next() {
                        self.set_timezone(z);
                    }
                }
                self.tz_chosen = true;
            }
            Message::City(i) => {
                if let Some(z) = self.city_zones.get(i).cloned() {
                    self.set_timezone(z);
                    self.tz_chosen = true;
                }
            }
            Message::Detected(Some(z)) if !self.tz_chosen && self.zones.contains(&z) => {
                self.set_timezone(z);
                self.tz_guessed = true;
            }
            Message::Detected(_) => {}
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
                            let option = |text: String, keep: bool| {
                                widget::radio(widget::text::body(text), (i, keep), self.choice, Message::PickDisk)
                            };
                            let mut card = widget::column::with_capacity(4)
                                .push(widget::text::heading(d.label()))
                                .spacing(spacing.space_xs);
                            match d.holds.as_deref() {
                                Some(what) if d.can_keep() => {
                                    card = card
                                        .push(option(
                                            format!("Next to {what}, in its {} of free space", gb(d.free)),
                                            true,
                                        ))
                                        .push(option(format!("Erase {what} and use the whole disk"), false));
                                }
                                Some(what) => {
                                    card = card.push(option(format!("Erase {what} and use the whole disk"), false)).push(
                                        widget::text::caption(format!(
                                            "To keep {what}, Iliada needs {} of free space next to it; this disk has {}.",
                                            gb(MIN_FREE),
                                            gb(d.free)
                                        )),
                                    );
                                }
                                None => card = card.push(option("Use the whole disk (it is empty)".into(), false)),
                            }
                            col = col.push(
                                widget::container(card)
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
                    "Iliada can go next to what is already on a disk, if there is room, or replace it.",
                    vec![content],
                    vec![
                        widget::button::standard("Back").on_press(Message::Go(Step::Welcome)),
                        widget::button::suggested("Continue")
                            .on_press_maybe(self.chosen().is_some().then_some(Message::Go(Step::You))),
                    ],
                )
            }
            Step::You => {
                let field =
                    |label: &'static str, input: widget::TextInput<'static, Message>| -> Element<'static, Message> {
                        widget::column::with_capacity(2)
                            .push(widget::text::caption_heading(label))
                            .push(input.width(Length::Fill))
                            .spacing(spacing.space_xxs)
                            .into()
                    };
                let mut content = vec![
                    field(
                        "Your name",
                        widget::text_input("Alex Papadopoulos", self.full_name.clone()).on_input(Message::FullName),
                    ),
                    field("Account name", widget::text_input("alex", self.user.clone()).on_input(Message::User)),
                    field(
                        "Computer name",
                        widget::text_input("alex-computer", self.hostname.clone()).on_input(Message::Hostname),
                    ),
                    field(
                        "Password",
                        widget::secure_input(
                            "Password",
                            self.password.clone(),
                            Some(Message::TogglePassword),
                            !self.show_password,
                        )
                        .on_input(Message::Password),
                    ),
                    field(
                        "Repeat the password",
                        widget::secure_input(
                            "Password",
                            self.again.clone(),
                            Some(Message::TogglePassword),
                            !self.show_password,
                        )
                        .on_input(Message::Again),
                    ),
                ];
                let region = REGIONS.iter().position(|r| *r == region_of(&self.timezone));
                let city = self.city_zones.iter().position(|z| *z == self.timezone);
                let mut zone = widget::column::with_capacity(3)
                    .push(widget::text::caption_heading("Time zone"))
                    .push(
                        widget::row::with_capacity(2)
                            .push(widget::dropdown(REGIONS, region, Message::Region).width(Length::FillPortion(2)))
                            .push(widget::dropdown(&self.cities, city, Message::City).width(Length::FillPortion(3)))
                            .spacing(spacing.space_xs),
                    )
                    .spacing(spacing.space_xxs);
                if self.tz_guessed && !self.tz_chosen {
                    zone = zone.push(widget::text::caption("Found from your internet connection."));
                }
                content.push(zone.into());
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
                    vec![
                        widget::column::with_children(content).spacing(spacing.space_s).into(),
                    ],
                    vec![
                        widget::button::standard("Back").on_press(Message::Go(Step::Disk)),
                        widget::button::suggested("Continue")
                            .on_press_maybe(self.you_ok().then_some(Message::Go(Step::Confirm))),
                    ],
                )
            }
            Step::Confirm => {
                let disk = self.chosen().map(Disk::label).unwrap_or_default();
                let what = self.chosen().and_then(|d| d.holds.clone()).unwrap_or_else(|| "the other system".into());
                let (title, body, go) = if self.keep() {
                    (
                        format!("Install Iliada next to {what}?"),
                        format!(
                            "{what} stays as it is. When the computer starts, Iliada starts; to start {what} \
                             instead, open the start-up menu (usually F9, F12 or Esc while the computer starts)."
                        ),
                        widget::button::suggested("Install"),
                    )
                } else {
                    (
                        "Erase the disk and install Iliada?".to_owned(),
                        "Everything on this disk will be erased. This can't be undone.".to_owned(),
                        widget::button::destructive("Erase and install"),
                    )
                };
                self.page(
                    title,
                    body,
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
                        go.on_press(Message::Install),
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
    fn time_zones_by_region() {
        let zones: Vec<String> =
            ["Asia/Nicosia", "Europe/Athens", "America/Argentina/Buenos_Aires", "America/New_York", "UTC"]
                .map(String::from)
                .to_vec();
        assert_eq!(region_of("Asia/Nicosia"), "Asia");
        assert_eq!(region_of("UTC"), "Other");
        assert_eq!(region_of("Etc/GMT+2"), "Other");
        let (names, z) = cities(&zones, "America");
        assert_eq!(names, ["Buenos Aires (Argentina)", "New York"]);
        assert_eq!(z, ["America/Argentina/Buenos_Aires", "America/New_York"]);
        assert_eq!(cities(&zones, "Other").0, ["UTC"]);
    }

    #[test]
    /// A 500 GB disk with ChromeOS Flex on it (as on an HP EliteDesk),
    /// with and without room next to it; the USB stick is left out.
    fn disks_and_what_they_hold() {
        let flex = |state_size: u64| {
            format!(
                r#"{{"blockdevices": [
                {{"name":"sda","path":"/dev/sda","size":500107862016,"model":"SSD","type":"disk","rm":false,"pttype":"gpt","children":[
                  {{"name":"sda1","size":{state_size},"type":"part","fstype":"ext4","label":"H-STATE","partlabel":"STATE","start":17862656}},
                  {{"name":"sda3","size":4294967296,"type":"part","fstype":"ext4","label":"ROOT-A","partlabel":"ROOT-A","start":9211904}},
                  {{"name":"sda12","size":268435456,"type":"part","fstype":"vfat","partlabel":"EFI-SYSTEM","start":299008}}]}},
                {{"name":"sdb","path":"/dev/sdb","size":15502147584,"type":"disk","rm":true,"label":"ILIADA_0_2_2","children":[
                  {{"name":"sdb1","size":3760128000,"type":"part","label":"ILIADA_0_2_2","start":0}}]}}]}}"#
            )
        };
        let full = parse_disks(flex(490962157568).as_bytes()).unwrap();
        assert_eq!(full.len(), 1, "the installation stick is not offered");
        assert_eq!(full[0].holds.as_deref(), Some("ChromeOS Flex"));
        assert!(!full[0].can_keep(), "no room: {}", full[0].free);
        let shrunk = parse_disks(flex(388694540288).as_bytes()).unwrap();
        assert!(shrunk[0].can_keep());
        assert_eq!(gb(shrunk[0].free), "102 GB");
    }

    #[test]
    fn names_and_ansi() {
        assert_eq!(user_from_name("Alex Papadopoulos"), "alex");
        assert_eq!(user_from_name("Ζωή"), "");
        assert_eq!(user_from_name("  9lives"), "");
        assert_eq!(strip_ansi("\x1b[1m==> Partitioning\x1b[0m"), "==> Partitioning");
    }
}
