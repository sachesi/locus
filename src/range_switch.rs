use crate::config;
use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::{gio, glib};

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct RangeSwitch {}

    #[glib::object_subclass]
    impl ObjectSubclass for RangeSwitch {
        const NAME: &'static str = "LocusRangeSwitch";
        type Type = super::RangeSwitch;
        type ParentType = adw::Bin;
    }

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

    impl WidgetImpl for RangeSwitch {}
    impl BinImpl for RangeSwitch {}
}

glib::wrapper! {
    /// Chooses how far back every graph reaches.
    pub struct RangeSwitch(ObjectSubclass<imp::RangeSwitch>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}
