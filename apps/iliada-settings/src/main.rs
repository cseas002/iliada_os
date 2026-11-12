//! Iliada Settings: updates, backup, command-line tools, services, about.
//!
//! A COSMIC app over the Iliada system service. Display, network, sound and
//! the rest stay in COSMIC's own Settings; this is what is Iliada's.

use cosmic::app::{Settings, Task};
use cosmic::iced::{Alignment, Length, Size};
use cosmic::widget::{self, nav_bar, settings};
use cosmic::{ApplicationExt, Core, Element, executor};
use futures_util::StreamExt;
use iliada_bundle::decisions::{Decision, Decisions};
use iliada_settings::{Progress, ago, open_url, run};
use iliada_system::client;
use iliada_system::protocol::{AssistantMode, AssistantSettings, Backup, Event, Provider, Request, Status, model_name};

/// Models offered in Settings (the first is the default). Claude models use
/// an Anthropic key; "vendor/model" ones an OpenRouter key.
const MODELS: [&str; 5] =
    ["claude-opus-5-5", "claude-fable-5-1", "claude-sonnet-5-5", "claude-haiku-5-5", "deepseek/deepseek-v4-flash"];
/// The models as people read them, in the same order.
static MODEL_NAMES: std::sync::LazyLock<Vec<String>> =
    std::sync::LazyLock::new(|| MODELS.iter().map(|m| model_name(m)).collect());
use iliada_assistant::email::{EmailAccount, Security};
use iliada_types::{Capability, FileScope};

const SECURITY_NAMES: [&str; 2] = ["SSL/TLS (port 465)", "STARTTLS (port 587)"];

fn security_index(s: Security) -> usize {
    usize::from(s == Security::Starttls)
}

/// The email account being edited in Settings → Assistant.
#[derive(Debug, Default)]
struct EmailForm {
    address: String,
    server: String,
    security: Security,
    password: String,
    /// The server was filled in from the address (so it follows it).
    suggested: bool,
}

impl EmailForm {
    fn load() -> Self {
        match iliada_assistant::email::load() {
            Some(a) => EmailForm { address: a.address, server: a.server, security: a.security, ..Default::default() },
            None => EmailForm::default(),
        }
    }
}

fn main() -> cosmic::iced::Result {
    let settings = Settings::default().size(Size::new(900.0, 680.0));
    cosmic::app::run::<App>(settings, ())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Updates,
    Backup,
    Tools,
    Services,
    Privacy,
    Assistant,
    About,
}

/// An app's permissions that are asked for (and can be changed here).
#[derive(Debug, Clone)]
struct AppPermissions {
    id: String,
    name: String,
    capabilities: Vec<Capability>,
}

fn askable(c: &Capability) -> bool {
    matches!(c, Capability::Camera | Capability::Microphone | Capability::Files(FileScope::KnownDir(_)))
}

fn extra_isolation_path() -> Option<std::path::PathBuf> {
    let home = std::path::PathBuf::from(std::env::var_os("HOME")?);
    let cfg = std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from).unwrap_or_else(|| home.join(".config"));
    Some(cfg.join("iliada/extra-isolation"))
}

/// Apps the user put in their own micro-VM (`iliada-run` reads the same file).
fn load_extra_isolation() -> Vec<String> {
    extra_isolation_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| t.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect())
        .unwrap_or_default()
}

fn save_extra_isolation(ids: &[String]) -> std::io::Result<()> {
    let Some(p) = extra_isolation_path() else { return Ok(()) };
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(p, ids.iter().map(|i| format!("{i}\n")).collect::<String>())
}

/// Installed apps, with the permissions they can be asked for.
fn installed_permissions() -> Vec<AppPermissions> {
    let mut out: Vec<AppPermissions> = std::fs::read_dir("/Applications")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| iliada_bundle::Bundle::open(e.path()).ok())
        .map(|b| {
            let capabilities: Vec<Capability> =
                b.permissions.requests.iter().map(|r| r.capability).filter(askable).collect();
            AppPermissions { id: b.info.id.as_str().to_owned(), name: b.info.name.clone(), capabilities }
        })
        .collect();
    out.sort_by_key(|a| a.name.to_lowercase());
    out
}

fn capability_label(c: &Capability) -> String {
    match c {
        Capability::Camera => "Camera".into(),
        Capability::Microphone => "Microphone".into(),
        Capability::Files(FileScope::KnownDir(d)) => format!("{} folder", d.name()),
        other => other.to_string(),
    }
}

#[derive(Debug, Clone)]
enum Confirm {
    Restore(String),
    SignOut,
}

