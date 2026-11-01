//! Reads the state of the machine from /proc and /sys. Runs on a worker thread: nothing in
//! here touches GTK, and a [`Sampler`] moves to the worker and back for each reading.

use crate::gpu::{self, Gpu};
use crate::procfs::{self, CpuTimes, DiskCounters, Memory};
use gtk::gio::{self, prelude::*};
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString};
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, Read};
use std::mem::MaybeUninit;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long the list of installed apps is kept before a process started from one it does
/// not have makes it read again.
const APPS_KEPT: Duration = Duration::from_secs(10);

/// How long after a reading without rates the next is taken, to measure them.
pub const BASELINE: Duration = Duration::from_millis(250);

/// Every how many readings a process's open files are looked through again for clients of
/// a graphics card, if their number has changed. Looking costs a system call per file, most
/// of what reading the processes would cost otherwise, so a client opened in between shows
/// up to this many readings late; one that closes is noticed at once.
const GPU_CLIENTS_RESCANNED: u64 = 5;

/// After how many readings the open files are looked through again even though their number
/// is the same, in case one was closed and a client opened in its place.
const GPU_CLIENTS_KEPT: u64 = 30;

/// Nanoseconds each kind of engine spent on a process's work, by card and engine, with how
/// many engines of the kind there are.
type Engines = HashMap<String, (u64, u64)>;

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
    /// The disks it lies on, by name; none when its device leads to no disk.
    pub disks: Vec<Arc<str>>,
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
    pub disks: Vec<Disk>,
    pub links: Vec<Link>,
    /// The mounted file systems, when they were asked for.
    pub filesystems: Option<Vec<FileSystem>>,
    pub gpus: Vec<Gpu>,
    /// What only the page of the processor shows, while it is shown.
    pub cpu_detail: Option<CpuDetail>,
    /// What only the page of the memory shows, while it is shown.
    pub memory_detail: Option<MemoryDetail>,
}

/// A disk and what it did since the last reading.
#[derive(Debug, Clone)]
pub struct Disk {
    pub name: Arc<str>,
    pub model: Option<Arc<str>>,
    /// Spinning platters, as opposed to flash.
    pub rotational: bool,
    pub size: u64,
    /// Bytes per second.
    pub read: f64,
    pub written: f64,
    /// Share of the time it had requests in progress, from 0 to 100.
    pub active: f64,
    /// How long a request took on average, in milliseconds, where any completed.
    pub response: Option<f64>,
    /// In degrees Celsius; read only while the page of the disks is shown.
    pub temperature: Option<f64>,
}

/// A network interface and what it did since the last reading.
#[derive(Debug, Clone)]
pub struct Link {
    pub name: Arc<str>,
    pub kind: LinkKind,
    /// Bytes per second.
    pub received: f64,
    pub sent: f64,
    /// Bytes since the machine started.
    pub total_received: u64,
    pub total_sent: u64,
    /// Read only while the page of the interfaces is shown.
    pub detail: Option<LinkDetail>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    Ethernet,
    Wireless,
    /// Bridges, tunnels and container links, counted where there is no hardware.
    Virtual,
}

