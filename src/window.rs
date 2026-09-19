use crate::config;
use crate::processes_page::ProcessesPage;
use crate::resources_page::ResourcesPage;
use crate::sampler::{Sampler, Snapshot};
use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::time::Duration;

mod imp {
    use super::*;

    #[derive(gtk::CompositeTemplate)]
    #[template(resource = "/io/github/sachesi/locus/ui/window.ui")]
    pub struct LocusWindow {
        #[template_child]
        pub toasts: TemplateChild<adw::ToastOverlay>,
        #[template_child]
        pub stack: TemplateChild<adw::ViewStack>,
        #[template_child]
        pub processes_page: TemplateChild<ProcessesPage>,
        #[template_child]
        pub resources_page: TemplateChild<ResourcesPage>,
        #[template_child]
        pub interval_menu: TemplateChild<gio::Menu>,

        pub settings: gio::Settings,
        /// Cuts the wait for the next reading short.
        pub wake: RefCell<Option<async_channel::Sender<()>>>,
    }

    impl Default for LocusWindow {
        fn default() -> Self {
            Self {
                toasts: TemplateChild::default(),
                stack: TemplateChild::default(),
                processes_page: TemplateChild::default(),
                resources_page: TemplateChild::default(),
                interval_menu: TemplateChild::default(),
                settings: gio::Settings::new(config::APP_ID),
                wake: RefCell::default(),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LocusWindow {
        const NAME: &'static str = "LocusWindow";
        type Type = super::LocusWindow;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            ProcessesPage::ensure_type();
            ResourcesPage::ensure_type();
            klass.bind_template();
            klass.bind_template_callbacks();

            klass.install_action("win.search", None, |win, _, _| {
                let imp = win.imp();
                imp.stack.set_visible_child_name("processes");
                imp.processes_page.focus_search();
            });
            klass.install_action(
                "win.page",
                Some(glib::VariantTy::STRING),
                |win, _, parameter| {
                    if let Some(page) = parameter.and_then(|p| p.get::<String>()) {
                        win.imp().stack.set_visible_child_name(&page);
                    }
                },
            );
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for LocusWindow {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

            let (width, height) = self.settings.get::<(i32, i32)>("window-size");
            obj.set_default_size(width, height);
            if self.settings.boolean("window-maximized") {
                obj.maximize();
            }

            self.settings
                .bind("show-all-processes", &*self.processes_page, "show-all")
                .get()
                .build();
            obj.add_action(&self.settings.create_action("show-all-processes"));
            obj.add_action(&self.settings.create_action("update-interval"));
            for (seconds, label) in [
                (1u32, gettext("Every Second")),
                (2, gettext("Every 2 Seconds")),
                (5, gettext("Every 5 Seconds")),
            ] {
                let item = gio::MenuItem::new(Some(&label), None);
                item.set_action_and_target_value(
                    Some("win.update-interval"),
                    Some(&seconds.to_variant()),
                );
                self.interval_menu.append_item(&item);
            }

            // A reading is taken at once when what it has to cover changes.
            let wake = glib::clone!(
                #[weak]
                obj,
                move || obj.wake()
            );
            let on_page = wake.clone();
            self.stack
                .connect_visible_child_name_notify(move |_| on_page());
            let on_suspended = wake.clone();
            obj.connect_suspended_notify(move |_| on_suspended());
            self.settings
                .connect_changed(Some("update-interval"), move |_, _| wake());

            obj.start_sampling();
        }
    }

    impl WidgetImpl for LocusWindow {}

    impl WindowImpl for LocusWindow {
        fn close_request(&self) -> glib::Propagation {
            let obj = self.obj();
            let (width, height) = obj.default_size();
            let _ = self.settings.set("window-size", (width, height));
            let _ = self
                .settings
                .set_boolean("window-maximized", obj.is_maximized());
            self.parent_close_request()
        }
    }

    impl ApplicationWindowImpl for LocusWindow {}
    impl AdwApplicationWindowImpl for LocusWindow {}

    #[gtk::template_callbacks]
    impl LocusWindow {
        #[template_callback(function)]
        fn is_processes(page: Option<&str>) -> bool {
            page == Some("processes")
        }
    }
}

glib::wrapper! {
    pub struct LocusWindow(ObjectSubclass<imp::LocusWindow>)
        @extends adw::ApplicationWindow, gtk::ApplicationWindow, gtk::Window, gtk::Widget,
        @implements gio::ActionGroup, gio::ActionMap, gtk::Accessible, gtk::Buildable,
                    gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl LocusWindow {
    pub fn new(app: &adw::Application) -> Self {
        glib::Object::builder().property("application", app).build()
    }

    /// Reads the machine on a worker, one reading at a time, for as long as the window
    /// lives. Processes are read only while their list is on screen: reading every one of
    /// them is most of what a reading costs.
    fn start_sampling(&self) {
        let (wake, woken) = async_channel::bounded(1);
        self.imp().wake.replace(Some(wake));
        let window = self.downgrade();
        glib::spawn_future_local(async move {
            // Made on the worker, since making it reads the processor's model.
            let mut sampler: Option<Sampler> = None;
            loop {
                let Some(win) = window.upgrade() else {
                    break;
                };
                let with_processes = win.shows_processes();
                let interval = win.interval();
                drop(win);

                let taken = sampler.take();
                let reading = gio::spawn_blocking(move || {
                    let mut sampler = taken.unwrap_or_default();
                    let snapshot = sampler.sample(with_processes);
                    (sampler, snapshot)
                })
                .await;
                // A reading that panicked took its sampler with it; the next starts afresh.
                let snapshot = reading.ok().map(|(returned, snapshot)| {
                    sampler = Some(returned);
                    snapshot
                });

                let Some(win) = window.upgrade() else {
                    break;
                };
                if let Some(snapshot) = &snapshot {
                    win.apply(snapshot);
                }
                drop(win);
                let _ = glib::future_with_timeout(interval, woken.recv()).await;
            }
        });
    }

    /// Takes a reading now rather than at the end of the interval.
    pub fn wake(&self) {
        if let Some(wake) = &*self.imp().wake.borrow() {
            let _ = wake.try_send(());
        }
    }

    fn shows_processes(&self) -> bool {
        self.imp().stack.visible_child_name().as_deref() == Some("processes")
            && !self.is_suspended()
    }

    fn interval(&self) -> Duration {
        let seconds = self.imp().settings.uint("update-interval").clamp(1, 10);
        Duration::from_secs(u64::from(seconds))
    }

    fn apply(&self, snapshot: &Snapshot) {
        let imp = self.imp();
        imp.resources_page.apply(&snapshot.system);
        if let Some(processes) = &snapshot.processes {
            imp.processes_page.apply(processes, &snapshot.system);
        }
    }
}
