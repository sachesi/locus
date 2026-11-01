use crate::format;
use crate::graph::Graph;
use crate::heat_cell::{self, HeatCell};
use crate::process_details::{self, Details, OpenFile, ThreadInfo};
use crate::procfs::Socket;
use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::time::Instant;

/// The nice value of each priority offered, in their order in the list.
const PRIORITIES: [i32; 3] = [10, 0, -5];

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/sachesi/locus/ui/details_pane.ui")]
    pub struct DetailsPane {
        #[template_child]
        pub title: TemplateChild<adw::WindowTitle>,
        #[template_child]
        pub stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub tab: TemplateChild<adw::ToggleGroup>,
        #[template_child]
        pub cpu_value: TemplateChild<gtk::Label>,
        #[template_child]
        pub cpu_graph: TemplateChild<Graph>,
        #[template_child]
        pub memory_value: TemplateChild<gtk::Label>,
        #[template_child]
        pub memory_graph: TemplateChild<Graph>,
        #[template_child]
        pub command: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub running_for: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub user: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub unit: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub threads_count: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub scheduling: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub priority: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub background: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub threads: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub files: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub sockets: TemplateChild<gtk::ListBox>,

        /// The processes shown, each with its start time, the first being what scheduling
        /// and the unit are read from.
        pub pids: RefCell<Vec<(u32, u64)>>,
        pub reading: Cell<bool>,
        /// Set while the controls are brought up to what was read, as opposed to the user
        /// changing them.
        pub updating: Cell<bool>,
        /// Time each thread had on a processor when last read, and when that was.
        pub thread_ticks: RefCell<HashMap<u32, u64>>,
        pub read_at: Cell<Option<Instant>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DetailsPane {
        const NAME: &'static str = "LocusDetailsPane";
        type Type = super::DetailsPane;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            Graph::ensure_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for DetailsPane {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            self.priority.connect_selected_notify(glib::clone!(
                #[weak]
                obj,
                move |row| obj.on_priority_chosen(row.selected())
            ));
            self.background.connect_active_notify(glib::clone!(
                #[weak]
                obj,
                move |row| obj.on_background_chosen(row.is_active())
            ));
            self.tab.connect_active_name_notify(glib::clone!(
                #[weak]
                obj,
                move |_| obj.read_again()
            ));
        }
    }

    impl WidgetImpl for DetailsPane {}
    impl BinImpl for DetailsPane {}
}