#[derive(Debug, Clone, Default)]
pub struct LinkDetail {
    /// In megabits per second, where the driver says.
    pub speed: Option<u32>,
    pub ipv4: Vec<String>,
    pub ipv6: Vec<String>,
    pub hardware_address: Option<String>,
    pub driver: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct CpuDetail {
    /// The average clock of the processors, in hertz.
    pub speed: Option<f64>,
    /// In degrees Celsius.
    pub temperature: Option<f64>,
    pub load: Option<f64>,
    pub processes: u64,
    pub threads: Option<u64>,
    /// Share of the last ten seconds some task waited for a processor.
    pub pressure: Option<f64>,
    /// Seconds since the machine started.
    pub uptime: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct MemoryDetail {
    /// Bytes stored in compressed swap and what they take up, where there is such swap.
    pub compressed_swap: Option<(u64, u64)>,
    /// Share of the last ten seconds some task waited for memory.
    pub pressure: Option<f64>,
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

/// What a reading covers besides the machine's totals, which it always reads.
#[derive(Debug, Clone, Copy, Default)]
pub struct Scope {
    pub processes: bool,
    pub filesystems: bool,
    /// The kind of device whose page is shown, to read what only that page needs.
    pub focus: Option<Focus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Cpu,
    Memory,
    Disk,
    Network,
    Gpu,
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub system: System,
    /// Every process but kernel threads, when they were asked for.
    pub processes: Option<Vec<ProcessSample>>,
    /// The reading had nothing earlier to measure rates against, of the machine or of the
    /// processes it lists, and leaves them to a second reading `BASELINE` later.
    pub unmeasured: bool,
}

/// What is remembered of a process between readings.
struct Tracked {
    start_time: u64,
    ticks: u64,
    io: Option<u64>,
    io_denied: bool,
    engines: Engines,
    /// The files the process has open as clients of a graphics card, when last looked.
    gpu_fds: Vec<u32>,
    /// How many files the process had open when last looked, and the reading that looked.
    fd_count: u64,
    fds_looked: u64,
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
    disks: HashMap<String, DiskCounters>,
    /// Whether an interface or a block device is hardware, by name.
    physical_links: HashMap<String, bool>,
    physical_disks: HashMap<String, bool>,
    disk_facts: HashMap<String, DiskFacts>,
    /// The disks each mounted device lies on, by the device's path.
    disks_of_source: HashMap<String, Vec<Arc<str>>>,
    /// The temperature sensor of the processor, once looked for.
    cpu_sensor: Option<Option<PathBuf>>,
    /// Share of its time each engine spent on the processes read, by card and engine.
    engine_use: HashMap<String, f64>,
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
            disk_facts: HashMap::new(),
            disks_of_source: HashMap::new(),
            cpu_sensor: None,
            engine_use: HashMap::new(),
            processes_at: None,
            processes: HashMap::new(),
            generation: 0,
            users: HashMap::new(),
            apps: AppIndex::default(),
        }
    }

    /// Reads the machine, and what else `scope` asks for. A first reading, of the machine
    /// or of its processes, has no rates, and is returned at once all the same, since what
    /// else it knows is of use before them.
    pub fn sample(&mut self, scope: Scope) -> Snapshot {
        let unmeasured = self.at.is_none() || (scope.processes && self.processes_at.is_none());
        let mut snapshot = self.take(scope);
        snapshot.unmeasured = unmeasured;
        snapshot
    }

    fn take(&mut self, scope: Scope) -> Snapshot {
        let now = Instant::now();
        let seconds = self.at.map(|at| now.duration_since(at).as_secs_f64());
        self.at = Some(now);
        let mut system = self.read_system(seconds.filter(|s| *s > 0.0), scope.focus);
        if scope.filesystems {
            system.filesystems = Some(self.read_filesystems());
        }
        let processes = if scope.processes {
            Some(self.read_processes(now))
        } else {
            // The counters kept would be stale by the time processes are asked for again.
            self.processes_at = None;
            None
        };
        if scope.focus == Some(Focus::Gpu) {
            self.add_engines(&mut system.gpus);
        }
        Snapshot {
            system,
            processes,
            unmeasured: false,
        }
    }

    /// Writes the use of each engine read from the processes into the cards it belongs to.
    fn add_engines(&self, gpus: &mut [Gpu]) {
        for gpu in gpus {
            let prefix = format!("{} ", gpu.slot);
            gpu.engines = self
                .engine_use
                .iter()
                .filter_map(|(key, share)| {
                    Some((key.strip_prefix(&prefix)?.to_owned(), share.min(100.0)))
                })
                .collect();
            gpu.engines.sort_by(|a, b| a.0.cmp(&b.0));
        }
    }

    fn read_system(&mut self, seconds: Option<f64>, focus: Option<Focus>) -> System {
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
            let counters: HashMap<String, (u64, u64)> = links
                .into_iter()
                .filter(|link| link.3 == hardware)
                .map(|(name, received, sent, _)| (name.to_owned(), (received, sent)))
                .collect();
            let (received, sent) = moved(&counters, &self.links);
            system.received = per_second(received);
            system.sent = per_second(sent);
            let addresses = (focus == Some(Focus::Network)).then(interface_addresses);
            system.links = counters
                .iter()
                .map(|(name, &(total_received, total_sent))| {
                    let (received, sent) = self.links.get(name).map_or((0, 0), |before| {
                        (
                            total_received.saturating_sub(before.0),
                            total_sent.saturating_sub(before.1),
                        )
                    });
                    Link {
                        name: Arc::from(name.as_str()),
                        kind: link_kind(name, hardware),
                        received: per_second(received),
                        sent: per_second(sent),
                        total_received,
                        total_sent,
                        detail: addresses
                            .as_ref()
                            .map(|addresses| link_detail(name, addresses.get(name.as_str()))),
                    }
                })
                .collect();
            system.links.sort_by(|a, b| a.name.cmp(&b.name));
            self.links = counters;
        }

        if read_text("/proc/diskstats", &mut self.text).is_ok() {
            // Partitions, device mapper and RAID devices repeat what their disks do.
            let counters: HashMap<String, DiskCounters> = procfs::parse_diskstats(&self.text)
                .into_iter()
                .filter(|(name, _)| is_physical(&mut self.physical_disks, "/sys/block", name))
                .map(|(name, counters)| (name.to_owned(), counters))
                .collect();
            let before = &self.disks;
            let sectors: HashMap<String, (u64, u64)> = counters
                .iter()
                .map(|(name, c)| (name.clone(), (c.sectors_read, c.sectors_written)))
                .collect();
            let sectors_before: HashMap<String, (u64, u64)> = before
                .iter()
                .map(|(name, c)| (name.clone(), (c.sectors_read, c.sectors_written)))
                .collect();
            let (read, written) = moved(&sectors, &sectors_before);
            system.read = per_second(read * 512);
            system.written = per_second(written * 512);
            system.disks = counters
                .iter()
                .map(|(name, now)| {
                    let was = before.get(name).copied().unwrap_or(*now);
                    let facts = self
                        .disk_facts
                        .entry(name.clone())
                        .or_insert_with(|| DiskFacts::read(name));
                    let requests = now.requests.saturating_sub(was.requests);
                    let active_ms = now.active_ms.saturating_sub(was.active_ms);
                    Disk {
                        name: Arc::from(name.as_str()),
                        model: facts.model.clone(),
                        rotational: facts.rotational,
                        size: facts.size,
                        read: per_second(now.sectors_read.saturating_sub(was.sectors_read) * 512),
                        written: per_second(
                            now.sectors_written.saturating_sub(was.sectors_written) * 512,
                        ),
                        active: seconds.map_or(0.0, |s| (active_ms as f64 / (s * 10.0)).min(100.0)),
                        response: (requests > 0).then(|| {
                            now.request_ms.saturating_sub(was.request_ms) as f64 / requests as f64
                        }),
                        temperature: facts
                            .sensor
                            .as_ref()
                            .filter(|_| focus == Some(Focus::Disk))
                            .and_then(|sensor| millidegrees(sensor)),
                    }
                })
                .collect();
            system.disks.sort_by(|a, b| a.name.cmp(&b.name));
            self.disks = counters;
        }

        match focus {
            Some(Focus::Cpu) => system.cpu_detail = Some(self.read_cpu_detail()),
            Some(Focus::Memory) => system.memory_detail = Some(self.read_memory_detail()),
            _ => {}
        }

        let gpu_detail = focus == Some(Focus::Gpu);
        system.gpus = self.gpus.iter().map(|card| card.read(gpu_detail)).collect();
        system
    }

