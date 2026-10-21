//! A fixed set of threads for every call into Stardust.
//!
//! Stardust keeps a 4 MB scratch arena for each thread that calls into it,
//! and doesn't reclaim it when the thread exits. nu runs a plugin call on a
//! new thread whenever its runner thread is busy, which it is while any
//! stream is open, so calling Stardust from there grows the plugin by about
//! 4 MB per call until it is OOM-killed. Calls run here instead, on at most
//! `size()` threads that live as long as the plugin.

use std::panic::{self, AssertUnwindSafe};
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};
use std::thread;

type Job = Box<dyn FnOnce() + Send + 'static>;

thread_local! {
    static WORKER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// How many workers: one per core, between 2 and 8.
fn size() -> usize {
    thread::available_parallelism().map_or(4, |n| n.get()).clamp(2, 8)
}

fn jobs() -> &'static Mutex<Sender<Job>> {
    static JOBS: OnceLock<Mutex<Sender<Job>>> = OnceLock::new();
    JOBS.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        let rx = std::sync::Arc::new(Mutex::new(rx));
        for i in 0..size() {
            let rx = rx.clone();
            thread::Builder::new()
                .name(format!("stardust {i}"))
                .spawn(move || {
                    WORKER.set(true);
                    loop {
                        let job = rx.lock().expect("pool lock").recv();
                        match job {
                            Ok(job) => job(),
                            Err(_) => return,
                        }
                    }
                })
                .expect("spawn stardust worker");
        }
        Mutex::new(tx)
    })
}

/// Runs `f` on a worker and returns its result, waiting for it. On a worker
/// already, runs it in place, so nested calls can't deadlock. A panic in
/// `f` resumes on this thread.
pub fn run<R: Send>(f: impl FnOnce() -> R + Send) -> R {
    if WORKER.get() {
        return f();
    }
    let (tx, rx) = mpsc::sync_channel(1);
    let job = move || {
        let _ = tx.send(panic::catch_unwind(AssertUnwindSafe(f)));
    };
    let job: Box<dyn FnOnce() + Send + '_> = Box::new(job);
    // SAFETY: the job borrows from this frame, so it must not outlive it.
    // It doesn't: this function returns only after the job has sent its
    // result, and the job sends as its last act. If the job is dropped
    // without running (the pool is gone), recv fails and we panic before
    // returning, while the borrows are still valid.
    let job: Job = unsafe { std::mem::transmute(job) };
    jobs().lock().expect("pool lock").send(job).expect("stardust pool is gone");
    match rx.recv().expect("stardust worker dropped a call") {
        Ok(value) => value,
        Err(payload) => panic::resume_unwind(payload),
    }
}

/// Drops `value` on a worker: releasing a Stardust handle calls into it.
pub fn drop<T: Send>(value: T) {
    run(move || std::mem::drop(value));
}
