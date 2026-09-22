//! Reads the state of the machine from /proc and /sys. Runs on a worker thread: nothing in
//! here touches GTK, and a [`Sampler`] moves to the worker and back for each reading.

use crate::gpu::{self, Gpu};
use crate::procfs::{self, CpuTimes, Memory};
use gtk::gio::{self, prelude::*};
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString};
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, Read};
use std::mem::MaybeUninit;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long the list of installed apps is kept before a process started from one it does
/// not have makes it read again.
const APPS_KEPT: Duration = Duration::from_secs(10);

/// How long a first reading waits for its second, so that the first rates shown are
/// measured rather than zero.
const BASELINE: Duration = Duration::from_millis(250);

#[derive(Debug, Clone)]
pub struct ProcessSample {
    pub pid: u32,
    pub ppid: u32,
    pub start_time: u64,
    /// The installed app the process is part of.
    pub app: Option<Arc<App>>,
    pub name: Arc<str>,
    pub command: Arc<str>,
    pub uid: u32,
    pub user: Arc<str>,
    /// Share of the whole machine's processor time, from 0 to 100.
    pub cpu: f64,
    /// Resident memory not shared with other processes, in bytes.
    pub memory: u64,
    /// Bytes per second read from and written to storage; `None` where the process does
    /// not let this user see it.
    pub disk: Option<u64>,
    /// Share of the time the busiest engine of a graphics card spent on the process's
    /// work, from 0 to 100; `None` where the process does not let this user see it.
    pub gpu: Option<f64>,
    pub stopped: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSystem {
    pub mount_point: String,
    pub source: String,
    pub fs_type: String,
    pub size: u64,
    pub used: u64,
    /// What an unprivileged user can still write, less than `size - used` where the file
    /// system keeps blocks back for root.
    pub available: u64,
}

#[derive(Debug, Clone, Default)]
pub struct System {
    pub cpu_model: Option<Arc<str>>,
    /// Processor usage of the whole machine, from 0 to 100.
    pub cpu: f64,
    /// Usage of each online processor, from 0 to 100.
    pub cores: Vec<f64>,
    pub memory: Memory,
    /// Bytes per second over the network interfaces and to and from the disks.
    pub received: f64,
    pub sent: f64,
    pub read: f64,
    pub written: f64,
    pub filesystems: Vec<FileSystem>,
    pub gpus: Vec<Gpu>,
}

/// An app a launcher shows, as its desktop file describes it.
#[derive(Debug)]
pub struct App {
    /// The desktop file's name.
    pub id: Arc<str>,
    pub name: Arc<str>,
    /// Its icon, in the form `gio::Icon::for_string` takes.
    pub icon: Option<Arc<str>>,
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub system: System,
    /// Every process but kernel threads, when they were asked for.
    pub processes: Option<Vec<ProcessSample>>,
}

/// What is remembered of a process between readings.
struct Tracked {
    start_time: u64,
    ticks: u64,
    io: Option<u64>,
    io_denied: bool,
    /// Nanoseconds each kind of engine spent on the process's work, by card and engine,
    /// with how many engines of the kind there are.
    engines: HashMap<String, (u64, u64)>,
    fds_denied: bool,
    name: Arc<str>,
    command: Arc<str>,
    uid: u32,
    user: Arc<str>,
    app: Option<Arc<App>>,
    /// Whether the app has been looked for again since the process was first seen.
    app_settled: bool,
    generation: u64,
}

/// The counters of the previous reading, which the next one's rates are measured against.
pub struct Sampler {
    ticks_per_second: f64,
    page_size: u64,
    cpu_model: Option<Arc<str>>,
    gpus: Vec<gpu::Card>,
    /// Reused for every file read, so a reading allocates only for what it keeps.
    text: String,
    stat: String,
    bytes: Vec<u8>,
    path: String,
    at: Option<Instant>,
    cpu: Vec<CpuTimes>,
    /// Bytes received and sent by each network interface counted, and read and written by
    /// each disk, at the last reading.
    links: HashMap<String, (u64, u64)>,
    disks: HashMap<String, (u64, u64)>,
    /// Whether an interface or a block device is hardware, by name.
    physical_links: HashMap<String, bool>,
    physical_disks: HashMap<String, bool>,
    processes_at: Option<Instant>,
    processes: HashMap<u32, Tracked>,
    generation: u64,
    users: HashMap<u32, Arc<str>>,
    apps: AppIndex,
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Sampler {
    pub fn new() -> Self {
        // SAFETY: sysconf only reads a configuration value.
        let (ticks, page) = unsafe {
            (
                libc::sysconf(libc::_SC_CLK_TCK),
                libc::sysconf(libc::_SC_PAGESIZE),
            )
        };
        let cpu_model = fs::read_to_string("/proc/cpuinfo")
            .ok()
            .and_then(|text| procfs::parse_cpu_model(&text))
            .map(Arc::from);
        Self {
            ticks_per_second: if ticks > 0 { ticks as f64 } else { 100.0 },
            page_size: u64::try_from(page).unwrap_or(4096),
            cpu_model,
            gpus: gpu::cards(),
            text: String::new(),
            stat: String::new(),
            bytes: Vec::new(),
            path: String::new(),
            at: None,
            cpu: Vec::new(),
            links: HashMap::new(),
            disks: HashMap::new(),
            physical_links: HashMap::new(),
            physical_disks: HashMap::new(),
            processes_at: None,
            processes: HashMap::new(),
            generation: 0,
            users: HashMap::new(),
            apps: AppIndex::default(),
        }
    }

    /// Reads the machine, and its processes when `with_processes` is set. Blocks for a
    /// moment when there is no earlier reading to measure rates against.
    pub fn sample(&mut self, with_processes: bool) -> Snapshot {
        if self.at.is_none() || (with_processes && self.processes_at.is_none()) {
            self.take(with_processes);
            std::thread::sleep(BASELINE);
        }
        self.take(with_processes)
    }

    fn take(&mut self, with_processes: bool) -> Snapshot {
        let now = Instant::now();
        let seconds = self.at.map(|at| now.duration_since(at).as_secs_f64());
        self.at = Some(now);
        let system = self.read_system(seconds.filter(|s| *s > 0.0));
        let processes = if with_processes {
            Some(self.read_processes(now))
        } else {
            // The counters kept would be stale by the time processes are asked for again.
            self.processes_at = None;
            None
        };
        Snapshot { system, processes }
    }

    fn read_system(&mut self, seconds: Option<f64>) -> System {
        let mut system = System {
            cpu_model: self.cpu_model.clone(),
            ..System::default()
        };
        let per_second = |bytes: u64| seconds.map_or(0.0, |s| bytes as f64 / s);

        if read_text("/proc/stat", &mut self.text).is_ok() {
            let times = procfs::parse_cpu_times(&self.text);
            let mut usage = times.iter().enumerate().map(|(i, now)| {
                self.cpu
                    .get(i)
                    .map_or(0.0, |before| now.usage_since(*before))
            });
            system.cpu = usage.next().unwrap_or(0.0);
            system.cores = usage.collect();
            self.cpu = times;
        }

        if read_text("/proc/meminfo", &mut self.text).is_ok() {
            system.memory = procfs::parse_meminfo(&self.text);
        }

        if read_text("/proc/net/dev", &mut self.text).is_ok() {
            let links: Vec<_> = procfs::parse_net_dev(&self.text)
                .into_iter()
                .filter(|(name, _, _)| *name != "lo")
                .map(|(name, received, sent)| {
                    let physical = is_physical(&mut self.physical_links, "/sys/class/net", name);
                    (name, received, sent, physical)
                })
                .collect();
            // Bridges, tunnels and container links carry traffic the hardware carries too,
            // so they count only where there is no hardware, as in a container.
            let hardware = links.iter().any(|link| link.3);
            let counters = links
                .into_iter()
                .filter(|link| link.3 == hardware)
                .map(|(name, received, sent, _)| (name.to_owned(), (received, sent)))
                .collect();
            let (received, sent) = moved(&counters, &self.links);
            system.received = per_second(received);
            system.sent = per_second(sent);
            self.links = counters;
        }

        if read_text("/proc/diskstats", &mut self.text).is_ok() {
            // Partitions, device mapper and RAID devices repeat what their disks do.
            let counters = procfs::parse_diskstats(&self.text)
                .into_iter()
                .filter(|(name, _, _)| is_physical(&mut self.physical_disks, "/sys/block", name))
                .map(|(name, read, written)| (name.to_owned(), (read * 512, written * 512)))
                .collect();
            let (read, written) = moved(&counters, &self.disks);
            system.read = per_second(read);
            system.written = per_second(written);
            self.disks = counters;
        }

        if read_text("/proc/self/mountinfo", &mut self.text).is_ok() {
            system.filesystems = procfs::parse_mountinfo(&self.text)
                .into_iter()
                .filter_map(|mount| {
                    let (size, used, available) = usage(&mount.mount_point)?;
                    (size > 0).then_some(FileSystem {
                        mount_point: mount.mount_point,
                        source: mount.source,
                        fs_type: mount.fs_type,
                        size,
                        used,
                        available,
                    })
                })
                .collect();
        }

        system.gpus = self.gpus.iter().map(gpu::Card::read).collect();
        system
    }

    fn read_processes(&mut self, now: Instant) -> Vec<ProcessSample> {
        let seconds = self
            .processes_at
            .map(|at| now.duration_since(at).as_secs_f64())
            .filter(|s| *s > 0.0);
        self.processes_at = Some(now);
        self.generation += 1;
        let cpus = self.cpu.len().saturating_sub(1).max(1) as f64;

        let Ok(entries) = fs::read_dir("/proc") else {
            return Vec::new();
        };
        let mut samples = Vec::with_capacity(self.processes.len());
        let mut clients = HashSet::new();
        for entry in entries.flatten() {
            let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse().ok()) else {
                continue;
            };
            // Processes come and go between listing the directory and reading their files;
            // one that has gone is skipped.
            if read_text(proc_path(&mut self.path, pid, "stat"), &mut self.stat).is_err() {
                continue;
            }
            let Some(stat) = procfs::parse_stat(&self.stat) else {
                continue;
            };
            if stat.kernel_thread {
                continue;
            }

            let known = self
                .processes
                .get(&pid)
                .is_some_and(|t| t.start_time == stat.start_time);
            if !known {
                let Some(tracked) = track(
                    pid,
                    &stat,
                    &mut self.path,
                    &mut self.text,
                    &mut self.bytes,
                    &mut self.users,
                ) else {
                    continue;
                };
                self.processes.insert(pid, tracked);
            }
            let Some(tracked) = self.processes.get_mut(&pid) else {
                continue;
            };
            tracked.generation = self.generation;
            if !tracked.app_settled {
                // A desktop moves an app it starts into a unit of its own once it has
                // started it, which the first look may come before.
                tracked.app_settled = known;
                let program = tracked.name.clone();
                tracked.app = app_of(
                    pid,
                    &program,
                    &mut self.path,
                    &mut self.text,
                    &mut self.bytes,
                    &mut self.apps,
                );
            }

            let cpu = match seconds {
                Some(s) if known => {
                    let ticks = stat.ticks.saturating_sub(tracked.ticks) as f64;
                    (ticks / self.ticks_per_second / s / cpus * 100.0).min(100.0)
                }
                _ => 0.0,
            };
            tracked.ticks = stat.ticks;

            let memory = read_text(proc_path(&mut self.path, pid, "statm"), &mut self.text)
                .ok()
                .and_then(|()| procfs::parse_statm(&self.text))
                .map_or(0, |(resident, shared)| {
                    resident.saturating_sub(shared) * self.page_size
                });

            let disk = if tracked.io_denied {
                None
            } else {
                match read_text(proc_path(&mut self.path, pid, "io"), &mut self.text) {
                    Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                        tracked.io_denied = true;
                        None
                    }
                    Err(_) => None,
                    Ok(()) => procfs::parse_io(&self.text).map(|(read, written)| {
                        let total = read + written;
                        let before = tracked.io.replace(total);
                        match (before, seconds) {
                            (Some(before), Some(s)) => {
                                (total.saturating_sub(before) as f64 / s) as u64
                            }
                            _ => 0,
                        }
                    }),
                }
            };

            let gpu = if tracked.fds_denied {
                None
            } else {
                match gpu_engines(pid, &mut self.path, &mut self.text, &mut clients) {
                    Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                        tracked.fds_denied = true;
                        None
                    }
                    Err(_) => Some(0.0),
                    Ok(engines) => {
                        let busiest = match seconds {
                            Some(s) if known => engines
                                .iter()
                                .map(|(key, &(ns, capacity))| {
                                    let before = tracked.engines.get(key).map_or(ns, |b| b.0);
                                    ns.saturating_sub(before) as f64 / (s * 1e7 * capacity as f64)
                                })
                                .fold(0.0, f64::max)
                                .min(100.0),
                            _ => 0.0,
                        };
                        tracked.engines = engines;
                        Some(busiest)
                    }
                }
            };