    fn read_cpu_detail(&mut self) -> CpuDetail {
        let mut detail = CpuDetail::default();
        let sensor = self.cpu_sensor.get_or_insert_with(cpu_sensor);
        detail.temperature = sensor.as_ref().and_then(|sensor| millidegrees(sensor));
        let mut speeds = Vec::with_capacity(self.cpu.len());
        for cpu in 0..self.cpu.len().saturating_sub(1) {
            self.path.clear();
            let _ = write!(
                self.path,
                "/sys/devices/system/cpu/cpu{cpu}/cpufreq/scaling_cur_freq"
            );
            if read_text(&self.path, &mut self.text).is_ok()
                && let Ok(khz) = self.text.trim().parse::<f64>()
            {
                speeds.push(khz * 1e3);
            }
        }
        if !speeds.is_empty() {
            detail.speed = Some(speeds.iter().sum::<f64>() / speeds.len() as f64);
        }
        if read_text("/proc/loadavg", &mut self.text).is_ok()
            && let Some((load, threads)) = procfs::parse_loadavg(&self.text)
        {
            detail.load = Some(load);
            detail.threads = Some(threads);
        }
        if read_text("/proc/pressure/cpu", &mut self.text).is_ok() {
            detail.pressure = procfs::parse_pressure(&self.text);
        }
        if read_text("/proc/uptime", &mut self.text).is_ok() {
            detail.uptime = self
                .text
                .split_ascii_whitespace()
                .next()
                .and_then(|seconds| seconds.parse::<f64>().ok())
                .map(|seconds| seconds as u64);
        }
        detail.processes = fs::read_dir("/proc").map_or(0, |entries| {
            entries
                .flatten()
                .filter(|entry| entry.file_name().as_bytes()[0].is_ascii_digit())
                .count() as u64
        });
        detail
    }