glib::wrapper! {
    /// More about the app or process selected in the list.
    pub struct DetailsPane(ObjectSubclass<imp::DetailsPane>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

/// What the list knows of what is selected.
pub struct Selected {
    pub name: String,
    pub subtitle: String,
    pub command: String,
    pub user: String,
    pub cpu: f64,
    pub memory: u64,
    pub own: bool,
    pub pids: Vec<(u32, u64)>,
}

impl DetailsPane {
    pub fn show_nothing(&self) {
        let imp = self.imp();
        imp.pids.borrow_mut().clear();
        imp.title.set_title(&gettext("Details"));
        imp.title.set_subtitle("");
        imp.stack.set_visible_child_name("nothing");
    }

    /// Shows `selected`, and reads what the list does not know.
    pub fn show(&self, selected: &Selected) {
        let imp = self.imp();
        if *imp.pids.borrow() != selected.pids {
            imp.pids.replace(selected.pids.clone());
            imp.cpu_graph.clear();
            imp.memory_graph.clear();
            imp.thread_ticks.borrow_mut().clear();
            imp.read_at.set(None);
        }
        imp.stack.set_visible_child_name("process");
        imp.title.set_title(&selected.name);
        imp.title.set_subtitle(&selected.subtitle);
        imp.cpu_graph.push(&[selected.cpu]);
        imp.memory_graph.push(&[selected.memory as f64]);
        imp.cpu_value.set_label(&format::percent(selected.cpu, 1));
        imp.memory_value.set_label(&format::size(selected.memory));
        imp.command.set_visible(!selected.command.is_empty());
        imp.command
            .set_subtitle(&glib::markup_escape_text(&selected.command));
        imp.user
            .set_subtitle(&glib::markup_escape_text(&selected.user));
        imp.scheduling.set_sensitive(selected.own);
        self.read_again();
    }

    fn read_again(&self) {
        let imp = self.imp();
        let pids = imp.pids.borrow().clone();
        if pids.is_empty() || imp.reading.replace(true) {
            return;
        }
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = pane)]
            self,
            async move {
                let asked = pids.clone();
                let details =
                    gio::spawn_blocking(move || process_details::read(&pids, ticks_per_second()))
                        .await;
                pane.imp().reading.set(false);
                if let Ok(details) = details
                    && *pane.imp().pids.borrow() == asked
                {
                    pane.apply(&details);
                }
            }
        ));
    }

    fn apply(&self, details: &Details) {
        let imp = self.imp();
        imp.updating.set(true);
        let nearest = (0..PRIORITIES.len())
            .min_by_key(|&index| PRIORITIES[index].abs_diff(details.nice))
            .unwrap_or(1);
        imp.priority
            .set_selected(u32::try_from(nearest).unwrap_or(1));
        imp.background.set_active(details.in_background);
        imp.updating.set(false);

        imp.running_for.set_visible(details.running_for.is_some());
        if let Some(seconds) = details.running_for {
            imp.running_for.set_subtitle(&format::uptime(seconds));
        }
        imp.unit.set_visible(details.unit.is_some());
        if let Some(unit) = &details.unit {
            imp.unit.set_subtitle(&glib::markup_escape_text(unit));
        }
        imp.threads_count
            .set_subtitle(&details.threads_count.to_string());
        match imp.tab.active_name().as_deref() {
            Some("threads") => self.show_threads(&details.threads),
            Some("files") => show_rows(&imp.files, details.files.as_deref(), file_row),
            Some("network") => show_rows(&imp.sockets, details.sockets.as_deref(), socket_row),
            _ => {}
        }
    }

    fn show_threads(&self, threads: &[ThreadInfo]) {
        let imp = self.imp();
        let seconds = imp
            .read_at
            .replace(Some(Instant::now()))
            .map(|at| at.elapsed().as_secs_f64());
        let mut known = imp.thread_ticks.borrow_mut();
        let cpus = glib::num_processors().max(1) as f64;
        let per_second = ticks_per_second() as f64;
        let row = |thread: &ThreadInfo| {
            let share = seconds.filter(|s| *s > 0.0).map_or(0.0, |s| {
                let ticks = known
                    .get(&thread.id)
                    .map_or(0, |before| thread.ticks.saturating_sub(*before));
                (ticks as f64 / per_second / s / cpus * 100.0).min(100.0)
            });
            thread_row(thread, share)
        };
        show_rows(&imp.threads, Some(threads), row);
        known.clear();
        known.extend(threads.iter().map(|thread| (thread.id, thread.ticks)));
    }

    fn on_priority_chosen(&self, index: u32) {
        let imp = self.imp();
        if imp.updating.get() {
            return;
        }
        let Some(&nice) = usize::try_from(index).ok().and_then(|i| PRIORITIES.get(i)) else {
            return;
        };
        self.change_scheduling(
            gettext("Could not change the priority: {reason}"),
            move |pid, start| process_details::set_nice(pid, start, nice),
        );
    }

    fn on_background_chosen(&self, background: bool) {
        if self.imp().updating.get() {
            return;
        }
        let message = if background {
            gettext("Could not run it in the background: {reason}")
        } else {
            gettext("Could not bring it back from the background: {reason}")
        };
        self.change_scheduling(message, move |pid, start| {
            process_details::set_in_background(pid, start, background)
        });
    }

    /// Applies `change` to each process shown on a worker; a failure is said in a toast, and
    /// the controls go back to what is read afterwards.
    fn change_scheduling(
        &self,
        failure: String,
        change: impl Fn(u32, u64) -> std::io::Result<()> + Send + 'static,
    ) {
        let pids = self.imp().pids.borrow().clone();
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = pane)]
            self,
            async move {
                let result = gio::spawn_blocking(move || {
                    pids.iter()
                        .map(|(pid, start)| change(*pid, *start))
                        .find(Result::is_err)
                        .unwrap_or(Ok(()))
                })
                .await;
                if let Ok(Err(error)) = result {
                    let reason = if error.kind() == std::io::ErrorKind::PermissionDenied {
                        gettext("administrator rights are needed")
                    } else {
                        error.to_string()
                    };
                    if let Some(overlay) = pane
                        .ancestor(adw::ToastOverlay::static_type())
                        .and_downcast::<adw::ToastOverlay>()
                    {
                        overlay.add_toast(adw::Toast::new(&failure.replace("{reason}", &reason)));
                    }
                }
                pane.read_again();
            }
        ));
    }
}