            samples.push(ProcessSample {
                pid,
                ppid: stat.ppid,
                start_time: stat.start_time,
                app: tracked.app.clone(),
                name: tracked.name.clone(),
                command: tracked.command.clone(),
                uid: tracked.uid,
                user: tracked.user.clone(),
                cpu,
                memory,
                disk,
                gpu,
                stopped: matches!(stat.state, 'T' | 't'),
            });
        }

        let generation = self.generation;
        self.processes.retain(|_, t| t.generation == generation);
        samples
    }
}

/// What stays the same for the life of a process, read when it is first seen.
fn track(
    pid: u32,
    stat: &procfs::Stat<'_>,
    path: &mut String,
    text: &mut String,
    bytes: &mut Vec<u8>,
    users: &mut HashMap<u32, Arc<str>>,
) -> Option<Tracked> {
    read_text(proc_path(path, pid, "status"), text).ok()?;
    let uid = procfs::parse_status_uid(text)?;
    bytes.clear();
    if let Ok(mut file) = File::open(proc_path(path, pid, "cmdline")) {
        file.read_to_end(bytes).ok();
    }
    let user = users
        .entry(uid)
        .or_insert_with(|| user_name(uid).unwrap_or_else(|| uid.to_string()).into())
        .clone();
    Some(Tracked {
        start_time: stat.start_time,
        ticks: stat.ticks,
        io: None,
        io_denied: false,
        engines: HashMap::new(),
        fds_denied: false,
        name: procfs::display_name(stat.comm, bytes).into(),
        command: procfs::command_line(bytes).into(),
        uid,
        user,
        app: None,
        app_settled: false,
        generation: 0,
    })
}

