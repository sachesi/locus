use crate::format;
use crate::process::{Process, Signal};
use crate::sampler::ProcessSample;
use crate::window::LocusWindow;
use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::{gdk, gio, glib, graphene};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io;
use std::time::Duration;

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
        pub stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub view: TemplateChild<gtk::ColumnView>,
        #[template_child]
        pub selection: TemplateChild<gtk::SingleSelection>,
        #[template_child]
        pub filter_model: TemplateChild<gtk::FilterListModel>,
        #[template_child]
        pub store: TemplateChild<gio::ListStore>,
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
        pub pid_column: TemplateChild<gtk::ColumnViewColumn>,
        #[template_child]
        pub empty_page: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub action_bar: TemplateChild<gtk::ActionBar>,
        #[template_child]
        pub selected_name: TemplateChild<gtk::Label>,
        #[template_child]
        pub selected_detail: TemplateChild<gtk::Label>,

        /// Set by the window's breakpoint; hides the columns that fit only when wide.
        #[property(get, set)]
        narrow: Cell<bool>,
        /// Whether the search bar is open; the query filters the list only while it is.
        #[property(get, set = Self::set_search_mode)]
        search_mode: Cell<bool>,
        /// Whether processes of other users are listed.
        #[property(get, set = Self::set_show_all)]
        show_all: Cell<bool>,

        pub filter: RefCell<Option<gtk::CustomFilter>>,
        pub query: RefCell<String>,
        pub by_pid: RefCell<HashMap<u32, Process>>,
        pub generation: Cell<u64>,
        pub own_uid: Cell<u32>,
        pub confirming: Cell<bool>,
        pub focus_before_menu: RefCell<Option<glib::WeakRef<gtk::Widget>>>,
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

            let filter = gtk::CustomFilter::new(glib::clone!(
                #[weak]
                obj,
                #[upgrade_or]
                false,
                move |item| item.downcast_ref::<Process>().is_some_and(|p| obj.shows(p))
            ));
            self.filter_model.set_filter(Some(&filter));
            self.filter.replace(Some(filter));

            let item = || gtk::ListItem::this_expression("item");
            self.name_column
                .set_factory(Some(&obj.cell_factory(0.0, false, move || {
                    item().chain_property::<Process>("name").upcast()
                })));
            self.user_column
                .set_factory(Some(&obj.cell_factory(0.0, false, move || {
                    item().chain_property::<Process>("user").upcast()
                })));
            self.cpu_column
                .set_factory(Some(&obj.cell_factory(1.0, true, move || {
                    item()
                        .chain_property::<Process>("cpu")
                        .chain_closure::<String>(glib::closure!(
                            |_: Option<glib::Object>, cpu: f64| format::percent(cpu, 1)
                        ))
                        .upcast()
                })));
            self.memory_column
                .set_factory(Some(&obj.cell_factory(1.0, true, move || {
                    item()
                        .chain_property::<Process>("memory")
                        .chain_closure::<String>(glib::closure!(
                            |_: Option<glib::Object>, memory: u64| format::size(memory)
                        ))
                        .upcast()
                })));
            self.disk_column
                .set_factory(Some(&obj.cell_factory(1.0, true, move || {
                    item()
                        .chain_property::<Process>("disk")
                        .chain_closure::<String>(glib::closure!(
                            |_: Option<glib::Object>, disk: i64| match u64::try_from(disk) {
                                // Left blank, so the few processes using the disks stand out.
                                Ok(0) => String::new(),
                                Ok(disk) => format::rate(disk as f64),
                                // Processes of other users do not show their input and output.
                                Err(_) => "—".to_owned(),
                            }
                        ))
                        .upcast()
                })));
            self.pid_column
                .set_factory(Some(&obj.cell_factory(1.0, true, move || {
                    item()
                        .chain_property::<Process>("pid")
                        .chain_closure::<String>(glib::closure!(
                            |_: Option<glib::Object>, pid: u32| pid.to_string()
                        ))
                        .upcast()
                })));

            let property = |name: &str| {
                gtk::PropertyExpression::new(Process::static_type(), gtk::Expression::NONE, name)
            };
            self.name_column
                .set_sorter(Some(&gtk::StringSorter::new(Some(property("name")))));
            self.user_column
                .set_sorter(Some(&gtk::StringSorter::new(Some(property("user")))));
            // Numbers sort largest first on the first click, which is what a monitor is
            // looked at for.
            for (column, name) in [
                (&self.cpu_column, "cpu"),
                (&self.memory_column, "memory"),
                (&self.disk_column, "disk"),
                (&self.pid_column, "pid"),
            ] {
                let sorter = gtk::NumericSorter::new(Some(property(name)));
                sorter.set_sort_order(gtk::SortType::Descending);
                column.set_sorter(Some(&sorter));
            }
            self.view
                .sort_by_column(Some(&*self.cpu_column), gtk::SortType::Ascending);

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

        #[template_callback]
        fn on_selection_changed(&self) {
            self.obj().update_selection();
        }

        #[template_callback]
        fn on_filtered_changed(&self) {
            let page = if self.by_pid.borrow().is_empty() {
                "loading"
            } else if self.filter_model.n_items() == 0 {
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

        fn set_search_mode(&self, search_mode: bool) {
            self.search_mode.set(search_mode);
            self.obj().refilter();
        }

        fn set_show_all(&self, show_all: bool) {
            self.show_all.set(show_all);
            self.obj().refilter();
            // Refiltering an empty list to an empty list changes no items.
            self.on_filtered_changed();
        }
    }
}