fn ticks_per_second() -> u64 {
    // SAFETY: sysconf only reads a configuration value.
    u64::try_from(unsafe { libc::sysconf(libc::_SC_CLK_TCK) }).unwrap_or(100)
}

/// Fills `list` with a row for each of `items`, or says that they could not be read, or that
/// there are none. The list is short, and shown only while its tab is open.
fn show_rows<T>(list: &gtk::ListBox, items: Option<&[T]>, row: impl Fn(&T) -> gtk::ListBoxRow) {
    list.remove_all();
    let empty = match items {
        None => Some(gettext("Only its owner may look at this")),
        Some([]) => Some(gettext("None")),
        Some(_) => None,
    };
    if let Some(message) = empty {
        let label = gtk::Label::builder()
            .label(message)
            .wrap(true)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .css_classes(["dim-label"])
            .build();
        list.append(
            &gtk::ListBoxRow::builder()
                .child(&label)
                .activatable(false)
                .build(),
        );
        return;
    }
    for item in items.unwrap_or_default() {
        list.append(&row(item));
    }
}

fn two_lines(title: &str, subtitle: &str, mono: bool) -> gtk::Box {
    let title = gtk::Label::builder()
        .label(title)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .tooltip_text(title)
        .build();
    if mono {
        title.add_css_class("monospace");
    }
    let subtitle = gtk::Label::builder()
        .label(subtitle)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["caption", "dim-label"])
        .build();
    let lines = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .hexpand(true)
        .build();
    lines.append(&title);
    lines.append(&subtitle);
    lines
}

fn padded(child: &impl IsA<gtk::Widget>) -> gtk::ListBoxRow {
    child.set_margin_top(8);
    child.set_margin_bottom(8);
    child.set_margin_start(12);
    child.set_margin_end(12);
    gtk::ListBoxRow::builder()
        .child(child)
        .activatable(false)
        .build()
}

fn thread_row(thread: &ThreadInfo, share: f64) -> gtk::ListBoxRow {
    // Translators: a thread of a process; {id} is its number.
    let id = gettext("Thread {id}").replace("{id}", &thread.id.to_string());
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    content.append(&two_lines(&thread.name, &id, false));
    let cell = HeatCell::default();
    cell.set_rounded(true);
    cell.set_valign(gtk::Align::Center);
    cell.set_width_request(64);
    cell.text().add_css_class("top-value");
    cell.text().set_text(Some(&format::percent(share, 1)));
    cell.set_level(heat_cell::cpu_level(share));
    content.append(&cell);
    padded(&content)
}

fn file_row(file: &OpenFile) -> gtk::ListBoxRow {
    let mode = match file.mode {
        0 => gettext("Read"),
        1 => gettext("Write"),
        _ => gettext("Read and Write"),
    };
    padded(&two_lines(&file.path, &mode, false))
}

fn socket_row(socket: &Socket) -> gtk::ListBoxRow {
    const LISTENING: u8 = 10;
    const ESTABLISHED: u8 = 1;
    let (address, state) = match socket.state {
        LISTENING => (socket.local, gettext("Listening")),
        ESTABLISHED => (socket.remote, gettext("Connected")),
        _ if socket.remote.port() == 0 => (socket.local, gettext("Open")),
        _ => (socket.remote, gettext("Open")),
    };
    padded(&two_lines(&address.to_string(), &state, false))
}