/// The time each kind of engine of each card has spent on the work of the process, by card
/// and engine. A client of a card that an earlier process shares, having started this one
/// or been started by it, is left to that process, which then has all of its time.
fn gpu_engines(
    pid: u32,
    path: &mut String,
    text: &mut String,
    clients: &mut HashSet<(String, u64)>,
) -> io::Result<HashMap<String, (u64, u64)>> {
    let mut engines: HashMap<String, (u64, u64)> = HashMap::new();
    for entry in fs::read_dir(proc_path(path, pid, "fd"))?.flatten() {
        let Ok(target) = fs::read_link(entry.path()) else {
            continue;
        };
        if !target.as_os_str().as_bytes().starts_with(b"/dev/dri/") {
            continue;
        }
        let fd = entry.file_name();
        path.clear();
        let _ = write!(path, "/proc/{pid}/fdinfo/{}", fd.to_string_lossy());
        if read_text(&*path, text).is_err() {
            continue;
        }
        let Some(client) = procfs::parse_drm_fdinfo(text) else {
            continue;
        };
        if !clients.insert((client.pdev.to_owned(), client.id)) {
            continue;
        }
        for (engine, ns, capacity) in client.engines {
            let total = engines
                .entry(format!("{} {engine}", client.pdev))
                .or_insert((0, capacity));
            total.0 += ns;
        }
    }
    Ok(engines)
}

