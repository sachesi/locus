//! Asks systemd about services over D-Bus and tells it to start and stop them. The calls
//! block, so everything here runs on a worker; nothing touches a widget.

use gtk::gio::{self, prelude::*};
use gtk::glib;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::Duration;

const SERVICE: &str = "org.freedesktop.systemd1";
const PATH: &str = "/org/freedesktop/systemd1";
const MANAGER: &str = "org.freedesktop.systemd1.Manager";
const UNIT: &str = "org.freedesktop.systemd1.Unit";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
/// How long a call that may wait for the user to authenticate is given.
const FOREVER: i32 = i32::MAX;

/// Which instance of systemd: the machine's, or the one running the user's session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    System,
    User,
}

impl Scope {
    pub fn connect(self) -> Result<gio::DBusConnection, glib::Error> {
        let bus = match self {
            Scope::System => gio::BusType::System,
            Scope::User => gio::BusType::Session,
        };
        gio::bus_get_sync(bus, gio::Cancellable::NONE)
    }
}

/// Whether the machine runs on systemd; it leaves this directory for programs to look for.
pub fn is_running() -> bool {
    Path::new("/run/systemd/system").is_dir()
}

/// What systemd says of a service.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServiceState {
    pub name: String,
    pub description: String,
    /// `active`, `failed`, `inactive`, `activating` or `deactivating`.
    pub active: String,
    /// What the service is doing within that, such as `running` or `exited`.
    pub sub: String,
    /// `enabled`, `disabled`, `static`, `masked` and so on; empty where it has no unit file.
    pub file_state: String,
}

fn call(
    connection: &gio::DBusConnection,
    interface: &str,
    path: &str,
    method: &str,
    parameters: Option<&glib::Variant>,
    timeout: i32,
    flags: gio::DBusCallFlags,
) -> Result<glib::Variant, glib::Error> {
    connection.call_sync(
        Some(SERVICE),
        path,
        interface,
        method,
        parameters,
        None,
        flags,
        timeout,
        gio::Cancellable::NONE,
    )
}

fn text(variant: &glib::Variant, index: usize) -> String {
    variant
        .child_value(index)
        .str()
        .unwrap_or_default()
        .to_owned()
}

/// The services loaded and those with a unit file, the name of each once.
pub fn list_services(scope: Scope) -> Result<Vec<ServiceState>, glib::Error> {
    let connection = scope.connect()?;
    let plain = gio::DBusCallFlags::NONE;
    let files = call(&connection, MANAGER, PATH, "ListUnitFiles", None, -1, plain)?.child_value(0);
    let mut file_states: HashMap<String, String> = HashMap::new();
    for file in files.iter() {
        let path = text(&file, 0);
        if let Some(name) = Path::new(&path).file_name().and_then(|name| name.to_str())
            && name.ends_with(".service")
        {
            file_states.insert(name.to_owned(), text(&file, 1));
        }
    }

    let units = call(&connection, MANAGER, PATH, "ListUnits", None, -1, plain)?.child_value(0);
    let mut services: HashMap<String, ServiceState> = HashMap::new();
    for unit in units.iter() {
        let name = text(&unit, 0);
        let (load, active) = (text(&unit, 2), text(&unit, 3));
        if !name.ends_with(".service") || (load == "not-found" && active == "inactive") {
            continue;
        }
        let file_state = file_states
            .get(&name)
            .or_else(|| file_states.get(&template_of(&name)))
            .cloned()
            .unwrap_or_default();
        services.insert(
            name.clone(),
            ServiceState {
                description: text(&unit, 1),
                sub: text(&unit, 4),
                name,
                active,
                file_state,
            },
        );
    }
    for (name, file_state) in file_states {
        // A template is not itself a service, only its instances are.
        if !name.ends_with("@.service") {
            services
                .entry(name.clone())
                .or_insert_with(|| ServiceState {
                    description: name.trim_end_matches(".service").to_owned(),
                    name,
                    active: "inactive".to_owned(),
                    sub: "dead".to_owned(),
                    file_state,
                });
        }
    }
    Ok(services.into_values().collect())
}

