//! `iliada-settings [page]`: opens Settings at one of Iliada's pages
//! (`updates`, `homer`, `privacy`, `backup`, `account`, `services`, `tools`,
//! `about`; Updates when none is given).
//!
//! There is one Settings app: COSMIC Settings, with Iliada's pages among its
//! own (nix/patches/cosmic-settings-iliada-pages.patch), so this opens it
//! there. `--standalone` (or a system without COSMIC Settings) shows
//! Iliada's pages alone in their own window, for development.

use cosmic::app::{Settings, Task};
use cosmic::iced::Size;
use cosmic::widget::{self, nav_bar};
use cosmic::{ApplicationExt, Core, Element, executor};
use iliada_settings::panel::{Message, Page, Panel};

fn main() -> cosmic::iced::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let standalone = args.iter().any(|a| a == "--standalone");
    let page = args.iter().filter(|a| !a.starts_with("--")).find_map(|a| Page::from_id(a)).unwrap_or(Page::Updates);
    if !standalone {
        use std::os::unix::process::CommandExt;
        // Only returns if COSMIC Settings could not be started.
        let err = std::process::Command::new("cosmic-settings").arg(format!("iliada-{}", page.id())).exec();
        eprintln!("cosmic-settings: {err}; showing Iliada's pages on their own");
    }
    let settings = Settings::default().size(Size::new(900.0, 680.0));
    cosmic::app::run::<App>(settings, page)
}

struct App {
    core: Core,
    nav: nav_bar::Model,
    panel: Panel,
}

impl cosmic::Application for App {
    type Executor = executor::Default;
    type Flags = Page;
    type Message = Message;
    const APP_ID: &'static str = "org.iliada.Settings";

    fn core(&self) -> &Core {
        &self.core
    }
    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, page: Page) -> (Self, Task<Message>) {
        let mut nav = nav_bar::Model::default();
        for p in Page::ALL {
            let e = nav.insert().text(p.title()).icon(widget::icon::from_name(p.icon())).data(p);
            if p == page {
                e.activate();
            }
        }
        let mut app = App { core, nav, panel: Panel::new(page) };
        app.set_header_title("Settings".into());
        let title = match app.core.main_window_id() {
            Some(id) => app.set_window_title("Settings".into(), id),
            None => Task::none(),
        };
        let enter = app.panel.enter(page);
        (app, Task::batch([title, enter]))
    }

    fn nav_model(&self) -> Option<&nav_bar::Model> {
        Some(&self.nav)
    }

    fn on_nav_select(&mut self, id: nav_bar::Id) -> Task<Message> {
        self.nav.activate(id);
        let page = self.nav.active_data::<Page>().copied().unwrap_or(Page::Updates);
        self.panel.enter(page)
    }

    fn dialog(&self) -> Option<Element<'_, Message>> {
        self.panel.dialog()
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        self.panel.update(message)
    }

    fn view(&self) -> Element<'_, Message> {
        self.panel.view()
    }
}
