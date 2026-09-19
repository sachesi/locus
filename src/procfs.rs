//! Parsers for the text files under /proc. They take the contents and never read anything
//! themselves, so the sampler decides when a file is read and the tests feed them fixtures.

/// Set in the flags of `/proc/<pid>/stat` for kernel threads.
const PF_KTHREAD: u64 = 0x0020_0000;

/// The fields of `/proc/<pid>/stat` the monitor uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stat<'a> {
    pub comm: &'a str,
    pub state: char,
    /// The parent's pid; 0 for what the kernel starts itself.
    pub ppid: u32,
    pub kernel_thread: bool,
    /// User plus system time, in clock ticks.
    pub ticks: u64,
    /// Clock ticks after boot at which the process started; with the pid, what tells one
    /// process from a later one given the same pid.
    pub start_time: u64,
}

pub fn parse_stat(text: &str) -> Option<Stat<'_>> {
    // The name sits in parentheses and may itself hold spaces and parentheses, so it ends
    // at the last closing one.
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    let comm = text.get(open + 1..close)?;
    let mut fields = text.get(close + 1..)?.split_ascii_whitespace();
    let state = fields.next()?.chars().next()?;
    let ppid = fields.next()?.parse().ok()?;
    // After the ppid: pgrp, session, tty_nr, tpgid, then flags.
    let flags: u64 = fields.nth(4)?.parse().ok()?;
    // minflt, cminflt, majflt, cmajflt, then utime and stime.
    let utime: u64 = fields.nth(4)?.parse().ok()?;
    let stime: u64 = fields.next()?.parse().ok()?;
    // cutime, cstime, priority, nice, num_threads, itrealvalue, then starttime.
    let start_time = fields.nth(6)?.parse().ok()?;
    Some(Stat {
        comm,
        state,
        ppid,
        kernel_thread: flags & PF_KTHREAD != 0,
        ticks: utime + stime,
        start_time,
    })
}

/// Resident and shared pages from `/proc/<pid>/statm`.
pub fn parse_statm(text: &str) -> Option<(u64, u64)> {
    let mut fields = text.split_ascii_whitespace().skip(1);
    let resident = fields.next()?.parse().ok()?;
    let shared = fields.next()?.parse().ok()?;
    Some((resident, shared))
}

/// Bytes read from and written to storage, from `/proc/<pid>/io`.
pub fn parse_io(text: &str) -> Option<(u64, u64)> {
    let read = field(text, "read_bytes:")?;
    let written = field(text, "write_bytes:")?;
    Some((read, written))
}

/// The real user id from `/proc/<pid>/status`.
pub fn parse_status_uid(text: &str) -> Option<u32> {
    let line = text.lines().find_map(|l| l.strip_prefix("Uid:"))?;
    line.split_ascii_whitespace().next()?.parse().ok()
}

/// The name to show for a process: the program in its command line where that is the
/// same program as its (truncated to 15 bytes) `comm`, else `comm`.
pub fn display_name(comm: &str, cmdline: &[u8]) -> String {
    let mut args = cmdline.split(|&b| b == 0).filter(|a| !a.is_empty());
    let Some(first) = args.next() else {
        return comm.to_owned();
    };
    let first = String::from_utf8_lossy(first);
    // Programs that rewrite their title join their arguments into the first with spaces,
    // as Chromium's helpers do, or add what they are doing after a colon, as sshd does.
    let program = match first.split_once(' ') {
        Some((program, _)) if args.next().is_none() => program.trim_end_matches(':'),
        _ => &first,
    };
    let base = program.rsplit('/').next().unwrap_or(program);
    if !comm.is_empty() && base.starts_with(comm) {
        base.to_owned()
    } else {
        comm.to_owned()
    }
}

/// `/proc/<pid>/cmdline` with its arguments separated by spaces.
pub fn command_line(cmdline: &[u8]) -> String {
    let args: Vec<_> = cmdline
        .split(|&b| b == 0)
        .filter(|a| !a.is_empty())
        .map(String::from_utf8_lossy)
        .collect();
    args.join(" ")
}

/// Time a processor spent busy and in total, in clock ticks.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CpuTimes {
    pub busy: u64,
    pub total: u64,
}