/// The app a process is part of: the one its unit is named after, else the one GLib
/// started it, or what it descends from, from.
fn app_of(
    pid: u32,
    program: &str,
    path: &mut String,
    text: &mut String,
    bytes: &mut Vec<u8>,
    apps: &mut AppIndex,
) -> Option<Arc<App>> {
    if read_text(proc_path(path, pid, "cgroup"), text).is_ok()
        && let Some(app) = procfs::cgroup_app_ids(text)
            .iter()
            .find_map(|id| apps.find(id, false))
    {
        return Some(app);
    }
    // Only its owner, and root, may read the environment of a process.
    bytes.clear();
    File::open(proc_path(path, pid, "environ"))
        .ok()?
        .read_to_end(bytes)
        .ok()?;
    if let Some(file) = procfs::launched_desktop_file(bytes) {
        let id = Path::new(file).file_stem()?.to_str()?;
        return apps.find(id, true);
    }
    // An app D-Bus starts itself, rather than through systemd, has neither, and is known
    // by its program.
    if procfs::started_by_dbus(bytes) {
        return apps.find_program(program);
    }
    None
}

/// The installed apps by desktop file name, `None` for those a launcher does not show.
#[derive(Default)]
struct AppIndex {
    apps: HashMap<String, Option<Arc<App>>>,
    /// The apps by the name of the program they run, where that program is theirs alone.
    programs: HashMap<String, Arc<App>>,
    read_at: Option<Instant>,
}