/// The template a unit is an instance of: `getty@.service` for `getty@tty1.service`.
fn template_of(name: &str) -> String {
    match name.split_once('@') {
        Some((template, _)) => format!("{template}@.service"),
        None => name.to_owned(),
    }
}

/// What a service used, by name, from its control group: bytes of memory now, and microseconds
/// on a processor so far. Memory and processor accounting are per group, so this reads no more
/// than two small files for each running service.
pub fn read_usage(scope: Scope) -> HashMap<String, (u64, u64)> {
    let mut usage = HashMap::new();
    add_usage(Path::new("/sys/fs/cgroup"), scope, false, 0, &mut usage);
    usage
}

fn add_usage(
    directory: &Path,
    scope: Scope,
    in_user_manager: bool,
    depth: usize,
    usage: &mut HashMap<String, (u64, u64)>,
) {
    if depth > 6 {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let path = entry.path();
        let is_manager = name.starts_with("user@") && name.ends_with(".service");
        if name.ends_with(".slice") {
            add_usage(&path, scope, in_user_manager, depth + 1, usage);
        } else if is_manager {
            add_usage(&path, scope, true, depth + 1, usage);
        }
        let ours = in_user_manager == (scope == Scope::User);
        if name.ends_with(".service") && ours {
            let memory = fs::read_to_string(path.join("memory.current"))
                .ok()
                .and_then(|text| text.trim().parse().ok())
                .unwrap_or(0);
            let cpu = fs::read_to_string(path.join("cpu.stat"))
                .ok()
                .as_deref()
                .and_then(parse_cpu_usage)
                .unwrap_or(0);
            usage.insert(name.to_owned(), (memory, cpu));
        }
    }
}

fn parse_cpu_usage(text: &str) -> Option<u64> {
    let line = text
        .lines()
        .find_map(|line| line.strip_prefix("usage_usec "))?;
    line.trim().parse().ok()
}

/// A change to make to a service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Start,
    Stop,
    Restart,
    Enable,
    Disable,
}

/// Does `action` to `name`. A change to the machine's services may ask the user to
/// authenticate, so this can take as long as they do.
pub fn perform(scope: Scope, name: &str, action: Action) -> Result<(), glib::Error> {
    let connection = scope.connect()?;
    let ask = gio::DBusCallFlags::ALLOW_INTERACTIVE_AUTHORIZATION;
    let job = |method: &str| {
        let parameters = (name, "replace").to_variant();
        call(
            &connection,
            MANAGER,
            PATH,
            method,
            Some(&parameters),
            FOREVER,
            ask,
        )
        .map(|_| ())
    };
    match action {
        Action::Start => job("StartUnit"),
        Action::Stop => job("StopUnit"),
        Action::Restart => job("RestartUnit"),
        Action::Enable => {
            let parameters = (vec![name], false, true).to_variant();
            call(
                &connection,
                MANAGER,
                PATH,
                "EnableUnitFiles",
                Some(&parameters),
                FOREVER,
                ask,
            )?;
            call(&connection, MANAGER, PATH, "Reload", None, FOREVER, ask).map(|_| ())
        }
        Action::Disable => {
            let parameters = (vec![name], false).to_variant();
            call(
                &connection,
                MANAGER,
                PATH,
                "DisableUnitFiles",
                Some(&parameters),
                FOREVER,
                ask,
            )?;
            call(&connection, MANAGER, PATH, "Reload", None, FOREVER, ask).map(|_| ())
        }
    }
}