glib::wrapper! {
    pub struct ProcessesPage(ObjectSubclass<imp::ProcessesPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ProcessesPage {
    /// Brings the list up to a new reading: updates the processes it had, adds the new ones
    /// and drops those that have exited.
    pub fn apply(&self, samples: &[ProcessSample]) {
        let imp = self.imp();
        let generation = imp.generation.get() + 1;
        imp.generation.set(generation);

        let mut added = Vec::new();
        let mut by_pid = imp.by_pid.borrow_mut();
        for sample in samples {
            match by_pid.get(&sample.pid) {
                Some(process) if process.start_time() == sample.start_time => {
                    process.update(sample, generation);
                }
                _ => {
                    let process = Process::new(sample, generation);
                    by_pid.insert(sample.pid, process.clone());
                    added.push(process);
                }
            }
        }
        by_pid.retain(|_, p| p.generation() == generation);
        drop(by_pid);

        imp.store.retain(|item| {
            item.downcast_ref::<Process>()
                .is_some_and(|p| p.generation() == generation)
        });
        imp.store.extend_from_slice(&added);
        // The sort list model does not watch the values it sorts by. Rows stay where they
        // are while the user is choosing what to do with one of them.
        let choosing =
            imp.confirming.get() || imp.context_bin.popover().is_some_and(|p| p.is_visible());
        if !choosing && let Some(sorter) = imp.view.sorter() {
            sorter.changed(gtk::SorterChange::Different);
        }
        self.update_selection();
    }

    pub fn focus_search(&self) {
        self.set_search_mode(true);
        self.imp().search_entry.grab_focus();
    }

    fn shows(&self, process: &Process) -> bool {
        let imp = self.imp();
        if !self.show_all() && process.uid() != imp.own_uid.get() {
            return false;
        }
        let query = imp.query.borrow();
        !self.search_mode()
            || query.is_empty()
            || process.name().to_lowercase().contains(&*query)
            || process.command().to_lowercase().contains(&*query)
            || process.pid().to_string() == *query
    }

    fn refilter(&self) {
        if let Some(filter) = &*self.imp().filter.borrow() {
            filter.changed(gtk::FilterChange::Different);
        }
    }

    fn selected(&self) -> Option<Process> {
        self.imp().selection.selected_item().and_downcast()
    }

    fn update_selection(&self) {
        let imp = self.imp();
        let process = self.selected();
        imp.action_bar.set_revealed(process.is_some());
        // The kernel lets a user signal only their own processes, and root every one.
        let own_uid = imp.own_uid.get();
        let signalled = process
            .as_ref()
            .filter(|p| own_uid == 0 || p.uid() == own_uid);
        self.action_set_enabled("process.end", signalled.is_some());
        self.action_set_enabled("process.kill", signalled.is_some());
        let stopped = signalled.map(Process::stopped);
        // Stopped, Locus could not take the signal to continue from itself.
        let is_locus = signalled.is_some_and(|p| p.pid() == std::process::id());
        self.action_set_enabled("process.stop", stopped == Some(false) && !is_locus);
        self.action_set_enabled("process.continue", stopped == Some(true));
        let Some(process) = process else {
            // The process the menu was opened for has exited.
            if let Some(popover) = imp.context_bin.popover() {
                popover.popdown();
            }
            return;
        };

        imp.selected_name.set_label(&process.name());
        // Translators: {pid} is the ID of the process, a number.
        let mut detail = gettext("ID {pid}").replace("{pid}", &process.pid().to_string());
        if process.stopped() {
            // Translators: {detail} is "ID 1234"; a stopped process is paused until it is
            // continued.
            detail = gettext("{detail}, stopped").replace("{detail}", &detail);
        }
        if process.uid() != own_uid {
            // Translators: {detail} is "ID 1234", or "ID 1234, stopped"; {user} is a login name.
            detail = gettext("{detail}, owned by {user}")
                .replace("{detail}", &detail)
                .replace("{user}", &process.user());
        }
        let command = process.command();
        if !command.is_empty() {
            detail = format!("{detail} · {command}");
        }
        if imp.selected_detail.label() != detail {
            imp.selected_detail.set_label(&detail);
            imp.selected_detail.set_tooltip_text(Some(&command));
        }
    }

    fn cell_factory(
        &self,
        xalign: f32,
        numeric: bool,
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
                text().bind(&cell, "text", Some(item));
                page.add_context_menu(&cell, item);
                item.set_child(Some(&cell));
            }
        ));
        factory
    }

    /// Right click, or a long press, on a cell selects its row and opens the process menu
    /// where it happened.
    fn add_context_menu(&self, cell: &gtk::Inscription, item: &gtk::ListItem) {
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

    fn open_context_menu(&self, cell: &gtk::Inscription, position: u32, x: f64, y: f64) {
        let imp = self.imp();
        if position == gtk::INVALID_LIST_POSITION {
            return;
        }
        imp.selection.set_selected(position);
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
        if self.selected().is_none() || !focus.is_ancestor(&*imp.view) {
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
        let Some(process) = self.selected() else {
            return;
        };
        let name = process.name();
        let (heading, body, response) = match signal {
            Signal::Kill => (
                gettext("Kill “{name}”?"),
                gettext(
                    // Translators: {pid} is the ID of the process, a number.
                    "Process {pid} ends at once, without a chance to save its work or clean up.",
                ),
                gettext("_Kill Process"),
            ),
            _ => (
                gettext("End “{name}”?"),
                // Translators: {pid} is the ID of the process, a number.
                gettext("Process {pid} is asked to quit. Work it has not saved may be lost."),
                gettext("_End Process"),
            ),
        };
        let dialog = adw::AlertDialog::new(
            Some(&heading.replace("{name}", &name)),
            Some(&body.replace("{pid}", &process.pid().to_string())),
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
                    page.send(&process, signal);
                }
            }
        ));
    }

    fn signal_selected(&self, signal: Signal) {
        if let Some(process) = self.selected() {
            self.send(&process, signal);
        }
    }

    fn send(&self, process: &Process, signal: Signal) {
        let sent = process.send(signal);
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
        let Err(error) = sent else {
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
        let toast = adw::Toast::new(&message.replace("{name}", &process.name()));
        if let Some(overlay) = self
            .ancestor(adw::ToastOverlay::static_type())
            .and_downcast::<adw::ToastOverlay>()
        {
            overlay.add_toast(toast);
        }
    }
}
