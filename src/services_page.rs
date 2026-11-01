use crate::format;
use crate::heat_cell::{self, HeatCell};
use crate::service::Service;
use crate::systemd::{self, Action, Scope, ServiceState};
use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::{gettext, ngettext};
use gtk::{gio, glib};
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How long the list stands between asking systemd again, as listing every unit is work for
/// the manager of the machine, whatever a reading costs here.
const REFRESHED: Duration = Duration::from_secs(5);

/// What one look at systemd found.
struct Listing {
    scope: Scope,
    services: Result<Vec<ServiceState>, glib::Error>,
    usage: HashMap<String, (u64, u64)>,
}

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate, glib::Properties)]
    #[template(resource = "/io/github/sachesi/locus/ui/services_page.ui")]
    #[properties(wrapper_type = super::ServicesPage)]
    pub struct ServicesPage {
        #[template_child]
        pub title: TemplateChild<adw::WindowTitle>,
        #[template_child]
        pub scope_group: TemplateChild<adw::ToggleGroup>,
        #[template_child]
        pub narrow_scope_group: TemplateChild<adw::ToggleGroup>,
        #[template_child]
        pub search_bar: TemplateChild<gtk::SearchBar>,
        #[template_child]
        pub search_entry: TemplateChild<gtk::SearchEntry>,
        #[template_child]
        pub banner: TemplateChild<adw::Banner>,
        #[template_child]
        pub stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub view: TemplateChild<gtk::ColumnView>,
        #[template_child]
        pub name_column: TemplateChild<gtk::ColumnViewColumn>,
        #[template_child]
        pub status_column: TemplateChild<gtk::ColumnViewColumn>,
        #[template_child]
        pub startup_column: TemplateChild<gtk::ColumnViewColumn>,
        #[template_child]
        pub cpu_column: TemplateChild<gtk::ColumnViewColumn>,
        #[template_child]
        pub memory_column: TemplateChild<gtk::ColumnViewColumn>,
        #[template_child]
        pub details_title: TemplateChild<adw::WindowTitle>,
        #[template_child]
        pub details_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub start_at_boot: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub unit_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub changed_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub file_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub log: TemplateChild<gtk::Label>,

        /// Set by the window's breakpoints; the pane goes over the list rather than beside it.
        #[property(get, set)]
        compact: Cell<bool>,
        /// Set by the window's breakpoint; leaves room for little more than the names.
        #[property(get, set)]
        narrow: Cell<bool>,
        /// Whether the pane with more about what is selected is open.
        #[property(get, set = Self::set_details_shown)]
        details_shown: Cell<bool>,

        pub store: OnceCell<gio::ListStore>,
        pub selection: OnceCell<gtk::SingleSelection>,
        pub filter: OnceCell<gtk::CustomFilter>,
        pub by_name: RefCell<HashMap<String, Service>>,
        pub generation: Cell<u64>,
        pub query: RefCell<String>,
        pub failed_only: Cell<bool>,
        pub reading: Cell<bool>,
        pub read_at: Cell<Option<Instant>>,
        /// When the processor time of the services was last read.
        pub usage_at: Cell<Option<Instant>>,
        /// Set while the switch is brought up to what systemd says.
        pub updating: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ServicesPage {
        const NAME: &'static str = "LocusServicesPage";
        type Type = super::ServicesPage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            Service::ensure_type();
            klass.bind_template();
            klass.bind_template_callbacks();
            klass.install_action("service.start", None, |page, _, _| {
                page.act(Action::Start);
            });
            klass.install_action("service.stop", None, |page, _, _| {
                page.confirm(Action::Stop);
            });
            klass.install_action("service.restart", None, |page, _, _| {
                page.confirm(Action::Restart);
            });
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for ServicesPage {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            self.search_bar.connect_entry(&*self.search_entry);
            obj.set_up_model();
            obj.set_up_columns();
            self.scope_group
                .bind_property("active-name", &*self.narrow_scope_group, "active-name")
                .bidirectional()
                .sync_create()
                .build();
            self.scope_group.connect_active_name_notify(glib::clone!(
                #[weak]
                obj,
                move |_| obj.on_scope_changed()
            ));
            self.start_at_boot.connect_active_notify(glib::clone!(
                #[weak]
                obj,
                move |row| obj.on_start_at_boot(row.is_active())
            ));
            obj.update_selection();
        }
    }

    impl WidgetImpl for ServicesPage {}
    impl BinImpl for ServicesPage {}

    #[gtk::template_callbacks]
    impl ServicesPage {
        /// Wide enough for the pane's tabs, and on the smallest window, for the window itself.
        #[template_callback(function)]
        fn pane_width(narrow: bool) -> f32 {
            if narrow { 300.0 } else { 370.0 }
        }

        /// The pane has the window's buttons while it is beside the list, at its end.
        #[template_callback(function)]
        fn title_buttons_shown(details_shown: bool, compact: bool) -> bool {
            !details_shown || compact
        }

        #[template_callback]
        fn on_search_changed(&self, entry: &gtk::SearchEntry) {
            self.query.replace(entry.text().to_lowercase());
            self.obj().refilter();
        }

        #[template_callback]
        fn on_banner_clicked(&self) {
            self.failed_only.set(!self.failed_only.get());
            self.obj().show_failures();
            self.obj().refilter();
        }

        #[template_callback]
        fn on_show_log(&self) {
            self.obj().show_full_log();
        }

        fn set_details_shown(&self, shown: bool) {
            self.details_shown.set(shown);
            self.obj().update_details();
        }
    }
}

