use crate::format;
use crate::process::{Place, Process, Signal};
use crate::sampler::{ProcessSample, System};
use crate::window::LocusWindow;
use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::{gettext, ngettext};
use gtk::{gdk, gio, glib, graphene};
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::{HashMap, HashSet};
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A value the kernel does not let this user read, drawn faint so that the few values
/// there are stand out.
const UNSHOWN: &str = "<span alpha=\"55%\">—</span>";

/// How long after the pointer last moved over the list its rows keep their order.
const AIMING: Duration = Duration::from_secs(1);

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate, glib::Properties)]
    #[template(resource = "/io/github/sachesi/locus/ui/processes_page.ui")]
    #[properties(wrapper_type = super::ProcessesPage)]
    pub struct ProcessesPage {
        #[template_child]
        pub search_bar: TemplateChild<gtk::SearchBar>,
        #[template_child]
        pub search_entry: TemplateChild<gtk::SearchEntry>,
        #[template_child]
        pub context_bin: TemplateChild<gtk::PopoverBin>,
        #[template_child]
        pub header: TemplateChild<gtk::Box>,
        #[template_child]
        pub apps_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub processes_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub stop_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub continue_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub end_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub kill_button: TemplateChild<gtk::Button>,
        /// The icon and label of each action button, kept while a narrow window shows the
        /// icon alone.
        pub button_contents: RefCell<Vec<(gtk::Button, adw::ButtonContent)>>,
        #[template_child]
        pub stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub view: TemplateChild<gtk::ColumnView>,
        #[template_child]
        pub name_column: TemplateChild<gtk::ColumnViewColumn>,
        #[template_child]
        pub user_column: TemplateChild<gtk::ColumnViewColumn>,
        #[template_child]
        pub cpu_column: TemplateChild<gtk::ColumnViewColumn>,
        #[template_child]
        pub memory_column: TemplateChild<gtk::ColumnViewColumn>,
        #[template_child]
        pub disk_column: TemplateChild<gtk::ColumnViewColumn>,
        #[template_child]
        pub gpu_column: TemplateChild<gtk::ColumnViewColumn>,
        #[template_child]
        pub pid_column: TemplateChild<gtk::ColumnViewColumn>,
        #[template_child]
        pub empty_page: TemplateChild<adw::StatusPage>,

        /// Set by the window's breakpoint; hides the columns that fit only when wide.
        #[property(get, set = Self::set_narrow)]
        narrow: Cell<bool>,
        /// Whether the search bar is open; the query filters the list only while it is.
        #[property(get, set = Self::set_search_mode)]
        search_mode: Cell<bool>,
        /// Whether processes of other users are listed.
        #[property(get, set = Self::set_show_all)]
        show_all: Cell<bool>,

        /// The rows heading the apps, each with its processes beneath it, and the processes
        /// that are not part of an app and are listed beneath no other.
        pub groups: OnceCell<[Process; 2]>,
        pub tree: OnceCell<gtk::TreeListModel>,
        /// The row heading the apps. The model lets go of a row no one holds, and with it
        /// the handler that closes the apps each time the row opens.
        pub apps_row: OnceCell<gtk::TreeListRow>,
        pub listed: OnceCell<gtk::FilterListModel>,
        pub selection: OnceCell<gtk::SingleSelection>,
        pub filter: OnceCell<gtk::CustomFilter>,
        pub query: RefCell<String>,
        /// What a search shows: the processes it matches and what they are listed beneath.
        pub found: RefCell<HashSet<Process>>,
        pub by_pid: RefCell<HashMap<u32, Process>>,
        pub apps: RefCell<HashMap<Arc<str>, Process>>,
        /// The last reading, listed again when the processes of other users are shown or
        /// hidden.
        pub samples: RefCell<Vec<ProcessSample>>,
        pub read: Cell<bool>,
        /// Whether the machine has a graphics card to list the use of.
        pub has_gpus: Cell<bool>,
        pub generation: Cell<u64>,
        pub own_uid: Cell<u32>,
        pub confirming: Cell<bool>,
        pub focus_before_menu: RefCell<Option<glib::WeakRef<gtk::Widget>>>,
        /// When the pointer last moved over the list, while it is over it.
        pub pointer_moved: Cell<Option<Instant>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ProcessesPage {
        const NAME: &'static str = "LocusProcessesPage";
        type Type = super::ProcessesPage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            Process::ensure_type();
            klass.bind_template();
            klass.bind_template_callbacks();

            klass.install_action("process.end", None, |page, _, _| {
                page.confirm(Signal::Terminate);
            });
            klass.install_action("process.kill", None, |page, _, _| {
                page.confirm(Signal::Kill);
            });
            klass.install_action("process.menu", None, |page, _, _| {
                page.open_context_menu_at_focus();
            });
            klass.install_action("process.stop", None, |page, _, _| {
                page.signal_selected(Signal::Stop);
            });
            klass.install_action("process.continue", None, |page, _, _| {
                page.signal_selected(Signal::Continue);
            });
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for ProcessesPage {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            // SAFETY: getuid cannot fail and touches no memory.
            self.own_uid.set(unsafe { libc::getuid() });
            // The entry sits in a clamp, where the bar does not find it by itself; connected,
            // it takes the keys typed over the list and closes on Escape.
            self.search_bar.connect_entry(&*self.search_entry);

            obj.set_up_model();
            obj.set_up_columns();
            let buttons = [
                &self.stop_button,
                &self.continue_button,
                &self.end_button,
                &self.kill_button,
            ];
            self.button_contents.replace(
                buttons
                    .into_iter()
                    .filter_map(|button| {
                        let content = button.child().and_downcast::<adw::ButtonContent>()?;
                        Some(((**button).clone(), content))
                    })
                    .collect(),
            );

            let motion = gtk::EventControllerMotion::new();
            motion.connect_motion(glib::clone!(
                #[weak]
                obj,
                move |_, _, _| obj.imp().pointer_moved.set(Some(Instant::now()))
            ));
            motion.connect_leave(glib::clone!(
                #[weak]
                obj,
                move |_| obj.imp().pointer_moved.set(None)
            ));
            self.view.add_controller(motion);

            if let Some(popover) = self.context_bin.popover() {
                popover.connect_closed(glib::clone!(
                    #[weak]
                    obj,
                    move |popover| obj.restore_focus(popover)
                ));
            }

            obj.update_selection();
        }
    }

    impl WidgetImpl for ProcessesPage {}
    impl BinImpl for ProcessesPage {}

    #[gtk::template_callbacks]
    impl ProcessesPage {
        #[template_callback]
        fn on_search_changed(&self, entry: &gtk::SearchEntry) {
            self.query.replace(entry.text().to_lowercase());
            self.obj().refilter();
        }

        pub fn on_listed_changed(&self) {
            let listed = self.listed.get().map_or(0, |listed| listed.n_items());
            let page = if !self.read.get() {
                "loading"
            } else if listed == 0 {
                "empty"
            } else {
                "list"
            };
            self.stack.set_visible_child_name(page);
            let hint = if self.obj().show_all() {
                gettext("Try a different search")
            } else {
                gettext(
                    "Only your own processes are searched. Turn on “Show All Processes” in the main menu to search every one.",
                )
            };
            self.empty_page.set_description(Some(&hint));
        }

        fn set_narrow(&self, narrow: bool) {
            self.narrow.set(narrow);
            self.header.set_orientation(if narrow {
                gtk::Orientation::Vertical
            } else {
                gtk::Orientation::Horizontal
            });
            let margin = if narrow { 12 } else { 18 };
            for widget in [
                self.header.upcast_ref::<gtk::Widget>(),
                self.stack.upcast_ref(),
            ] {
                widget.set_margin_start(margin);
                widget.set_margin_end(margin);
            }
            self.stack.set_margin_bottom(margin);
            self.show_columns();
            // The labels of the buttons do not fit beside one another in a narrow window,
            // in every language.
            for (button, content) in &*self.button_contents.borrow() {
                if narrow {
                    button.set_icon_name(&content.icon_name());
                    button.set_tooltip_text(Some(&content.label().replace('_', "")));
                } else {
                    button.set_child(Some(content));
                    button.set_tooltip_text(None);
                }
            }
        }

        /// Shows the GPU column where there is a card and room for it.
        pub fn show_columns(&self) {
            self.gpu_column
                .set_visible(self.has_gpus.get() && !self.narrow.get());
        }

        fn set_search_mode(&self, search_mode: bool) {
            self.search_mode.set(search_mode);
            self.obj().refilter();
        }

        fn set_show_all(&self, show_all: bool) {
            self.show_all.set(show_all);
            let obj = self.obj();
            if self.read.get() {
                let samples = self.samples.take();
                obj.list(&samples);
                self.samples.replace(samples);
            }
            // Listing an empty search again changes no items.
            self.on_listed_changed();
        }
    }
}