/// The properties of an object of systemd's under one of its interfaces.
fn properties_of(
    connection: &gio::DBusConnection,
    path: &str,
    interface: &str,
) -> Result<HashMap<String, glib::Variant>, glib::Error> {
    let parameters = (interface,).to_variant();
    let plain = gio::DBusCallFlags::NONE;
    let all = call(
        connection,
        PROPERTIES,
        path,
        "GetAll",
        Some(&parameters),
        -1,
        plain,
    )?;
    let mut properties = HashMap::new();
    for entry in all.child_value(0).iter() {
        if let Some(value) = entry.child_value(1).as_variant() {
            properties.insert(text(&entry, 0), value);
        }
    }
    Ok(properties)
}

/// The properties of a unit, loading it if it has to be.
pub fn unit_properties(
    scope: Scope,
    name: &str,
) -> Result<HashMap<String, glib::Variant>, glib::Error> {
    let connection = scope.connect()?;
    let plain = gio::DBusCallFlags::NONE;
    let path = call(
        &connection,
        MANAGER,
        PATH,
        "LoadUnit",
        Some(&(name,).to_variant()),
        -1,
        plain,
    )?;
    properties_of(&connection, &text(&path, 0), UNIT)
}

fn microseconds(properties: &HashMap<String, glib::Variant>, key: &str) -> u64 {
    properties
        .get(key)
        .and_then(glib::Variant::get)
        .unwrap_or(0)
}

/// How long a unit took to come up, where it has come up.
fn start_time(properties: &HashMap<String, glib::Variant>) -> Option<Duration> {
    let began = microseconds(properties, "InactiveExitTimestampMonotonic");
    let ended = microseconds(properties, "ActiveEnterTimestampMonotonic");
    (began > 0 && ended > began).then(|| Duration::from_micros(ended - began))
}

/// How long an app started by the user's systemd took to start, where it was.
pub fn unit_start_time(scope: Scope, name: &str) -> Option<Duration> {
    start_time(&unit_properties(scope, name).ok()?)
}

/// The monotonic times systemd records of a boot, in microseconds. Those from before the
/// kernel count back from when it started.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct BootStamps {
    firmware: u64,
    loader: u64,
    userspace: u64,
    finish: u64,
}

/// How long the last boot took, each part where it was measured.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BootTimes {
    pub firmware: Duration,
    pub loader: Duration,
    pub kernel: Duration,
    pub system: Duration,
    pub session: Duration,
    /// When it happened, in microseconds since the epoch.
    pub at: u64,
}

impl BootTimes {
    pub fn total(&self) -> Duration {
        self.firmware + self.loader + self.kernel + self.system + self.session
    }
}

/// `None` while the machine is still starting. A container has no firmware or kernel of its
/// own, and its first stamp counts from when the machine it runs on started.
fn parts_of(stamps: BootStamps, in_container: bool) -> Option<BootTimes> {
    let micros = Duration::from_micros;
    if stamps.finish == 0 {
        return None;
    }
    let mut times = BootTimes {
        system: micros(stamps.finish.saturating_sub(stamps.userspace)),
        ..BootTimes::default()
    };
    if !in_container {
        times.firmware = micros(stamps.firmware.saturating_sub(stamps.loader));
        times.loader = micros(stamps.loader);
        times.kernel = micros(stamps.userspace);
    }
    Some(times)
}

pub fn read_boot_times() -> Result<Option<BootTimes>, glib::Error> {
    let connection = Scope::System.connect()?;
    let system = properties_of(&connection, PATH, MANAGER)?;
    let stamps = BootStamps {
        firmware: microseconds(&system, "FirmwareTimestampMonotonic"),
        loader: microseconds(&system, "LoaderTimestampMonotonic"),
        userspace: microseconds(&system, "UserspaceTimestampMonotonic"),
        finish: microseconds(&system, "FinishTimestampMonotonic"),
    };
    let Some(mut times) = parts_of(stamps, Path::new("/run/systemd/container").exists()) else {
        return Ok(None);
    };
    times.at = microseconds(&system, "UserspaceTimestamp");
    if let Ok(connection) = Scope::User.connect()
        && let Ok(user) = properties_of(&connection, PATH, MANAGER)
    {
        let began = microseconds(&user, "UserspaceTimestampMonotonic");
        let ended = microseconds(&user, "FinishTimestampMonotonic");
        times.session = Duration::from_micros(ended.saturating_sub(began));
    }
    Ok(Some(times))
}

