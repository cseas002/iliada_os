//! Settings → Account (and the Setup Assistant's sign-in step): the Iliada
//! account on this computer, for the person using it. No administrator
//! needed: it is theirs, kept in their home folder.

use cosmic::iced::task::Handle;
use cosmic::iced::{Alignment, Length, Task};
use cosmic::widget::{self, settings};
use cosmic::{Element, theme};
use iliada_account::{self as account, DeviceCode, Files, KeyStep, Manifest, Status};

use crate::{ago, open_url};

#[derive(Debug, Clone)]
pub enum Msg {
    SignIn,
    Code(Result<DeviceCode, String>),
    OpenPage,
    CancelSignIn,
    SignedIn(Result<(String, KeyStep), String>),
    /// The person saved the recovery key shown to them.
    KeySaved,
    ShowKey,
    HideKey,
    CopyKey,
    RecoveryInput(String),
    UseRecoveryKey,
    KeyUsed(Result<(), String>),
    BackUp,
    BackedUp(Result<account::Backup, String>),
    Latest(Result<Option<Manifest>, String>),
    /// Ask first (the page's owner shows a dialog), then `Restore`.
    Restore,
    Restored(Result<(Manifest, usize), String>),
    SignOut,
    SignedOut,
    Dismiss,
}

#[derive(Debug, Default)]
enum Step {
    #[default]
    Idle,
    Asking,
    Code(DeviceCode),
    /// Showing the recovery key: just made (`true`), or asked for again.
    Key(String, bool),
}

#[derive(Debug)]
pub struct Account {
    files: Option<Files>,
    server: Option<String>,
    status: Status,
    step: Step,
    wait: Option<Handle>,
    recovery: String,
    latest: Option<Result<Option<Manifest>, String>>,
    /// The account was made here just now (nothing to restore).
    fresh: bool,
    busy: Option<&'static str>,
    pub error: Option<String>,
    pub notice: Option<String>,
}

impl Default for Account {
    fn default() -> Self {
        let files = Files::user().ok();
        let status = files.as_ref().map(account::status).unwrap_or(Status::SignedOut);
        Account {
            files,
            server: account::default_server(),
            status,
            step: Step::Idle,
            wait: None,
            recovery: String::new(),
            latest: None,
            fresh: false,
            busy: None,
            error: None,
            notice: None,
        }
    }
}

/// An error as a sentence for the page.
fn text_err(e: anyhow::Error) -> String {
    let s = format!("{e:#}");
    let mut c = s.chars();
    match c.next() {
        Some(first) => first.to_uppercase().chain(c).collect(),
        None => s,
    }
}

impl Account {
    /// Whether this computer can use an Iliada account at all.
    pub fn available(&self) -> bool {
        self.server.is_some() || self.status != Status::SignedOut
    }

    pub fn signed_in(&self) -> bool {
        matches!(self.status, Status::Ready { .. })
    }

    /// Reads the saved state again; when signed in, looks for a backup.
    pub fn refresh(&mut self) -> Task<Msg> {
        if let Some(f) = &self.files {
            self.status = account::status(f);
        }
        if self.signed_in() && self.latest.is_none() {
            let files = self.files.clone().expect("signed in");
            return Task::perform(async move { account::latest(&files).await.map_err(text_err) }, Msg::Latest);
        }
        Task::none()
    }

