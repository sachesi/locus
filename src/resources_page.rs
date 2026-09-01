use crate::format;
use crate::graph::Graph;
use crate::sampler::{FileSystem, System};
use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::glib;
use std::cell::{Cell, RefCell};

/// The widgets of one processor's graph.
struct CoreView {
    graph: Graph,
    value: gtk::Label,
}

/// The widgets of one file system's line.
struct FileSystemView {
    mount_point: String,
    free: gtk::Label,
    bar: gtk::LevelBar,
}

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate, glib::Properties)]
    #[template(resource = "/io/github/sachesi/locus/ui/resources_page.ui")]
    #[properties(wrapper_type = super::ResourcesPage)]
    pub struct ResourcesPage {
        #[template_child]
        pub cpu_model: TemplateChild<gtk::Label>,
        #[template_child]
        pub cpu_value: TemplateChild<gtk::Label>,
        #[template_child]
        pub cpu_graph: TemplateChild<Graph>,
        #[template_child]
        pub cores: TemplateChild<gtk::FlowBox>,
        #[template_child]
        pub cards: TemplateChild<gtk::Box>,
        #[template_child]
        pub memory_value: TemplateChild<gtk::Label>,
        #[template_child]
        pub memory_graph: TemplateChild<Graph>,
        #[template_child]
        pub memory_detail: TemplateChild<gtk::Label>,
        #[template_child]
        pub swap_legend: TemplateChild<gtk::Box>,
        #[template_child]
        pub swap_detail: TemplateChild<gtk::Label>,
        #[template_child]
        pub network_graph: TemplateChild<Graph>,
        #[template_child]
        pub received: TemplateChild<gtk::Label>,
        #[template_child]
        pub sent: TemplateChild<gtk::Label>,
        #[template_child]
        pub disk_graph: TemplateChild<Graph>,
        #[template_child]
        pub read: TemplateChild<gtk::Label>,
        #[template_child]
        pub written: TemplateChild<gtk::Label>,
        #[template_child]
        pub filesystems_card: TemplateChild<gtk::Box>,
        #[template_child]
        pub filesystems: TemplateChild<gtk::Box>,

        /// Set by the window's breakpoints; stacks the memory, network and disk cards
        /// rather than setting them side by side.
        #[property(get, set = Self::set_compact)]
        compact: Cell<bool>,

        pub(super) core_views: RefCell<Vec<CoreView>>,
        pub(super) filesystem_views: RefCell<Vec<FileSystemView>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ResourcesPage {
        const NAME: &'static str = "LocusResourcesPage";
        type Type = super::ResourcesPage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            Graph::ensure_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for ResourcesPage {
        fn constructed(&self) {
            self.parent_constructed();
            self.network_graph.set_scale_label(format::rate);
            self.disk_graph.set_scale_label(format::rate);
        }
    }

    impl WidgetImpl for ResourcesPage {}
    impl BinImpl for ResourcesPage {}

    impl ResourcesPage {
        fn set_compact(&self, compact: bool) {
            self.compact.set(compact);
            self.cards.set_orientation(if compact {
                gtk::Orientation::Vertical
            } else {
                gtk::Orientation::Horizontal
            });
        }
    }
}

glib::wrapper! {
    pub struct ResourcesPage(ObjectSubclass<imp::ResourcesPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ResourcesPage {
    pub fn apply(&self, system: &System) {
        let imp = self.imp();

        if let Some(model) = &system.cpu_model
            && imp.cpu_model.label() != **model
        {
            imp.cpu_model.set_label(model);
            imp.cpu_model.set_visible(true);
        }
        imp.cpu_value.set_label(&format::percent(system.cpu, 0));
        imp.cpu_graph.push(&[system.cpu]);
        self.apply_cores(&system.cores);

        let memory = &system.memory;
        let share = |used: u64, total: u64| {
            if total == 0 {
                0.0
            } else {
                used as f64 * 100.0 / total as f64
            }
        };
        let memory_share = share(memory.used, memory.total);
        imp.memory_value
            .set_label(&format::percent(memory_share, 0));
        imp.memory_detail
            .set_label(&format::used_of(memory.used, memory.total));
        imp.swap_legend.set_visible(memory.swap_total > 0);
        if memory.swap_total > 0 {
            imp.swap_detail
                .set_label(&format::used_of(memory.swap_used, memory.swap_total));
            imp.memory_graph
                .push(&[memory_share, share(memory.swap_used, memory.swap_total)]);
        } else {
            imp.memory_graph.push(&[memory_share]);
        }

        imp.network_graph.push(&[system.received, system.sent]);
        imp.received.set_label(&format::rate(system.received));
        imp.sent.set_label(&format::rate(system.sent));

        imp.disk_graph.push(&[system.read, system.written]);
        imp.read.set_label(&format::rate(system.read));
        imp.written.set_label(&format::rate(system.written));
        self.apply_filesystems(&system.filesystems);
    }

    fn apply_cores(&self, cores: &[f64]) {
        let imp = self.imp();
        let mut views = imp.core_views.borrow_mut();
        if views.len() != cores.len() {
            imp.cores.remove_all();
            views.clear();
            for number in 1..=cores.len() {
                let (view, widget) = core_view(number);
                imp.cores.append(&widget);
                views.push(view);
            }
            // One processor is the whole machine, already drawn above.
            imp.cores.set_visible(cores.len() > 1);
        }
        for (view, &usage) in views.iter().zip(cores) {
            view.graph.push(&[usage]);
            view.value.set_label(&format::percent(usage, 0));
        }
    }

    fn apply_filesystems(&self, filesystems: &[FileSystem]) {
        let imp = self.imp();
        let mut views = imp.filesystem_views.borrow_mut();
        let same = views.len() == filesystems.len()
            && views
                .iter()
                .zip(filesystems)
                .all(|(view, fs)| view.mount_point == fs.mount_point);
        if !same {
            while let Some(child) = imp.filesystems.first_child() {
                imp.filesystems.remove(&child);
            }
            views.clear();
            for filesystem in filesystems {
                let (view, widget) = filesystem_view(filesystem);
                imp.filesystems.append(&widget);
                views.push(view);
            }
            imp.filesystems_card.set_visible(!filesystems.is_empty());
        }
        for (view, filesystem) in views.iter().zip(filesystems) {
            view.free
                .set_label(&format::free_of(filesystem.available, filesystem.size));
            view.bar
                .set_value(filesystem.used as f64 / filesystem.size as f64);
        }
    }
}

fn core_view(number: usize) -> (CoreView, gtk::Widget) {
    let graph = Graph::default();
    graph.set_maximum(100.0);
    graph.set_plain(true);
    graph.set_height_request(36);
    let name = gettext("CPU {number}").replace("{number}", &number.to_string());
    graph.update_property(&[gtk::accessible::Property::Label(&name)]);
    let label = gtk::Label::builder()
        .label(&name)
        .xalign(0.0)
        .hexpand(true)
        .css_classes(["caption", "dim-label"])
        .build();
    let value = gtk::Label::builder()
        .css_classes(["caption", "numeric"])
        .build();
    let caption = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    caption.append(&label);
    caption.append(&value);
    let widget = gtk::Box::new(gtk::Orientation::Vertical, 4);
    widget.append(&graph);
    widget.append(&caption);
    let child = gtk::FlowBoxChild::builder()
        .child(&widget)
        .focusable(false)
        .build();
    (CoreView { graph, value }, child.upcast())
}

fn filesystem_view(filesystem: &FileSystem) -> (FileSystemView, gtk::Widget) {
    let name = gtk::Label::builder()
        .label(&filesystem.mount_point)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .build();
    let device = gtk::Label::builder()
        .label(format!("{} · {}", filesystem.fs_type, filesystem.source))
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .css_classes(["caption", "dim-label"])
        .build();
    let names = gtk::Box::new(gtk::Orientation::Vertical, 0);
    names.set_hexpand(true);
    names.append(&name);
    names.append(&device);
    let free = gtk::Label::builder()
        .valign(gtk::Align::End)
        .css_classes(["dim-label", "numeric"])
        .build();
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    top.append(&names);
    top.append(&free);
    let bar = gtk::LevelBar::new();
    // The default offsets colour a nearly empty bar as a warning, as for a battery; here
    // the bar turns to one past nine tenths full.
    for offset in [
        gtk::LEVEL_BAR_OFFSET_LOW,
        gtk::LEVEL_BAR_OFFSET_HIGH,
        gtk::LEVEL_BAR_OFFSET_FULL,
    ] {
        bar.remove_offset_value(Some(offset));
    }
    bar.add_offset_value("roomy", 0.9);
    bar.add_offset_value("almost-full", 1.0);
    bar.update_property(&[gtk::accessible::Property::Label(&filesystem.mount_point)]);
    let widget = gtk::Box::new(gtk::Orientation::Vertical, 6);
    widget.append(&top);
    widget.append(&bar);
    (
        FileSystemView {
            mount_point: filesystem.mount_point.clone(),
            free,
            bar,
        },
        widget.upcast(),
    )
}
