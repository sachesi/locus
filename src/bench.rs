//! What a reading costs, with and without the processes, measured against a load the
//! benchmark starts itself so that numbers from different machines compare: `just bench`.

use crate::sampler::Sampler;
use std::alloc::{GlobalAlloc, Layout};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

const PROCESSES: usize = 500;
/// Files each process of the load holds open, about what a desktop session has in all.
const FILES: usize = 40;
/// Processes of the load that hold a render node open, as clients of a graphics card.
const GPU_CLIENTS: usize = 20;
const READINGS: usize = 40;

static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static ALLOCATED: AtomicU64 = AtomicU64::new(0);

struct Counting;

// SAFETY: every call goes on to the system allocator unchanged.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        ALLOCATED.fetch_add(layout.size() as u64, Ordering::Relaxed);
        // SAFETY: the caller's guarantees for `layout` are the system allocator's.
        unsafe { std::alloc::System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` came from this allocator, which is the system one.
        unsafe { std::alloc::System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        ALLOCATED.fetch_add(new_size as u64, Ordering::Relaxed);
        // SAFETY: as for `dealloc`, and the caller's guarantees for `new_size`.
        unsafe { std::alloc::System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

/// The processes of the load, killed when it is dropped.
struct Load(Vec<Child>);

impl Load {
    fn start() -> Self {
        let render_node = std::fs::read_dir("/dev/dri")
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .find(|path| path.to_string_lossy().contains("renderD"))
            .and_then(|path| {
                std::ffi::CString::new(path.into_os_string().into_encoded_bytes()).ok()
            });
        let children = (0..PROCESSES)
            .map(|i| {
                let gpu = (i < GPU_CLIENTS).then(|| render_node.clone()).flatten();
                let mut command = Command::new("sleep");
                command.arg("600").stdin(Stdio::null());
                // SAFETY: open is async-signal-safe, and the paths were made before the fork.
                unsafe {
                    command.pre_exec(move || {
                        for _ in 0..FILES {
                            libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY);
                        }
                        if let Some(node) = &gpu {
                            libc::open(node.as_ptr(), libc::O_RDWR);
                        }
                        Ok(())
                    });
                }
                command.spawn().expect("start a process of the load")
            })
            .collect();
        Self(children)
    }
}

impl Drop for Load {
    fn drop(&mut self) {
        for child in &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Processor time of the calling thread, in microseconds, the kernel's work on its behalf
/// included.
fn thread_time() -> f64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: clock_gettime writes a timespec into `time`, which lives through the call.
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) };
    time.tv_sec as f64 * 1e6 + time.tv_nsec as f64 / 1e3
}

fn peak_resident_kib() -> i64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: getrusage fills in the whole struct when it returns 0.
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return 0;
    }
    // SAFETY: getrusage returned 0.
    unsafe { usage.assume_init() }.ru_maxrss
}

fn measure(sampler: &mut Sampler, with_processes: bool) {
    let mut times = Vec::with_capacity(READINGS);
    let mut listed = 0;
    let (allocations, allocated) = (
        ALLOCATIONS.load(Ordering::Relaxed),
        ALLOCATED.load(Ordering::Relaxed),
    );
    for _ in 0..READINGS {
        let start = thread_time();
        let snapshot = sampler.sample(with_processes);
        times.push(thread_time() - start);
        listed = snapshot.processes.map_or(0, |p| p.len());
    }
    times.sort_by(f64::total_cmp);
    let readings = READINGS as u64;
    println!(
        "{:<20} {:>9.0} µs median {:>9.0} µs worst {:>8} allocations {:>8} KiB allocated {:>5} processes",
        if with_processes {
            "with processes"
        } else {
            "without processes"
        },
        times[READINGS / 2],
        times[READINGS - 1],
        (ALLOCATIONS.load(Ordering::Relaxed) - allocations) / readings,
        (ALLOCATED.load(Ordering::Relaxed) - allocated) / readings / 1024,
        listed,
    );
}

#[test]
#[ignore = "a benchmark, run by `just bench`"]
fn cost_of_a_reading() {
    let load = Load::start();
    let mut sampler = Sampler::new();
    // The first readings read the installed apps and look up every process's details.
    sampler.sample(true);
    sampler.sample(true);
    println!("per reading, over {READINGS} readings:");
    measure(&mut sampler, true);
    measure(&mut sampler, false);
    println!("peak resident {} MiB", peak_resident_kib() / 1024);
    drop(load);
}
