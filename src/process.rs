use crate::procfs;
use crate::sampler::{App, ProcessSample};
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gio, glib};
use std::fs;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::Arc;

mod imp {
    use super::*;
    use std::cell::{Cell, OnceCell, RefCell};

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
        /// The app's icon, for the row of an app.
        #[property(get, set, nullable)]
        icon: RefCell<Option<gio::Icon>>,
        /// Whether this stands for an app, its processes listed beneath it.
        #[property(get, set)]
        is_app: Cell<bool>,
        /// How many processes are listed directly beneath this one.
        #[property(get, set)]
        child_count: Cell<u32>,
        pub app_id: RefCell<Option<Arc<str>>>,
        pub children: OnceCell<gio::ListStore>,
        pub place: RefCell<Place>,
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

    /// The row of an app, whose values are those of its processes added up.
    pub fn for_app(app: &App) -> Self {
        let icon = app
            .icon
            .as_deref()
            .and_then(|icon| gio::Icon::for_string(icon).ok());
        let process: Self = glib::Object::builder()
            .property("name", &*app.name)
            .property("icon", icon)
            .property("is-app", true)
            .build();
        process.imp().app_id.replace(Some(app.id.clone()));
        process
    }

    pub fn app_id(&self) -> Option<Arc<str>> {
        self.imp().app_id.borrow().clone()
    }

    pub fn set_app_id(&self, app_id: Option<Arc<str>>) {
        self.imp().app_id.replace(app_id);
    }

    /// The processes listed directly beneath this one.
    pub fn children(&self) -> gio::ListStore {
        self.imp()
            .children
            .get_or_init(gio::ListStore::new::<Process>)
            .clone()
    }

    /// What this is listed beneath, if it is listed beneath anything.
    pub fn parent(&self) -> Option<Process> {
        match &*self.imp().place.borrow() {
            Place::Under(parent) => parent.upgrade(),
            _ => None,
        }
    }

    /// Takes the added up values of the processes of the app this stands for.
    pub fn add_up(&self, processes: &[Process]) {
        let _batch = self.freeze_notify();
        let cpu: f64 = processes.iter().map(Process::cpu).sum();
        if self.cpu() != cpu {
            self.set_cpu(cpu);
        }
        let memory: u64 = processes.iter().map(Process::memory).sum();
        if self.memory() != memory {
            self.set_memory(memory);
        }
        let readable: Vec<i64> = processes
            .iter()
            .map(Process::disk)
            .filter(|disk| *disk >= 0)
            .collect();
        let disk = if readable.is_empty() {
            -1
        } else {
            readable.iter().sum()
        };
        if self.disk() != disk {
            self.set_disk(disk);
        }
        if let Some(first) = processes.first()
            && self.uid() != first.uid()
        {
            self.set_uid(first.uid());
            self.set_user(first.user());
        }
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

    /// Signals the process from a worker thread, as checking it is still the same process
    /// reads its entry under /proc.
    pub async fn send(&self, signal: Signal) -> io::Result<()> {
        let (pid, start_time) = (self.pid(), self.start_time());
        gio::spawn_blocking(move || send_signal(pid, start_time, signal))
            .await
            .unwrap_or_else(|_| Err(io::Error::other("the worker sending the signal failed")))
    }
}

/// Where a process is listed.
#[derive(Debug, Default)]
pub enum Place {
    #[default]
    Nowhere,
    /// Among the apps.
    Apps,
    /// At the top of the processes that are not part of an app.
    Top,
    Under(glib::WeakRef<Process>),
}

impl Place {
    pub fn is(&self, other: &Place) -> bool {
        match (self, other) {
            (Place::Nowhere, Place::Nowhere)
            | (Place::Apps, Place::Apps)
            | (Place::Top, Place::Top) => true,
            (Place::Under(a), Place::Under(b)) => a
                .upgrade()
                .is_some_and(|a| b.upgrade().as_ref() == Some(&a)),
            _ => false,
        }
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
