use crate::config;
use crate::cpu_page::CpuPage;
use crate::disk_page::DiskPage;
use crate::format;
use crate::gpu::Gpu;
use crate::gpu_page::GpuPage;
use crate::memory_page::MemoryPage;
use crate::network_page::NetworkPage;
use crate::processes_page::ProcessesPage;
use crate::sampler::{self, Disk, Focus, Link, LinkKind, Sampler, Scope, Snapshot, System};
use crate::sidebar_row::SidebarRow;
use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::{gio, glib};
use std::cell::{OnceCell, RefCell};
use std::time::Duration;

/// A device that comes and goes with the hardware: its row in the sidebar and its page.
struct DeviceView<P> {
    key: String,
    row: SidebarRow,
    page: P,
}

mod imp {
    use super::*;

    #[derive(gtk::CompositeTemplate)]
    #[template(resource = "/io/github/sachesi/locus/ui/window.ui")]
    pub struct LocusWindow {
        #[template_child]
        pub toasts: TemplateChild<adw::ToastOverlay>,
        #[template_child]
        pub split_view: TemplateChild<adw::NavigationSplitView>,
        #[template_child]
        pub sidebar: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub content_page: TemplateChild<adw::NavigationPage>,
        #[template_child]
        pub stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub processes_page: TemplateChild<ProcessesPage>,
        #[template_child]
        pub cpu_page: TemplateChild<CpuPage>,
        #[template_child]
        pub memory_page: TemplateChild<MemoryPage>,

        /// The rows of the pages that are always there, the first of which heads the devices.
        pub cpu_row: OnceCell<SidebarRow>,
        pub memory_row: OnceCell<SidebarRow>,
        pub(super) disks: RefCell<Vec<DeviceView<DiskPage>>>,
        pub(super) links: RefCell<Vec<DeviceView<NetworkPage>>>,
        pub(super) gpus: RefCell<Vec<DeviceView<GpuPage>>>,

        pub settings: gio::Settings,
        /// Cuts the wait for the next reading short.
        pub wake: RefCell<Option<async_channel::Sender<()>>>,
        /// Fades the pages in once the first reading has filled them, so the window does
        /// not open on empty graphs.
        pub reveal: OnceCell<adw::TimedAnimation>,
    }

