use crate::procfs;
use crate::sampler::ProcessSample;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use std::fs;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

mod imp {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::Process)]
    pub struct Process {
        #[property(get, set)]
        pid: Cell<u32>,
        #[property(get, set)]
        name: RefCell<String>,
        #[property(get, set)]
        command: RefCell<String>,
        #[property(get, set)]
        uid: Cell<u32>,
        #[property(get, set)]
        user: RefCell<String>,
        #[property(get, set)]
        cpu: Cell<f64>,
        #[property(get, set)]
        memory: Cell<u64>,
        /// Bytes per second to and from storage, or -1 where it cannot be read.
        #[property(get, set, minimum = -1)]
        disk: Cell<i64>,
        #[property(get, set)]
        stopped: Cell<bool>,
        pub start_time: Cell<u64>,
        /// The last reading that listed this process.
        pub generation: Cell<u64>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Process {
        const NAME: &'static str = "LocusProcess";
        type Type = super::Process;
    }

    #[glib::derived_properties]
    impl ObjectImpl for Process {}
}

glib::wrapper! {
    pub struct Process(ObjectSubclass<imp::Process>);
}

impl Process {
    pub fn new(sample: &ProcessSample, generation: u64) -> Self {
        let process: Self = glib::Object::builder()
            .property("pid", sample.pid)
            .property("name", &*sample.name)
            .property("command", &*sample.command)
            .property("uid", sample.uid)
            .property("user", &*sample.user)
            .build();
        process.imp().start_time.set(sample.start_time);
        process.update(sample, generation);
        process
    }

    pub fn start_time(&self) -> u64 {
        self.imp().start_time.get()
    }

    pub fn generation(&self) -> u64 {
        self.imp().generation.get()
    }

    /// Takes the values of a newer reading of the same process. Only what changed is set,
    /// so the rows showing it redraw only for that.
    pub fn update(&self, sample: &ProcessSample, generation: u64) {
        self.imp().generation.set(generation);
        let _batch = self.freeze_notify();
        if self.cpu() != sample.cpu {
            self.set_cpu(sample.cpu);
        }
        if self.memory() != sample.memory {
            self.set_memory(sample.memory);
        }
        let disk = sample
            .disk
            .map_or(-1, |d| i64::try_from(d).unwrap_or(i64::MAX));
        if self.disk() != disk {
            self.set_disk(disk);
        }
        if self.stopped() != sample.stopped {
            self.set_stopped(sample.stopped);
        }
    }

    pub fn send(&self, signal: Signal) -> io::Result<()> {
        send_signal(self.pid(), self.start_time(), signal)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Terminate,
    Kill,
    Stop,
    Continue,
}

impl Signal {
    fn number(self) -> libc::c_int {
        match self {
            Signal::Terminate => libc::SIGTERM,
            Signal::Kill => libc::SIGKILL,
            Signal::Stop => libc::SIGSTOP,
            Signal::Continue => libc::SIGCONT,
        }
    }
}

/// Signals the process with `pid` if it is still the one that started at `start_time`,
/// and not a later one that was given the same pid.
fn send_signal(pid: u32, start_time: u64, signal: Signal) -> io::Result<()> {
    let pid = libc::pid_t::try_from(pid).map_err(|_| io::Error::from_raw_os_error(libc::ESRCH))?;
    // SAFETY: pidfd_open takes a pid and flags and returns a new descriptor or -1.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = RawFd::try_from(fd).map_err(|_| io::Error::from_raw_os_error(libc::EBADF))?;
    // SAFETY: the descriptor was just returned by pidfd_open and nothing else owns it.
    let pidfd = unsafe { OwnedFd::from_raw_fd(fd) };

    // The descriptor holds on to whatever process has the pid now; checking the start time
    // after opening it makes sure that is the process the user chose.
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    if procfs::parse_stat(&stat).map(|s| s.start_time) != Some(start_time) {
        return Err(io::Error::from_raw_os_error(libc::ESRCH));
    }

    // SAFETY: `pidfd` is an open pidfd for the call's duration; a null siginfo makes the
    // kernel fill it in as kill() would; no flags are defined.
    let status = unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            pidfd.as_raw_fd(),
            signal.number(),
            std::ptr::null::<libc::siginfo_t>(),
            0,
        )
    };
    if status < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use std::process::Command;

    fn start_time(pid: u32) -> u64 {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        procfs::parse_stat(&stat).unwrap().start_time
    }

    #[test]
    fn signals_reach_the_process_they_were_meant_for() {
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id();
        let started = start_time(pid);

        let stale = send_signal(pid, started + 1, Signal::Terminate).unwrap_err();
        assert_eq!(stale.raw_os_error(), Some(libc::ESRCH));

        send_signal(pid, started, Signal::Kill).unwrap();
        let status = child.wait().unwrap();
        assert_eq!(status.signal(), Some(libc::SIGKILL));
    }
}