    pub fn update(&mut self, msg: Msg) -> Task<Msg> {
        let Some(files) = self.files.clone() else {
            self.error = Some("There is no home folder to keep the account in.".into());
            return Task::none();
        };
        match msg {
            Msg::SignIn => {
                let Some(server) = self.server.clone() else { return Task::none() };
                self.error = None;
                self.notice = None;
                self.step = Step::Asking;
                return Task::perform(
                    async move { account::start_sign_in(&server).await.map_err(text_err) },
                    Msg::Code,
                );
            }
            Msg::Code(Err(e)) => {
                self.step = Step::Idle;
                self.error = Some(e);
            }
            Msg::Code(Ok(code)) => {
                open_url(code.page());
                let server = self.server.clone().expect("asked it");
                let c = code.clone();
                self.step = Step::Code(code);
                let (task, handle) = Task::perform(
                    async move { account::finish_sign_in(&files, &server, &c).await.map_err(text_err) },
                    Msg::SignedIn,
                )
                .abortable();
                self.wait = Some(handle);
                return task;
            }
            Msg::OpenPage => {
                if let Step::Code(c) = &self.step {
                    open_url(c.page());
                }
            }
            Msg::CancelSignIn => {
                if let Some(h) = self.wait.take() {
                    h.abort();
                }
                self.step = Step::Idle;
            }
            Msg::SignedIn(r) => {
                self.wait = None;
                self.step = Step::Idle;
                self.latest = None;
                match r {
                    Err(e) => self.error = Some(e),
                    Ok((email, KeyStep::Created(key))) => {
                        self.notice = Some(format!("Signed in as {email}."));
                        self.step = Step::Key(key, true);
                        self.fresh = true;
                    }
                    Ok((email, KeyStep::Needed)) => self.notice = Some(format!("Signed in as {email}.")),
                    Ok((email, KeyStep::Ready)) => self.notice = Some(format!("Signed in as {email}.")),
                }
                return self.refresh();
            }
            Msg::KeySaved => {
                self.step = Step::Idle;
                // The first backup right away: the account isn't empty for long.
                return self.update(Msg::BackUp);
            }
            Msg::ShowKey => {
                if let Some(k) = account::recovery_key(&files) {
                    self.step = Step::Key(k, false);
                }
            }
            Msg::HideKey => self.step = Step::Idle,
            Msg::CopyKey => {
                if let Step::Key(k, _) = &self.step {
                    return cosmic::iced::clipboard::write(k.clone());
                }
            }
            Msg::RecoveryInput(s) => self.recovery = s,
            Msg::UseRecoveryKey => {
                let text = self.recovery.clone();
                self.busy = Some("Checking the recovery key");
                self.error = None;
                return Task::perform(
                    async move { account::use_recovery_key(&files, &text).await.map_err(text_err) },
                    Msg::KeyUsed,
                );
            }
            Msg::KeyUsed(r) => {
                self.busy = None;
                match r {
                    Ok(()) => {
                        self.recovery.clear();
                        self.notice = Some("This computer can read your backup now.".into());
                        self.latest = None;
                        return self.refresh();
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            Msg::BackUp => {
                self.busy = Some("Backing up");
                self.error = None;
                return Task::perform(
                    async move { account::backup(&files, true).await.map_err(text_err) },
                    Msg::BackedUp,
                );
            }
            Msg::BackedUp(r) => {
                self.busy = None;
                match r {
                    Ok(_) => {
                        self.notice = Some("Backed up.".into());
                        self.latest = None;
                    }
                    Err(e) => self.error = Some(format!("The backup didn't work: {e}")),
                }
                return self.refresh();
            }
            Msg::Latest(r) => self.latest = Some(r),
            Msg::Restore => {
                self.busy = Some("Restoring");
                self.error = None;
                return Task::perform(async move { account::restore(&files).await.map_err(text_err) }, Msg::Restored);
            }
            Msg::Restored(r) => {
                self.busy = None;
                match r {
                    Ok((m, _)) => {
                        self.notice = Some(format!("Your settings from {} are back on this computer.", m.computer))
                    }
                    Err(e) => self.error = Some(format!("Couldn't restore: {e}")),
                }
            }
            Msg::SignOut => {
                self.busy = Some("Signing out");
                return Task::perform(
                    async move {
                        let _ = account::sign_out(&files).await;
                    },
                    |_| Msg::SignedOut,
                );
            }
            Msg::SignedOut => {
                self.busy = None;
                self.step = Step::Idle;
                self.latest = None;
                self.notice = Some("Signed out. Your account and its backup are still there.".into());
                return self.refresh();
            }
            Msg::Dismiss => {
                self.error = None;
                self.notice = None;
            }
        }
        Task::none()
    }

    /// The account's latest backup came from another computer (or this one).
    pub fn latest(&self) -> Option<&Manifest> {
        self.latest.as_ref()?.as_ref().ok()?.as_ref()
    }

    /// Signing in or showing the key: the page shows only that.
    pub fn in_flow(&self) -> bool {
        !matches!(self.step, Step::Idle) || self.busy.is_some()
    }

    fn idle(&self) -> bool {
        self.busy.is_none() && matches!(self.step, Step::Idle)
    }

    pub fn view(&self) -> Element<'_, Msg> {
        let mut items: Vec<Element<'_, Msg>> = Vec::new();
        if let Some(b) = self.banner() {
            items.push(b);
        }
        if !self.available() {
            items.push(
                settings::section()
                    .title("Iliada account")
                    .add(info("Not available yet", "This computer isn't set up with an Iliada account server."))
                    .into(),
            );
            return settings::view_column(items).into();
        }
        match &self.step {
            Step::Code(c) => {
                items.push(code_card(c));
                return settings::view_column(items).into();
            }
            Step::Key(k, new) => {
                items.push(key_card(k, *new));
                return settings::view_column(items).into();
            }
            Step::Idle | Step::Asking => {}
        }
        let idle = self.idle();
        match &self.status {
            Status::SignedOut => items.push(
                settings::section()
                    .title("Iliada account")
                    .add(
                        settings::item::builder("Sign in to your Iliada account")
                            .description(
                                "Your desktop settings, Privacy answers and assistant conversations are backed up, \
                                 encrypted on this computer first so only you can read them. On a new computer, \
                                 sign in and they come back. New here? Signing in makes your account.",
                            )
                            .control(widget::button::suggested("Sign in").on_press_maybe(idle.then_some(Msg::SignIn))),
                    )
                    .into(),
            ),
            Status::NeedsKey { email } => {
                items.push(self.recovery_section().title("Iliada account").add(signed_in_as(email, idle)).into())
            }
            Status::Ready { email, last_backup, last_error } => {
                let last = match (last_backup, last_error) {
                    (_, Some(e)) => format!("The last backup didn't work: {e}"),
                    (Some(t), None) => format!("Last backup: {}.", ago(*t)),
                    (None, None) => "Not backed up yet.".into(),
                };
                items.push(
                    settings::section()
                        .title("Iliada account")
                        .add(signed_in_as(email, idle))
                        .add(
                            settings::item::builder("Backup")
                                .description(format!(
                                    "{last} Iliada backs up every hour while you're logged in, \
                                     encrypted on this computer first."
                                ))
                                .control(
                                    widget::button::standard("Back up now").on_press_maybe(idle.then_some(Msg::BackUp)),
                                ),
                        )
                        .add(
                            settings::item::builder("Recovery key")
                                .description("A new computer needs it once to read your backup.")
                                .control(widget::button::standard("Show").on_press(Msg::ShowKey)),
                        )
                        .into(),
                );
                let mut what = settings::section().title("What's backed up");
                for (_, label) in account::PARTS {
                    what = what.add(info(label, ""));
                }
                items.push(what.into());
                let restore = match &self.latest {
                    None => info("Looking for your backup…", ""),
                    Some(Err(e)) => info("Couldn't look for your backup", e),
                    Some(Ok(None)) => info("Your account has no backup yet", ""),
                    Some(Ok(Some(m))) => restore_item(m, "Restore…", idle),
                };
                items.push(settings::section().title("Restore").add(restore).into());
            }
        }
        settings::view_column(items).into()
    }

    /// The Setup Assistant's shorter version: sign in, the recovery key, and
    /// bringing back the account's backup.
    pub fn view_setup(&self) -> Element<'_, Msg> {
        let mut col = widget::column::with_capacity(3).spacing(theme::spacing().space_m).align_x(Alignment::Center);
        if let Some(b) = self.banner() {
            col = col.push(b);
        }
        let idle = self.idle();
        let content: Element<'_, Msg> = match (&self.step, &self.status) {
            (Step::Code(c), _) => code_card(c),
            (Step::Key(k, new), _) => key_card(k, *new),
            (_, Status::SignedOut) => {
                widget::button::suggested("Sign in").on_press_maybe(idle.then_some(Msg::SignIn)).into()
            }
            (_, Status::NeedsKey { .. }) => self.recovery_section().into(),
            (_, Status::Ready { email, .. }) => {
                let mut s = settings::section().add(settings::item_row(vec![
                    widget::text::body(format!("Signed in as {email}")).width(Length::Fill).into(),
                ]));
                // A new account has nothing to bring back yet.
                if let (Some(m), false) = (self.latest(), self.fresh) {
                    s = s.add(restore_item(m, "Restore", idle));
                }
                s.into()
            }
        };
        col = col.push(content);
        col.into()
    }

