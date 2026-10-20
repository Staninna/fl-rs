//! The worker threads the mixer runs the inserts of one rank on side by
//! side: inserts that send nothing to each other, so that each owns all it
//! touches.
//!
//! One pool serves the process. Its workers wait for a job by spinning a
//! little and then parking, run it with denormals flushed as the engine's
//! own thread does, and say when they are done. [`Pool::run`] hands every
//! worker the same job, runs it on the calling thread too, and returns
//! once every worker is out of it, so the job may borrow from the caller's
//! stack. Nothing here allocates once the pool is up.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, TryLockError};
use std::thread::{self, Thread};

/// How many times a worker checks for the next job before it parks. The
/// inserts of the next rank come a few microseconds after the last ones,
/// and a parked thread takes tens of them to wake.
const SPIN: u32 = 1 << 14;

/// A job as the workers see it: the caller's closure, with its lifetime
/// taken off. [`Pool::run`] waits for every worker to be out of it before
/// the closure can go.
#[derive(Clone, Copy)]
struct Job(*const (dyn Fn() + Sync + 'static));

// SAFETY: the closure is `Sync`, so it may be called from any thread, and
// `Pool::run` keeps it alive while the workers hold the pointer.
#[allow(unsafe_code)]
unsafe impl Send for Job {}

struct Shared {
    /// The job of the current round, `None` between rounds.
    job: Mutex<Option<Job>>,
    /// Counts the rounds. A worker runs the job once each time it moves.
    round: AtomicUsize,
    /// The workers still in the current round.
    running: AtomicUsize,
    /// Whether a worker's call of the job panicked.
    panicked: AtomicBool,
}

/// Worker threads, and a lock that lets one caller at a time hand them a
/// job.
pub(crate) struct Pool {
    shared: Arc<Shared>,
    threads: Vec<Thread>,
    /// Held through a round. A caller that finds it taken runs its job on
    /// its own thread.
    busy: Mutex<()>,
}

impl Pool {
    /// A pool of `workers` threads besides the caller's.
    pub(crate) fn new(workers: usize) -> Self {
        let shared = Arc::new(Shared {
            job: Mutex::new(None),
            round: AtomicUsize::new(0),
            running: AtomicUsize::new(0),
            panicked: AtomicBool::new(false),
        });
        let threads = (0..workers)
            .filter_map(|n| {
                let shared = Arc::clone(&shared);
                let worker = thread::Builder::new().name(format!("fl-rs mix {n}"));
                let worker = worker.spawn(move || flushed(|| work(&shared)));
                worker.ok().map(|handle| handle.thread().clone())
            })
            .collect();
        Pool {
            shared,
            threads,
            busy: Mutex::new(()),
        }
    }

    /// How many threads besides the caller's run a job.
    pub(crate) fn workers(&self) -> usize {
        self.threads.len()
    }

    /// Call `job` on every worker and on this thread, and return once all
    /// of them are done with it. A job shares its work out itself: each
    /// call takes pieces until none are left. When another caller has the
    /// pool, or it has no workers, `job` runs here alone.
    pub(crate) fn run(&self, job: &(dyn Fn() + Sync)) {
        let _round = match self.busy.try_lock() {
            Ok(round) => round,
            Err(TryLockError::Poisoned(round)) => round.into_inner(),
            Err(TryLockError::WouldBlock) => return job(),
        };
        if self.threads.is_empty() {
            return job();
        }
        let shared = &*self.shared;
        #[allow(unsafe_code)]
        // SAFETY: only the lifetime changes. The pointer is taken back out
        // of `shared.job` below, after every worker has left the round,
        // and `job` outlives this call.
        let erased: *const (dyn Fn() + Sync + 'static) = unsafe {
            std::mem::transmute::<*const (dyn Fn() + Sync + '_), *const (dyn Fn() + Sync + 'static)>(
                job,
            )
        };
        *lock(&shared.job) = Some(Job(erased));
        shared.running.store(self.threads.len(), Ordering::Release);
        shared.round.fetch_add(1, Ordering::Release);
        for thread in &self.threads {
            thread.unpark();
        }
        // A panic here waits for the workers too before it goes on up.
        let mine = catch_unwind(AssertUnwindSafe(job));
        while shared.running.load(Ordering::Acquire) != 0 {
            std::hint::spin_loop();
        }
        *lock(&shared.job) = None;
        if let Err(panic) = mine {
            std::panic::resume_unwind(panic);
        }
        if shared.panicked.swap(false, Ordering::AcqRel) {
            panic!("a mixer worker panicked");
        }
    }
}

/// The process's pool, with as many workers as `FL_RS_THREADS` says or one
/// fewer than the processors this process may run on, at most seven.
pub(crate) fn shared() -> &'static Pool {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(|| {
        let asked = std::env::var("FL_RS_THREADS").ok();
        let threads = asked.and_then(|n| n.trim().parse::<usize>().ok());
        let cores = thread::available_parallelism().map_or(1, usize::from);
        let threads = threads.unwrap_or(cores.min(8)).max(1);
        Pool::new(threads - 1)
    })
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Run `body` with denormals flushed to zero, as the engine's own thread
/// mixes.
fn flushed(body: impl FnOnce()) {
    #[allow(unsafe_code)]
    // SAFETY: as in `Engine::process_with`: the flags change only how
    // numbers below f32's normal range round, and they are put back when
    // `body` returns.
    unsafe {
        no_denormals::no_denormals(body);
    }
}

/// A worker's life: wait for a round, run its job, say so, and again.
fn work(shared: &Shared) {
    let mut seen = 0;
    loop {
        let mut spins = 0;
        loop {
            let round = shared.round.load(Ordering::Acquire);
            if round != seen {
                seen = round;
                break;
            }
            if spins < SPIN {
                spins += 1;
                std::hint::spin_loop();
            } else {
                thread::park();
            }
        }
        let job = *lock(&shared.job);
        if let Some(Job(job)) = job {
            #[allow(unsafe_code)]
            // SAFETY: `Pool::run` keeps the closure alive until `running`
            // is back to 0, which this worker's decrement below is part of.
            let job = unsafe { &*job };
            if catch_unwind(AssertUnwindSafe(job)).is_err() {
                shared.panicked.store(true, Ordering::Release);
            }
        }
        shared.running.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_piece_is_taken_once_by_some_thread() {
        let pool = Pool::new(3);
        let mut pieces = [0u32; 100];
        for round in 1..=50 {
            let left = Mutex::new(pieces.iter_mut());
            pool.run(&|| {
                while let Some(piece) = lock(&left).next() {
                    *piece += 1;
                }
            });
            assert!(pieces.iter().all(|n| *n == round));
        }
    }

    #[test]
    fn a_panic_in_a_job_reaches_the_caller_and_the_pool_goes_on() {
        let pool = Pool::new(2);
        let calls = AtomicUsize::new(0);
        let failed = catch_unwind(AssertUnwindSafe(|| {
            pool.run(&|| {
                if calls.fetch_add(1, Ordering::SeqCst) == 1 {
                    panic!("once");
                }
            })
        }));
        assert!(failed.is_err());
        let done = AtomicUsize::new(0);
        pool.run(&|| {
            done.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(done.load(Ordering::SeqCst), 3);
    }
}
