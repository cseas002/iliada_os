//! The Iliada Setup Assistant: runs on first login. Sign in to the Iliada
//! account (settings come back, encrypted backups go on), restore apps and
//! settings from a GitHub backup, or start fresh: choose the first apps
//! (they install in the background) and turn backups on.
//!
//!   iliada-setup              always opens
//!   iliada-setup --first-run  opens only if setup hasn't been completed

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use cosmic::app::{Settings, Task};
use cosmic::iced::{Alignment, Background, Length, Size};
use cosmic::widget;
use cosmic::{ApplicationExt, Core, Element, executor};
use futures_util::StreamExt;
use iliada_catalog::AppEntry;
use iliada_settings::account::{self, Account};
use iliada_settings::{Progress, ago, open_url, run};
use iliada_store::{Store, StoreConfig};
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
    let settings = Settings::default().size(Size::new(880.0, 680.0));
    cosmic::app::run::<App>(settings, ())
}

#[derive(Debug, Clone, PartialEq)]
enum Step {
    Welcome,
    /// Sign in to the Iliada account (when this computer has one set up).
    Account,
    /// Restore a backup or start fresh.
    Choice,
    /// Waiting for the GitHub sign-in (then: `then`).
    SignIn {
        then: Box<Step>,
    },
    PickBackup,
    /// Choose the first apps (when starting fresh).
    Apps,
    NameBackup,
    Working,
    Done,
}

// Messages are moved around once each; boxing the large one buys nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
enum Message {
    Status(Result<Status, String>),
    Go(Step),
    Account(account::Msg),
    SignIn(Step),
    OpenGitHub,
    Backups(Result<Vec<Backup>, String>),
    Restore(String),
    Name(String),
    EnableBackup,
    Progress(Progress),
    Picks(Result<Vec<Pick>, String>),
    Icon(String, Option<Vec<u8>>, bool),
    Toggle(String),
    InstallApps,
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
    account: Account,
    /// The "Choose your apps" page: None while the Store is asked.
    picks: Option<Result<Vec<Pick>, String>>,
    /// Names of the ticked apps.
    picked: BTreeSet<String>,
    icons: BTreeMap<String, widget::icon::Handle>,
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

    /// The ticked apps that aren't installed yet, in the page's order.
    fn chosen(&self) -> Vec<String> {
        let Some(Ok(picks)) = &self.picks else { return Vec::new() };
        picks.iter().filter(|p| self.picked.contains(&p.name) && !p.installed).map(|p| p.name.clone()).collect()
    }

    fn card<'a>(&'a self, p: &'a Pick) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();
        let selected = self.picked.contains(&p.name) && !p.installed;
        let icon: Element<_> = match self.icons.get(&p.key) {
            Some(h) => widget::icon::icon(h.clone()).size(40).into(),
            None => widget::icon::from_name(if p.browser { "web-browser" } else { "application-x-executable" })
                .size(40)
                .icon()
                .into(),
        };
        let mark: Element<_> = if p.installed {
            widget::text::caption("Installed").into()
        } else if selected {
            widget::icon::from_name("object-select-symbolic").size(16).icon().into()
        } else {
            widget::Space::new().width(16).into()
        };
        let text = widget::column::with_capacity(2)
            .push(widget::text::heading(p.name.as_str()))
            .push(widget::text::caption(p.blurb))
            .spacing(spacing.space_xxxs)
            .width(Length::Fill);
        let row = widget::row::with_capacity(3)
            .push(icon)
            .push(text)
            .push(mark)
            .spacing(spacing.space_s)
            .align_y(Alignment::Center);
        widget::button::custom(row)
            .on_press_maybe((!p.installed).then(|| Message::Toggle(p.name.clone())))
            .class(card_class(selected))
            .padding(spacing.space_s)
            .width(Length::Fixed(236.0))
            .height(Length::Fixed(84.0))
            .into()
    }

    fn page<'a>(&'a self, title: &'a str, body: &'a str, content: Vec<Element<'a, Message>>) -> Element<'a, Message> {
        self.page_with(title, body, content, 520.0)
    }

    fn page_with<'a>(
        &'a self,
        title: &'a str,
        body: &'a str,
        content: Vec<Element<'a, Message>>,
        width: f32,
    ) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();
        let mut col = widget::column::with_capacity(content.len() + 3)
            .push(widget::text::title2(title))
            .push(widget::text::body(body))
            .spacing(spacing.space_m)
            .align_x(Alignment::Center)
            .max_width(width);
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
        // Scrolls when it doesn't fit (a recovery key and its explanation).
        widget::container(widget::scrollable(widget::container(col).center_x(Length::Fill).padding(spacing.space_l)))
            .center_y(Length::Fill)
            .into()
    }
}

