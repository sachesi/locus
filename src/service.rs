use crate::systemd::ServiceState;
use gettextrs::gettext;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

mod imp {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::Service)]
    pub struct Service {
        #[property(get, set)]
        name: RefCell<String>,
        #[property(get, set)]
        description: RefCell<String>,
        /// What it is doing, in words.
        #[property(get, set)]
        status: RefCell<String>,
        /// Sorts by how much a status wants attention, failed first.
        #[property(get, set)]
        rank: Cell<u32>,
        /// Whether it starts with the machine, in words.
        #[property(get, set)]
        startup: RefCell<String>,
        #[property(get, set)]
        file_state: RefCell<String>,
        #[property(get, set)]
        active: RefCell<String>,
        /// Share of the machine's processor time, from 0 to 100.
        #[property(get, set)]
        cpu: Cell<f64>,
        #[property(get, set)]
        memory: Cell<u64>,
        /// Time on a processor at the last reading, in microseconds.
        pub cpu_used: Cell<u64>,
        pub generation: Cell<u64>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Service {
        const NAME: &'static str = "LocusService";
        type Type = super::Service;
    }

    #[glib::derived_properties]
    impl ObjectImpl for Service {}
}

glib::wrapper! {
    /// A row of the list of services.
    pub struct Service(ObjectSubclass<imp::Service>);
}

impl Service {
    pub fn new(state: &ServiceState) -> Self {
        let service: Self = glib::Object::builder()
            .property("name", &state.name)
            .build();
        service.update(state);
        service
    }

    /// Takes what systemd says now. Only what changed is set, so a row redraws only for that.
    pub fn update(&self, state: &ServiceState) {
        if self.description() != state.description {
            self.set_description(state.description.as_str());
        }
        let (status, rank) = status_of(&state.active, &state.sub);
        if self.status() != status {
            self.set_status(status);
            self.set_rank(rank);
        }
        if self.active() != state.active {
            self.set_active(state.active.as_str());
        }
        if self.file_state() != state.file_state {
            self.set_file_state(state.file_state.as_str());
            self.set_startup(startup_of(&state.file_state));
        }
    }

    pub fn is_failed(&self) -> bool {
        self.active() == "failed"
    }
}

fn status_of(active: &str, sub: &str) -> (String, u32) {
    match (active, sub) {
        ("failed", _) => (gettext("Failed"), 0),
        ("active", "exited") => (gettext("Finished"), 2),
        ("active", _) => (gettext("Running"), 1),
        ("activating", _) => (gettext("Starting"), 1),
        ("deactivating", _) => (gettext("Stopping"), 1),
        _ => (gettext("Stopped"), 3),
    }
}

fn startup_of(file_state: &str) -> String {
    match file_state {
        "enabled" | "enabled-runtime" => gettext("Enabled"),
        "disabled" => gettext("Disabled"),
        "static" | "indirect" | "generated" | "transient" => gettext("Automatic"),
        "masked" | "masked-runtime" => gettext("Masked"),
        _ => String::new(),
    }
}