// Messages are moved around once each; boxing the large one buys nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
enum Message {
    Refresh,
    Status(Result<Status, String>),
    Run(Request),
    Progress(Progress),
    ToolInput(String),
    AddTool,
    BackupName(String),
    OpenSignIn,
    LoadBackups,
    Backups(Result<Vec<Backup>, String>),
    Ask(Confirm),
    Permission(String, Capability, bool),
    ExtraIsolation(String, bool),
    AssistantSet(AssistantSettings),
    AssistantKey(String),
    SaveKey,
    EmailAddress(String),
    EmailServer(String),
    EmailSecurity(usize),
    EmailPassword(String),
    SaveEmail,
    RemoveEmail,
    Confirmed,
    Cancel,
    Dismiss,
}

struct Job {
    request: Request,
    line: String,
}

struct App {
    core: Core,
    nav: nav_bar::Model,
    status: Option<Result<Status, String>>,
    job: Option<Job>,
    sign_in: Option<(String, String)>,
    error: Option<String>,
    notice: Option<String>,
    tool_input: String,
    backup_name: String,
    backups: Option<Result<Vec<Backup>, String>>,
    confirm: Option<Confirm>,
    privacy: Vec<AppPermissions>,
    decisions: Decisions,
    extra_isolation: Vec<String>,
    key_input: String,
    email: EmailForm,
}

fn load_status() -> Task<Message> {
    cosmic::task::future(async { Message::Status(client::status().await.map_err(|e| format!("{e:#}"))) })
}

fn load_backups() -> Task<Message> {
    cosmic::task::future(async {
        Message::Backups(client::call_as(&Request::BackupList, &mut |_| {}).await.map_err(|e| format!("{e:#}")))
    })
}

/// What to tell the user when a request succeeds.
fn success(request: &Request, result: &serde_json::Value) -> Option<String> {
    let changed = result.get("changed").and_then(|c| c.as_bool()).unwrap_or(true);
    Some(match request {
        Request::AddTools { packages } if changed => format!("Installed {}.", packages.join(", ")),
        Request::RemoveTools { packages } if changed => format!("Removed {}.", packages.join(", ")),
        Request::SetService { .. } | Request::Apply if changed => "Settings applied.".into(),
        Request::UpdateApps => "Apps are up to date.".into(),
        Request::UpdateSystem => match result["updated"].as_bool() {
            Some(true) => format!(
                "Iliada {} is installed. Restart the computer to finish.",
                result["version"].as_str().unwrap_or("")
            ),
            _ => "Iliada is up to date.".into(),
        },
        Request::CheckSystemUpdate => match result["newer"].as_bool() {
            Some(true) => format!("Iliada {} is available.", result["latest"].as_str().unwrap_or("")),
            _ => "Iliada is up to date.".into(),
        },
        Request::BackupSignIn => format!("Signed in as {}.", result["account"].as_str().unwrap_or("")),
        Request::BackupEnable { .. } | Request::BackupNow => "Backed up.".into(),
        Request::Restore { .. } => "Restored. Your apps are being installed in the background.".into(),
        Request::BackupSignOut => "Signed out. Backups are off.".into(),
        Request::SetAssistant { .. } if changed => "Saved.".into(),
        _ => return None,
    })
}

impl App {
    fn page(&self) -> Page {
        self.nav.active_data::<Page>().copied().unwrap_or(Page::Updates)
    }

    fn status(&self) -> Option<&Status> {
        self.status.as_ref().and_then(|s| s.as_ref().ok())
    }

    /// Changes are possible: an administrator, and nothing running.
    fn can_change(&self) -> bool {
        self.job.is_none() && self.status().is_some_and(|s| s.admin && s.busy.is_none())
    }

    fn start(&mut self, request: Request) -> Task<Message> {
        if self.job.is_some() {
            return Task::none();
        }
        self.error = None;
        self.notice = None;
        self.job = Some(Job { request: request.clone(), line: String::new() });
        cosmic::task::stream(run(request).map(Message::Progress))
    }