    fn recovery_section(&self) -> settings::Section<'_, Msg> {
        let idle = self.idle();
        let input = widget::text_input("ABCD-EFGH-…", &self.recovery)
            .on_input(Msg::RecoveryInput)
            .on_submit(|_| Msg::UseRecoveryKey)
            .font(cosmic::font::mono())
            .width(Length::Fill);
        let go = widget::button::suggested("Continue")
            .on_press_maybe((idle && !self.recovery.trim().is_empty()).then_some(Msg::UseRecoveryKey));
        let row = widget::row::with_capacity(2)
            .push(input)
            .push(go)
            .spacing(theme::spacing().space_xs)
            .align_y(Alignment::Center);
        settings::section()
            .add(info(
                "Type your recovery key",
                "Your backup is encrypted with a key only your computers have. This computer needs \
                 it once: type the recovery key you saved when you made your account.",
            ))
            .add(settings::item_row(vec![row.into()]))
    }

    fn banner(&self) -> Option<Element<'_, Msg>> {
        let spacing = theme::spacing();
        let row = if let Some(what) = self.busy.or(matches!(self.step, Step::Asking).then_some("Asking for a code")) {
            widget::row::with_capacity(2)
                .push(widget::progress_bar::circular::Circular::new().size(20.0))
                .push(widget::text::heading(format!("{what}…")).width(Length::Fill))
        } else {
            let (text, icon) = match (&self.error, &self.notice) {
                (Some(e), _) => (e.as_str(), "dialog-error-symbolic"),
                (None, Some(n)) => (n.as_str(), "emblem-ok-symbolic"),
                _ => return None,
            };
            widget::row::with_capacity(3)
                .push(widget::icon::from_name(icon).size(20).icon())
                .push(widget::text::body(text).width(Length::Fill))
                .push(widget::button::icon(widget::icon::from_name("window-close-symbolic")).on_press(Msg::Dismiss))
        };
        Some(card(row.spacing(spacing.space_s).align_y(Alignment::Center).into()))
    }
}