impl CpuTimes {
    /// Percentage of `self - earlier` spent busy.
    pub fn usage_since(self, earlier: CpuTimes) -> f64 {
        let total = self.total.saturating_sub(earlier.total);
        if total == 0 {
            return 0.0;
        }
        let busy = self.busy.saturating_sub(earlier.busy).min(total);
        busy as f64 * 100.0 / total as f64
    }
}

/// The processor times of `/proc/stat`: the whole machine first, then each online CPU.
pub fn parse_cpu_times(text: &str) -> Vec<CpuTimes> {
    text.lines()
        .take_while(|l| l.starts_with("cpu"))
        .filter_map(|line| {
            let values: Vec<u64> = line
                .split_ascii_whitespace()
                .skip(1)
                .map_while(|v| v.parse().ok())
                .collect();
            // user nice system idle iowait irq softirq steal; guest and guest_nice are
            // counted in user and nice already.
            let total: u64 = values.iter().take(8).sum();
            let idle = values.get(3)? + values.get(4).copied().unwrap_or(0);
            Some(CpuTimes {
                busy: total.saturating_sub(idle),
                total,
            })
        })
        .collect()
}

/// Memory and swap, in bytes.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Memory {
    pub total: u64,
    pub used: u64,
    pub swap_total: u64,
    pub swap_used: u64,
}

pub fn parse_meminfo(text: &str) -> Memory {
    let kib = |key: &str| field(text, key).unwrap_or(0) * 1024;
    let total = kib("MemTotal:");
    let available = kib("MemAvailable:");
    let swap_total = kib("SwapTotal:");
    let swap_free = kib("SwapFree:");
    Memory {
        total,
        used: total.saturating_sub(available),
        swap_total,
        swap_used: swap_total.saturating_sub(swap_free),
    }
}

/// Interface name, bytes received and bytes sent, from `/proc/net/dev`.
pub fn parse_net_dev(text: &str) -> Vec<(&str, u64, u64)> {
    text.lines()
        .skip(2)
        .filter_map(|line| {
            let (name, counters) = line.split_once(':')?;
            let mut counters = counters.split_ascii_whitespace();
            let received = counters.next()?.parse().ok()?;
            let sent = counters.nth(7)?.parse().ok()?;
            Some((name.trim(), received, sent))
        })
        .collect()
}

/// Device name, sectors read and sectors written, from `/proc/diskstats`. Its sectors are
/// 512 bytes whatever the device's own are.
pub fn parse_diskstats(text: &str) -> Vec<(&str, u64, u64)> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_ascii_whitespace().skip(2);
            let name = fields.next()?;
            let read = fields.nth(2)?.parse().ok()?;
            let written = fields.nth(3)?.parse().ok()?;
            Some((name, read, written))
        })
        .collect()
}

/// A mounted file system from `/proc/self/mountinfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    pub device: (u32, u32),
    pub mount_point: String,
    pub fs_type: String,
    pub source: String,
}

/// The file systems on block devices, each once: a device mounted in several places (btrfs
/// subvolumes, bind mounts) is listed at its shortest mount point.
pub fn parse_mountinfo(text: &str) -> Vec<Mount> {
    let mut mounts: Vec<Mount> = Vec::new();
    for line in text.lines() {
        let Some(mount) = parse_mount(line) else {
            continue;
        };
        if !mount.source.starts_with("/dev/") || mount.fs_type == "squashfs" {
            continue;
        }
        match mounts.iter_mut().find(|m| m.device == mount.device) {
            Some(known) if mount.mount_point.len() < known.mount_point.len() => *known = mount,
            Some(_) => {}
            None => mounts.push(mount),
        }
    }
    mounts.sort_by(|a, b| a.mount_point.cmp(&b.mount_point));
    mounts
}

fn parse_mount(line: &str) -> Option<Mount> {
    let (mount, filesystem) = line.split_once(" - ")?;
    let mut fields = mount.split(' ');
    let (major, minor) = fields.nth(2)?.split_once(':')?;
    let mount_point = unescape(fields.nth(1)?);
    let mut filesystem = filesystem.split(' ');
    let fs_type = filesystem.next()?.to_owned();
    let source = unescape(filesystem.next()?);
    Some(Mount {
        device: (major.parse().ok()?, minor.parse().ok()?),
        mount_point,
        fs_type,
        source,
    })
}