glib::wrapper! {
    pub struct ServicesPage(ObjectSubclass<imp::ServicesPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ServicesPage {
    fn scope(&self) -> Scope {
        if self.imp().scope_group.active_name().as_deref() == Some("user") {
            Scope::User
        } else {
            Scope::System
        }
    }

    /// Asks systemd about the services, unless it was asked only a moment ago.
    pub fn refresh(&self) {
        let imp = self.imp();
        if imp.read_at.get().is_some_and(|at| at.elapsed() < REFRESHED) {
            return;
        }
        self.read_now();
    }

    fn read_now(&self) {
        let imp = self.imp();
        if imp.reading.replace(true) {
            return;
        }
        imp.read_at.set(Some(Instant::now()));
        let scope = self.scope();
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = page)]
            self,
            async move {
                let listing = gio::spawn_blocking(move || Listing {
                    scope,
                    services: systemd::list_services(scope),
                    usage: systemd::read_usage(scope),
                })
                .await;
                page.imp().reading.set(false);
                if let Ok(listing) = listing
                    && listing.scope == page.scope()
                {
                    page.apply(listing);
                }
            }
        ));
    }

    fn on_scope_changed(&self) {
        let imp = self.imp();
        if let Some(store) = imp.store.get() {
            store.remove_all();
        }
        imp.by_name.borrow_mut().clear();
        imp.usage_at.set(None);
        imp.failed_only.set(false);
        imp.stack.set_visible_child_name("loading");
        self.read_now();
    }

    fn apply(&self, listing: Listing) {
        let imp = self.imp();
        let services = match listing.services {
            Ok(services) => services,
            Err(_) => {
                imp.title.set_subtitle("");
                imp.banner.set_revealed(false);
                imp.stack.set_visible_child_name("unavailable");
                return;
            }
        };
        let Some(store) = imp.store.get() else {
            return;
        };
        let generation = imp.generation.get() + 1;
        imp.generation.set(generation);
        let now = Instant::now();
        let seconds = imp
            .usage_at
            .replace(Some(now))
            .map(|at| now.duration_since(at).as_secs_f64());
        let cpus = f64::from(glib::num_processors().max(1));
        {
            let mut by_name = imp.by_name.borrow_mut();
            let mut added = Vec::new();
            for state in &services {
                let service = by_name.entry(state.name.clone()).or_insert_with(|| {
                    let service = Service::new(state);
                    added.push(service.clone());
                    service
                });
                service.update(state);
                service.imp().generation.set(generation);
                let (memory, used) = listing.usage.get(&state.name).copied().unwrap_or((0, 0));
                if service.memory() != memory {
                    service.set_memory(memory);
                }
                let before = service.imp().cpu_used.replace(used);
                let cpu = seconds.filter(|s| *s > 0.0).map_or(0.0, |s| {
                    (used.saturating_sub(before) as f64 / 1e6 / s / cpus * 100.0).min(100.0)
                });
                if service.cpu() != cpu {
                    service.set_cpu(cpu);
                }
            }
            // All at once, or the view holds on to the row it first had at its top as those
            // that sort above it arrive, and opens partway down the list.
            store.extend_from_slice(&added);
            by_name.retain(|_, service| {
                let listed = service.imp().generation.get() == generation;
                if !listed && let Some(position) = store.find(service) {
                    store.remove(position);
                }
                listed
            });
        }
        let running = services.iter().filter(|s| s.active == "active").count();
        let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        imp.title.set_subtitle(&format::listed(
            &ngettext("{count} Service", "{count} Services", count(services.len()))
                .replace("{count}", &services.len().to_string()),
            &ngettext("{count} Running", "{count} Running", count(running))
                .replace("{count}", &running.to_string()),
        ));
        self.show_failures();
        self.refilter();
        if let Some(sorter) = imp.view.sorter() {
            sorter.changed(gtk::SorterChange::Different);
        }
        self.update_selection();
        self.update_details();
    }

    /// The banner says how many services have failed, and takes the list to them and back.
    fn show_failures(&self) {
        let imp = self.imp();
        let failed = imp
            .by_name
            .borrow()
            .values()
            .filter(|service| service.is_failed())
            .count();
        if failed == 0 {
            imp.failed_only.set(false);
        }
        imp.banner.set_revealed(failed > 0);
        imp.banner.set_title(
            &ngettext(
                "{count} service has failed",
                "{count} services have failed",
                u32::try_from(failed).unwrap_or(u32::MAX),
            )
            .replace("{count}", &failed.to_string()),
        );
        imp.banner.set_button_label(Some(&if imp.failed_only.get() {
            gettext("Show All")
        } else {
            gettext("Show")
        }));
    }

    fn refilter(&self) {
        let imp = self.imp();
        if let Some(filter) = imp.filter.get() {
            filter.changed(gtk::FilterChange::Different);
        }
        let listed = imp
            .selection
            .get()
            .map_or(0, |selection| selection.n_items());
        let known = !imp.by_name.borrow().is_empty();
        if imp.stack.visible_child_name().as_deref() != Some("loading") || known {
            imp.stack
                .set_visible_child_name(if listed == 0 { "empty" } else { "list" });
        }
    }

    fn set_up_model(&self) {
        let imp = self.imp();
        let store = gio::ListStore::new::<Service>();
        let filter = gtk::CustomFilter::new(glib::clone!(
            #[weak(rename_to = page)]
            self,
            #[upgrade_or]
            false,
            move |item| item
                .downcast_ref::<Service>()
                .is_some_and(|service| page.shows(service))
        ));
        let filtered = gtk::FilterListModel::new(Some(store.clone()), Some(filter.clone()));
        let sorted = gtk::SortListModel::new(Some(filtered), imp.view.sorter());
        let selection = gtk::SingleSelection::builder()
            .autoselect(false)
            .can_unselect(true)
            .build();
        selection.set_model(Some(&sorted));
        selection.connect_selected_item_notify(glib::clone!(
            #[weak(rename_to = page)]
            self,
            move |_| {
                page.update_selection();
                page.update_details();
            }
        ));
        imp.view.set_model(Some(&selection));
        let _ = imp.store.set(store);
        let _ = imp.filter.set(filter);
        let _ = imp.selection.set(selection);
    }

    fn shows(&self, service: &Service) -> bool {
        let imp = self.imp();
        if imp.failed_only.get() && !service.is_failed() {
            return false;
        }
        let query = imp.query.borrow();
        query.is_empty()
            || service.name().to_lowercase().contains(&*query)
            || service.description().to_lowercase().contains(&*query)
    }

    fn set_up_columns(&self) {
        let imp = self.imp();
        imp.name_column.set_factory(Some(&name_factory()));
        imp.status_column.set_factory(Some(&text_factory(
            || {
                service_of_item()
                    .chain_property::<Service>("status")
                    .upcast()
            },
            true,
        )));
        imp.startup_column.set_factory(Some(&text_factory(
            || {
                service_of_item()
                    .chain_property::<Service>("startup")
                    .upcast()
            },
            false,
        )));
        imp.cpu_column.set_factory(Some(&heat_factory(
            || {
                service_of_item()
                    .chain_property::<Service>("cpu")
                    .chain_closure::<String>(glib::closure!(|_: Option<glib::Object>, cpu: f64| {
                        if cpu < 0.05 {
                            String::new()
                        } else {
                            format::percent(cpu, 1)
                        }
                    }))
                    .upcast()
            },
            || {
                service_of_item()
                    .chain_property::<Service>("cpu")
                    .chain_closure::<u32>(glib::closure!(|_: Option<glib::Object>, cpu: f64| {
                        heat_cell::cpu_level(cpu)
                    }))
                    .upcast()
            },
        )));
        imp.memory_column.set_factory(Some(&heat_factory(
            || {
                service_of_item()
                    .chain_property::<Service>("memory")
                    .chain_closure::<String>(glib::closure!(
                        |_: Option<glib::Object>, memory: u64| if memory == 0 {
                            String::new()
                        } else {
                            format::size(memory)
                        }
                    ))
                    .upcast()
            },
            || {
                service_of_item()
                    .chain_property::<Service>("memory")
                    .chain_closure::<u32>(glib::closure!(|_: Option<glib::Object>, memory: u64| {
                        heat_cell::memory_level(memory)
                    }))
                    .upcast()
            },
        )));

        let property = |name: &str| {
            gtk::PropertyExpression::new(Service::static_type(), gtk::Expression::NONE, name)
        };
        imp.name_column
            .set_sorter(Some(&gtk::StringSorter::new(Some(property("description")))));
        imp.startup_column
            .set_sorter(Some(&gtk::StringSorter::new(Some(property("startup")))));
        // Those that want attention come first, then those alike by name.
        let by_status = gtk::MultiSorter::new();
        by_status.append(gtk::NumericSorter::new(Some(property("rank"))));
        by_status.append(gtk::StringSorter::new(Some(property("description"))));
        imp.status_column.set_sorter(Some(&by_status));
        for (column, name) in [(&imp.cpu_column, "cpu"), (&imp.memory_column, "memory")] {
            let sorter = gtk::NumericSorter::new(Some(property(name)));
            sorter.set_sort_order(gtk::SortType::Descending);
            column.set_sorter(Some(&sorter));
        }
        imp.view
            .sort_by_column(Some(&*imp.status_column), gtk::SortType::Ascending);
    }

    fn selected(&self) -> Option<Service> {
        self.imp().selection.get()?.selected_item().and_downcast()
    }

    fn update_selection(&self) {
        let selected = self.selected();
        let active = selected.as_ref().map(Service::active);
        let is = |state: &str| active.as_deref() == Some(state);
        let startable = selected.is_some() && !is("active") && !is("activating");
        let stoppable = is("active") || is("activating");
        self.action_set_enabled("service.start", startable);
        self.action_set_enabled("service.stop", stoppable);
        self.action_set_enabled("service.restart", selected.is_some());
    }

    /// Brings the pane, while it is open, up to what is selected.
    fn update_details(&self) {
        let imp = self.imp();
        if !self.details_shown() {
            return;
        }
        let Some(service) = self.selected() else {
            imp.details_title.set_title(&gettext("Details"));
            imp.details_title.set_subtitle("");
            imp.details_stack.set_visible_child_name("nothing");
            return;
        };
        imp.details_stack.set_visible_child_name("service");
        imp.details_title.set_title(&service.description());
        imp.details_title.set_subtitle(&service.status());
        imp.unit_row
            .set_subtitle(&glib::markup_escape_text(&service.name()));
        imp.updating.set(true);
        let state = service.file_state();
        imp.start_at_boot.set_active(state.starts_with("enabled"));
        imp.start_at_boot
            .set_sensitive(matches!(state.as_str(), "enabled" | "disabled"));
        imp.updating.set(false);

        let (scope, name) = (self.scope(), service.name());
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = page)]
            self,
            async move {
                let asked = name.clone();
                let read = gio::spawn_blocking(move || {
                    let log = systemd::read_log(scope, &name, 12).unwrap_or_default();
                    let unit = systemd::unit_properties(scope, &name)
                        .ok()
                        .map(|properties| {
                            let text = |key: &str| {
                                properties
                                    .get(key)
                                    .and_then(|value| value.str().map(str::to_owned))
                                    .unwrap_or_default()
                            };
                            let changed = properties
                                .get("StateChangeTimestamp")
                                .and_then(glib::Variant::get::<u64>)
                                .unwrap_or(0);
                            (text("FragmentPath"), changed)
                        });
                    (log, unit)
                })
                .await;
                if let Ok((log, unit)) = read
                    && page
                        .selected()
                        .is_some_and(|service| service.name() == asked)
                {
                    page.show_read(&log, unit);
                }
            }
        ));
    }

    fn show_read(&self, log: &str, unit: Option<(String, u64)>) {
        let imp = self.imp();
        let log = log.trim();
        if log.is_empty() {
            imp.log.set_label(&gettext("No entries that can be read"));
        } else {
            imp.log.set_label(log);
        }
        let (file, changed) = unit.unwrap_or_default();
        imp.file_row.set_visible(!file.is_empty());
        imp.file_row.set_subtitle(&glib::markup_escape_text(&file));
        let when = i64::try_from(changed)
            .ok()
            .filter(|at| *at > 0)
            .and_then(|at| glib::DateTime::from_unix_local(at / 1_000_000).ok())
            .and_then(|at| at.format("%c").ok());
        imp.changed_row.set_visible(when.is_some());
        if let Some(when) = when {
            imp.changed_row.set_subtitle(&when);
        }
    }

    fn show_full_log(&self) {
        let Some(service) = self.selected() else {
            return;
        };
        let (scope, name) = (self.scope(), service.name());
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = page)]
            self,
            async move {
                let log = gio::spawn_blocking(move || {
                    systemd::read_log(scope, &name, 1000).unwrap_or_default()
                })
                .await
                .unwrap_or_default();
                let text = gtk::TextView::builder()
                    .editable(false)
                    .monospace(true)
                    .wrap_mode(gtk::WrapMode::WordChar)
                    .left_margin(12)
                    .right_margin(12)
                    .top_margin(12)
                    .bottom_margin(12)
                    .build();
                text.buffer().set_text(&log);
                let scrolled = gtk::ScrolledWindow::builder()
                    .child(&text)
                    .vexpand(true)
                    .build();
                let view = adw::ToolbarView::new();
                view.add_top_bar(&adw::HeaderBar::new());
                view.set_content(Some(&scrolled));
                let dialog = adw::Dialog::builder()
                    .title(service.description())
                    .content_width(760)
                    .content_height(560)
                    .child(&view)
                    .build();
                dialog.present(Some(&page));
            }
        ));
    }

    /// Stopping or restarting can take down what relies on the service, so it asks first.
    fn confirm(&self, action: Action) {
        let Some(service) = self.selected() else {
            return;
        };
        let (heading, response) = match action {
            Action::Stop => (gettext("Stop “{name}”?"), gettext("_Stop Service")),
            _ => (gettext("Restart “{name}”?"), gettext("_Restart Service")),
        };
        let dialog = adw::AlertDialog::new(
            Some(&heading.replace("{name}", &service.description())),
            Some(&gettext("What relies on this service may stop working.")),
        );
        dialog.add_responses(&[("cancel", &gettext("_Cancel")), ("act", &response)]);
        dialog.set_response_appearance("act", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = page)]
            self,
            async move {
                if dialog.choose_future(Some(&page)).await == "act" {
                    page.act(action);
                }
            }
        ));
    }

    fn on_start_at_boot(&self, enabled: bool) {
        if !self.imp().updating.get() {
            self.act(if enabled {
                Action::Enable
            } else {
                Action::Disable
            });
        }
    }

    /// Does `action` to the selected service on a worker, and looks again when it is done.
    fn act(&self, action: Action) {
        let Some(service) = self.selected() else {
            return;
        };
        let (scope, name) = (self.scope(), service.name());
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = page)]
            self,
            async move {
                let result =
                    gio::spawn_blocking(move || systemd::perform(scope, &name, action)).await;
                if let Ok(Err(error)) = result {
                    page.show_failure(&service, action, &error);
                }
                page.read_now();
            }
        ));
    }

    fn show_failure(&self, service: &Service, action: Action, error: &glib::Error) {
        let message = match action {
            Action::Start => gettext("Could not start “{name}”: {reason}"),
            Action::Stop => gettext("Could not stop “{name}”: {reason}"),
            Action::Restart => gettext("Could not restart “{name}”: {reason}"),
            Action::Enable | Action::Disable => {
                gettext("Could not change when “{name}” starts: {reason}")
            }
        };
        let text = message
            .replace("{name}", &service.description())
            .replace("{reason}", &reason_of(error));
        if let Some(overlay) = self
            .ancestor(adw::ToastOverlay::static_type())
            .and_downcast::<adw::ToastOverlay>()
        {
            overlay.add_toast(adw::Toast::new(&text));
        }
    }
}