/// What the "Choose your apps" page offers, in order: (Store id, or
/// `chrome`; a few plain words; ticked at first; a browser).
const PICKS: &[(&str, &str, bool, bool)] = &[
    // Chrome is the browser most people know: ticked (where Google makes it,
    // Intel and AMD; elsewhere Firefox is, see `picks`).
    ("org.debian.firefox-esr", "Fast, private browsing", false, true),
    (CHROME, "Google's browser", true, true),
    ("org.debian.chromium", "Chrome without Google's extras", false, true),
    ("org.debian.libreoffice-writer", "Documents, like Word", true, false),
    ("org.debian.libreoffice-calc", "Spreadsheets, like Excel", true, false),
    ("org.debian.libreoffice-impress", "Slides, like PowerPoint", false, false),
    ("org.debian.thunderbird", "Email and calendar", false, false),
    ("org.debian.vlc-bin", "Plays any video or music", true, false),
    // Iliada comes with Photos and Document Reader: these are extras.
    ("org.debian.loupe", "Another photo viewer", false, false),
    ("org.debian.evince", "Read and fill in PDFs", false, false),
    ("org.debian.gimp", "Edit photos", false, false),
    ("org.debian.inkscape", "Drawings and logos", false, false),
    ("org.debian.audacity", "Record and edit sound", false, false),
    ("org.debian.gnome-clocks", "Alarms, timers, world clocks", false, false),
    ("org.debian.file-roller", "Open and make zip files", false, false),
    ("org.debian.transmission-gtk", "Download torrents", false, false),
    ("org.debian.gnome-mines", "A quick game", false, false),
    ("org.debian.gnome-sudoku", "A number puzzle", false, false),
];
const CHROME: &str = "chrome";
/// Google's .deb is about this big; it isn't in the catalog.
const CHROME_SIZE: u64 = 120_000_000;

/// One app on the "Choose your apps" page.
#[derive(Debug, Clone, PartialEq)]
struct Pick {
    /// Store id, or `chrome`.
    key: String,
    /// What `iliada install` is given, and what the person reads.
    name: String,
    blurb: &'static str,
    size: u64,
    icon: Option<String>,
    browser: bool,
    /// Ticked when the page opens.
    default: bool,
    installed: bool,
}

/// The page's apps: those of [`PICKS`] this computer can get.
fn picks(apps: &[AppEntry], installed: &BTreeSet<String>, arch: &str) -> Vec<Pick> {
    PICKS
        .iter()
        .filter_map(|&(key, blurb, default, browser)| {
            let (name, size, icon) = if key == CHROME {
                // Google makes Chrome for Intel/AMD only.
                (arch == "x86_64").then(|| ("Google Chrome".to_owned(), CHROME_SIZE, None))?
            } else {
                let a = apps.iter().find(|a| a.id == key)?;
                (a.name.clone(), a.local_archive().map_or(0, |x| x.size), a.icon.clone())
            };
            let installed = installed.contains(key) || installed.contains(&name);
            // No Chrome here (ARM): Firefox is the browser ticked instead.
            let default = default || (key == "org.debian.firefox-esr" && arch != "x86_64");
            Some(Pick { key: key.to_owned(), name, blurb, size, icon, browser, default, installed })
        })
        .collect()
}