impl AppIndex {
    /// The app of the desktop file `id` names, without ".desktop". A process `launched`
    /// from a desktop file the index does not have may be from an app installed since it
    /// was read, so it is read again, though not more often than every [`APPS_KEPT`].
    fn find(&mut self, id: &str, launched: bool) -> Option<Arc<App>> {
        let file = format!("{id}.desktop");
        let stale = self.read_at.is_none_or(|at| {
            launched && !self.apps.contains_key(&file) && at.elapsed() > APPS_KEPT
        });
        if stale {
            self.read();
        }
        self.apps.get(&file).cloned().flatten()
    }

    fn find_program(&mut self, program: &str) -> Option<Arc<App>> {
        if self.read_at.is_none() {
            self.read();
        }
        self.programs.get(program).cloned()
    }

    fn read(&mut self) {
        // Interpreters and launchers run many apps, and name none of them.
        const SHARED: [&str; 12] = [
            "sh", "bash", "env", "python", "python3", "perl", "ruby", "node", "gjs", "java",
            "flatpak", "wine",
        ];
        self.apps.clear();
        self.programs.clear();
        for info in gio::AppInfo::all() {
            let Some(file) = info.id() else {
                continue;
            };
            let app = info.should_show().then(|| {
                Arc::new(App {
                    id: Arc::from(file.trim_end_matches(".desktop")),
                    name: Arc::from(info.display_name().as_str()),
                    icon: info
                        .icon()
                        .and_then(|icon| IconExt::to_string(&icon))
                        .map(|icon| Arc::from(icon.as_str())),
                })
            });
            // Not the executable, which is null for an entry that D-Bus alone starts.
            let command = info.commandline();
            let program = command
                .as_deref()
                .and_then(|command| command.to_str())
                .and_then(|command| command.split_whitespace().next())
                .map(|program| program.rsplit('/').next().unwrap_or(program));
            if let Some(app) = &app
                && let Some(program) = program
                && !SHARED.contains(&program)
            {
                self.programs
                    .entry(program.to_owned())
                    .or_insert_with(|| app.clone());
            }
            self.apps.insert(file.to_string(), app);
        }
        self.read_at = Some(Instant::now());
    }
}

fn read_text(path: impl AsRef<Path>, into: &mut String) -> io::Result<()> {
    into.clear();
    File::open(path)?.read_to_string(into).map(drop)
}

fn proc_path<'a>(path: &'a mut String, pid: u32, file: &str) -> &'a str {
    path.clear();
    let _ = write!(path, "/proc/{pid}/{file}");
    path
}