/// Why systemd refused, to go in a sentence. Without a program that can ask the user to
/// authenticate, systemd gives its own error for what amounts to a refusal.
fn reason_of(error: &glib::Error) -> String {
    let remote = gio::DBusError::remote_error(error);
    let refused = matches!(
        remote.as_deref(),
        Some(
            "org.freedesktop.DBus.Error.AccessDenied"
                | "org.freedesktop.DBus.Error.AuthFailed"
                | "org.freedesktop.DBus.Error.InteractiveAuthorizationRequired"
        )
    );
    if refused {
        return gettext("permission was not given");
    }
    let message = error.message();
    match message
        .strip_prefix("GDBus.Error:")
        .and_then(|rest| rest.split_once(": "))
    {
        Some((_, reason)) => reason.to_owned(),
        None => message.to_owned(),
    }
}

fn service_of_item() -> gtk::PropertyExpression {
    gtk::ListItem::this_expression("item")
}

fn name_factory() -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("a list item");
        let title = gtk::Inscription::builder()
            .text_overflow(gtk::InscriptionOverflow::EllipsizeEnd)
            .hexpand(true)
            .build();
        let name = gtk::Inscription::builder()
            .text_overflow(gtk::InscriptionOverflow::EllipsizeEnd)
            .css_classes(["caption", "dim-label"])
            .build();
        service_of_item()
            .chain_property::<Service>("description")
            .bind(&title, "text", Some(item));
        service_of_item()
            .chain_property::<Service>("name")
            .bind(&name, "text", Some(item));
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .margin_top(5)
            .margin_bottom(5)
            .margin_start(12)
            .margin_end(6)
            .build();
        content.append(&title);
        content.append(&name);
        item.set_child(Some(&content));
    });
    factory
}

fn text_factory(
    text: impl Fn() -> gtk::Expression + 'static,
    coloured: bool,
) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("a list item");
        let label = gtk::Label::builder()
            .xalign(0.0)
            .margin_start(6)
            .margin_end(6)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        text().bind(&label, "label", Some(item));
        if coloured {
            service_of_item()
                .chain_property::<Service>("rank")
                .chain_closure::<Vec<String>>(glib::closure!(
                    |_: Option<glib::Object>, rank: u32| match rank {
                        0 => vec!["error".to_owned()],
                        1 => vec!["success".to_owned()],
                        _ => vec!["dim-label".to_owned()],
                    }
                ))
                .bind(&label, "css-classes", Some(item));
        }
        item.set_child(Some(&label));
    });
    factory
}

fn heat_factory(
    text: impl Fn() -> gtk::Expression + 'static,
    level: impl Fn() -> gtk::Expression + 'static,
) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("a list item");
        let cell = HeatCell::default();
        level().bind(&cell, "level", Some(item));
        text().bind(cell.text(), "text", Some(item));
        item.set_child(Some(&cell));
    });
    factory
}