fn signed_in_as(email: &str, idle: bool) -> Element<'_, Msg> {
    settings::item::builder(format!("Signed in as {email}"))
        .control(widget::button::standard("Sign out").on_press_maybe(idle.then_some(Msg::SignOut)))
        .into()
}

fn code_card(c: &DeviceCode) -> Element<'_, Msg> {
    let spacing = theme::spacing();
    let buttons = widget::row::with_capacity(2)
        .push(widget::button::standard("Cancel").on_press(Msg::CancelSignIn))
        .push(widget::button::suggested("Open the sign-in page").on_press(Msg::OpenPage))
        .spacing(spacing.space_s);
    let col = widget::column::with_capacity(5)
        .push(widget::text::body("Sign in on your Iliada account page, then approve this code:"))
        .push(widget::text::title1(c.user_code.as_str()))
        .push(buttons)
        .push(widget::text::caption(format!("{} · Waiting for you to approve it…", c.verification_uri)))
        .spacing(spacing.space_s)
        .align_x(Alignment::Center);
    card(col.width(Length::Fill).into())
}

fn key_card(key: &str, new: bool) -> Element<'_, Msg> {
    let spacing = theme::spacing();
    let text = if new {
        "Your backup is encrypted on this computer with a key that only your computers have, so nobody else \
         can read it, Iliada included. Write this recovery key down or print it, and keep it somewhere safe: \
         a new computer needs it once. If you lose every computer and this key, the backup can't be read."
    } else {
        "A new computer needs this once to read your backup. Keep it somewhere safe, away from this computer."
    };
    let done = if new { "I've saved it" } else { "Done" };
    let buttons = widget::row::with_capacity(2)
        .push(widget::button::standard("Copy").on_press(Msg::CopyKey))
        .push(widget::button::suggested(done).on_press(if new { Msg::KeySaved } else { Msg::HideKey }))
        .spacing(spacing.space_s);
    let col = widget::column::with_capacity(4)
        .push(widget::text::title4("Your recovery key"))
        .push(widget::text::title4(key).font(cosmic::font::mono()))
        .push(widget::text::body(text))
        .push(buttons)
        .spacing(spacing.space_m)
        .align_x(Alignment::Center);
    card(col.width(Length::Fill).into())
}

fn info<'a>(title: &'a str, description: &'a str) -> Element<'a, Msg> {
    let mut col = widget::column::with_capacity(2).push(widget::text::body(title));
    if !description.is_empty() {
        col = col.push(widget::text::caption(description));
    }
    settings::item_row(vec![col.spacing(2).width(Length::Fill).into()]).into()
}

fn card(content: Element<'_, Msg>) -> Element<'_, Msg> {
    widget::container(content)
        .padding(theme::spacing().space_s)
        .class(theme::Container::Card)
        .width(Length::Fill)
        .into()
}

fn restore_item<'a>(m: &'a Manifest, button: &'static str, idle: bool) -> Element<'a, Msg> {
    settings::item::builder(format!("Backup from {}", m.computer))
        .description(format!(
            "Made {}. Restoring puts its settings on this computer; the desktop changes right away.",
            ago(m.created)
        ))
        .control(widget::button::standard(button).on_press_maybe(idle.then_some(Msg::Restore)))
        .into()
}
