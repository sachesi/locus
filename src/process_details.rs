//! What the details pane shows of a process, read on a worker while the pane is open, and
//! changing how a process is scheduled. Nothing here touches GTK.

use crate::procfs::{self, Scheduling, Socket};
use std::collections::HashMap;
use std::fs;
use std::io;

/// How many files and threads are listed at most, as a browser can have thousands.
pub const LISTED: usize = 200;

const SCHED_IDLE: u32 = 5;

#[derive(Debug, Default, Clone)]
pub struct Details {
    /// The systemd unit holding the first process, where there is one.
    pub unit: Option<String>,
    pub nice: i32,
    pub threads_count: u64,
    pub in_background: bool,
    /// Seconds since the first process started.
    pub running_for: Option<u64>,
    pub threads: Vec<ThreadInfo>,
    /// The files and sockets open; `None` where the kernel does not let this user look.
    pub files: Option<Vec<OpenFile>>,
    pub sockets: Option<Vec<Socket>>,
}

#[derive(Debug, Clone)]
pub struct ThreadInfo {
    pub id: u32,
    pub name: String,
    pub ticks: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenFile {
    pub path: String,
    /// 0 to read, 1 to write, 2 both.
    pub mode: u32,
}

/// Reads what the pane shows of `pids`, the processes of an app or a single process.
pub fn read(pids: &[(u32, u64)], ticks_per_second: u64) -> Details {
    let mut details = Details::default();
    let Some(&(first, start_time)) = pids.first() else {
        return details;
    };
    if let Some(unit) = fs::read_to_string(format!("/proc/{first}/cgroup"))
        .ok()
        .as_deref()
        .and_then(unit_of_cgroup)
    {
        details.unit = Some(unit.to_owned());
    }
    if let Ok(text) = fs::read_to_string(format!("/proc/{first}/stat"))
        && let Some(scheduling) = procfs::parse_scheduling(&text)
    {
        details.nice = scheduling.nice;
        details.in_background = scheduling.policy == SCHED_IDLE;
    }
    let started = start_time as f64 / ticks_per_second.max(1) as f64;
    details.running_for = Some((time_since_boot() - started).max(0.0) as u64);

    let mut threads = Vec::new();
    let mut files = Vec::new();
    let mut inodes = Vec::new();
    let mut readable = false;
    for (pid, _) in pids {
        details.threads_count += fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .as_deref()
            .and_then(procfs::parse_scheduling)
            .map_or(0, |s: Scheduling| s.threads);
        add_threads(*pid, &mut threads);
        readable |= add_files(*pid, &mut files, &mut inodes).is_ok();
    }
    threads.sort_by_key(|thread: &ThreadInfo| std::cmp::Reverse(thread.ticks));
    threads.truncate(LISTED);
    details.threads = threads;
    files.sort_by(|a: &OpenFile, b: &OpenFile| a.path.cmp(&b.path));
    files.dedup();
    files.truncate(LISTED);
    if readable {
        details.files = Some(files);
        details.sockets = Some(sockets_of(first, &inodes));
    }
    details
}

/// Seconds since the machine started, on the clock a process's start time is on, which
/// `/proc/uptime` in a container is not.
fn time_since_boot() -> f64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: clock_gettime writes a timespec into `time`, which lives through the call.
    unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut time) };
    time.tv_sec as f64 + time.tv_nsec as f64 / 1e9
}

fn unit_of_cgroup(text: &str) -> Option<&str> {
    let path = text.lines().find_map(|line| line.strip_prefix("0::"))?;
    path.rsplit('/')
        .find(|part| part.ends_with(".service") || part.ends_with(".scope"))
}

fn add_threads(pid: u32, threads: &mut Vec<ThreadInfo>) {
    let Ok(tasks) = fs::read_dir(format!("/proc/{pid}/task")) else {
        return;
    };
    for task in tasks.flatten() {
        let Some(id) = task.file_name().to_str().and_then(|id| id.parse().ok()) else {
            continue;
        };
        let Ok(stat) = fs::read_to_string(task.path().join("stat")) else {
            continue;
        };
        let Some(parsed) = procfs::parse_stat(&stat) else {
            continue;
        };
        threads.push(ThreadInfo {
            id,
            name: parsed.comm.to_owned(),
            ticks: parsed.ticks,
        });
    }
}

