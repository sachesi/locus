use crate::graph::Graph;
use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;
use std::cell::{OnceCell, RefCell};

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct SidebarRow {
        pub page: RefCell<String>,
        pub title: gtk::Label,
        pub subtitle: gtk::Label,
        pub graph: OnceCell<Graph>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SidebarRow {
        const NAME: &'static str = "LocusSidebarRow";
        type Type = super::SidebarRow;
        type ParentType = gtk::ListBoxRow;
    }

    impl ObjectImpl for SidebarRow {}
    impl WidgetImpl for SidebarRow {}
    impl ListBoxRowImpl for SidebarRow {}
}

glib::wrapper! {
    /// A row of the sidebar, which shows a page: one with an icon, or, for a device, with a
    /// small graph of it and what it is doing now.
    pub struct SidebarRow(ObjectSubclass<imp::SidebarRow>)
        @extends gtk::ListBoxRow, gtk::Widget,
        @implements gtk::Accessible, gtk::Actionable, gtk::Buildable, gtk::ConstraintTarget;
}

impl SidebarRow {
    fn with_start(page: &str, title: &str, start: &gtk::Widget) -> Self {
        let row: Self = glib::Object::new();
        let imp = row.imp();
        imp.page.replace(page.to_owned());
        imp.title.set_label(title);
        imp.title.set_xalign(0.0);
        imp.title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        imp.subtitle.set_xalign(0.0);
        imp.subtitle.set_ellipsize(gtk::pango::EllipsizeMode::End);
        imp.subtitle.set_visible(false);
        imp.subtitle.add_css_class("caption");
        imp.subtitle.add_css_class("dim-label");
        imp.subtitle.add_css_class("numeric");
        let names = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .valign(gtk::Align::Center)
            .hexpand(true)
            .build();
        names.append(&imp.title);
        names.append(&imp.subtitle);
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        content.append(start);
        content.append(&names);
        row.set_child(Some(&content));
        row
    }

    pub fn for_page(page: &str, title: &str, icon: &str) -> Self {
        Self::with_start(page, title, gtk::Image::from_icon_name(icon).upcast_ref())
    }

    pub fn for_device(page: &str, title: &str, maximum: f64) -> Self {
        let graph = Graph::default();
        graph.set_plain(true);
        graph.set_maximum(maximum);
        graph.set_size_request(50, 32);
        graph.set_valign(gtk::Align::Center);
        graph.update_property(&[gtk::accessible::Property::Label(title)]);
        let row = Self::with_start(page, title, graph.upcast_ref());
        let _ = row.imp().graph.set(graph);
        row
    }

    pub fn page(&self) -> String {
        self.imp().page.borrow().clone()
    }

    pub fn title(&self) -> String {
        self.imp().title.label().into()
    }

    pub fn set_title(&self, title: &str) {
        self.imp().title.set_label(title);
    }

    pub fn set_subtitle(&self, subtitle: &str) {
        let label = &self.imp().subtitle;
        label.set_label(subtitle);
        label.set_visible(true);
    }

    /// Adds a reading to the graph of a device.
    pub fn push(&self, values: &[f64]) {
        if let Some(graph) = self.imp().graph.get() {
            graph.push(values);
        }
    }
}