    impl Default for LocusWindow {
        fn default() -> Self {
            Self {
                toasts: TemplateChild::default(),
                split_view: TemplateChild::default(),
                sidebar: TemplateChild::default(),
                content_page: TemplateChild::default(),
                stack: TemplateChild::default(),
                processes_page: TemplateChild::default(),
                cpu_page: TemplateChild::default(),
                memory_page: TemplateChild::default(),
                cpu_row: OnceCell::new(),
                memory_row: OnceCell::new(),
                disks: RefCell::default(),
                links: RefCell::default(),
                gpus: RefCell::default(),
                settings: gio::Settings::new(config::APP_ID),
                wake: RefCell::default(),
                reveal: OnceCell::new(),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LocusWindow {
        const NAME: &'static str = "LocusWindow";
        type Type = super::LocusWindow;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            CpuPage::ensure_type();
            MemoryPage::ensure_type();
            ProcessesPage::ensure_type();
            klass.bind_template();
            klass.bind_template_callbacks();

            klass.install_action("win.search", None, |win, _, _| {
                let imp = win.imp();
                win.show_page("processes");
                imp.processes_page.focus_search();
            });
            klass.install_action(
                "win.page",
                Some(glib::VariantTy::STRING),
                |win, _, parameter| {
                    if let Some(page) = parameter.and_then(|p| p.get::<String>()) {
                        win.show_page(&page);
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
            self.settings
                .bind("process-arrangement", &*self.processes_page, "arrangement")
                .build();
            format::set_binary(self.settings.boolean("binary-units"));

            obj.set_up_sidebar();

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
            let on_interval = wake.clone();
            self.settings.connect_changed(None, move |_, key| {
                if key.ends_with("-update-interval") {
                    on_interval();
                }
            });
            self.settings.connect_changed(
                Some("binary-units"),
                glib::clone!(
                    #[weak]
                    obj,
                    move |settings, key| {
                        format::set_binary(settings.boolean(key));
                        obj.imp().processes_page.show_units();
                        wake();
                    }
                ),
            );

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
        #[template_callback]
        fn on_row_activated(&self, row: &gtk::ListBoxRow) {
            if let Some(row) = row.downcast_ref::<SidebarRow>() {
                self.obj().show_page(&row.page());
            }
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
    /// lives. Processes and file systems are read only while their page is on screen:
    /// reading every process is most of what a reading costs, and a file system may be
    /// slow to answer.
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
                let scope = Scope {
                    processes: win.needs_processes(),
                    filesystems: win.shows_kind("disk:"),
                    focus: win.focus(),
                };
                let interval = win.interval();
                drop(win);

                let taken = sampler.take();
                let reading = gio::spawn_blocking(move || {
                    let mut sampler = taken.unwrap_or_default();
                    let snapshot = sampler.sample(scope);
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
                let wait = if snapshot.is_some_and(|s| s.unmeasured) {
                    sampler::BASELINE
                } else {
                    interval
                };
                let _ = glib::future_with_timeout(wait, woken.recv()).await;
            }
        });
    }

    /// Takes a reading now rather than at the end of the interval.
    pub fn wake(&self) {
        if let Some(wake) = &*self.imp().wake.borrow() {
            let _ = wake.try_send(());
        }
    }

    fn set_up_sidebar(&self) {
        let imp = self.imp();
        imp.sidebar.append(&SidebarRow::for_page(
            "processes",
            &gettext("Processes"),
            "view-list-symbolic",
        ));
        let cpu = SidebarRow::for_device("cpu", &gettext("CPU"), 100.0);
        let memory = SidebarRow::for_device("memory", &gettext("Memory"), 100.0);
        imp.sidebar.append(&cpu);
        imp.sidebar.append(&memory);
        // The heading of the devices goes above the first of them.
        let first = cpu.clone();
        imp.sidebar.set_header_func(move |row, _| {
            if row == first.upcast_ref::<gtk::ListBoxRow>() {
                let heading = gtk::Label::builder()
                    .label(gettext("Performance"))
                    .xalign(0.0)
                    .margin_top(12)
                    .margin_bottom(6)
                    .margin_start(12)
                    .css_classes(["caption-heading", "dim-label"])
                    .build();
                row.set_header(Some(&heading));
            } else {
                row.set_header(gtk::Widget::NONE);
            }
        });
        let _ = imp.cpu_row.set(cpu);
        let _ = imp.memory_row.set(memory);
        self.select_row("processes");
    }

    /// Shows `page`, where it exists, and the row that stands for it.
    fn show_page(&self, page: &str) {
        let imp = self.imp();
        if imp.stack.child_by_name(page).is_none() {
            return;
        }
        imp.stack.set_visible_child_name(page);
        self.select_row(page);
        imp.split_view.set_show_content(true);
    }

    fn select_row(&self, page: &str) {
        let imp = self.imp();
        let mut index = 0;
        while let Some(row) = imp.sidebar.row_at_index(index) {
            if let Some(row) = row.downcast_ref::<SidebarRow>()
                && row.page() == page
            {
                imp.sidebar.select_row(Some(row));
                imp.content_page.set_title(&row.title());
                return;
            }
            index += 1;
        }
    }

    /// Whether `page` is on screen, in a window that is.
    fn shows(&self, page: &str) -> bool {
        self.imp().stack.visible_child_name().as_deref() == Some(page) && !self.is_suspended()
    }

    /// Whether a page whose name starts with `kind` is on screen, in a window that is.
    fn shows_kind(&self, kind: &str) -> bool {
        !self.is_suspended()
            && self
                .imp()
                .stack
                .visible_child_name()
                .is_some_and(|page| page.starts_with(kind))
    }

    /// The Processes page lists the processes; the CPU and GPU pages name the ones using the
    /// most, and a GPU page adds up what they do on each engine.
    fn needs_processes(&self) -> bool {
        self.shows("processes") || self.shows("cpu") || self.shows_kind("gpu:")
    }

    fn focus(&self) -> Option<Focus> {
        if self.shows("cpu") {
            Some(Focus::Cpu)
        } else if self.shows("memory") {
            Some(Focus::Memory)
        } else if self.shows_kind("disk:") {
            Some(Focus::Disk)
        } else if self.shows_kind("network:") {
            Some(Focus::Network)
        } else if self.shows_kind("gpu:") {
            Some(Focus::Gpu)
        } else {
            None
        }
    }

    /// The update interval of the page on screen.
    fn interval(&self) -> Duration {
        let imp = self.imp();
        let key = if imp.stack.visible_child_name().as_deref() == Some("processes") {
            "processes-update-interval"
        } else {
            "resources-update-interval"
        };
        let seconds = imp.settings.uint(key).clamp(1, 10);
        Duration::from_secs(u64::from(seconds))
    }

    fn apply(&self, snapshot: &Snapshot) {
        let imp = self.imp();
        let system = &snapshot.system;
        let unmeasured = snapshot.unmeasured;
        self.sync_devices(system);
        self.apply_sidebar(system, unmeasured);
        imp.cpu_page.apply(system, self.shows("cpu"), unmeasured);
        imp.memory_page
            .apply(system, self.shows("memory"), unmeasured);
        self.apply_devices(system, unmeasured);
        if let Some(processes) = &snapshot.processes {
            if self.shows("processes") {
                imp.processes_page.apply(processes, system);
            } else if self.shows("cpu") {
                imp.cpu_page.show_top(processes);
            } else {
                for view in imp.gpus.borrow().iter() {
                    if self.shows(&view.key) {
                        view.page.show_top(processes);
                    }
                }
            }
        }
        imp.reveal.get_or_init(|| {
            let stack = &*imp.stack;
            let target = adw::PropertyAnimationTarget::new(stack, "opacity");
            let fade = adw::TimedAnimation::new(stack, 0.0, 1.0, 250, target);
            fade.play();
            fade
        });
    }

    /// Adds rows and pages for the devices that have appeared and removes those of the ones
    /// gone. The rows go back in their order, the disks first, then the network and the cards.
    fn sync_devices(&self, system: &System) {
        let imp = self.imp();
        let disk_keys: Vec<String> = system
            .disks
            .iter()
            .map(|d| format!("disk:{}", d.name))
            .collect();
        let link_keys: Vec<String> = system
            .links
            .iter()
            .map(|l| format!("network:{}", l.name))
            .collect();
        let gpu_keys: Vec<String> = system
            .gpus
            .iter()
            .map(|g| format!("gpu:{}", g.slot))
            .collect();
        let same = |keys: &[String], known: Vec<String>| keys == known.as_slice();
        if same(
            &disk_keys,
            imp.disks.borrow().iter().map(|v| v.key.clone()).collect(),
        ) && same(
            &link_keys,
            imp.links.borrow().iter().map(|v| v.key.clone()).collect(),
        ) && same(
            &gpu_keys,
            imp.gpus.borrow().iter().map(|v| v.key.clone()).collect(),
        ) {
            return;
        }
        let page_before = imp.stack.visible_child_name();
        {
            let mut disks = imp.disks.borrow_mut();
            self.sync_views(&mut disks, &disk_keys, |index, key| {
                let disk = &system.disks[index];
                let title = disk_title(index, disk);
                let page = DiskPage::default();
                page.set_name(&title, disk);
                self.add_device(key, &title, 100.0, page)
            });
            let mut links = imp.links.borrow_mut();
            self.sync_views(&mut links, &link_keys, |index, key| {
                let link = &system.links[index];
                let title = link_title(link);
                let page = NetworkPage::default();
                page.set_name(&title, "");
                self.add_device(key, &title, 0.0, page)
            });
            let mut gpus = imp.gpus.borrow_mut();
            let several = system.gpus.len() > 1;
            self.sync_views(&mut gpus, &gpu_keys, |index, key| {
                let gpu = &system.gpus[index];
                let title = gpu_title(index, several);
                let page = GpuPage::default();
                page.set_name(&title, gpu);
                self.add_device(key, &title, 100.0, page)
            });
            for row in disks
                .iter()
                .map(|view| &view.row)
                .chain(links.iter().map(|view| &view.row))
                .chain(gpus.iter().map(|view| &view.row))
            {
                if row.parent().is_some() {
                    imp.sidebar.remove(row);
                }
                imp.sidebar.append(row);
            }
        }
        match page_before {
            Some(page) if imp.stack.child_by_name(&page).is_some() => self.select_row(&page),
            _ => self.show_page("processes"),
        }
    }

    /// Keeps the views of `keys`, making those missing with `make` and dropping the others.
    fn sync_views<P: IsA<gtk::Widget>>(
        &self,
        views: &mut Vec<DeviceView<P>>,
        keys: &[String],
        make: impl Fn(usize, &str) -> DeviceView<P>,
    ) {
        let imp = self.imp();
        let mut known = std::mem::take(views);
        for view in known.iter().filter(|view| !keys.contains(&view.key)) {
            imp.stack.remove(&view.page);
            imp.sidebar.remove(&view.row);
        }
        for (index, key) in keys.iter().enumerate() {
            match known.iter().position(|view| view.key == *key) {
                Some(at) => views.push(known.swap_remove(at)),
                None => views.push(make(index, key)),
            }
        }
    }

    fn add_device<P: IsA<gtk::Widget>>(
        &self,
        key: &str,
        title: &str,
        maximum: f64,
        page: P,
    ) -> DeviceView<P> {
        self.imp().stack.add_named(&page, Some(key));
        DeviceView {
            key: key.to_owned(),
            row: SidebarRow::for_device(key, title, maximum),
            page,
        }
    }

    fn apply_sidebar(&self, system: &System, unmeasured: bool) {
        let imp = self.imp();
        let memory = &system.memory;
        let memory_share = format::share(memory.used, memory.total);
        let (Some(cpu), Some(memory_row)) = (imp.cpu_row.get(), imp.memory_row.get()) else {
            return;
        };
        if !unmeasured {
            cpu.push(&[system.cpu]);
            memory_row.push(&[memory_share]);
        }
        if self.is_suspended() {
            return;
        }
        if !unmeasured {
            cpu.set_subtitle(&format::percent(system.cpu, 0));
        }
        memory_row.set_subtitle(&format::listed(
            &format::percent(memory_share, 0),
            &format::size(memory.used),
        ));
    }

    fn apply_devices(&self, system: &System, unmeasured: bool) {
        let imp = self.imp();
        let visible = !self.is_suspended();
        for (view, disk) in imp.disks.borrow().iter().zip(&system.disks) {
            if !unmeasured {
                view.row.push(&[disk.active]);
                if visible {
                    view.row.set_subtitle(&disk_subtitle(disk));
                }
            }
            view.page.apply(disk, self.shows(&view.key), unmeasured);
        }
        if let Some(filesystems) = &system.filesystems {
            for (index, (view, disk)) in imp.disks.borrow().iter().zip(&system.disks).enumerate() {
                if self.shows(&view.key) {
                    view.page
                        .show_filesystems(&disk.name, index == 0, filesystems);
                }
            }
        }
        for (view, link) in imp.links.borrow().iter().zip(&system.links) {
            if !unmeasured {
                view.row.push(&[link.received, link.sent]);
                if visible {
                    view.row.set_subtitle(&link_subtitle(link));
                }
            }
            if let Some(detail) = link.detail.as_ref().filter(|_| self.shows(&view.key)) {
                view.page
                    .set_name(&link_title(link), &link_connection(detail.speed));
            }
            view.page.apply(link, self.shows(&view.key), unmeasured);
        }
        for (view, gpu) in imp.gpus.borrow().iter().zip(&system.gpus) {
            if !unmeasured {
                view.row.push(&[gpu.usage.unwrap_or(0.0)]);
            }
            if visible {
                view.row.set_subtitle(&gpu_subtitle(gpu));
            }
            view.page.apply(gpu, self.shows(&view.key), unmeasured);
        }
    }
}

fn disk_title(index: usize, disk: &Disk) -> String {
    // Translators: one of the disks; {number} counts from 1, {name} is the kernel's name for it.
    gettext("Disk {number} ({name})")
        .replace("{number}", &(index + 1).to_string())
        .replace("{name}", &disk.name)
}

fn disk_subtitle(disk: &Disk) -> String {
    let kind = if disk.rotational {
        gettext("HDD")
    } else {
        gettext("SSD")
    };
    format::listed(&kind, &format::percent(disk.active, 0))
}

fn link_title(link: &Link) -> String {
    let title = match link.kind {
        // Translators: a network interface; {name} is the kernel's name for it.
        LinkKind::Ethernet => gettext("Ethernet ({name})"),
        // Translators: a network interface; {name} is the kernel's name for it.
        LinkKind::Wireless => gettext("Wi-Fi ({name})"),
        // Translators: a network interface; {name} is the kernel's name for it.
        LinkKind::Virtual => gettext("Network ({name})"),
    };
    title.replace("{name}", &link.name)
}

fn link_subtitle(link: &Link) -> String {
    format!(
        "↓ {}  ↑ {}",
        format::rate(link.received),
        format::rate(link.sent)
    )
}

fn link_connection(speed: Option<u32>) -> String {
    match speed.filter(|speed| *speed > 0) {
        Some(_) => gettext("Connected"),
        None => String::new(),
    }
}

fn gpu_title(index: usize, several: bool) -> String {
    if several {
        // Translators: one of several graphics cards; {number} counts from 1.
        gettext("GPU {number}").replace("{number}", &(index + 1).to_string())
    } else {
        gettext("GPU")
    }
}

fn gpu_subtitle(gpu: &Gpu) -> String {
    if gpu.asleep {
        return gettext("Asleep");
    }
    let usage = gpu.usage.map(|usage| format::percent(usage, 0));
    let temperature = gpu.temperature.map(format::temperature);
    match (usage, temperature) {
        (Some(usage), Some(temperature)) => format::listed(&usage, &temperature),
        (Some(value), None) | (None, Some(value)) => value,
        (None, None) => String::new(),
    }
}
