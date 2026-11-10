//! The permission prompt: drawn by Iliada, never by the app asking.
//!
//!   iliada-prompt --app GIMP --wants "access your Pictures folder" \
//!                 --purpose "…" [--icon /Applications/GIMP.app/Contents/Resources/gimp.png]
//!
//! Exit status: 0 allowed, 1 not allowed, 2 closed without answering.
//! `iliada-run` runs it before starting an app and remembers the answer.

use std::path::PathBuf;

use cosmic::app::{Settings, Task};
use cosmic::iced::{Alignment, Length, Size};
use cosmic::widget;
use cosmic::{ApplicationExt, Core, Element, executor};

#[derive(Debug, Clone, Default)]
struct Ask {
    app: String,
    wants: String,
    purpose: String,
    icon: Option<PathBuf>,
}

/// Untrusted text (it comes from the app's bundle): no control characters,
/// bounded length.
fn clean(s: &str, max: usize) -> String {
    let s: String = s.chars().filter(|c| !c.is_control()).collect();
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s,
    }
}

fn parse() -> Option<Ask> {
    let mut a = Ask::default();
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let v = it.next()?;
        match flag.as_str() {
            "--app" => a.app = clean(&v, 60),
            "--wants" => a.wants = clean(&v, 120),
            "--purpose" => a.purpose = clean(&v, 300),
            "--icon" => a.icon = Some(PathBuf::from(v)),
            _ => return None,
        }
    }
    (!a.app.is_empty() && !a.wants.is_empty()).then_some(a)
}

fn main() {
    let Some(ask) = parse() else {
        eprintln!("usage: iliada-prompt --app NAME --wants TEXT [--purpose TEXT] [--icon PATH]");
        std::process::exit(2);
    };
    let settings = Settings::default().size(Size::new(460.0, 300.0)).resizable(None);
    let _ = cosmic::app::run::<App>(settings, ask);
    // The window was closed without an answer.
    std::process::exit(2);
}

#[derive(Debug, Clone)]
enum Message {
    Answer(bool),
}

struct App {
    core: Core,
    ask: Ask,
}

impl cosmic::Application for App {
    type Executor = executor::Default;
    type Flags = Ask;
    type Message = Message;
    const APP_ID: &'static str = "org.iliada.Prompt";

    fn core(&self) -> &Core {
        &self.core
    }
    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, ask: Ask) -> (Self, Task<Message>) {
        let mut app = App { core, ask };
        app.core.window.show_headerbar = false;
        let title = format!("{} wants permission", app.ask.app);
        let t = match app.core.main_window_id() {
            Some(id) => app.set_window_title(title, id),
            None => Task::none(),
        };
        (app, t)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Answer(allow) => std::process::exit(if allow { 0 } else { 1 }),
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();
        let icon: Element<_> = match &self.ask.icon {
            Some(p) if p.is_file() => widget::icon::icon(widget::icon::from_path(p.clone())).size(64).into(),
            _ => widget::icon::from_name("dialog-question").size(64).icon().into(),
        };
        let mut col = widget::column::with_capacity(5)
            .push(icon)
            .push(widget::text::title4(format!("“{}” would like to {}.", self.ask.app, self.ask.wants)))
            .spacing(spacing.space_s)
            .align_x(Alignment::Center);
        if !self.ask.purpose.is_empty() {
            col = col.push(widget::text::caption(self.ask.purpose.as_str()));
        }
        col = col.push(widget::text::caption("You can change this later in Iliada Settings → Privacy."));
        let buttons = widget::row::with_capacity(2)
            .push(widget::button::standard("Don't Allow").on_press(Message::Answer(false)))
            .push(widget::button::suggested("Allow").on_press(Message::Answer(true)))
            .spacing(spacing.space_s);
        col = col.push(buttons);
        widget::container(col).center(Length::Fill).padding(spacing.space_m).into()
    }
}