async fn load_picks() -> Result<Vec<Pick>, String> {
    let store = StoreConfig::from_env().and_then(Store::new).map_err(|e| format!("{e:#}"))?;
    let catalog = store.catalog().await.map_err(|e| format!("{e:#}"))?;
    // Both ids and names: apps built on this computer (Chrome) have no Store id.
    let installed = store.installed().into_values().flat_map(|i| [i.id, i.name]).collect();
    Ok(picks(&catalog.apps, &installed, std::env::consts::ARCH))
}

/// "About 850 MB", "About 1.2 GB".
fn download_size(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("About {:.1} GB", bytes as f64 / 1e9)
    } else {
        format!("About {} MB", (bytes / 10_000_000).max(1) * 10)
    }
}

/// "Firefox", "Firefox and VLC", "Firefox, VLC and GIMP".
fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Installs `names` after this window has closed: a user service, so it
/// carries on alone and ends with a notification.
fn install_in_background(names: &[String]) -> std::io::Result<()> {
    let iliada = "/run/current-system/sw/bin/iliada";
    let iliada = if std::path::Path::new(iliada).exists() { iliada } else { "iliada" };
    // The browser chosen (Chrome first) opens links and sits in the dock.
    let browser = ["Google Chrome", "Firefox", "Chromium"].into_iter().find(|b| names.iter().any(|n| n == b));
    let mut install = vec![iliada, "install", "--build", "--keep-going", "--notify"];
    if let Some(b) = browser {
        install.extend(["--default-browser", b]);
    }
    install.push("--");
    let started = std::process::Command::new("systemd-run")
        .args([
            "--user",
            "--collect",
            "--quiet",
            "--unit=iliada-first-apps",
            "--description=Installing the apps chosen in the Setup Assistant",
            // From this environment: user services have a short PATH.
            "--setenv=PATH",
        ])
        .args(&install)
        .args(names)
        .status()
        .is_ok_and(|s| s.success());
    if started {
        return Ok(());
    }
    std::process::Command::new(install[0])
        .args(&install[1..])
        .args(names)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(drop)
}