glib::wrapper! {
    pub struct ProcessesPage(ObjectSubclass<imp::ProcessesPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ProcessesPage {
    /// Brings the list up to a new reading: updates the processes it had, adds the new ones,
    /// drops those that have exited and moves those whose parent has changed.
    pub fn apply(&self, samples: &[ProcessSample], system: &System) {
        let imp = self.imp();
        let has_gpus = !system.gpus.is_empty();
        if imp.has_gpus.replace(has_gpus) != has_gpus {
            imp.show_columns();
        }
        self.show_totals(samples, system);
        self.list(samples);
        imp.samples.replace(samples.to_vec());
        let first = !imp.read.replace(true);
        imp.on_listed_changed();
        if first {
            // The view keeps the first row it was given in sight while the rows are sorted
            // under it, which would open the list partway down.
            imp.view
                .scroll_to(0, None, gtk::ListScrollFlags::NONE, None);
        }
    }

    pub fn focus_search(&self) {
        self.set_search_mode(true);
        self.imp().search_entry.grab_focus();
    }

    /// The apps and the processes that are not part of one each head a tree of what
    /// started what, sorted by the view's sorter one level at a time, the apps first.
    fn set_up_model(&self) {
        let imp = self.imp();
        let groups = [
            Process::group(&gettext("Apps")),
            Process::group(&gettext("Processes")),
        ];
        let root = gio::ListStore::new::<Process>();
        root.extend_from_slice(&groups);
        let apps = groups[0].clone();
        let apps_first = gtk::CustomSorter::new(move |a, b| {
            let is_apps = |item: &glib::Object| item == apps.upcast_ref::<glib::Object>();
            match (is_apps(a), is_apps(b)) {
                (true, false) => gtk::Ordering::Smaller,
                (false, true) => gtk::Ordering::Larger,
                _ => gtk::Ordering::Equal,
            }
        });
        let sorter = gtk::MultiSorter::new();
        sorter.append(apps_first);
        if let Some(columns) = imp.view.sorter() {
            sorter.append(columns);
        }
        let filter = gtk::CustomFilter::new(glib::clone!(
            #[weak(rename_to = page)]
            self,
            #[upgrade_or]
            false,
            move |row| {
                row.downcast_ref::<gtk::TreeListRow>()
                    .and_then(gtk::TreeListRow::item)
                    .and_downcast::<Process>()
                    .is_some_and(|process| page.shows(&process))
            }
        ));
        let tree = gtk::TreeListModel::new(root, false, true, |item| {
            item.downcast_ref::<Process>()
                .map(|process| process.children().upcast())
        });
        // An app's processes are there to be looked into, and stay out of the way until
        // they are; other processes are shown with what they started.
        if let Some(apps_row) = tree.row(0) {
            apps_row.connect_expanded_notify(|apps_row| {
                if apps_row.is_expanded() {
                    let mut index = 0;
                    while let Some(row) = apps_row.child_row(index) {
                        row.set_expanded(false);
                        index += 1;
                    }
                }
            });
            let _ = imp.apps_row.set(apps_row);
        }
        let sorted = gtk::SortListModel::new(
            Some(tree.clone()),
            Some(gtk::TreeListRowSorter::new(Some(sorter))),
        );
        let listed = gtk::FilterListModel::new(Some(sorted), Some(filter.clone()));
        listed.connect_items_changed(glib::clone!(
            #[weak(rename_to = page)]
            self,
            move |_, _, _, _| page.imp().on_listed_changed()
        ));
        let selection = gtk::SingleSelection::builder()
            .autoselect(false)
            .can_unselect(true)
            .build();
        // Given with the others, the model would come first, and have its first row
        // selected before autoselect is turned off.
        selection.set_model(Some(&listed));
        selection.connect_selected_item_notify(glib::clone!(
            #[weak(rename_to = page)]
            self,
            move |_| page.update_selection()
        ));
        imp.view.set_model(Some(&selection));

        let _ = imp.groups.set(groups);
        let _ = imp.tree.set(tree);
        let _ = imp.listed.set(listed);
        let _ = imp.selection.set(selection);
        let _ = imp.filter.set(filter);
    }

    fn set_up_columns(&self) {
        let imp = self.imp();
        imp.name_column.set_factory(Some(&self.name_factory()));
        imp.user_column.set_factory(Some(&self.cell_factory(
            0.0,
            false,
            false,
            None,
            move || process_of_row().chain_property::<Process>("user").upcast(),
        )));
        imp.cpu_column.set_factory(Some(&self.cell_factory(
            1.0,
            true,
            false,
            None,
            move || {
                process_of_row()
                    .chain_property::<Process>("cpu")
                    .chain_closure::<String>(glib::closure!(|_: Option<glib::Object>, cpu: f64| {
                        format::percent(cpu, 1)
                    }))
                    .upcast()
            },
        )));
        imp.memory_column.set_factory(Some(&self.cell_factory(
            1.0,
            true,
            false,
            None,
            move || {
                process_of_row()
                    .chain_property::<Process>("memory")
                    .chain_closure::<String>(glib::closure!(
                        |_: Option<glib::Object>, memory: u64| format::size(memory)
                    ))
                    .upcast()
            },
        )));
        let unshown = process_of_row()
            .chain_property::<Process>("disk")
            .chain_closure::<Option<String>>(glib::closure!(
                |_: Option<glib::Object>, disk: i64| (disk < 0)
                    .then(|| gettext("Disk use of other users' processes is not shown"))
            ))
            .upcast();
        imp.disk_column.set_factory(Some(&self.cell_factory(
            1.0,
            true,
            true,
            Some(unshown),
            move || {
                process_of_row()
                    .chain_property::<Process>("disk")
                    .chain_closure::<String>(glib::closure!(
                        |_: Option<glib::Object>, disk: i64| match u64::try_from(disk) {
                            // Left blank, so the few processes using the disks stand out.
                            Ok(0) => String::new(),
                            Ok(disk) => format::rate(disk as f64),
                            // Processes of other users do not show their input and output.
                            Err(_) => UNSHOWN.to_owned(),
                        }
                    ))
                    .upcast()
            },
        )));
        let unshown = process_of_row()
            .chain_property::<Process>("gpu")
            .chain_closure::<Option<String>>(glib::closure!(|_: Option<glib::Object>, gpu: f64| {
                (gpu < 0.0).then(|| gettext("GPU use of other users' processes is not shown"))
            }))
            .upcast();
        imp.gpu_column.set_factory(Some(&self.cell_factory(
            1.0,
            true,
            true,
            Some(unshown),
            move || {
                process_of_row()
                    .chain_property::<Process>("gpu")
                    .chain_closure::<String>(glib::closure!(|_: Option<glib::Object>, gpu: f64| {
                        if gpu < 0.0 {
                            UNSHOWN.to_owned()
                        } else if gpu < 0.05 {
                            // Left blank, so the few processes using a card stand out.
                            String::new()
                        } else {
                            format::percent(gpu, 1)
                        }
                    }))
                    .upcast()
            },
        )));
        imp.pid_column.set_factory(Some(&self.cell_factory(
            1.0,
            true,
            false,
            None,
            move || {
                process_of_row()
                    .chain_property::<Process>("pid")
                    .chain_closure::<String>(glib::closure!(
                        // An app's row has no ID of its own.
                        |_: Option<glib::Object>, pid: u32| if pid == 0 {
                            String::new()
                        } else {
                            pid.to_string()
                        }
                    ))
                    .upcast()
            },
        )));

        let property = |name: &str| {
            gtk::PropertyExpression::new(Process::static_type(), gtk::Expression::NONE, name)
        };
        imp.name_column
            .set_sorter(Some(&gtk::StringSorter::new(Some(property("name")))));
        imp.user_column
            .set_sorter(Some(&gtk::StringSorter::new(Some(property("user")))));
        // Numbers sort largest first on the first click, which is what a monitor is
        // looked at for.
        for (column, name) in [
            (&imp.cpu_column, "cpu"),
            (&imp.memory_column, "memory"),
            (&imp.disk_column, "disk"),
            (&imp.gpu_column, "gpu"),
            (&imp.pid_column, "pid"),
        ] {
            let sorter = gtk::NumericSorter::new(Some(property(name)));
            sorter.set_sort_order(gtk::SortType::Descending);
            column.set_sorter(Some(&sorter));
        }
        imp.view
            .sort_by_column(Some(&*imp.cpu_column), gtk::SortType::Ascending);
    }

    /// Lists the processes of a reading that are to be shown, each beneath what started it,
    /// or beneath its app.
    fn list(&self, samples: &[ProcessSample]) {
        let imp = self.imp();
        let generation = imp.generation.get() + 1;
        imp.generation.set(generation);
        let own_uid = imp.own_uid.get();
        let show_all = self.show_all();

        // The objects are brought up to date first, and the tree changed after, as changing
        // it runs the handlers that look at them.
        let mut listed = Vec::new();
        let mut gone = Vec::new();
        {
            let mut by_pid = imp.by_pid.borrow_mut();
            for sample in samples.iter().filter(|s| show_all || s.uid == own_uid) {
                let process = match by_pid.get(&sample.pid) {
                    Some(process) if process.start_time() == sample.start_time => {
                        process.update(sample, generation);
                        process.clone()
                    }
                    _ => {
                        let process = Process::new(sample, generation);
                        gone.extend(by_pid.insert(sample.pid, process.clone()));
                        process
                    }
                };
                process.set_app_id(sample.app.as_ref().map(|app| app.id.clone()));
                listed.push((process, sample));
            }
            by_pid.retain(|_, process| {
                let listed = process.generation() == generation;
                if !listed {
                    gone.push(process.clone());
                }
                listed
            });
        }
        for process in &gone {
            self.detach(process);
        }

        let mut members: HashMap<Arc<str>, Vec<Process>> = HashMap::new();
        for (process, sample) in &listed {
            if let Some(app) = &sample.app {
                members
                    .entry(app.id.clone())
                    .or_default()
                    .push(process.clone());
            }
        }
        let mut new_apps = Vec::new();
        let mut gone_apps = Vec::new();
        {
            let mut apps = imp.apps.borrow_mut();
            for app in listed.iter().filter_map(|(_, sample)| sample.app.as_ref()) {
                if !apps.contains_key(&app.id) {
                    let row = Process::for_app(app);
                    apps.insert(app.id.clone(), row.clone());
                    new_apps.push(row);
                }
            }
            apps.retain(|id, row| {
                let running = members.contains_key(id);
                if !running {
                    gone_apps.push(row.clone());
                }
                running
            });
            for (id, processes) in &members {
                if let Some(row) = apps.get(id) {
                    row.add_up(processes);
                }
            }
        }
        for row in &new_apps {
            self.attach(row, Place::Apps);
        }
        self.collapse(&new_apps);

        let places: Vec<(Process, Place)> = {
            let by_pid = imp.by_pid.borrow();
            let apps = imp.apps.borrow();
            listed
                .iter()
                .map(|(process, sample)| {
                    let parent = by_pid.get(&sample.ppid);
                    let place = match &sample.app {
                        Some(app) => match parent {
                            Some(parent) if parent.app_id().as_ref() == Some(&app.id) => {
                                Place::Under(parent.downgrade())
                            }
                            _ => apps
                                .get(&app.id)
                                .map_or(Place::Top, |row| Place::Under(row.downgrade())),
                        },
                        // What init starts is at the top, not beneath it.
                        None => match parent {
                            Some(parent) if sample.ppid != 1 && parent.app_id().is_none() => {
                                Place::Under(parent.downgrade())
                            }
                            _ => Place::Top,
                        },
                    };
                    (process.clone(), place)
                })
                .collect()
        };
        for (process, place) in places {
            if !process.imp().place.borrow().is(&place) {
                self.detach(&process);
                self.attach(&process, place);
            }
        }
        for row in &gone_apps {
            self.detach(row);
        }

        self.show_counts();
        if self.searching() {
            self.refilter();
        }
        // The sort list models do not watch the values they sort by. Rows stay where they
        // are while the user is choosing what to do with one of them, or aiming the pointer
        // at one, so a click lands on the process it was meant for.
        let aiming = imp
            .pointer_moved
            .get()
            .is_some_and(|moved| moved.elapsed() < AIMING);
        let choosing = aiming
            || imp.confirming.get()
            || imp.context_bin.popover().is_some_and(|p| p.is_visible());
        if !choosing && let Some(sorter) = imp.view.sorter() {
            sorter.changed(gtk::SorterChange::Different);
        }
        self.update_selection();
    }

    /// The row a process listed in `place` is beneath.
    fn owner_of(&self, place: &Place) -> Option<Process> {
        let groups = self.imp().groups.get()?;
        match place {
            Place::Nowhere => None,
            Place::Apps => Some(groups[0].clone()),
            Place::Top => Some(groups[1].clone()),
            Place::Under(parent) => parent.upgrade(),
        }
    }

    fn detach(&self, process: &Process) {
        let place = process.imp().place.replace(Place::Nowhere);
        if let Some(owner) = self.owner_of(&place) {
            let store = owner.children();
            if let Some(position) = store.find(process) {
                store.remove(position);
                owner.set_child_count(store.n_items());
            }
        }
    }

    fn attach(&self, process: &Process, place: Place) {
        if let Some(owner) = self.owner_of(&place) {
            let store = owner.children();
            store.append(process);
            owner.set_child_count(store.n_items());
        }
        process.imp().place.replace(place);
    }

    /// Closes the rows of apps that have just been listed, which open as every row does.
    fn collapse(&self, apps: &[Process]) {
        let imp = self.imp();
        let Some(apps_row) = imp.apps_row.get() else {
            return;
        };
        let Some(groups) = imp.groups.get() else {
            return;
        };
        let store = groups[0].children();
        for app in apps {
            if let Some(index) = store.find(app)
                && let Some(row) = apps_row.child_row(index)
            {
                row.set_expanded(false);
            }
        }
    }

    fn show_counts(&self) {
        let imp = self.imp();
        let apps = imp.apps.borrow().len();
        let processes = imp.by_pid.borrow().len();
        let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        imp.apps_label.set_label(
            &ngettext("{count} Running App", "{count} Running Apps", count(apps))
                .replace("{count}", &apps.to_string()),
        );
        imp.processes_label.set_label(
            &ngettext(
                "{count} Running Process",
                "{count} Running Processes",
                count(processes),
            )
            .replace("{count}", &processes.to_string()),
        );
    }

    /// Heads the columns of the machine's use with how much of it there is in all.
    fn show_totals(&self, samples: &[ProcessSample], system: &System) {
        let imp = self.imp();
        // The busiest card, where the cards say how busy they are; else what the processes
        // that can be read keep them busy with.
        let gpu = system
            .gpus
            .iter()
            .filter_map(|gpu| gpu.usage)
            .reduce(f64::max)
            .unwrap_or_else(|| {
                samples
                    .iter()
                    .filter_map(|sample| sample.gpu)
                    .sum::<f64>()
                    .min(100.0)
            });
        let memory = &system.memory;
        let memory_share = if memory.total == 0 {
            0.0
        } else {
            memory.used as f64 * 100.0 / memory.total as f64
        };
        for (column, title, total) in [
            (
                &imp.cpu_column,
                gettext("CPU"),
                format::percent(system.cpu, 0),
            ),
            (
                &imp.memory_column,
                gettext("Memory"),
                format::percent(memory_share, 0),
            ),
            (
                &imp.disk_column,
                gettext("Disk"),
                format::rate(system.read + system.written),
            ),
            (&imp.gpu_column, gettext("GPU"), format::percent(gpu, 0)),
        ] {
            column.set_title(Some(&format!("{title}\n{total}")));
        }
    }

    fn searching(&self) -> bool {
        self.search_mode() && !self.imp().query.borrow().is_empty()
    }

    fn shows(&self, process: &Process) -> bool {
        !self.searching() || self.imp().found.borrow().contains(process)
    }

    /// Finds what the search matches, opens what it is listed beneath and filters the rest
    /// out.
    fn refilter(&self) {
        let imp = self.imp();
        let mut found = HashSet::new();
        if self.searching() {
            let query = imp.query.borrow();
            let matches = |process: &Process| {
                process.name().to_lowercase().contains(&*query)
                    || process.command().to_lowercase().contains(&*query)
                    || (process.pid() != 0 && process.pid().to_string() == *query)
            };
            let by_pid = imp.by_pid.borrow();
            let apps = imp.apps.borrow();
            for process in by_pid.values().chain(apps.values()).filter(|p| matches(p)) {
                // Up to the first already found, which stops it going round a loop that a
                // pid given out again can make.
                let mut next = Some(process.clone());
                while let Some(process) = next {
                    if !found.insert(process.clone()) {
                        break;
                    }
                    next = self.owner_of(&process.imp().place.borrow());
                }
            }
        }
        imp.found.replace(found);
        if self.searching() {
            self.expand_found();
        }
        if let Some(filter) = imp.filter.get() {
            filter.changed(gtk::FilterChange::Different);
        }
    }

    fn expand_found(&self) {
        let imp = self.imp();
        let Some(tree) = imp.tree.get() else {
            return;
        };
        // Opening a row adds its children after it, where the loop comes to them next.
        let mut position = 0;
        while let Some(row) = tree.item(position).and_downcast::<gtk::TreeListRow>() {
            if !row.is_expanded()
                && let Some(process) = row.item().and_downcast::<Process>()
                && process.child_count() > 0
                && imp.found.borrow().contains(&process)
            {
                row.set_expanded(true);
            }
            position += 1;
        }
    }

    fn selected(&self) -> Option<Process> {
        self.imp()
            .selection
            .get()?
            .selected_item()
            .and_downcast::<gtk::TreeListRow>()?
            .item()
            .and_downcast()
    }

    /// What the actions on `process` signal: the process, or every process of the app
    /// its row stands for, those of them the kernel lets this user signal.
    fn targets(&self, process: &Process) -> Vec<Process> {
        let own_uid = self.imp().own_uid.get();
        // The kernel lets a user signal only their own processes, and root every one.
        let signalled = |process: &Process| own_uid == 0 || process.uid() == own_uid;
        if process.is_group() {
            Vec::new()
        } else if process.is_app() {
            let app = process.app_id();
            self.imp()
                .by_pid
                .borrow()
                .values()
                .filter(|member| member.app_id() == app && signalled(member))
                .cloned()
                .collect()
        } else if signalled(process) {
            vec![process.clone()]
        } else {
            Vec::new()
        }
    }

    fn update_selection(&self) {
        let imp = self.imp();
        let selected = self.selected();
        let targets = selected
            .as_ref()
            .map(|process| self.targets(process))
            .unwrap_or_default();
        self.action_set_enabled("process.end", !targets.is_empty());
        self.action_set_enabled("process.kill", !targets.is_empty());
        let single = selected
            .as_ref()
            .filter(|process| !process.is_app() && !targets.is_empty());
        let stopped = single.map(Process::stopped);
        // Stopped, Locus could not take the signal to continue from itself.
        let is_locus = single.is_some_and(|p| p.pid() == std::process::id());
        self.action_set_enabled("process.stop", stopped == Some(false) && !is_locus);
        self.action_set_enabled("process.continue", stopped == Some(true));
        imp.stop_button.set_visible(stopped != Some(true));
        imp.continue_button.set_visible(stopped == Some(true));
        if selected.is_none() {
            // The process the menu was opened for has exited.
            if let Some(popover) = imp.context_bin.popover() {
                popover.popdown();
            }
        }
    }

    /// The first column: what the row stands for, its icon, and whether it is stopped,
    /// beneath what it is listed under.
    fn name_factory(&self) -> gtk::SignalListItemFactory {
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(glib::clone!(
            #[weak(rename_to = page)]
            self,
            move |_, item| {
                let item = item.downcast_ref::<gtk::ListItem>().expect("a list item");
                let icon = gtk::Image::new();
                let name = gtk::Inscription::builder()
                    .hexpand(true)
                    .text_overflow(gtk::InscriptionOverflow::EllipsizeEnd)
                    .build();
                let stopped = gtk::Image::builder()
                    .icon_name("media-playback-pause-symbolic")
                    .tooltip_text(gettext("Stopped"))
                    .build();
                let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                content.append(&icon);
                content.append(&name);
                content.append(&stopped);
                let expander = gtk::TreeExpander::builder().child(&content).build();

                process_of_row()
                    .chain_property::<Process>("name")
                    .bind(&name, "text", Some(item));
                process_of_row()
                    .chain_property::<Process>("command")
                    .chain_closure::<Option<String>>(glib::closure!(
                        |_: Option<glib::Object>, command: String| (!command.is_empty())
                            .then_some(command)
                    ))
                    .bind(&name, "tooltip-text", Some(item));
                process_of_row().chain_property::<Process>("stopped").bind(
                    &stopped,
                    "visible",
                    Some(item),
                );
                process_of_row()
                    .chain_property::<Process>("icon")
                    .chain_closure::<gio::Icon>(glib::closure!(
                        |_: Option<glib::Object>, icon: Option<gio::Icon>| {
                            icon.unwrap_or_else(|| {
                                gio::ThemedIcon::new("application-x-executable-symbolic").upcast()
                            })
                        }
                    ))
                    .bind(&icon, "gicon", Some(item));
                process_of_row()
                    .chain_property::<Process>("is-app")
                    .chain_closure::<i32>(glib::closure!(
                        |_: Option<glib::Object>, is_app: bool| if is_app { 24 } else { 16 }
                    ))
                    .bind(&icon, "pixel-size", Some(item));
                process_of_row()
                    .chain_property::<Process>("is-group")
                    .chain_closure::<bool>(glib::closure!(
                        |_: Option<glib::Object>, is_group: bool| !is_group
                    ))
                    .bind(&icon, "visible", Some(item));
                process_of_row()
                    .chain_property::<Process>("is-group")
                    .chain_closure::<Vec<String>>(glib::closure!(
                        |_: Option<glib::Object>, is_group: bool| if is_group {
                            vec!["heading".to_owned()]
                        } else {
                            Vec::new()
                        }
                    ))
                    .bind(&name, "css-classes", Some(item));
                process_of_row()
                    .chain_property::<Process>("child-count")
                    .chain_closure::<bool>(glib::closure!(
                        |_: Option<glib::Object>, children: u32| children == 0
                    ))
                    .bind(&expander, "hide-expander", Some(item));
                page.add_context_menu(expander.upcast_ref(), item);
                item.set_child(Some(&expander));
            }
        ));
        factory.connect_bind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().expect("a list item");
            if let Some(expander) = item.child().and_downcast::<gtk::TreeExpander>() {
                expander.set_list_row(item.item().and_downcast_ref::<gtk::TreeListRow>());
            }
        });
        factory.connect_unbind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().expect("a list item");
            if let Some(expander) = item.child().and_downcast::<gtk::TreeExpander>() {
                expander.set_list_row(None);
            }
        });
        factory
    }

    /// A cell showing `text`, or, where `markup` is set, Pango markup.
    fn cell_factory(
        &self,
        xalign: f32,
        numeric: bool,
        markup: bool,
        tooltip: Option<gtk::Expression>,
        text: impl Fn() -> gtk::Expression + 'static,
    ) -> gtk::SignalListItemFactory {
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(glib::clone!(
            #[weak(rename_to = page)]
            self,
            move |_, item| {
                let item = item.downcast_ref::<gtk::ListItem>().expect("a list item");
                let cell = gtk::Inscription::builder()
                    .xalign(xalign)
                    .text_overflow(gtk::InscriptionOverflow::EllipsizeEnd)
                    .build();
                if numeric {
                    cell.add_css_class("numeric");
                }
                text().bind(&cell, if markup { "markup" } else { "text" }, Some(item));
                // The rows heading the groups have a name alone.
                process_of_row()
                    .chain_property::<Process>("is-group")
                    .chain_closure::<bool>(glib::closure!(
                        |_: Option<glib::Object>, is_group: bool| !is_group
                    ))
                    .bind(&cell, "visible", Some(item));
                if let Some(tooltip) = &tooltip {
                    tooltip.bind(&cell, "tooltip-text", Some(item));
                }
                page.add_context_menu(cell.upcast_ref(), item);
                item.set_child(Some(&cell));
            }
        ));
        factory
    }

    /// Right click, or a long press, on a cell selects its row and opens the process menu
    /// where it happened.
    fn add_context_menu(&self, cell: &gtk::Widget, item: &gtk::ListItem) {
        let open = glib::clone!(
            #[weak(rename_to = page)]
            self,
            #[weak]
            cell,
            #[weak]
            item,
            move |x: f64, y: f64| page.open_context_menu(&cell, item.position(), x, y)
        );
        let click = gtk::GestureClick::builder()
            .button(gdk::BUTTON_SECONDARY)
            .build();
        let on_click = open.clone();
        click.connect_pressed(move |gesture, _, x, y| {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            on_click(x, y);
        });
        cell.add_controller(click);
        let press = gtk::GestureLongPress::builder().touch_only(true).build();
        press.connect_pressed(move |gesture, x, y| {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            open(x, y);
        });
        cell.add_controller(press);
    }

    fn open_context_menu(&self, cell: &gtk::Widget, position: u32, x: f64, y: f64) {
        let imp = self.imp();
        if position == gtk::INVALID_LIST_POSITION {
            return;
        }
        if let Some(selection) = imp.selection.get() {
            selection.set_selected(position);
        }
        if self.selected().is_some_and(|process| process.is_group()) {
            return;
        }
        let Some(point) =
            cell.compute_point(&*imp.context_bin, &graphene::Point::new(x as f32, y as f32))
        else {
            return;
        };
        self.popup_context_menu(&gdk::Rectangle::new(
            point.x() as i32,
            point.y() as i32,
            1,
            1,
        ));
    }

    /// Opens the process menu beneath the row with the keyboard focus, which is the one
    /// selected.
    fn open_context_menu_at_focus(&self) {
        let imp = self.imp();
        let Some(focus) = self.root().and_then(|root| root.focus()) else {
            return;
        };
        if self.selected().is_none_or(|process| process.is_group())
            || !focus.is_ancestor(&*imp.view)
        {
            return;
        }
        let Some(bounds) = focus.compute_bounds(&*imp.context_bin) else {
            return;
        };
        self.popup_context_menu(&gdk::Rectangle::new(
            bounds.x() as i32,
            bounds.y() as i32,
            bounds.width() as i32,
            bounds.height() as i32,
        ));
    }

    fn popup_context_menu(&self, pointing_to: &gdk::Rectangle) {
        let imp = self.imp();
        let Some(popover) = imp.context_bin.popover() else {
            return;
        };
        let focus = self.root().and_then(|root| root.focus());
        imp.focus_before_menu
            .replace(focus.map(|widget| widget.downgrade()));
        popover.set_pointing_to(Some(pointing_to));
        // PopoverBin's own popup points the popover at the whole bin again.
        popover.popup();
    }

    /// Gives the focus back to where it was when the menu opened, unless what the menu
    /// opened, such as a confirmation, has taken it.
    fn restore_focus(&self, popover: &gtk::Popover) {
        let Some(widget) = self
            .imp()
            .focus_before_menu
            .take()
            .and_then(|widget| widget.upgrade())
        else {
            return;
        };
        let focus = self.root().and_then(|root| root.focus());
        if focus.is_none_or(|focus| focus.is_ancestor(popover)) {
            widget.grab_focus();
        }
    }

    /// Ending and killing lose whatever the process had not saved, so they ask first.
    fn confirm(&self, signal: Signal) {
        let Some(selected) = self.selected() else {
            return;
        };
        let targets = self.targets(&selected);
        if targets.is_empty() {
            return;
        }
        let name = selected.name();
        let count = u32::try_from(targets.len()).unwrap_or(u32::MAX);
        let (heading, body, response) = match signal {
            Signal::Kill => (
                gettext("Kill “{name}”?"),
                if selected.is_app() {
                    ngettext(
                        "Its {count} process ends at once, without a chance to save its work or clean up.",
                        "Its {count} processes end at once, without a chance to save their work or clean up.",
                        count,
                    )
                } else {
                    gettext(
                        // Translators: {pid} is the ID of the process, a number.
                        "Process {pid} ends at once, without a chance to save its work or clean up.",
                    )
                },
                if selected.is_app() {
                    gettext("_Kill App")
                } else {
                    gettext("_Kill Process")
                },
            ),
            _ => (
                gettext("End “{name}”?"),
                if selected.is_app() {
                    ngettext(
                        "Its {count} process is asked to quit. Work it has not saved may be lost.",
                        "Its {count} processes are asked to quit. Work they have not saved may be lost.",
                        count,
                    )
                } else {
                    // Translators: {pid} is the ID of the process, a number.
                    gettext("Process {pid} is asked to quit. Work it has not saved may be lost.")
                },
                if selected.is_app() {
                    gettext("_End App")
                } else {
                    gettext("_End Process")
                },
            ),
        };
        let dialog = adw::AlertDialog::new(
            Some(&heading.replace("{name}", &name)),
            Some(
                &body
                    .replace("{pid}", &selected.pid().to_string())
                    .replace("{count}", &targets.len().to_string()),
            ),
        );
        dialog.add_responses(&[("cancel", &gettext("_Cancel")), ("signal", &response)]);
        dialog.set_response_appearance("signal", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = page)]
            self,
            async move {
                page.imp().confirming.set(true);
                let response = dialog.choose_future(Some(&page)).await;
                page.imp().confirming.set(false);
                if response == "signal" {
                    page.send(&selected, &targets, signal).await;
                }
            }
        ));
    }

    fn signal_selected(&self, signal: Signal) {
        let Some(selected) = self.selected() else {
            return;
        };
        let targets = self.targets(&selected);
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = page)]
            self,
            async move { page.send(&selected, &targets, signal).await }
        ));
    }

    /// Signals `targets`, the processes of what `shown` is the row of.
    async fn send(&self, shown: &Process, targets: &[Process], signal: Signal) {
        let mut failure = None;
        for target in targets {
            if let Err(error) = target.send(signal).await {
                // Some processes of an app end on their own while the others are signalled.
                let exited = error.raw_os_error() == Some(libc::ESRCH);
                if !(exited && shown.is_app()) {
                    failure.get_or_insert(error);
                }
            }
        }
        // The list shows what the signal did at once, rather than at the next reading; a
        // process asked to quit gets a moment to do so first.
        glib::timeout_add_local_once(
            Duration::from_millis(250),
            glib::clone!(
                #[weak(rename_to = page)]
                self,
                move || {
                    if let Some(window) = page.root().and_downcast::<LocusWindow>() {
                        window.wake();
                    }
                }
            ),
        );
        let Some(error) = failure else {
            return;
        };
        let message = if error.raw_os_error() == Some(libc::ESRCH) {
            gettext("“{name}” has already exited")
        } else {
            let failure = match signal {
                Signal::Terminate => {
                    // Translators: {reason} is "it belongs to another user", or an error
                    // message of the system; so too in the three messages after this one.
                    gettext("Could not end “{name}”: {reason}")
                }
                Signal::Kill => gettext("Could not kill “{name}”: {reason}"),
                Signal::Stop => gettext("Could not stop “{name}”: {reason}"),
                Signal::Continue => gettext("Could not continue “{name}”: {reason}"),
            };
            let reason = if error.kind() == io::ErrorKind::PermissionDenied {
                gettext("it belongs to another user")
            } else {
                error.to_string()
            };
            failure.replace("{reason}", &reason)
        };
        let toast = adw::Toast::new(&message.replace("{name}", &shown.name()));
        if let Some(overlay) = self
            .ancestor(adw::ToastOverlay::static_type())
            .and_downcast::<adw::ToastOverlay>()
        {
            overlay.add_toast(toast);
        }
    }
}

/// The process a row of the list stands for, from its list item.
fn process_of_row() -> gtk::PropertyExpression {
    gtk::ListItem::this_expression("item").chain_property::<gtk::TreeListRow>("item")
}