    fn banner(&self) -> Option<Element<'_, Message>> {
        let spacing = cosmic::theme::spacing();
        if let Some(job) = &self.job {
            let what = match job.request.activity() {
                "" => "Working",
                w => w,
            };
            let mut col = widget::column::with_capacity(2).push(widget::text::heading(format!("{what}…")));
            if !job.line.is_empty() {
                col = col.push(widget::text::caption(job.line.as_str()));
            }
            let row = widget::row::with_capacity(2)
                .push(widget::progress_bar::circular::Circular::new().size(20.0))
                .push(col.spacing(spacing.space_xxxs).width(Length::Fill))
                .spacing(spacing.space_s)
                .align_y(Alignment::Center);
            return Some(card(row.into()));
        }
        let (text, is_error) = match (&self.error, &self.notice) {
            (Some(e), _) => (e.as_str(), true),
            (None, Some(n)) => (n.as_str(), false),
            _ => return None,
        };
        let icon = if is_error { "dialog-error-symbolic" } else { "emblem-ok-symbolic" };
        let row = widget::row::with_capacity(3)
            .push(widget::icon::from_name(icon).size(20).icon())
            .push(widget::text::body(text).width(Length::Fill))
            .push(widget::button::icon(widget::icon::from_name("window-close-symbolic")).on_press(Message::Dismiss))
            .spacing(spacing.space_s)
            .align_y(Alignment::Center);
        Some(card(row.into()))
    }

    fn updates(&self, s: &Status) -> Element<'_, Message> {
        let ok = self.can_change();
        let u = &s.system_update;
        let newer = u.latest.as_ref().filter(|l| iliada_system::update::newer(l, &s.version));
        let state = match (newer, u.last_check, &u.last_error) {
            (Some(l), _, _) => format!("Iliada {l} is available."),
            (None, _, Some(e)) => format!("Could not check for updates: {e}"),
            (None, Some(t), None) => format!("Up to date (checked {}).", ago(t)),
            (None, None, None) => "Iliada looks for updates every day.".into(),
        };
        let action = match newer {
            Some(_) => widget::button::suggested("Update now")
                .on_press_maybe(ok.then_some(Message::Run(Request::UpdateSystem))),
            None => widget::button::standard("Check now")
                .on_press_maybe(ok.then_some(Message::Run(Request::CheckSystemUpdate))),
        };
        let system = settings::section()
            .title("Iliada")
            .add(settings::item::builder(format!("Iliada {}", s.version)).description(state).control(action));
        let apps = settings::section().title("Apps").add(
            settings::item::builder("App updates")
                .description("Apps from the Store update automatically, every few hours.")
                .control(
                    widget::button::standard("Update apps now")
                        .on_press_maybe(ok.then_some(Message::Run(Request::UpdateApps))),
                ),
        );
        settings::view_column(vec![system.into(), apps.into()]).into()
    }

    fn backup(&self, s: &Status) -> Element<'_, Message> {
        let ok = self.can_change();
        let b = &s.backup;
        let intro = "Your apps and settings, kept in a private repository on your GitHub account. \
                     On a new computer, restore them in a few clicks.";
        let mut items: Vec<Element<_>> = Vec::new();
        if !b.available {
            items.push(
                settings::section()
                    .title("Backup")
                    .add(info(
                        "Backup is not available",
                        Some(
                            "This system has no GitHub app configured for backups (iliada.system.githubClientId)."
                                .into(),
                        ),
                    ))
                    .into(),
            );
            return settings::view_column(items).into();
        }
        if let Some((code, url)) = &self.sign_in {
            let spacing = cosmic::theme::spacing();
            let col = widget::column::with_capacity(4)
                .push(widget::text::body("Open GitHub and enter this code:"))
                .push(widget::text::title1(code.as_str()))
                .push(widget::button::suggested("Open GitHub").on_press(Message::OpenSignIn))
                .push(widget::text::caption(format!("{url} · Waiting for you to approve it…")))
                .spacing(spacing.space_s)
                .align_x(Alignment::Center);
            items.push(card(col.width(Length::Fill).into()));
            return settings::view_column(items).into();
        }
        match (&b.account, &b.repo) {
            (None, _) => {
                items.push(
                    settings::section()
                        .title("Backup")
                        .add(
                            settings::item::builder("Back up to GitHub").description(intro).control(
                                widget::button::suggested("Sign in with GitHub")
                                    .on_press_maybe(ok.then_some(Message::Run(Request::BackupSignIn))),
                            ),
                        )
                        .into(),
                );
            }
            (Some(account), None) => {
                let name_ok = iliada_system::github::valid_name(&self.backup_name);
                let input = widget::text_input("laptop", &self.backup_name)
                    .on_input(Message::BackupName)
                    .on_submit(|_| Message::Run(Request::BackupEnable { name: String::new() }))
                    .width(Length::Fixed(220.0));
                let start = widget::button::suggested("Back up").on_press_maybe(
                    (ok && name_ok).then(|| Message::Run(Request::BackupEnable { name: self.backup_name.clone() })),
                );
                let row = widget::row::with_capacity(3)
                    .push(widget::text::body(github_prefix(account)))
                    .push(input)
                    .push(start)
                    .spacing(cosmic::theme::spacing().space_xs)
                    .align_y(Alignment::Center);
                items.push(
                    settings::section()
                        .title("Backup")
                        .add(info("Name this backup", Some(intro.into())))
                        .add(settings::item_row(vec![row.into()]))
                        .add(
                            settings::item::builder(format!("Signed in as {account}")).control(
                                widget::button::standard("Sign out")
                                    .on_press_maybe(ok.then_some(Message::Ask(Confirm::SignOut))),
                            ),
                        )
                        .into(),
                );
                items.push(self.restore_list(ok));
            }
            (Some(account), Some(repo)) => {
                let last = match (b.last_backup, &b.last_error) {
                    (_, Some(e)) => format!("The last backup failed: {e}"),
                    (Some(t), None) => format!("Last backup: {}.", ago(t)),
                    (None, None) => "Not backed up yet.".into(),
                };
                items.push(
                    settings::section()
                        .title("Backup")
                        .add(
                            settings::item::builder(format!("Backing up to github.com/{repo}"))
                                .description(format!("{last} Iliada backs up after every change, and every hour."))
                                .control(
                                    widget::button::standard("Back up now")
                                        .on_press_maybe(ok.then_some(Message::Run(Request::BackupNow))),
                                ),
                        )
                        .add(
                            settings::item::builder(format!("Signed in as {account}")).control(
                                widget::button::standard("Sign out")
                                    .on_press_maybe(ok.then_some(Message::Ask(Confirm::SignOut))),
                            ),
                        )
                        .into(),
                );
                items.push(self.restore_list(ok));
            }
        }
        settings::view_column(items).into()
    }

    fn restore_list(&self, ok: bool) -> Element<'_, Message> {
        let mut section = settings::section().title("Restore");
        match &self.backups {
            None => {
                section = section.add(
                    settings::item::builder("Restore from a backup")
                        .description("Bring back the apps and settings of another computer.")
                        .control(widget::button::standard("Show backups").on_press(Message::LoadBackups)),
                )
            }
            Some(Err(e)) => {
                section = section.add(
                    settings::item::builder("Could not list your backups")
                        .description(e.as_str())
                        .control(widget::button::standard("Retry").on_press(Message::LoadBackups)),
                )
            }
            Some(Ok(list)) if list.is_empty() => section = section.add(info("No backups yet", None)),
            Some(Ok(list)) => {
                for b in list {
                    let updated = b.updated.split('T').next().unwrap_or("").to_owned();
                    section = section.add(
                        settings::item::builder(b.name.clone())
                            .description(format!("github.com/{} · last backup {updated}", b.repo))
                            .control(
                                widget::button::standard("Restore")
                                    .on_press_maybe(ok.then(|| Message::Ask(Confirm::Restore(b.repo.clone())))),
                            ),
                    );
                }
            }
        }
        section.into()
    }

    fn tools(&self, s: &Status) -> Element<'_, Message> {
        let ok = self.can_change();
        let mut installed = settings::section().title("Installed tools");
        if s.tools.is_empty() {
            installed = installed.add(info("No tools installed", None));
        }
        for t in &s.tools {
            installed = installed.add(
                settings::item::builder(t.clone()).control(
                    widget::button::standard("Remove")
                        .on_press_maybe(ok.then(|| Message::Run(Request::RemoveTools { packages: vec![t.clone()] }))),
                ),
            );
        }
        let valid = iliada_system::settings::valid_tool_name(self.tool_input.trim());
        let input = widget::text_input("Package name, e.g. ripgrep", &self.tool_input)
            .on_input(Message::ToolInput)
            .on_submit(|_| Message::AddTool)
            .width(Length::Fill);
        let add = widget::row::with_capacity(2)
            .push(input)
            .push(widget::button::suggested("Install").on_press_maybe((ok && valid).then_some(Message::AddTool)))
            .spacing(cosmic::theme::spacing().space_xs)
            .align_y(Alignment::Center);
        let add = settings::section()
            .title("Add a tool")
            .add(info(
                "Command-line tools",
                Some("Programs for the Terminal, like vim or git, installed for everyone on this computer. Apps with windows come from the Store.".into()),
            ))
            .add(settings::item_row(vec![add.into()]));
        settings::view_column(vec![installed.into(), add.into()]).into()
    }

    fn services(&self, s: &Status) -> Element<'_, Message> {
        let ok = self.can_change();
        let mut section = settings::section().title("Services");
        for svc in &s.services {
            let name = svc.name.clone();
            let toggle = ok.then_some(move |on| Message::Run(Request::SetService { name: name.clone(), on }));
            section = section.add(settings::item::builder(svc.description.clone()).toggler_maybe(svc.on, toggle));
        }
        settings::view_column(vec![section.into()]).into()
    }

    fn privacy(&self) -> Element<'_, Message> {
        let mut items: Vec<Element<_>> = vec![info(
            "What apps may use",
            Some(
                "Every app runs in its own sandbox and sees only its own files. Apps ask the first time \
                 they need a folder, the camera or the microphone. Changes apply the next time the app opens."
                    .into(),
            ),
        )];
        if self.privacy.is_empty() {
            items.push(info("No apps are installed yet", None));
        }
        for app in &self.privacy {
            let mut section = settings::section().title(app.name.clone());
            for cap in &app.capabilities {
                let decided = self.decisions.get(&app.id, cap);
                let (id, c) = (app.id.clone(), *cap);
                let mut item = settings::item::builder(capability_label(cap));
                if decided.is_none() {
                    item = item.description("Not asked yet");
                }
                section = section.add(
                    item.toggler(decided == Some(Decision::Allow), move |on| Message::Permission(id.clone(), c, on)),
                );
            }
            let id = app.id.clone();
            section = section.add(
                settings::item::builder("Extra isolation")
                    .description("Run it in its own virtual machine: stronger isolation, slower start and graphics, no internet or sound yet.")
                    .toggler(self.extra_isolation.contains(&app.id), move |on| Message::ExtraIsolation(id.clone(), on)),
            );
            items.push(section.into());
        }
        settings::view_column(items).into()
    }

    fn assistant(&self, s: &Status) -> Element<'_, Message> {
        let ok = self.can_change();
        let a = s.assistant.clone();
        let on = {
            let a = a.clone();
            ok.then_some(move |enabled| Message::AssistantSet(AssistantSettings { enabled, ..a.clone() }))
        };
        let main = settings::section().title("Assistant").add(
            settings::item::builder("Use the assistant")
                .description("Ask Iliada to do things for you in plain words, in the Assistant app. It uses Claude (Anthropic) or a model on OpenRouter, with your own API key.")
                .toggler_maybe(a.enabled, on),
        );
        let mut items: Vec<Element<_>> = vec![main.into()];
        if a.enabled {
            let mode_item = |mode: AssistantMode, title: &'static str, desc: &'static str| {
                let next = AssistantSettings { mode, ..a.clone() };
                settings::item::builder(title).description(desc).control(widget::radio(
                    widget::text::body(""),
                    mode,
                    Some(a.mode),
                    move |_| Message::AssistantSet(next.clone()),
                ))
            };
            let mut modes = settings::section()
                .title("What it may do")
                .add(mode_item(
                    AssistantMode::Ask,
                    "Ask me first",
                    "It shows every action and does it only if you allow it.",
                ))
                .add(mode_item(
                    AssistantMode::Auto,
                    "Act on its own",
                    "It installs, removes and changes things without asking.",
                ));
            if a.mode == AssistantMode::Auto && !a.recommended_for_auto() {
                modes = modes.add(info(
                    format!("{} may make more mistakes on its own", model_name(&a.model)),
                    Some("Opus, Fable and Sonnet are the most careful. Every action is recorded either way.".into()),
                ));
            }
            let full = {
                let a = a.clone();
                ok.then_some(move |full_access| Message::AssistantSet(AssistantSettings { full_access, ..a.clone() }))
            };
            modes = modes.add(
                settings::item::builder("Full access")
                    .description("It may also run commands as you: read and change your files, change desktop settings, anything you can do without an administrator's password.")
                    .toggler_maybe(a.full_access, full),
            );
            items.push(modes.into());
            let selected = MODELS.iter().position(|m| *m == a.model);
            let model_a = a.clone();
            let model = settings::section().title("Model").add(
                settings::item::builder("Model")
                    .description("Opus is the default: careful and reliable. DeepSeek V4 Flash (through OpenRouter) costs a fraction of a cent per request.")
                    .control(widget::dropdown(&MODEL_NAMES[..], selected, move |i| {
                        Message::AssistantSet(AssistantSettings { model: MODELS[i].to_owned(), ..model_a.clone() })
                    })),
            );
            items.push(model.into());
            let key_row = widget::row::with_capacity(2)
                .push(
                    widget::secure_input("sk-ant-… or sk-or-…", self.key_input.as_str(), None, true)
                        .on_input(Message::AssistantKey)
                        .on_submit(|_| Message::SaveKey)
                        .width(Length::Fill),
                )
                .push(widget::button::suggested("Save key").on_press(Message::SaveKey))
                .spacing(cosmic::theme::spacing().space_xs)
                .align_y(Alignment::Center);
            let provider = Provider::of(&a.model);
            let have_key = iliada_assistant::api::load_key(provider).is_some();
            let spent = iliada_assistant::total_spent();
            let mut key = settings::section()
                .title("API key")
                .add(info(
                    if have_key {
                        format!("Your {} API key is saved", provider.name())
                    } else {
                        format!("Add your {} API key", provider.name())
                    },
                    Some(
                        match provider {
                            Provider::Anthropic => "From console.anthropic.com.",
                            Provider::OpenRouter => "From openrouter.ai/keys.",
                        }
                        .to_owned()
                            + " Kept only for you on this computer; never backed up.",
                    ),
                ))
                .add(settings::item_row(vec![key_row.into()]));
            if spent > 0.0 {
                key = key.add(info(format!("Spent so far: ${spent:.4}"), None));
            }
            items.push(key.into());
            items.push(self.email_section());
        }
        settings::view_column(items).into()
    }

    /// The account the assistant sends email from (per user, like the key).
    fn email_section(&self) -> Element<'_, Message> {
        let space = cosmic::theme::spacing().space_xs;
        let saved = iliada_assistant::email::load();
        let mut section = settings::section().title("Email").add(info(
            match &saved {
                Some(a) => format!("The assistant sends email as {}", a.address),
                None => "Let the assistant send email for you".to_owned(),
            },
            Some(
                "From your own email account. You see every email before it is sent. For Gmail, iCloud and Yahoo use an app password from your account's security settings.".to_owned(),
            ),
        ));
        let field = |label: &'static str, input: Element<'static, Message>| -> Element<'static, Message> {
            widget::row::with_capacity(2)
                .push(widget::text::body(label).width(Length::Fixed(140.0)))
                .push(input)
                .spacing(space)
                .align_y(Alignment::Center)
                .into()
        };
        section = section
            .add(settings::item_row(vec![field(
                "Email address",
                widget::text_input("you@example.com", self.email.address.clone())
                    .on_input(Message::EmailAddress)
                    .width(Length::Fill)
                    .into(),
            )]))
            .add(settings::item_row(vec![field(
                "Outgoing server",
                widget::text_input("smtp.example.com", self.email.server.clone())
                    .on_input(Message::EmailServer)
                    .width(Length::Fill)
                    .into(),
            )]))
            .add(settings::item_row(vec![field(
                "Connection",
                widget::dropdown(
                    &SECURITY_NAMES[..],
                    Some(security_index(self.email.security)),
                    Message::EmailSecurity,
                )
                .into(),
            )]))
            .add(settings::item_row(vec![field(
                "Password",
                widget::secure_input(
                    if iliada_assistant::email::has_password() { "Saved (type to change)" } else { "Password" },
                    self.email.password.clone(),
                    None,
                    true,
                )
                .on_input(Message::EmailPassword)
                .on_submit(|_| Message::SaveEmail)
                .width(Length::Fill)
                .into(),
            )]));
        let mut buttons = widget::row::with_capacity(2).spacing(space);
        if saved.is_some() {
            buttons = buttons.push(widget::button::standard("Remove").on_press(Message::RemoveEmail));
        }
        buttons = buttons.push(widget::button::suggested("Save").on_press(Message::SaveEmail));
        section.add(settings::item_row(vec![widget::space::horizontal().into(), buttons.into()])).into()
    }

    fn about(&self, s: &Status) -> Element<'_, Message> {
        let section = settings::section()
            .title("This computer")
            .add(settings::item("Name", widget::text::body(s.hostname.clone())))
            .add(settings::item("Iliada", widget::text::body(s.version.clone())))
            .add(settings::item("Configuration", widget::text::body("/etc/iliada (every change is recorded)")));
        settings::view_column(vec![section.into()]).into()
    }
}