    fn read_memory_detail(&mut self) -> MemoryDetail {
        let mut detail = MemoryDetail::default();
        if read_text("/proc/pressure/memory", &mut self.text).is_ok() {
            detail.pressure = procfs::parse_pressure(&self.text);
        }
        let Ok(devices) = fs::read_dir("/sys/block") else {
            return detail;
        };
        for device in devices.flatten() {
            if device.file_name().as_bytes().starts_with(b"zram")
                && read_text(device.path().join("mm_stat"), &mut self.text).is_ok()
                && let Some((stored, compressed)) = procfs::parse_zram_stat(&self.text)
            {
                let total = detail.compressed_swap.get_or_insert((0, 0));
                total.0 += stored;
                total.1 += compressed;
            }
        }
        detail
    }

    fn read_filesystems(&mut self) -> Vec<FileSystem> {
        if read_text("/proc/self/mountinfo", &mut self.text).is_err() {
            return Vec::new();
        }
        procfs::parse_mountinfo(&self.text)
            .into_iter()
            .filter_map(|mount| {
                let (size, used, available) = usage(&mount.mount_point)?;
                let disks = self
                    .disks_of_source
                    .entry(mount.source.clone())
                    .or_insert_with(|| disks_of_source(&mount.source))
                    .clone();
                (size > 0).then_some(FileSystem {
                    disks,
                    mount_point: mount.mount_point,
                    source: mount.source,
                    fs_type: mount.fs_type,
                    size,
                    used,
                    available,
                })
            })
            .collect()
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
        self.engine_use.clear();
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
                // Staggered by pid, so each reading looks through a share of the processes.
                let turn = !known
                    || (self.generation + u64::from(pid)).is_multiple_of(GPU_CLIENTS_RESCANNED);
                let rescan = turn && tracked.files_changed(pid, self.generation, &mut self.path);
                match gpu_engines(
                    pid,
                    &mut tracked.gpu_fds,
                    rescan,
                    &mut self.path,
                    &mut self.text,
                    &mut clients,
                ) {
                    Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                        tracked.fds_denied = true;
                        None
                    }
                    Err(_) => Some(0.0),
                    Ok(engines) => {
                        let mut busiest: f64 = 0.0;
                        if let Some(s) = seconds.filter(|_| known) {
                            for (key, &(ns, capacity)) in &engines {
                                let before = tracked.engines.get(key).map_or(ns, |b| b.0);
                                let share =
                                    ns.saturating_sub(before) as f64 / (s * 1e7 * capacity as f64);
                                busiest = busiest.max(share);
                                if share > 0.0 {
                                    *self.engine_use.entry(key.clone()).or_insert(0.0) += share;
                                }
                            }
                        }
                        let busiest = busiest.min(100.0);
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

impl Tracked {
    /// Whether the process may have opened or closed files since they were last looked
    /// through, and if so, notes that they are being looked through now.
    fn files_changed(&mut self, pid: u32, generation: u64, path: &mut String) -> bool {
        // Kernels before 6.2 count no files here, and give 0.
        let count = fs::metadata(proc_path(path, pid, "fd")).map_or(0, |fds| fds.len());
        let changed = count == 0
            || count != self.fd_count
            || generation - self.fds_looked >= GPU_CLIENTS_KEPT;
        if changed {
            self.fd_count = count;
            self.fds_looked = generation;
        }
        changed
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
        gpu_fds: Vec::new(),
        fd_count: 0,
        fds_looked: 0,
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
///
/// Reads only the clients in `fds` unless told to `rescan`, or one of them is no longer a
/// client; then it looks through every open file of the process and puts the clients
/// found in `fds`.
fn gpu_engines(
    pid: u32,
    fds: &mut Vec<u32>,
    rescan: bool,
    path: &mut String,
    text: &mut String,
    clients: &mut HashSet<(String, u64)>,
) -> io::Result<Engines> {
    if !rescan && let Some(engines) = client_engines(pid, fds, path, text, clients) {
        return Ok(engines);
    }
    fds.clear();
    let mut engines = Engines::new();
    for entry in fs::read_dir(proc_path(path, pid, "fd"))?.flatten() {
        let Ok(target) = fs::read_link(entry.path()) else {
            continue;
        };
        if !target.as_os_str().as_bytes().starts_with(b"/dev/dri/") {
            continue;
        }
        let Some(fd) = entry.file_name().to_str().and_then(|n| n.parse().ok()) else {
            continue;
        };
        if add_client(pid, fd, &mut engines, &mut Vec::new(), path, text, clients).is_some() {
            fds.push(fd);
        }
    }
    Ok(engines)
}

/// The engines of the clients open as `fds`, or `None`, claiming none of them, when one is
/// no longer a client.
fn client_engines(
    pid: u32,
    fds: &[u32],
    path: &mut String,
    text: &mut String,
    clients: &mut HashSet<(String, u64)>,
) -> Option<Engines> {
    let mut engines = Engines::new();
    let mut claimed = Vec::new();
    for &fd in fds {
        if add_client(pid, fd, &mut engines, &mut claimed, path, text, clients).is_none() {
            for client in &claimed {
                clients.remove(client);
            }
            return None;
        }
    }
    Some(engines)
}

/// Adds the time of the client open as `fd` to `engines`, unless an earlier process has
/// claimed the client, and notes it in `claimed` if not. `None` if `fd` is not a client.
fn add_client(
    pid: u32,
    fd: u32,
    engines: &mut Engines,
    claimed: &mut Vec<(String, u64)>,
    path: &mut String,
    text: &mut String,
    clients: &mut HashSet<(String, u64)>,
) -> Option<()> {
    path.clear();
    let _ = write!(path, "/proc/{pid}/fdinfo/{fd}");
    read_text(&*path, text).ok()?;
    let client = procfs::parse_drm_fdinfo(text)?;
    let key = (client.pdev.to_owned(), client.id);
    if clients.contains(&key) {
        return Some(());
    }
    for (engine, ns, capacity) in client.engines {
        let total = engines
            .entry(format!("{} {engine}", client.pdev))
            .or_insert((0, capacity));
        total.0 += ns;
    }
    clients.insert(key.clone());
    claimed.push(key);
    Some(())
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
            // executable() panics on an entry that only D-Bus starts, which has none.
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

/// Reads a file of /proc or /sys whole. Such a file gives all it has to a read with room
/// for it, so one that gives less than there was room for has ended: the read that would
/// return nothing is not made, nor the one asking for the file's size, which it does not
/// know.
fn read_text(path: impl AsRef<Path>, into: &mut String) -> io::Result<()> {
    into.clear();
    let file = File::open(path)?;
    // SAFETY: the bytes are checked to be UTF-8 before this returns, and cleared if not.
    let bytes = unsafe { into.as_mut_vec() };
    let read = read_until_short(&file, bytes);
    if read.is_ok() && std::str::from_utf8(bytes).is_ok() {
        return Ok(());
    }
    bytes.clear();
    read.and(Err(io::ErrorKind::InvalidData.into()))
}

fn read_until_short(file: &File, bytes: &mut Vec<u8>) -> io::Result<()> {
    loop {
        bytes.reserve(bytes.capacity().max(4096) - bytes.len());
        let room = bytes.spare_capacity_mut();
        // SAFETY: read writes at most `room.len()` bytes into `room`, which is that long.
        let count = unsafe { libc::read(file.as_raw_fd(), room.as_mut_ptr().cast(), room.len()) };
        let Ok(count) = usize::try_from(count) else {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        };
        let short = count < room.len();
        // SAFETY: read filled in `count` bytes past the length.
        unsafe { bytes.set_len(bytes.len() + count) };
        if short {
            return Ok(());
        }
    }
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

/// What stays the same about a disk.
struct DiskFacts {
    model: Option<Arc<str>>,
    rotational: bool,
    size: u64,
    sensor: Option<PathBuf>,
}

impl DiskFacts {
    fn read(name: &str) -> Self {
        let block = Path::new("/sys/block").join(name);
        let text = |path: PathBuf| {
            fs::read_to_string(path)
                .ok()
                .map(|text| text.trim().to_owned())
                .filter(|text| !text.is_empty())
        };
        // A sector is 512 bytes in this file whatever the device's own are.
        let sectors = text(block.join("size")).and_then(|size| size.parse::<u64>().ok());
        let hwmon = |under: &Path| {
            fs::read_dir(under)
                .ok()?
                .flatten()
                .find(|entry| entry.file_name().as_bytes().starts_with(b"hwmon"))
                .map(|entry| entry.path().join("temp1_input"))
        };
        Self {
            model: text(block.join("device/model")).map(Arc::from),
            rotational: text(block.join("queue/rotational")).as_deref() == Some("1"),
            size: sectors.unwrap_or(0) * 512,
            sensor: hwmon(&block.join("device/hwmon")).or_else(|| hwmon(&block.join("device"))),
        }
    }
}

/// The disks a mounted device lies on: the disk of a partition, and what a device mapper or
/// RAID device is made of, down to the disks. Empty for a device that leads to no disk.
fn disks_of_source(source: &str) -> Vec<Arc<str>> {
    let device = fs::canonicalize(source).unwrap_or_else(|_| PathBuf::from(source));
    let Some(name) = device.file_name().and_then(|name| name.to_str()) else {
        return Vec::new();
    };
    // In a container /dev may not have the device, though /sys still says what it is.
    let mapped = source
        .strip_prefix("/dev/mapper/")
        .filter(|_| !device.exists())
        .and_then(mapped_device);
    let mut disks = Vec::new();
    add_disks_under(mapped.as_deref().unwrap_or(name), 0, &mut disks);
    disks
}

/// The kernel's name for the device mapper device called `name`.
fn mapped_device(name: &str) -> Option<String> {
    fs::read_dir("/sys/block")
        .ok()?
        .flatten()
        .find_map(|entry| {
            let mapped = fs::read_to_string(entry.path().join("dm/name")).ok()?;
            (mapped.trim() == name).then(|| entry.file_name().into_string().ok())?
        })
}

fn add_disks_under(name: &str, depth: usize, disks: &mut Vec<Arc<str>>) {
    // Only a loop in what sysfs says could go this deep.
    if depth > 8 {
        return;
    }
    let block = Path::new("/sys/class/block").join(name);
    if block.join("partition").exists() {
        if let Some(parent) = fs::canonicalize(&block)
            .ok()
            .and_then(|path| Some(path.parent()?.file_name()?.to_str()?.to_owned()))
        {
            add_disks_under(&parent, depth + 1, disks);
        }
        return;
    }
    let slaves: Vec<String> = fs::read_dir(block.join("slaves"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    if !slaves.is_empty() {
        for slave in &slaves {
            add_disks_under(slave, depth + 1, disks);
        }
    } else if block.join("device").exists() && !disks.iter().any(|disk| &**disk == name) {
        disks.push(Arc::from(name));
    }
}

fn link_kind(name: &str, hardware: bool) -> LinkKind {
    if !hardware {
        LinkKind::Virtual
    } else if Path::new("/sys/class/net")
        .join(name)
        .join("wireless")
        .exists()
    {
        LinkKind::Wireless
    } else {
        LinkKind::Ethernet
    }
}

fn link_detail(name: &str, addresses: Option<&(Vec<String>, Vec<String>)>) -> LinkDetail {
    let class = Path::new("/sys/class/net").join(name);
    let (ipv4, ipv6) = addresses.cloned().unwrap_or_default();
    LinkDetail {
        // A link that is down reports -1, or fails to read at all.
        speed: fs::read_to_string(class.join("speed"))
            .ok()
            .and_then(|speed| speed.trim().parse().ok()),
        ipv4,
        ipv6,
        hardware_address: fs::read_to_string(class.join("address"))
            .ok()
            .map(|address| address.trim().to_owned())
            .filter(|address| !address.is_empty() && address != "00:00:00:00:00:00"),
        driver: fs::read_link(class.join("device/driver"))
            .ok()
            .and_then(|driver| Some(driver.file_name()?.to_str()?.to_owned())),
    }
}

/// The IPv4 and IPv6 addresses of each interface.
fn interface_addresses() -> HashMap<String, (Vec<String>, Vec<String>)> {
    let mut addresses: HashMap<String, (Vec<String>, Vec<String>)> = HashMap::new();
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs points `list` at a list it allocates when it returns 0.
    if unsafe { libc::getifaddrs(&mut list) } != 0 {
        return addresses;
    }
    let mut entry = list;
    while !entry.is_null() {
        // SAFETY: `entry` is a node of the list, alive until freeifaddrs below, whose name is
        // NUL-terminated and whose address, where not null, is as long as its family says.
        unsafe {
            let name = CStr::from_ptr((*entry).ifa_name).to_string_lossy();
            let address = (*entry).ifa_addr;
            if !address.is_null() {
                let found = addresses.entry(name.into_owned()).or_default();
                match i32::from((*address).sa_family) {
                    libc::AF_INET => {
                        let ip = (*address.cast::<libc::sockaddr_in>()).sin_addr.s_addr;
                        found
                            .0
                            .push(std::net::Ipv4Addr::from(u32::from_be(ip)).to_string());
                    }
                    libc::AF_INET6 => {
                        let ip = (*address.cast::<libc::sockaddr_in6>()).sin6_addr.s6_addr;
                        found.1.push(std::net::Ipv6Addr::from(ip).to_string());
                    }
                    _ => {}
                }
            }
            entry = (*entry).ifa_next;
        }
    }
    // SAFETY: `list` came from getifaddrs and nothing refers to it any longer.
    unsafe { libc::freeifaddrs(list) };
    addresses
}

/// The sensor of the processor package, among those the kernel lists.
fn cpu_sensor() -> Option<PathBuf> {
    const NAMES: [&str; 5] = [
        "k10temp",
        "coretemp",
        "zenpower",
        "cpu_thermal",
        "cpu-thermal",
    ];
    fs::read_dir("/sys/class/hwmon")
        .ok()?
        .flatten()
        .find(|entry| {
            fs::read_to_string(entry.path().join("name"))
                .is_ok_and(|name| NAMES.contains(&name.trim()))
        })
        .map(|entry| entry.path().join("temp1_input"))
}

fn millidegrees(sensor: &Path) -> Option<f64> {
    let text = fs::read_to_string(sensor).ok()?;
    Some(text.trim().parse::<f64>().ok()? / 1000.0)
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
        let snapshot = sampler.sample(Scope {
            processes: true,
            filesystems: true,
            focus: None,
        });
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
        assert!(system.filesystems.is_some(), "file systems were asked for");
    }

    #[test]
    fn a_gpu_client_closed_between_looks_is_noticed() {
        use std::os::fd::AsRawFd;
        let Some(node) = fs::read_dir("/dev/dri")
            .into_iter()
            .flatten()
            .flatten()
            .find(|entry| entry.file_name().to_string_lossy().starts_with("renderD"))
        else {
            return;
        };
        let pid = std::process::id();
        let (mut path, mut text, mut fds) = (String::new(), String::new(), Vec::new());
        let client = File::options()
            .read(true)
            .write(true)
            .open(node.path())
            .expect("open the render node");
        let fd = u32::try_from(client.as_raw_fd()).expect("an open file's number");
        gpu_engines(
            pid,
            &mut fds,
            true,
            &mut path,
            &mut text,
            &mut HashSet::new(),
        )
        .expect("look through the test's own files");
        assert!(fds.contains(&fd));

        drop(client);
        // Likely given the same number, which is then no longer a client.
        let _other = File::open("/dev/null").expect("open /dev/null");
        gpu_engines(
            pid,
            &mut fds,
            false,
            &mut path,
            &mut text,
            &mut HashSet::new(),
        )
        .expect("look through the test's own files");
        assert!(!fds.contains(&fd));
    }

    #[test]
    fn details_are_read_for_the_page_shown() {
        let mut sampler = Sampler::new();
        let plain = sampler.sample(Scope::default());
        assert!(plain.system.cpu_detail.is_none() && plain.system.memory_detail.is_none());
        let cpu = sampler.sample(Scope {
            focus: Some(Focus::Cpu),
            ..Scope::default()
        });
        assert!(cpu.system.cpu_detail.expect("asked for").processes > 0);
        let memory = sampler.sample(Scope {
            focus: Some(Focus::Memory),
            ..Scope::default()
        });
        assert!(memory.system.memory_detail.is_some());
    }

    #[test]
    fn only_what_is_asked_for_is_read() {
        let mut sampler = Sampler::new();
        let totals = sampler.sample(Scope::default());
        assert!(totals.processes.is_none() && totals.system.filesystems.is_none());
        let processes = Scope {
            processes: true,
            ..Scope::default()
        };
        assert!(sampler.sample(processes).processes.is_some());
    }
}