/// The services of the machine that took the longest to come up during boot, the slowest first.
pub fn slowest_services(count: usize) -> Result<Vec<(String, Duration)>, glib::Error> {
    let connection = Scope::System.connect()?;
    let plain = gio::DBusCallFlags::NONE;
    let finish = microseconds(
        &properties_of(&connection, PATH, MANAGER)?,
        "FinishTimestampMonotonic",
    );
    let units = call(&connection, MANAGER, PATH, "ListUnits", None, -1, plain)?.child_value(0);
    let mut times = Vec::new();
    for unit in units.iter() {
        let name = text(&unit, 0);
        if !name.ends_with(".service") {
            continue;
        }
        let Ok(properties) = properties_of(&connection, &text(&unit, 6), UNIT) else {
            continue;
        };
        // One restarted since says nothing about the boot.
        if finish > 0 && microseconds(&properties, "ActiveEnterTimestampMonotonic") > finish {
            continue;
        }
        if let Some(time) = start_time(&properties) {
            times.push((name, time));
        }
    }
    times.sort_by_key(|(_, time)| std::cmp::Reverse(*time));
    times.truncate(count);
    Ok(times)
}

/// The last lines the journal has of a service.
pub fn read_log(scope: Scope, name: &str, lines: u32) -> Result<String, glib::Error> {
    let lines = lines.to_string();
    let unit = format!("--unit={name}");
    let user_unit = format!("--user-unit={name}");
    let which = match scope {
        Scope::System => unit.as_str(),
        Scope::User => user_unit.as_str(),
    };
    let arguments = [
        "journalctl",
        which,
        "--lines",
        &lines,
        "--no-pager",
        "--output=short",
        "--quiet",
    ];
    let arguments: Vec<&std::ffi::OsStr> = arguments.iter().map(std::ffi::OsStr::new).collect();
    let process = gio::Subprocess::newv(
        &arguments,
        gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_SILENCE,
    )?;
    let (output, _) = process.communicate_utf8(None, gio::Cancellable::NONE)?;
    Ok(output.map(String::from).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_of_instances() {
        assert_eq!(template_of("getty@tty1.service"), "getty@.service");
        assert_eq!(template_of("sshd.service"), "sshd.service");
    }

    #[test]
    fn processor_time_of_a_group() {
        assert_eq!(
            parse_cpu_usage("usage_usec 1234\nuser_usec 1000\nsystem_usec 234\n"),
            Some(1234)
        );
        assert_eq!(parse_cpu_usage("nr_periods 0\n"), None);
    }

    #[test]
    fn a_boot_splits_into_its_parts() {
        let stamps = BootStamps {
            firmware: 5_000_000,
            loader: 2_000_000,
            userspace: 1_500_000,
            finish: 7_500_000,
        };
        let times = parts_of(stamps, false).unwrap();
        assert_eq!(times.firmware, Duration::from_secs(3));
        assert_eq!(times.loader, Duration::from_secs(2));
        assert_eq!(times.kernel, Duration::from_millis(1500));
        assert_eq!(times.system, Duration::from_secs(6));
        assert_eq!(times.total(), Duration::from_millis(12500));
        let in_container = parts_of(stamps, true).unwrap();
        assert_eq!(in_container.total(), Duration::from_secs(6));
        assert_eq!(parts_of(BootStamps::default(), false), None);
    }

    #[test]
    fn listing_where_systemd_runs() {
        let Ok(services) = list_services(Scope::System) else {
            return;
        };
        assert!(
            services
                .iter()
                .all(|service| service.name.ends_with(".service"))
        );
        assert!(
            !services
                .iter()
                .any(|service| service.name.ends_with("@.service"))
        );
    }
}