/// Adds the files `pid` has open, and the inodes of its sockets.
fn add_files(pid: u32, files: &mut Vec<OpenFile>, inodes: &mut Vec<u64>) -> io::Result<()> {
    for entry in fs::read_dir(format!("/proc/{pid}/fd"))?.flatten() {
        let Ok(target) = fs::read_link(entry.path()) else {
            continue;
        };
        let target = target.to_string_lossy();
        if let Some(inode) = target
            .strip_prefix("socket:[")
            .and_then(|rest| rest.strip_suffix(']'))
            .and_then(|inode| inode.parse().ok())
        {
            inodes.push(inode);
        } else if target.starts_with('/') {
            let info = format!("/proc/{pid}/fdinfo/{}", entry.file_name().to_string_lossy());
            let mode = fs::read_to_string(info)
                .ok()
                .as_deref()
                .and_then(procfs::parse_fd_mode)
                .unwrap_or(0);
            files.push(OpenFile {
                path: target.into_owned(),
                mode,
            });
        }
    }
    Ok(())
}

/// The network sockets among `inodes`, which are of a process in the network namespace of
/// `pid`. Other sockets, local to the machine, are in tables of their own.
fn sockets_of(pid: u32, inodes: &[u64]) -> Vec<Socket> {
    let mut found: HashMap<u64, Socket> = HashMap::new();
    for table in ["tcp", "tcp6", "udp", "udp6"] {
        if let Ok(text) = fs::read_to_string(format!("/proc/{pid}/net/{table}")) {
            for socket in procfs::parse_sockets(&text) {
                if inodes.contains(&socket.inode) {
                    found.insert(socket.inode, socket);
                }
            }
        }
    }
    let mut sockets: Vec<Socket> = found.into_values().collect();
    sockets.sort_by_key(|socket| socket.local);
    sockets
}

/// Calls `change` for each thread of `pid`, if it is still the process that started at
/// `start_time`, and returns the first failure. Scheduling is the threads', not the process's.
fn for_each_thread(
    pid: u32,
    start_time: u64,
    change: impl Fn(libc::pid_t) -> io::Result<()>,
) -> io::Result<()> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    if procfs::parse_stat(&stat).map(|s| s.start_time) != Some(start_time) {
        return Err(io::Error::from_raw_os_error(libc::ESRCH));
    }
    let mut failure = None;
    for task in fs::read_dir(format!("/proc/{pid}/task"))?.flatten() {
        let Some(id) = task.file_name().to_str().and_then(|id| id.parse().ok()) else {
            continue;
        };
        if let Err(error) = change(id)
            && error.raw_os_error() != Some(libc::ESRCH)
        {
            failure.get_or_insert(error);
        }
    }
    failure.map_or(Ok(()), Err)
}

/// Sets the nice value of every thread of the process.
pub fn set_nice(pid: u32, start_time: u64, nice: i32) -> io::Result<()> {
    for_each_thread(pid, start_time, |thread| {
        let id = libc::id_t::try_from(thread).map_err(|_| io::ErrorKind::InvalidInput)?;
        // SAFETY: setpriority takes plain integers and touches no memory of ours.
        if unsafe { libc::setpriority(libc::PRIO_PROCESS, id, nice) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    })
}

/// Has every thread of the process run only when nothing else wants the processor, or
/// scheduled normally again.
pub fn set_in_background(pid: u32, start_time: u64, background: bool) -> io::Result<()> {
    let policy = if background {
        libc::SCHED_IDLE
    } else {
        libc::SCHED_OTHER
    };
    for_each_thread(pid, start_time, |thread| {
        let parameters = libc::sched_param { sched_priority: 0 };
        // SAFETY: `parameters` lives through the call, which only reads it.
        if unsafe { libc::sched_setscheduler(thread, policy, &parameters) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn start_time(pid: u32) -> u64 {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        procfs::parse_stat(&stat).unwrap().start_time
    }

    #[test]
    fn details_of_this_process() {
        let pid = std::process::id();
        let open = fs::File::open("/proc/self/stat").unwrap();
        let details = read(&[(pid, start_time(pid))], 100);
        drop(open);
        assert!(details.threads_count >= 1);
        assert!(details.threads.iter().any(|t| t.id == pid));
        assert!(details.files.is_some());
    }

    #[test]
    fn a_process_can_be_niced_and_sent_to_the_background() {
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id();
        let started = start_time(pid);

        let stale = set_nice(pid, started + 1, 5).unwrap_err();
        assert_eq!(stale.raw_os_error(), Some(libc::ESRCH));

        set_nice(pid, started, 5).unwrap();
        set_in_background(pid, started, true).unwrap();
        let details = read(&[(pid, started)], 100);
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(details.nice, 5);
        assert!(details.in_background);
    }

    #[test]
    fn unit_from_the_cgroup() {
        let text = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-foot-12.scope\n";
        assert_eq!(unit_of_cgroup(text), Some("app-foot-12.scope"));
        assert_eq!(unit_of_cgroup("0::/init.slice\n"), None);
    }
}
