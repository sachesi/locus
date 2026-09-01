mod config;
mod format;
mod graph;
mod process;
mod processes_page;
mod procfs;
mod resources_page;
mod sampler;
mod window;

use adw::prelude::*;
use gettextrs::gettext;
use gtk::{gio, glib};
use window::LocusWindow;

fn main() -> glib::ExitCode {
    // SAFETY: the first thing the program does; no thread has been started.
    unsafe { gettextrs::setlocale(gettextrs::LocaleCategory::LcAll, "") };
    // Without the catalogues the interface stays in English, which is no reason to stop.
    gettextrs::bindtextdomain(config::GETTEXT_PACKAGE, config::LOCALEDIR).ok();
    gettextrs::bind_textdomain_codeset(config::GETTEXT_PACKAGE, "UTF-8").ok();
    gettextrs::textdomain(config::GETTEXT_PACKAGE).ok();

    gio::resources_register_include!("locus.gresource").expect("register resources");
    glib::set_application_name("Locus");

    // The resource base path has AdwApplication load style.css, the bundled icons and the
    // shortcuts dialog from the bundle.
    let app = adw::Application::builder()
        .application_id(config::APP_ID)
        .resource_base_path(config::RESOURCE_PATH)
        .build();
    app.connect_startup(setup_actions);
    // One window per instance: launching again brings it back to the front.
    app.connect_activate(|app| {
        let window = app
            .active_window()
            .unwrap_or_else(|| LocusWindow::new(app).upcast());
        window.present();
    });
    app.run()
}

fn setup_actions(app: &adw::Application) {
    let quit = gio::ActionEntry::builder("quit")
        .activate(|app: &adw::Application, _, _| app.quit())
        .build();
    let about = gio::ActionEntry::builder("about")
        .activate(|app: &adw::Application, _, _| show_about(app))
        .build();
    app.add_action_entries([quit, about]);
    app.set_accels_for_action("app.quit", &["<Control>q"]);
    app.set_accels_for_action("window.close", &["<Control>w"]);
    app.set_accels_for_action("win.search", &["<Control>f"]);
    app.set_accels_for_action("win.page::resources", &["<Alt>1"]);
    app.set_accels_for_action("win.page::processes", &["<Alt>2"]);
}

fn show_about(app: &adw::Application) {
    adw::AboutDialog::builder()
        .application_name("Locus")
        .application_icon(config::APP_ID)
        .version(env!("CARGO_PKG_VERSION"))
        .developer_name("sachesi")
        .license_type(gtk::License::Gpl30)
        .comments(gettext(
            "See what the processor, the memory, the network and the disks are busy with, and which processes keep them busy.",
        ))
        .translator_credits(gettext("translator-credits"))
        .website("https://github.com/sachesi/locus")
        .issue_url("https://github.com/sachesi/locus/issues")
        .build()
        .present(app.active_window().as_ref());
}