/// The bytes moved since the last reading, over the devices counted in both: one that
/// has only now appeared, or is no longer counted, moves nothing.
fn moved(now: &HashMap<String, (u64, u64)>, before: &HashMap<String, (u64, u64)>) -> (u64, u64) {
    now.iter()
        .filter_map(|(name, (a, b))| {
            let (a_before, b_before) = before.get(name)?;
            Some((a.saturating_sub(*a_before), b.saturating_sub(*b_before)))
        })
        .fold((0, 0), |(a, b), (da, db)| (a + da, b + db))
}

/// Whether `name` under `class` is backed by a device, remembered once asked.
fn is_physical(known: &mut HashMap<String, bool>, class: &str, name: &str) -> bool {
    if let Some(&physical) = known.get(name) {
        return physical;
    }
    let physical = Path::new(class).join(name).join("device").exists();
    known.insert(name.to_owned(), physical);
    physical
}

/// Size, used and available bytes of the file system mounted at `path`.
fn usage(path: &str) -> Option<(u64, u64, u64)> {
    let path = CString::new(path).ok()?;
    let mut stat = MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `path` is NUL-terminated and outlives the call; statvfs writes a whole
    // `struct statvfs` into `stat` when it returns 0.
    if unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: statvfs returned 0, so it filled `stat` in.
    let stat = unsafe { stat.assume_init() };
    // The fields are 32 bits wide on 32-bit targets without large file support.
    #[allow(clippy::useless_conversion)]
    let (block, blocks, free, available) = (
        u64::from(stat.f_frsize),
        u64::from(stat.f_blocks),
        u64::from(stat.f_bfree),
        u64::from(stat.f_bavail),
    );
    let size = blocks * block;
    Some((size, size.saturating_sub(free * block), available * block))
}

/// The login name of `uid`, through NSS, so users from systemd-homed or a directory
/// service are named too.
fn user_name(uid: u32) -> Option<String> {
    let mut buffer = vec![0u8; 1024];
    loop {
        let mut entry = MaybeUninit::<libc::passwd>::uninit();
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        // SAFETY: every pointer is to memory of the given size that lives through the
        // call; getpwuid_r stores strings in `buffer` and points `found` at `entry`.
        let status = unsafe {
            libc::getpwuid_r(
                uid,
                entry.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut found,
            )
        };
        if status == libc::ERANGE && buffer.len() < 1 << 20 {
            buffer.resize(buffer.len() * 2, 0);
            continue;
        }
        if status != 0 || found.is_null() {
            return None;
        }
        // SAFETY: `found` points at `entry`, filled in, whose name points into `buffer`;
        // both are alive here.
        let name = unsafe { CStr::from_ptr((*found).pw_name) };
        return Some(name.to_string_lossy().into_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_that_comes_or_goes_moves_nothing() {
        let before = HashMap::from([
            ("eth0".to_owned(), (100, 10)),
            ("usb0".to_owned(), (5_000, 500)),
        ]);
        let now = HashMap::from([
            ("eth0".to_owned(), (150, 30)),
            ("wlan0".to_owned(), (9_000_000, 1)),
        ]);
        assert_eq!(moved(&now, &before), (50, 20));
    }

    #[test]
    fn reading_the_running_machine() {
        let mut sampler = Sampler::new();
        let snapshot = sampler.sample(true);
        let system = &snapshot.system;
        assert!(!system.cores.is_empty());
        assert!((0.0..=100.0).contains(&system.cpu));
        assert!(system.memory.total > 0 && system.memory.used <= system.memory.total);

        let processes = snapshot.processes.expect("processes were asked for");
        let own = processes
            .iter()
            .find(|p| p.pid == std::process::id())
            .expect("the test's own process is listed");
        assert!(own.memory > 0);
        assert!(!own.name.is_empty());
        // SAFETY: getuid cannot fail and touches no memory.
        assert_eq!(own.uid, unsafe { libc::getuid() });
        assert!(own.disk.is_some(), "a process may read its own io");
    }

    #[test]
    fn processes_left_out_when_not_asked_for() {
        let mut sampler = Sampler::new();
        assert!(sampler.sample(false).processes.is_none());
        assert!(sampler.sample(true).processes.is_some());
    }
}