/// Undoes the octal escapes mountinfo writes for space, tab, newline and backslash.
fn unescape(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && let Some(code) = field.get(i + 1..i + 4)
            && let Ok(byte) = u8::from_str_radix(code, 8)
        {
            out.push(byte);
            i += 4;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The desktop file ids, without ".desktop", that the unit holding a process may be named
/// after, from `/proc/<pid>/cgroup`, likeliest first. Desktops start an app in a unit of its
/// own, `app-[<launcher>-]<id>[@<instance>].service` or `app-[<launcher>-]<id>-<n>.scope`,
/// and D-Bus starts the apps it activates as `dbus-:<connection>-<id>@<instance>.service`.
pub fn cgroup_app_ids(text: &str) -> Vec<String> {
    let Some(path) = text.lines().find_map(|line| line.strip_prefix("0::")) else {
        return Vec::new();
    };
    let unit = path.rsplit('/').next().unwrap_or_default();
    let name = if let Some(scope) = unit.strip_suffix(".scope") {
        scope.rsplit_once('-').map_or(scope, |(name, _)| name)
    } else if let Some(service) = unit.strip_suffix(".service") {
        service.split_once('@').map_or(service, |(name, _)| name)
    } else {
        return Vec::new();
    };
    // Dashes inside the id are escaped, so the first bare one ends the launcher's name.
    if let Some(rest) = name.strip_prefix("app-") {
        let mut ids = vec![unescape_unit(rest)];
        if let Some((_, id)) = rest.split_once('-') {
            ids.push(unescape_unit(id));
        }
        ids
    } else if let Some(rest) = name.strip_prefix("dbus-:") {
        rest.split_once('-')
            .map(|(_, id)| vec![unescape_unit(id)])
            .unwrap_or_default()
    } else {
        Vec::new()
    }
}

/// Undoes systemd's escaping in a unit name, which writes a dash as `\x2d`.
fn unescape_unit(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut rest = name;
    while let Some(at) = rest.find("\\x") {
        out.push_str(&rest[..at]);
        let code = rest
            .get(at + 2..at + 4)
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match code {
            Some(byte) => {
                out.push(char::from(byte));
                rest = &rest[at + 4..];
            }
            None => {
                out.push('\\');
                rest = &rest[at + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The value of the variable `name` in `/proc/<pid>/environ`.
pub fn environment_variable<'a>(environ: &'a [u8], name: &str) -> Option<&'a str> {
    environ
        .split(|&b| b == 0)
        .find_map(|variable| variable.strip_prefix(name.as_bytes())?.strip_prefix(b"="))
        .and_then(|value| std::str::from_utf8(value).ok())
}

/// The desktop file GLib started the process, or the one it descends from, from.
pub fn launched_desktop_file(environ: &[u8]) -> Option<&str> {
    environment_variable(environ, "GIO_LAUNCHED_DESKTOP_FILE").filter(|path| !path.is_empty())
}

/// Whether D-Bus started the process, as it starts a service it is asked for.
pub fn started_by_dbus(environ: &[u8]) -> bool {
    environment_variable(environ, "DBUS_STARTER_BUS_TYPE").is_some()
}

/// The processor's name from `/proc/cpuinfo`, where it gives one.
pub fn parse_cpu_model(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        (key.trim() == "model name")
            .then(|| value.trim().to_owned())
            .filter(|v| !v.is_empty())
    })
}

/// The number after `key` on its own line of a `key: value` file.
fn field(text: &str, key: &str) -> Option<u64> {
    let rest = text.lines().find_map(|l| l.strip_prefix(key))?;
    rest.split_ascii_whitespace().next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_with_parentheses_and_spaces_in_the_name() {
        let text = "4242 (Web Content (x)) S 1 4242 4242 0 -1 4194560 100 0 0 0 \
                    250 50 0 0 20 0 30 0 987654 123 456";
        let stat = parse_stat(text).unwrap();
        assert_eq!(stat.comm, "Web Content (x)");
        assert_eq!(stat.state, 'S');
        assert_eq!(stat.ppid, 1);
        assert!(!stat.kernel_thread);
        assert_eq!(stat.ticks, 300);
        assert_eq!(stat.start_time, 987654);
    }

    #[test]
    fn apps_by_the_units_desktops_start_them_in() {
        let scope = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/\
                     app-gnome-org.gnome.TextEditor-4242.scope\n";
        assert_eq!(
            cgroup_app_ids(scope),
            ["gnome-org.gnome.TextEditor", "org.gnome.TextEditor"]
        );
        let flatpak = "0::/user.slice/app-flatpak-org.mozilla.firefox-1234.scope\n";
        assert_eq!(cgroup_app_ids(flatpak)[1], "org.mozilla.firefox");
        let escaped = "0::/app.slice/app-org.example.Two\\x2dWords@autostart.service\n";
        assert_eq!(cgroup_app_ids(escaped), ["org.example.Two-Words"]);
        let activated = "0::/app.slice/dbus-:1.2-org.gnome.Nautilus@0.service\n";
        assert_eq!(cgroup_app_ids(activated), ["org.gnome.Nautilus"]);
        assert!(cgroup_app_ids("0::/system.slice/sshd.service\n").is_empty());
        assert!(cgroup_app_ids("0::/user.slice/user-1000.slice/session-2.scope\n").is_empty());
    }

    #[test]
    fn desktop_file_glib_launched_from() {
        let environ = b"HOME=/home/me\0GIO_LAUNCHED_DESKTOP_FILE=/usr/share/applications/foot.desktop\0GIO_LAUNCHED_DESKTOP_FILE_PID=12\0";
        assert_eq!(
            launched_desktop_file(environ),
            Some("/usr/share/applications/foot.desktop")
        );
        assert_eq!(launched_desktop_file(b"HOME=/home/me\0"), None);
        assert!(started_by_dbus(
            b"DBUS_STARTER_BUS_TYPE=session\0HOME=/home/me\0"
        ));
        assert!(!started_by_dbus(environ));
    }

    #[test]
    fn kernel_threads_are_flagged() {
        let text = "2 (kthreadd) S 0 0 0 0 -1 2129984 0 0 0 0 0 1 0 0 20 0 1 0 1 0 0";
        assert!(parse_stat(text).unwrap().kernel_thread);
    }

    #[test]
    fn truncated_stat_is_refused() {
        assert_eq!(parse_stat("12 (sh) S 1 12"), None);
    }

    #[test]
    fn statm_resident_and_shared() {
        assert_eq!(parse_statm("5000 1200 300 10 0 800 0\n"), Some((1200, 300)));
    }

    #[test]
    fn io_counts_storage_bytes() {
        let text = "rchar: 99\nwchar: 98\nsyscr: 1\nsyscw: 1\nread_bytes: 4096\n\
                    write_bytes: 8192\ncancelled_write_bytes: 0\n";
        assert_eq!(parse_io(text), Some((4096, 8192)));
    }

    #[test]
    fn real_uid_from_status() {
        let text = "Name:\tsu\nUmask:\t0022\nUid:\t1000\t0\t0\t0\nGid:\t1000\t1000\t1000\t1000\n";
        assert_eq!(parse_status_uid(text), Some(1000));
    }

    #[test]
    fn name_from_the_command_line_when_comm_is_cut_short() {
        let cmdline = b"/usr/libexec/gnome-software-service\0--flag\0";
        assert_eq!(
            display_name("gnome-software-", cmdline),
            "gnome-software-service"
        );
    }

    #[test]
    fn name_of_a_retitled_process() {
        let cmdline = b"/opt/chromium/chrome --type=renderer --lang=en-US\0";
        assert_eq!(display_name("chrome", cmdline), "chrome");
    }

    #[test]
    fn name_of_a_process_that_reports_its_state() {
        let cmdline = b"systemd-userwork: waiting...\0";
        assert_eq!(display_name("systemd-userwor", cmdline), "systemd-userwork");
    }

    #[test]
    fn name_of_a_script_is_its_comm() {
        let cmdline = b"/usr/bin/python3\0/usr/bin/backup.py\0";
        assert_eq!(display_name("backup.py", cmdline), "backup.py");
    }

    #[test]
    fn name_without_a_command_line() {
        assert_eq!(display_name("kworker/0:1", b""), "kworker/0:1");
    }

    #[test]
    fn command_line_joins_arguments() {
        assert_eq!(command_line(b"sleep\x00100\x00"), "sleep 100");
    }

    #[test]
    fn cpu_times_whole_and_per_core() {
        let text = "cpu  100 0 50 800 50 0 0 0 20 0\n\
                    cpu0 60 0 20 400 20 0 0 0 10 0\n\
                    cpu1 40 0 30 400 30 0 0 0 10 0\n\
                    intr 1 2 3\n";
        let times = parse_cpu_times(text);
        assert_eq!(times.len(), 3);
        assert_eq!(
            times[0],
            CpuTimes {
                busy: 150,
                total: 1000
            }
        );
        assert_eq!(
            times[1],
            CpuTimes {
                busy: 80,
                total: 500
            }
        );
    }

    #[test]
    fn usage_between_two_readings() {
        let earlier = CpuTimes {
            busy: 100,
            total: 1000,
        };
        let later = CpuTimes {
            busy: 150,
            total: 1200,
        };
        assert!((later.usage_since(earlier) - 25.0).abs() < 1e-9);
        assert_eq!(earlier.usage_since(earlier), 0.0);
    }

    #[test]
    fn meminfo_counts_available_as_free() {
        let text = "MemTotal:       16000000 kB\nMemFree:         1000000 kB\n\
                    MemAvailable:    6000000 kB\nSwapTotal:       8000000 kB\n\
                    SwapFree:        7000000 kB\n";
        let memory = parse_meminfo(text);
        assert_eq!(memory.total, 16_000_000 * 1024);
        assert_eq!(memory.used, 10_000_000 * 1024);
        assert_eq!(memory.swap_used, 1_000_000 * 1024);
    }

    #[test]
    fn net_dev_counters() {
        let text = "Inter-|   Receive |  Transmit\n face |bytes packets|bytes packets\n\
                    \x20   lo: 500 5 0 0 0 0 0 0 500 5 0 0 0 0 0 0\n\
                    wlp3s0:1000 10 0 0 0 0 0 0 2000 20 0 0 0 0 0 0\n";
        assert_eq!(
            parse_net_dev(text),
            vec![("lo", 500, 500), ("wlp3s0", 1000, 2000)]
        );
    }

    #[test]
    fn diskstats_sectors() {
        let text = " 259       0 nvme0n1 100 0 2048 10 50 0 4096 20 0 30 30 0 0 0 0 0 0\n";
        assert_eq!(parse_diskstats(text), vec![("nvme0n1", 2048, 4096)]);
    }

    #[test]
    fn mountinfo_keeps_block_devices_once() {
        let text = "\
22 1 0:21 / /proc rw - proc proc rw
35 1 0:35 /root / rw,relatime shared:1 - btrfs /dev/nvme0n1p3 rw
36 35 0:35 /home /home rw,relatime shared:2 - btrfs /dev/nvme0n1p3 rw
37 35 259:1 / /boot/efi rw - vfat /dev/nvme0n1p1 rw
38 35 7:0 / /snap/core/1 ro - squashfs /dev/loop0 ro
39 35 8:17 / /run/media/me/My\\040Disk rw - ext4 /dev/sdb1 rw
";
        let mounts = parse_mountinfo(text);
        let points: Vec<_> = mounts.iter().map(|m| m.mount_point.as_str()).collect();
        assert_eq!(points, ["/", "/boot/efi", "/run/media/me/My Disk"]);
        assert_eq!(mounts[0].fs_type, "btrfs");
        assert_eq!(mounts[0].source, "/dev/nvme0n1p3");
    }

    #[test]
    fn cpu_model_name() {
        let text = "processor\t: 0\nvendor_id\t: AuthenticAMD\nmodel name\t: AMD Ryzen 7 7840U\n";
        assert_eq!(parse_cpu_model(text).as_deref(), Some("AMD Ryzen 7 7840U"));
        assert_eq!(parse_cpu_model("processor\t: 0\n"), None);
    }
}