/// A card that can be ticked: an accent border when it is.
fn card_class(selected: bool) -> cosmic::theme::Button {
    let style = move |hovered: bool, theme: &cosmic::Theme| {
        let c = theme.cosmic();
        let component = &theme.cosmic().primary(theme.transparent).component;
        let mut s = widget::button::Style::new();
        s.background = Some(Background::Color(if hovered { component.hover } else { component.base }.into()));
        s.border_radius = c.corner_radii.radius_m.into();
        s.border_width = if selected { 2.0 } else { 1.0 };
        s.border_color = if selected { c.accent.base.into() } else { component.divider.into() };
        s.text_color = Some(component.on.into());
        s.icon_color = Some(if selected { c.accent.base.into() } else { component.on.into() });
        s
    };
    cosmic::theme::Button::Custom {
        active: Box::new(move |_, t| style(false, t)),
        disabled: Box::new(move |t| style(false, t)),
        hovered: Box::new(move |_, t| style(true, t)),
        pressed: Box::new(move |_, t| style(true, t)),
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
            account: Account::default(),
            picks: None,
            picked: BTreeSet::new(),
            icons: BTreeMap::new(),
        };
        app.set_header_title("Welcome".into());
        let title = match app.core.main_window_id() {
            Some(id) => app.set_window_title("Welcome to Iliada".into(), id),
            None => Task::none(),
        };
        let status =
            cosmic::task::future(async { Message::Status(client::status().await.map_err(|e| format!("{e:#}"))) });
        // Asked now, so the apps page is ready when the person gets there.
        let picks = cosmic::task::future(async { Message::Picks(load_picks().await) });
        (app, Task::batch([title, status, picks]))
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Status(s) => {
                if let Ok(s) = &s
                    && self.name.is_empty()
                {
                    self.name = s.hostname.clone();
                }
                self.status = s.ok();
            }
            Message::Go(step) => {
                self.error = None;
                if step == Step::Account {
                    if !self.account.available() {
                        return self.update(Message::Go(Step::Choice));
                    }
                    self.step = step;
                    return self.account.refresh().map(account_action);
                }
                if self.step == Step::Account && self.account.signed_in() {
                    self.summary.push("Your settings are backed up to your Iliada account, encrypted.".into());
                }
                self.step = match step {
                    // Nothing to restore from or back up to: on to the apps, then the end.
                    Step::Choice if !self.backup_possible() => Step::Apps,
                    Step::NameBackup if !self.backup_possible() => Step::Done,
                    step => step,
                };
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
            Message::Account(m) => return self.account.update(m).map(account_action),
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
            Message::Picks(p) => {
                let mut tasks = Vec::new();
                if let Ok(list) = &p {
                    self.picked = list.iter().filter(|p| p.default && !p.installed).map(|p| p.name.clone()).collect();
                    for pick in list {
                        let Some(url) = pick.icon.clone() else { continue };
                        let (key, svg) = (pick.key.clone(), url.ends_with(".svg"));
                        tasks.push(cosmic::task::future(async move {
                            let bytes =
                                async { reqwest::get(&url).await.ok()?.bytes().await.ok().map(|b| b.to_vec()) }.await;
                            Message::Icon(key, bytes, svg)
                        }));
                    }
                }
                self.picks = Some(p);
                return Task::batch(tasks);
            }
            Message::Icon(key, Some(bytes), svg) => {
                let handle =
                    if svg { widget::icon::from_svg_bytes(bytes) } else { widget::icon::from_raster_bytes(bytes) };
                self.icons.insert(key, handle);
            }
            Message::Icon(_, None, _) => {}
            Message::Toggle(name) => {
                if !self.picked.remove(&name) {
                    self.picked.insert(name);
                }
            }
            Message::InstallApps => {
                let mut names = self.chosen();
                // Chrome is built on this computer: after the quick Store downloads.
                names.sort_by_key(|n| n == "Google Chrome");
                if !names.is_empty() {
                    if let Err(e) = install_in_background(&names) {
                        self.error = Some(format!("Could not start installing the apps: {e}"));
                        return Task::none();
                    }
                    let are = if names.len() == 1 { "is" } else { "are" };
                    self.summary.push(format!(
                        "{} {are} downloading. Each one appears in Applications when it's ready.",
                        and_list(&names)
                    ));
                }
                return self.update(Message::Go(Step::NameBackup));
            }
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
                vec![widget::button::suggested("Continue").on_press(Message::Go(Step::Account)).into()],
            ),
            Step::Account => {
                let next = if self.account.in_flow() {
                    None
                } else if self.account.signed_in() {
                    Some(widget::button::suggested("Continue"))
                } else {
                    Some(widget::button::standard("Not now"))
                };
                self.page(
                    "Your Iliada account",
                    "Sign in to keep your desktop settings, Privacy answers and conversations with Homer \
                     backed up, encrypted so only you can read them. New here? Signing in makes your account.",
                    std::iter::once(self.account.view_setup().map(Message::Account))
                        .chain(next.map(|b| b.on_press(Message::Go(Step::Choice)).into()))
                        .collect(),
                )
            }
            Step::Choice => self.page(
                "Have you used Iliada before?",
                "If you backed up another computer to GitHub, bring its apps and settings here.",
                vec![buttons(vec![
                    widget::button::suggested("Restore from a backup").on_press(Message::SignIn(Step::PickBackup)),
                    widget::button::standard("Set up as new").on_press(Message::Go(Step::Apps)),
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
                content.push(widget::button::text("Set up as new instead").on_press(Message::Go(Step::Apps)).into());
                self.page("Choose a backup", "Its apps and settings replace this computer's.", content)
            }
            Step::Apps => {
                // After the apps: the backup, when there can be one.
                let mut content: Vec<Element<_>> = Vec::new();
                match &self.picks {
                    None => {
                        content.push(widget::progress_bar::circular::Circular::new().size(24.0).into());
                        content.push(widget::text::caption("Looking in the Store…").into());
                        content.push(widget::button::text("Skip").on_press(Message::Go(Step::NameBackup)).into());
                    }
                    Some(Err(_)) => {
                        content.push(
                            widget::text::body(
                                "The Store can't be reached right now (is the internet connected?). \
                                 Get apps from the Store in the dock any time.",
                            )
                            .into(),
                        );
                        content
                            .push(widget::button::suggested("Continue").on_press(Message::Go(Step::NameBackup)).into());
                    }
                    Some(Ok(picks)) => {
                        for (label, browser) in [("Browser", true), ("Apps", false)] {
                            let cards: Vec<Element<_>> =
                                picks.iter().filter(|p| p.browser == browser).map(|p| self.card(p)).collect();
                            if cards.is_empty() {
                                continue;
                            }
                            content.push(
                                widget::column::with_capacity(2)
                                    .push(widget::text::title4(label))
                                    .push(
                                        widget::flex_row(cards)
                                            .column_spacing(spacing.space_xs)
                                            .row_spacing(spacing.space_xs),
                                    )
                                    .spacing(spacing.space_xs)
                                    .width(Length::Fill)
                                    .into(),
                            );
                        }
                        let chosen = self.chosen();
                        let size: u64 = picks.iter().filter(|p| chosen.contains(&p.name)).map(|p| p.size).sum();
                        content.push(
                            widget::text::caption(if chosen.is_empty() {
                                "Nothing to download.".to_owned()
                            } else {
                                format!("{} to download, while you carry on.", download_size(size))
                            })
                            .into(),
                        );
                        content.push(buttons(vec![
                            widget::button::suggested(if chosen.is_empty() {
                                "Continue"
                            } else {
                                "Install and continue"
                            })
                            .on_press(Message::InstallApps),
                        ]));
                    }
                }
                self.page_with(
                    "Choose your apps",
                    "Pick what you'd like to start with. Each app comes sandboxed and sees only what you allow. \
                     The Store has more.",
                    content,
                    760.0,
                )
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
                content.push(widget::text::caption("Change these any time in Settings.").into());
                content.push(widget::button::suggested("Start using Iliada").on_press(Message::Finish).into());
                self.page("You're all set", "Find apps in Applications and the Store, both in the dock.", content)
            }
        }
    }
}

fn account_action(m: account::Msg) -> cosmic::Action<Message> {
    cosmic::Action::App(Message::Account(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, name: &str, size: u64) -> AppEntry {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": name, "summary": "", "category": "", "version": "1",
            "archives": { std::env::consts::ARCH: { "url": "https://x/a", "sha256": "0", "size": size } }
        }))
        .unwrap()
    }

    #[test]
    fn the_page_offers_what_the_store_has() {
        let apps = [app("org.debian.vlc-bin", "VLC", 170_000_000), app("org.debian.firefox-esr", "Firefox", 1)];
        let installed = BTreeSet::from(["VLC".to_owned()]);
        let p = picks(&apps, &installed, "x86_64");
        let names: Vec<_> = p.iter().map(|p| p.name.as_str()).collect();
        // In the page's order; Chrome is offered on Intel/AMD even though it isn't in the Store.
        assert_eq!(names, ["Firefox", "Google Chrome", "VLC"]);
        // Chrome is the ticked browser on Intel/AMD; Firefox is offered.
        assert!(p[0].browser && !p[0].default && !p[0].installed);
        assert!(p[1].browser && p[1].default);
        assert!(p[2].installed && p[2].size == 170_000_000);
        // No Chrome for ARM.
        let arm = picks(&apps, &installed, "aarch64");
        assert!(arm.iter().all(|p| p.key != CHROME));
        assert!(arm[0].default, "no Chrome on ARM: Firefox is ticked");
    }

    #[test]
    fn sizes_and_lists_read_plainly() {
        assert_eq!(download_size(847_000_000), "About 840 MB");
        assert_eq!(download_size(1_234_000_000), "About 1.2 GB");
        assert_eq!(download_size(5), "About 10 MB");
        assert_eq!(and_list(&["Firefox".into(), "VLC".into(), "GIMP".into()]), "Firefox, VLC and GIMP");
    }
}
