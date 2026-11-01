use crate::config;
use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::{gio, glib};
use std::cell::Cell;

mod imp {
    use super::*;

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::RangeSwitch)]
    pub struct RangeSwitch {
        /// Whether this is the one in a bar of its own, for a window too narrow for the other
        /// to fit in the header.
        #[property(get, set, construct_only)]
        in_bar: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RangeSwitch {
        const NAME: &'static str = "LocusRangeSwitch";
        type Type = super::RangeSwitch;
        type ParentType = adw::Bin;
    }

    #[glib::derived_properties]
    impl ObjectImpl for RangeSwitch {
        fn constructed(&self) {
            self.parent_constructed();
            let group = adw::ToggleGroup::new();
            group.set_tooltip_text(Some(&gettext("How Far Back the Graphs Reach")));
            for (name, label) in [
                ("1min", gettext("1 min")),
                ("2min", gettext("2 min")),
                ("5min", gettext("5 min")),
            ] {
                let toggle = adw::Toggle::new();
                toggle.set_name(Some(name));
                toggle.set_label(Some(&label));
                group.add(toggle);
            }
            gio::Settings::new(config::APP_ID)
                .bind("graph-range", &group, "active-name")
                .build();
            self.obj().set_child(Some(&group));
        }
    }

    impl WidgetImpl for RangeSwitch {
        fn root(&self) {
            self.parent_root();
            let obj = self.obj();
            if let Some(window) = obj.root() {
                let binding = window
                    .bind_property("narrow", &*obj, "visible")
                    .sync_create();
                if self.in_bar.get() {
                    binding.build();
                } else {
                    binding.invert_boolean().build();
                }
            }
        }
    }

    impl BinImpl for RangeSwitch {}
}

glib::wrapper! {
    /// Chooses how far back every graph reaches.
    pub struct RangeSwitch(ObjectSubclass<imp::RangeSwitch>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}
