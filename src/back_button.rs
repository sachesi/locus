use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::glib;
use gtk::prelude::*;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct BackButton {}

    #[glib::object_subclass]
    impl ObjectSubclass for BackButton {
        const NAME: &'static str = "LocusBackButton";
        type Type = super::BackButton;
        type ParentType = gtk::Button;
    }

    impl ObjectImpl for BackButton {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_icon_name("go-previous-symbolic");
            obj.set_tooltip_text(Some(&gettext("Back")));
            obj.set_action_name(Some("win.show-sidebar"));
        }
    }

    impl WidgetImpl for BackButton {
        fn root(&self) {
            self.parent_root();
            let obj = self.obj();
            if let Some(window) = obj.root() {
                window
                    .bind_property("collapsed", &*obj, "visible")
                    .sync_create()
                    .build();
            }
        }
    }

    impl ButtonImpl for BackButton {}
}

glib::wrapper! {
    /// Goes back to the sidebar, where the window is too narrow to show it beside a page.
    pub struct BackButton(ObjectSubclass<imp::BackButton>)
        @extends gtk::Button, gtk::Widget,
        @implements gtk::Accessible, gtk::Actionable, gtk::Buildable, gtk::ConstraintTarget;
}
