//! The Iliada Store.
//!
//! A COSMIC app over the `iliada-store` engine: browse the signed catalog,
//! install, update and open apps. Installing is exactly dragging the `.app`
//! into Applications, after the download is verified against the catalog.

use std::collections::BTreeMap;

use std::sync::Arc;

use cosmic::app::{Settings, Task};
use cosmic::iced::{Alignment, Length, Size};
use cosmic::widget::{self, nav_bar};
use cosmic::{Core, Element, executor};
use iliada_catalog::{AppEntry, Catalog};
use iliada_store::{Installed, Store, StoreConfig};

const CATEGORIES: [&str; 6] = ["Internet", "Office", "Graphics", "Media", "Utilities", "Games"];

fn main() -> cosmic::iced::Result {
    let settings = Settings::default().size(Size::new(960.0, 720.0));
    cosmic::app::run::<App>(settings, ())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Page {
    Discover,
    Category(&'static str),
    Installed,
}

#[derive(Debug, Clone)]
enum Status {
    Idle,
    Working(&'static str),
    Failed(String),
}

#[derive(Debug, Clone)]
enum Message {
    Loaded(Result<Catalog, String>),
    Icon(String, Option<Vec<u8>>, bool),
    Search(String),
    Install(String),
    Done(String, Result<(), String>),
    Open(String),
    Refresh,
}

struct App {
    core: Core,
    nav: nav_bar::Model,
    store: Arc<Store>,
    catalog: Option<Result<Catalog, String>>,
    installed: BTreeMap<String, Installed>,
    icons: BTreeMap<String, widget::icon::Handle>,
    status: BTreeMap<String, Status>,
    search: String,
}

impl App {
    fn page(&self) -> Page {
        self.nav.active_data::<Page>().cloned().unwrap_or(Page::Discover)
    }

    fn load_catalog(&self) -> Task<Message> {
        let store = self.store.clone();
        cosmic::task::future(async move { Message::Loaded(store.catalog().await.map_err(|e| format!("{e:#}"))) })
    }

    fn row<'a>(&'a self, app: &'a AppEntry) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();
        let icon: Element<_> = match self.icons.get(&app.id) {
            Some(h) => widget::icon::icon(h.clone()).size(56).into(),
            None => widget::icon::from_name("application-x-executable").size(56).icon().into(),
        };
        let mut meta = format!("{:.0} MB", app.archive.size as f64 / 1e6);
        if app.network {
            meta.push_str("  ·  Uses the internet");
        }
        let text = widget::column::with_capacity(3)
            .push(widget::text::title4(app.name.as_str()))
            .push(widget::text::body(app.summary.as_str()))
            .push(widget::text::caption(meta))
            .spacing(spacing.space_xxxs)
            .width(Length::Fill);

        let installed = self.installed.get(&app.id);
        let action: Element<_> = match self.status.get(&app.id) {
            Some(Status::Working(what)) => widget::row::with_capacity(2)
                .push(widget::progress_bar::circular::Circular::new().size(20.0))
                .push(widget::text::body(*what))
                .spacing(spacing.space_xs)
                .align_y(Alignment::Center)
                .into(),
            _ => match installed {
                Some(i) if i.version != app.version => {
                    widget::button::suggested("Update").on_press(Message::Install(app.id.clone())).into()
                }
                Some(_) => widget::button::standard("Open").on_press(Message::Open(app.id.clone())).into(),
                None => widget::button::suggested("Get").on_press(Message::Install(app.id.clone())).into(),
            },
        };
        let mut col = widget::column::with_capacity(2).push(
            widget::row::with_capacity(3)
                .push(icon)
                .push(text)
                .push(action)
                .spacing(spacing.space_m)
                .align_y(Alignment::Center),
        );
        if let Some(Status::Failed(e)) = self.status.get(&app.id) {
            col = col.push(widget::text::caption(format!("Could not install: {e}")));
        }
        widget::container(col.spacing(spacing.space_xxs))
            .padding(spacing.space_s)
            .class(cosmic::theme::Container::Card)
            .width(Length::Fill)
            .into()
    }
}

impl cosmic::Application for App {
    type Executor = executor::Default;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = "org.iliada.Store";

    fn core(&self) -> &Core {
        &self.core
    }
    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: ()) -> (Self, Task<Message>) {
        let mut nav = nav_bar::Model::default();
        nav.insert().text("Discover").icon(widget::icon::from_name("starred-symbolic")).data(Page::Discover).activate();
        for c in CATEGORIES {
            nav.insert().text(c).data(Page::Category(c));
        }
        nav.insert().text("Installed").icon(widget::icon::from_name("folder-download-symbolic")).data(Page::Installed);

        let cfg = StoreConfig::from_env().expect("built-in catalog key");
        let store = Arc::new(Store::new(cfg).expect("store"));
        let mut app = App {
            core,
            nav,
            installed: store.installed(),
            store,
            catalog: None,
            icons: BTreeMap::new(),
            status: BTreeMap::new(),
            search: String::new(),
        };
        let title = app.set_window_title("Store".into());
        let load = app.load_catalog();
        (app, Task::batch([title, load]))
    }

    fn nav_model(&self) -> Option<&nav_bar::Model> {
        Some(&self.nav)
    }

    fn on_nav_select(&mut self, id: nav_bar::Id) -> Task<Message> {
        self.nav.activate(id);
        Task::none()
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Loaded(result) => {
                let mut tasks = Vec::new();
                if let Ok(catalog) = &result {
                    for app in &catalog.apps {
                        let Some(url) = app.icon.clone() else { continue };
                        let (id, svg) = (app.id.clone(), url.ends_with(".svg"));
                        tasks.push(cosmic::task::future(async move {
                            let bytes = async { reqwest::get(&url).await.ok()?.bytes().await.ok().map(|b| b.to_vec()) }.await;
                            Message::Icon(id, bytes, svg)
                        }));
                    }
                }
                self.catalog = Some(result);
                return Task::batch(tasks);
            }
            Message::Icon(id, Some(bytes), svg) => {
                let handle = if svg {
                    widget::icon::from_svg_bytes(bytes)
                } else {
                    widget::icon::from_raster_bytes(bytes)
                };
                self.icons.insert(id, handle);
            }
            Message::Icon(_, None, _) => {}
            Message::Search(s) => self.search = s,
            Message::Install(id) => {
                let Some(Ok(catalog)) = &self.catalog else { return Task::none() };
                let Some(app) = catalog.apps.iter().find(|a| a.id == id).cloned() else { return Task::none() };
                let what = if self.installed.contains_key(&id) { "Updating…" } else { "Installing…" };
                self.status.insert(id.clone(), Status::Working(what));
                let store = self.store.clone();
                return cosmic::task::future(async move {
                    let r = store.install(&app, &|_, _| {}).await.map(|_| ()).map_err(|e| format!("{e:#}"));
                    Message::Done(id, r)
                });
            }
            Message::Done(id, r) => {
                match r {
                    Ok(()) => {
                        self.status.insert(id, Status::Idle);
                    }
                    Err(e) => {
                        self.status.insert(id, Status::Failed(e));
                    }
                }
                self.installed = self.store.installed();
            }
            Message::Open(id) => {
                if let Some(app) = self.installed.get(&id) {
                    let _ = std::process::Command::new("iliada-run").arg(&app.path).spawn();
                }
            }
            Message::Refresh => {
                self.installed = self.store.installed();
                return self.load_catalog();
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();
        let page = self.page();
        let header = widget::row::with_capacity(2)
            .push(
                widget::text_input::search_input("Search apps", &self.search)
                    .on_input(Message::Search)
                    .on_clear(Message::Search(String::new()))
                    .width(Length::Fill),
            )
            .push(widget::button::icon(widget::icon::from_name("view-refresh-symbolic")).on_press(Message::Refresh))
            .spacing(spacing.space_s)
            .align_y(Alignment::Center);

        let body: Element<_> = match &self.catalog {
            None => widget::container(widget::text::body("Loading the catalog…")).center(Length::Fill).into(),
            Some(Err(e)) => widget::container(
                widget::column::with_capacity(2)
                    .push(widget::text::title4("The Store is not available"))
                    .push(widget::text::body(e.as_str()))
                    .spacing(spacing.space_s)
                    .align_x(Alignment::Center),
            )
            .center(Length::Fill)
            .into(),
            Some(Ok(catalog)) => {
                let needle = self.search.to_lowercase();
                let apps: Vec<&AppEntry> = catalog
                    .apps
                    .iter()
                    .filter(|a| match &page {
                        Page::Discover => true,
                        Page::Category(c) => a.category == *c,
                        Page::Installed => self.installed.contains_key(&a.id),
                    })
                    .filter(|a| {
                        needle.is_empty()
                            || a.name.to_lowercase().contains(&needle)
                            || a.summary.to_lowercase().contains(&needle)
                    })
                    .collect();
                if apps.is_empty() {
                    widget::container(widget::text::body("No apps here yet.")).center(Length::Fill).into()
                } else {
                    let mut list = widget::column::with_capacity(apps.len()).spacing(spacing.space_s);
                    for app in apps {
                        list = list.push(self.row(app));
                    }
                    widget::scrollable(list).height(Length::Fill).into()
                }
            }
        };
        widget::column::with_capacity(2)
            .push(header)
            .push(body)
            .spacing(spacing.space_m)
            .padding(spacing.space_m)
            .into()
    }
}

