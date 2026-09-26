use crate::config;
use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::{gio, glib};

/// The update intervals offered, in seconds.
const INTERVALS: [u32; 4] = [1, 2, 5, 10];

mod imp {
    use super::*;

    #[derive(gtk::CompositeTemplate)]
    #[template(resource = "/io/github/sachesi/locus/ui/preferences_dialog.ui")]
    pub struct PreferencesDialog {
        #[template_child]
        pub resources_interval: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub filesystems_interval: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub processes_interval: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub binary_units: TemplateChild<adw::SwitchRow>,

        pub settings: gio::Settings,
    }

    impl Default for PreferencesDialog {
        fn default() -> Self {
            Self {
                resources_interval: TemplateChild::default(),
                filesystems_interval: TemplateChild::default(),
                processes_interval: TemplateChild::default(),
                binary_units: TemplateChild::default(),
                settings: gio::Settings::new(config::APP_ID),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PreferencesDialog {
        const NAME: &'static str = "LocusPreferencesDialog";
        type Type = super::PreferencesDialog;
        type ParentType = adw::PreferencesDialog;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for PreferencesDialog {
        fn constructed(&self) {
            self.parent_constructed();
            let labels = [
                gettext("Every Second"),
                gettext("Every 2 Seconds"),
                gettext("Every 5 Seconds"),
                gettext("Every 10 Seconds"),
            ];
            let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
            for (row, key) in [
                (&*self.resources_interval, "resources-update-interval"),
                (&*self.filesystems_interval, "filesystems-update-interval"),
                (&*self.processes_interval, "processes-update-interval"),
            ] {
                row.set_model(Some(&gtk::StringList::new(&labels)));
                self.settings
                    .bind(key, row, "selected")
                    .mapping(|variant, _| {
                        // An interval set outside the dialog shows as the nearest offered.
                        let seconds = variant.get::<u32>()?;
                        let nearest =
                            (0..INTERVALS.len()).min_by_key(|&i| INTERVALS[i].abs_diff(seconds))?;
                        Some(u32::try_from(nearest).ok()?.to_value())
                    })
                    .set_mapping(|value, _| {
                        let index = usize::try_from(value.get::<u32>().ok()?).ok()?;
                        Some(INTERVALS.get(index)?.to_variant())
                    })
                    .build();
            }
            self.settings
                .bind("binary-units", &*self.binary_units, "active")
                .build();
        }
    }

    impl WidgetImpl for PreferencesDialog {}
    impl AdwDialogImpl for PreferencesDialog {}
    impl PreferencesDialogImpl for PreferencesDialog {}
}

glib::wrapper! {
    pub struct PreferencesDialog(ObjectSubclass<imp::PreferencesDialog>)
        @extends adw::PreferencesDialog, adw::Dialog, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for PreferencesDialog {
    fn default() -> Self {
        glib::Object::new()
    }
}