/// A settings row that only informs.
fn info<'a>(title: impl Into<String>, description: Option<String>) -> Element<'a, Message> {
    let mut col = widget::column::with_capacity(2).push(widget::text::body(title.into()));
    if let Some(d) = description {
        col = col.push(widget::text::caption(d));
    }
    settings::item_row(vec![col.spacing(2).width(Length::Fill).into()]).into()
}

fn github_prefix(account: &str) -> String {
    format!("github.com/{account}/{}", iliada_system::github::PREFIX)
}

fn card(content: Element<'_, Message>) -> Element<'_, Message> {
    widget::container(content)
        .padding(cosmic::theme::spacing().space_s)
        .class(cosmic::theme::Container::Card)
        .width(Length::Fill)
        .into()
}

impl cosmic::Application for App {
    type Executor = executor::Default;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = "org.iliada.Settings";

    fn core(&self) -> &Core {
        &self.core
    }
    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: ()) -> (Self, Task<Message>) {
        let mut nav = nav_bar::Model::default();
        let pages = [
            ("Updates", "software-update-available-symbolic", Page::Updates),
            ("Backup", "document-save-symbolic", Page::Backup),
            ("Command-line tools", "utilities-terminal-symbolic", Page::Tools),
            ("Services", "network-server-symbolic", Page::Services),
            ("Privacy", "security-high-symbolic", Page::Privacy),
            ("Assistant", "user-available-symbolic", Page::Assistant),
            ("About", "help-about-symbolic", Page::About),
        ];
        for (i, (text, icon, page)) in pages.into_iter().enumerate() {
            let e = nav.insert().text(text).icon(widget::icon::from_name(icon)).data(page);
            if i == 0 {
                e.activate();
            }
        }
        let mut app = App {
            core,
            nav,
            status: None,
            job: None,
            sign_in: None,
            error: None,
            notice: None,
            tool_input: String::new(),
            backup_name: String::new(),
            backups: None,
            confirm: None,
            privacy: Vec::new(),
            decisions: Decisions::default(),
            extra_isolation: Vec::new(),
            key_input: String::new(),
            email: EmailForm::load(),
        };
        app.set_header_title("Iliada Settings".into());
        let title = match app.core.main_window_id() {
            Some(id) => app.set_window_title("Iliada Settings".into(), id),
            None => Task::none(),
        };
        (app, Task::batch([title, load_status()]))
    }

    fn nav_model(&self) -> Option<&nav_bar::Model> {
        Some(&self.nav)
    }

    fn on_nav_select(&mut self, id: nav_bar::Id) -> Task<Message> {
        self.nav.activate(id);
        // "Iliada is up to date" belongs to the page it came from. Something
        // still running keeps showing.
        self.error = None;
        self.notice = None;
        if self.page() == Page::Privacy {
            self.privacy = installed_permissions();
            self.decisions = Decisions::default_path().map(|p| Decisions::load(&p)).unwrap_or_default();
            self.extra_isolation = load_extra_isolation();
        }
        load_status()
    }

    fn dialog(&self) -> Option<Element<'_, Message>> {
        let (title, body, action) = match self.confirm.as_ref()? {
            Confirm::Restore(repo) => (
                "Restore this backup?",
                format!(
                    "The apps and settings from github.com/{repo} replace this computer's. \
                     From now on, this computer backs up there."
                ),
                widget::button::destructive("Restore"),
            ),
            Confirm::SignOut => (
                "Sign out of GitHub?",
                "Backups stop. Your backups on GitHub are kept.".to_owned(),
                widget::button::destructive("Sign out"),
            ),
        };
        Some(
            widget::dialog()
                .title(title)
                .body(body)
                .primary_action(action.on_press(Message::Confirmed))
                .secondary_action(widget::button::standard("Cancel").on_press(Message::Cancel))
                .into(),
        )
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Refresh => return load_status(),
            Message::Status(s) => {
                if let (Ok(st), true) = (&s, self.backup_name.is_empty()) {
                    self.backup_name = st.hostname.clone();
                }
                self.status = Some(s);
            }
            Message::Run(Request::BackupEnable { name }) if name.is_empty() => {
                let name = self.backup_name.trim().to_owned();
                if iliada_system::github::valid_name(&name) && self.can_change() {
                    return self.start(Request::BackupEnable { name });
                }
            }
            Message::Run(request) => return self.start(request),
            Message::Progress(Progress::Event(e)) => match e {
                Event::Log { line } => {
                    if let Some(job) = &mut self.job {
                        job.line = line.chars().take(120).collect();
                    }
                }
                Event::SignInCode { code, url } => {
                    open_url(&url);
                    self.sign_in = Some((code, url));
                }
                _ => {}
            },
            Message::Progress(Progress::Finished(result)) => {
                let job = self.job.take();
                self.sign_in = None;
                match (job, result) {
                    (Some(job), Ok(v)) => {
                        self.notice = success(&job.request, &v);
                        if matches!(job.request, Request::AddTools { .. }) {
                            self.tool_input.clear();
                        }
                        if matches!(job.request, Request::BackupSignIn) {
                            return Task::batch([load_status(), load_backups()]);
                        }
                        if matches!(job.request, Request::BackupSignOut) {
                            self.backups = None;
                        }
                    }
                    (_, Err(e)) => self.error = Some(e),
                    (None, Ok(_)) => {}
                }
                return load_status();
            }
            Message::ToolInput(s) => self.tool_input = s,
            Message::AddTool => {
                let name = self.tool_input.trim().to_owned();
                if iliada_system::settings::valid_tool_name(&name) && self.can_change() {
                    return self.start(Request::AddTools { packages: vec![name] });
                }
            }
            Message::BackupName(s) => self.backup_name = s,
            Message::OpenSignIn => {
                if let Some((_, url)) = &self.sign_in {
                    open_url(url);
                }
            }
            Message::LoadBackups => {
                self.backups = None;
                return load_backups();
            }
            Message::Backups(b) => self.backups = Some(b),
            Message::Ask(c) => self.confirm = Some(c),
            Message::AssistantSet(a) => return self.start(Request::SetAssistant { settings: a }),
            Message::AssistantKey(k) => self.key_input = k,
            Message::EmailAddress(a) => {
                // Known providers: fill in their server (until it's edited).
                if self.email.server.is_empty() || self.email.suggested {
                    match EmailAccount::suggested(&a) {
                        Some(s) => {
                            self.email.server = s.server;
                            self.email.security = s.security;
                            self.email.suggested = true;
                        }
                        None if self.email.suggested => {
                            self.email.server.clear();
                            self.email.suggested = false;
                        }
                        None => {}
                    }
                }
                self.email.address = a;
            }
            Message::EmailServer(s) => {
                self.email.server = s;
                self.email.suggested = false;
            }
            Message::EmailSecurity(i) => {
                self.email.security = if i == 1 { Security::Starttls } else { Security::Tls };
            }
            Message::EmailPassword(p) => self.email.password = p,
            Message::SaveEmail => {
                let account = EmailAccount {
                    address: self.email.address.trim().to_owned(),
                    server: self.email.server.trim().to_owned(),
                    security: self.email.security,
                    ..Default::default()
                };
                let password = (!self.email.password.is_empty()).then_some(self.email.password.as_str());
                if password.is_none() && !iliada_assistant::email::has_password() {
                    self.error = Some("Type the email account's password first.".into());
                } else {
                    match iliada_assistant::email::save(&account, password) {
                        Err(e) => self.error = Some(format!("Could not save the email account: {e:#}")),
                        Ok(()) => {
                            self.email.password.clear();
                            self.notice = Some(format!(
                                "The assistant can now send email as {}. Only you can read the password.",
                                account.address
                            ));
                        }
                    }
                }
            }
            Message::RemoveEmail => match iliada_assistant::email::remove() {
                Err(e) => self.error = Some(format!("Could not remove the email account: {e:#}")),
                Ok(()) => {
                    self.email = EmailForm::default();
                    self.notice = Some("The email account was removed.".into());
                }
            },
            Message::SaveKey => {
                let k = self.key_input.trim().to_owned();
                match iliada_assistant::api::save_key(&k) {
                    Err(e) => self.error = Some(format!("Could not save the key: {e:#}")),
                    Ok(p) => {
                        self.key_input.clear();
                        self.notice =
                            Some(format!("{} key saved. Only you can read it; it is never backed up.", p.name()));
                    }
                }
            }
            Message::ExtraIsolation(app, on) => {
                self.extra_isolation.retain(|a| *a != app);
                if on {
                    self.extra_isolation.push(app);
                }
                if let Err(e) = save_extra_isolation(&self.extra_isolation) {
                    self.error = Some(format!("Could not save: {e}"));
                }
            }
            Message::Permission(app, cap, allow) => {
                self.decisions.set(&app, &cap, if allow { Decision::Allow } else { Decision::Deny });
                if let Some(p) = Decisions::default_path()
                    && let Err(e) = self.decisions.save(&p)
                {
                    self.error = Some(format!("Could not save: {e}"));
                }
            }
            Message::Cancel => self.confirm = None,
            Message::Confirmed => {
                return match self.confirm.take() {
                    Some(Confirm::Restore(repo)) => self.start(Request::Restore { repo }),
                    Some(Confirm::SignOut) => self.start(Request::BackupSignOut),
                    None => Task::none(),
                };
            }
            Message::Dismiss => {
                self.error = None;
                self.notice = None;
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();
        let body: Element<_> = match &self.status {
            None => widget::container(widget::text::body("Loading…")).center(Length::Fill).into(),
            Some(Err(e)) => widget::container(
                widget::column::with_capacity(3)
                    .push(widget::text::title4("Settings are not available"))
                    .push(widget::text::body(e.as_str()))
                    .push(widget::button::standard("Try again").on_press(Message::Refresh))
                    .spacing(spacing.space_s)
                    .align_x(Alignment::Center),
            )
            .center(Length::Fill)
            .into(),
            Some(Ok(s)) => {
                let mut col = widget::column::with_capacity(4).spacing(spacing.space_m);
                if let Some(b) = self.banner() {
                    col = col.push(b);
                }
                if !s.admin {
                    col = col.push(card(widget::text::body("Only an administrator can change these settings.").into()));
                } else if let (Some(busy), None) = (&s.busy, &self.job) {
                    col = col.push(card(widget::text::body(format!("Iliada is busy: {busy}…")).into()));
                }
                col = col.push(match self.page() {
                    Page::Updates => self.updates(s),
                    Page::Backup => self.backup(s),
                    Page::Tools => self.tools(s),
                    Page::Services => self.services(s),
                    Page::Privacy => self.privacy(),
                    Page::Assistant => self.assistant(s),
                    Page::About => self.about(s),
                });
                widget::scrollable(widget::container(col.max_width(760)).width(Length::Fill).align_x(Alignment::Center))
                    .height(Length::Fill)
                    .into()
            }
        };
        widget::container(body).padding(spacing.space_m).into()
    }
}
