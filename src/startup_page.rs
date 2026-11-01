use crate::autostart::{self, Entry};
use crate::format;
use crate::segment_bar::{SegmentBar, Tone};
use crate::stat::Stat;
use crate::systemd::{self, BootTimes, Scope};
use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::{gio, glib};
use std::cell::Cell;
use std::time::{Duration, Instant};

/// How long what is shown stands before it is read again on coming back to the page.
const REFRESHED: Duration = Duration::from_secs(30);
const SLOWEST_SHOWN: usize = 5;

/// What the page shows, read together on a worker.
struct Reading {
    entries: Vec<(Entry, Option<Duration>)>,
    boot: Option<BootTimes>,
    slowest: Vec<(String, Duration)>,
}

fn read(with_systemd: bool) -> Reading {
    let entries = autostart::list()
        .into_iter()
        .map(|entry| {
            let time = with_systemd
                .then(|| systemd::unit_start_time(Scope::User, &autostart::unit_of(&entry.id)))
                .flatten();
            (entry, time)
        })
        .collect();
    Reading {
        entries,
        boot: with_systemd
            .then(|| systemd::read_boot_times().ok().flatten())
            .flatten(),
        slowest: if with_systemd {
            systemd::slowest_services(SLOWEST_SHOWN).unwrap_or_default()
        } else {
            Vec::new()
        },
    }
}

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/sachesi/locus/ui/startup_page.ui")]
    pub struct StartupPage {
        #[template_child]
        pub apps: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub boot: TemplateChild<gtk::Box>,
        #[template_child]
        pub boot_total: TemplateChild<gtk::Label>,
        #[template_child]
        pub boot_date: TemplateChild<gtk::Label>,
        #[template_child]
        pub boot_bar: TemplateChild<SegmentBar>,
        #[template_child]
        pub firmware: TemplateChild<Stat>,
        #[template_child]
        pub loader: TemplateChild<Stat>,
        #[template_child]
        pub kernel: TemplateChild<Stat>,
        #[template_child]
        pub system: TemplateChild<Stat>,
        #[template_child]
        pub session: TemplateChild<Stat>,
        #[template_child]
        pub slowest_group: TemplateChild<gtk::Box>,
        #[template_child]
        pub slowest: TemplateChild<gtk::ListBox>,

        pub read_at: Cell<Option<Instant>>,
        pub reading: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for StartupPage {
        const NAME: &'static str = "LocusStartupPage";
        type Type = super::StartupPage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            SegmentBar::ensure_type();
            Stat::ensure_type();
            klass.bind_template();
            klass.bind_template_callbacks();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for StartupPage {}
    impl WidgetImpl for StartupPage {}
    impl BinImpl for StartupPage {}

    #[gtk::template_callbacks]
    impl StartupPage {
        #[template_callback]
        fn on_add_app(&self) {
            self.obj().choose_app();
        }
    }
}

glib::wrapper! {
    pub struct StartupPage(ObjectSubclass<imp::StartupPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl StartupPage {
    /// Reads what the page shows, unless that was only a moment ago.
    pub fn refresh(&self) {
        if self
            .imp()
            .read_at
            .get()
            .is_none_or(|at| at.elapsed() >= REFRESHED)
        {
            self.read_now();
        }
    }

    fn read_now(&self) {
        let imp = self.imp();
        if imp.reading.replace(true) {
            return;
        }
        imp.read_at.set(Some(Instant::now()));
        let with_systemd = systemd::is_running();
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = page)]
            self,
            async move {
                let reading = gio::spawn_blocking(move || read(with_systemd)).await;
                page.imp().reading.set(false);
                if let Ok(reading) = reading {
                    page.apply(&reading);
                }
            }
        ));
    }

    fn apply(&self, reading: &Reading) {
        let imp = self.imp();
        imp.apps.remove_all();
        for (entry, time) in &reading.entries {
            imp.apps.append(&self.app_row(entry, *time));
        }
        imp.boot.set_visible(reading.boot.is_some());
        if let Some(boot) = &reading.boot {
            self.show_boot(boot);
        }
        imp.slowest.remove_all();
        imp.slowest_group.set_visible(!reading.slowest.is_empty());
        for (name, time) in &reading.slowest {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(name))
                .build();
            let time = gtk::Label::new(Some(&format::seconds(time.as_secs_f64())));
            time.add_css_class("dim-label");
            row.add_suffix(&time);
            imp.slowest.append(&row);
        }
    }

    fn show_boot(&self, boot: &BootTimes) {
        let imp = self.imp();
        imp.boot_total
            .set_label(&format::seconds(boot.total().as_secs_f64()));
        let when = i64::try_from(boot.at)
            .ok()
            .filter(|at| *at > 0)
            .and_then(|at| glib::DateTime::from_unix_local(at / 1_000_000).ok())
            .and_then(|at| at.format("%c").ok());
        imp.boot_date.set_label(when.as_deref().unwrap_or_default());
        let parts = [
            (&imp.firmware, boot.firmware, Tone::Neutral),
            (&imp.loader, boot.loader, Tone::SoftNeutral),
            (&imp.kernel, boot.kernel, Tone::SoftAccent),
            (&imp.system, boot.system, Tone::Accent),
            (&imp.session, boot.session, Tone::SoftAccent),
        ];
        let mut segments = Vec::new();
        for (stat, time, tone) in parts {
            let seconds = time.as_secs_f64();
            stat.show_value((seconds > 0.0).then(|| format::seconds(seconds)));
            if seconds > 0.0 {
                segments.push((seconds, tone));
            }
        }
        imp.boot_bar.set_segments(segments);
    }

    fn app_row(&self, entry: &Entry, time: Option<Duration>) -> adw::ActionRow {
        let place = if entry.in_user_folder && !entry.in_system_folder {
            gettext("Your Autostart Folder")
        } else {
            gettext("System")
        };
        let measured = time.map_or_else(
            || gettext("Not Measured"),
            |time| format::seconds(time.as_secs_f64()),
        );
        let row = adw::ActionRow::builder()
            .title(glib::markup_escape_text(&entry.name))
            .subtitle(format::listed(&place, &measured))
            .build();
        let icon = gtk::Image::builder()
            .icon_name(entry.icon.as_deref().unwrap_or("application-x-executable"))
            .pixel_size(32)
            .css_classes(["lowres-icon"])
            .build();
        row.add_prefix(&icon);
        if entry.in_user_folder && !entry.in_system_folder {
            let remove = gtk::Button::builder()
                .icon_name("user-trash-symbolic")
                .tooltip_text(gettext("Remove From Startup"))
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .build();
            let shown = entry.clone();
            remove.connect_clicked(glib::clone!(
                #[weak(rename_to = page)]
                self,
                move |_| page.confirm_remove(&shown)
            ));
            row.add_suffix(&remove);
        }
        let switch = gtk::Switch::builder()
            .active(entry.enabled)
            .valign(gtk::Align::Center)
            .build();
        switch.update_property(&[gtk::accessible::Property::Label(
            &gettext("Start “{name}” at Login").replace("{name}", &entry.name),
        )]);
        let shown = entry.clone();
        switch.connect_state_set(glib::clone!(
            #[weak(rename_to = page)]
            self,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, enabled| {
                page.set_enabled(&shown, enabled);
                glib::Propagation::Proceed
            }
        ));
        row.add_suffix(&switch);
        row.set_activatable_widget(Some(&switch));
        row
    }

    fn set_enabled(&self, entry: &Entry, enabled: bool) {
        if let Err(error) = autostart::set_enabled(entry, enabled) {
            glib::g_warning!(
                "locus",
                "could not change autostart entry {}: {error}",
                entry.id
            );
            self.toast(
                &gettext("Could not change when “{name}” starts").replace("{name}", &entry.name),
            );
            // Only then: rows rebuilt under a working switch would take the keyboard focus away.
            self.read_now();
        }
    }

    fn confirm_remove(&self, entry: &Entry) {
        let dialog = adw::AlertDialog::new(
            Some(&gettext("Remove “{name}” From Startup?").replace("{name}", &entry.name)),
            Some(&gettext(
                "It will no longer start with your session. The app itself stays installed.",
            )),
        );
        dialog.add_responses(&[
            ("cancel", &gettext("_Cancel")),
            ("remove", &gettext("_Remove")),
        ]);
        dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        let entry = entry.clone();
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = page)]
            self,
            async move {
                if dialog.choose_future(Some(&page)).await != "remove" {
                    return;
                }
                if let Err(error) = autostart::remove(&entry) {
                    glib::g_warning!(
                        "locus",
                        "could not remove autostart entry {}: {error}",
                        entry.id
                    );
                    page.toast(
                        &gettext("Could not remove “{name}”").replace("{name}", &entry.name),
                    );
                }
                page.read_now();
            }
        ));
    }

    /// Offers the installed apps to start with the session, searched by name.
    fn choose_app(&self) {
        let mut apps: Vec<gio::AppInfo> = gio::AppInfo::all()
            .into_iter()
            .filter(AppInfoExt::should_show)
            .collect();
        apps.sort_by_key(|app| app.display_name().to_lowercase());

        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .margin_top(6)
            .margin_bottom(18)
            .margin_start(12)
            .margin_end(12)
            .valign(gtk::Align::Start)
            .build();
        let dialog = adw::Dialog::builder()
            .title(gettext("Add App"))
            .content_width(420)
            .content_height(560)
            .build();
        for app in apps {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&app.display_name()))
                .activatable(true)
                .build();
            if let Some(icon) = app.icon() {
                let image = gtk::Image::from_gicon(&icon);
                image.set_pixel_size(32);
                image.add_css_class("lowres-icon");
                row.add_prefix(&image);
            }
            row.connect_activated(glib::clone!(
                #[weak(rename_to = page)]
                self,
                #[weak]
                dialog,
                move |_| {
                    if let Err(error) = autostart::add(app.id().as_deref().unwrap_or_default()) {
                        glib::g_warning!(
                            "locus",
                            "could not add {} to autostart: {error}",
                            app.display_name()
                        );
                        page.toast(
                            &gettext("Could not add “{name}”")
                                .replace("{name}", &app.display_name()),
                        );
                    }
                    dialog.close();
                    page.read_now();
                }
            ));
            list.append(&row);
        }

        let search = gtk::SearchEntry::builder()
            .placeholder_text(gettext("Search apps"))
            .margin_start(12)
            .margin_end(12)
            .margin_bottom(6)
            .build();
        list.set_filter_func(glib::clone!(
            #[weak]
            search,
            #[upgrade_or]
            true,
            move |row| {
                let query = search.text().to_lowercase();
                row.downcast_ref::<adw::ActionRow>()
                    .is_none_or(|row| row.title().to_lowercase().contains(&query))
            }
        ));
        search.connect_search_changed(glib::clone!(
            #[weak]
            list,
            move |_| list.invalidate_filter()
        ));

        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&list)
            .vexpand(true)
            .build();
        let view = adw::ToolbarView::new();
        view.add_top_bar(&adw::HeaderBar::new());
        view.add_top_bar(&search);
        view.set_content(Some(&scrolled));
        dialog.set_child(Some(&view));
        dialog.set_focus(Some(&search));
        dialog.present(Some(self));
    }

    fn toast(&self, text: &str) {
        if let Some(overlay) = self
            .ancestor(adw::ToastOverlay::static_type())
            .and_downcast::<adw::ToastOverlay>()
        {
            overlay.add_toast(adw::Toast::new(text));
        }
    }
}
