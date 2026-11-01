use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use std::cell::RefCell;

mod imp {
    use super::*;

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::Stat)]
    pub struct Stat {
        #[property(get, set)]
        title: RefCell<String>,
        #[property(get, set)]
        value: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Stat {
        const NAME: &'static str = "LocusStat";
        type Type = super::Stat;
        type ParentType = gtk::FlowBoxChild;
    }

    #[glib::derived_properties]
    impl ObjectImpl for Stat {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            let title = gtk::Label::builder()
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .css_classes(["caption", "dim-label"])
                .build();
            let value = gtk::Label::builder()
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .css_classes(["title-3", "numeric"])
                .build();
            obj.bind_property("title", &title, "label")
                .sync_create()
                .build();
            obj.bind_property("value", &value, "label")
                .sync_create()
                .build();
            let content = gtk::Box::new(gtk::Orientation::Vertical, 2);
            content.append(&title);
            content.append(&value);
            obj.set_child(Some(&content));
            obj.set_focusable(false);
        }
    }

    impl WidgetImpl for Stat {}
    impl FlowBoxChildImpl for Stat {}
}

glib::wrapper! {
    /// A named value in a grid of them, hidden while there is no value to show.
    pub struct Stat(ObjectSubclass<imp::Stat>)
        @extends gtk::FlowBoxChild, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Stat {
    pub fn show_value(&self, value: Option<String>) {
        self.set_visible(value.is_some());
        if let Some(value) = value {
            self.set_value(value);
        }
    }
}
