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
//!
//! The same workers also lend a hand to a plugin that renders its voices
//! side by side ([`plugins::Workers`]): such a plugin, on whatever thread
//! it plays, puts its job on a desk of the pool's board, runs it itself,
//! and waits only for the calls of it that other threads have already
//! taken. A worker between rounds, or the caller of [`Pool::run`] while
//! it waits for the workers, takes such a call. Nobody ever waits for a
//! call that has not started, so a plugin playing on a worker in the
//! middle of a round cannot lock the pool up.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
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
    /// The plugins' jobs that are open for any idle thread to join.
    board: Board,
}

/// How many plugins can share their voices out at once. One more waits
/// for no desk: it renders its voices on its own thread.
const DESKS: usize = 8;

/// The low half of [`Desk::calls`]: how many more calls of the job other
/// threads may take.
const LEFT: u64 = 0xffff_ffff;
/// One call running, in the high half of [`Desk::calls`].
const RUNNING: u64 = 1 << 32;

/// One plugin's open job.
struct Desk {
    /// The plugin that has the desk.
    taken: AtomicBool,
    /// The calls the job may still take ([`LEFT`]) and the calls of it
    /// running on other threads ([`RUNNING`]), in one word so that a
    /// thread joins only a job that is still open, and the plugin knows
    /// when the last one is out.
    calls: AtomicU64,
    job: Mutex<Option<Job>>,
    panicked: AtomicBool,
}

struct Board {
    desks: [Desk; DESKS],
    /// How many desks have a job open, so that an idle thread looks at
    /// one word and not at every desk.
    open: AtomicUsize,
}

impl Board {
    fn new() -> Self {
        Board {
            desks: std::array::from_fn(|_| Desk {
                taken: AtomicBool::new(false),
                calls: AtomicU64::new(0),
                job: Mutex::new(None),
                panicked: AtomicBool::new(false),
            }),
            open: AtomicUsize::new(0),
        }
    }

    /// Take one call of an open job and run it. Whether there was one.
    fn help(&self) -> bool {
        if self.open.load(Ordering::Acquire) == 0 {
            return false;
        }
        for desk in &self.desks {
            let mut calls = desk.calls.load(Ordering::Acquire);
            while calls & LEFT != 0 {
                let joined = calls - 1 + RUNNING;
                match desk.calls.compare_exchange_weak(
                    calls,
                    joined,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => {
                        let job = *lock(&desk.job);
                        if let Some(Job(job)) = job {
                            #[allow(unsafe_code)]
                            // SAFETY: `Pool::fan` keeps the closure alive
                            // until no call is running, and this one is
                            // counted until the decrement below.
                            let job = unsafe { &*job };
                            if catch_unwind(AssertUnwindSafe(job)).is_err() {
                                desk.panicked.store(true, Ordering::Release);
                            }
                        }
                        desk.calls.fetch_sub(RUNNING, Ordering::AcqRel);
                        return true;
                    }
                    Err(now) => calls = now,
                }
            }
        }
        false
    }
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
            board: Board::new(),
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
            // A plugin on a worker may be sharing its voices out.
            if !shared.board.help() {
                std::hint::spin_loop();
            }
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

impl plugins::Workers for Pool {
    fn helpers(&self) -> usize {
        self.threads.len()
    }

    fn run(&self, helpers: usize, job: &(dyn Fn() + Sync)) {
        self.fan(helpers, job);
    }
}

impl Pool {
    /// Open `job` to up to `helpers` threads of the pool besides this one,
    /// run it here, and return once every call of it is over. The workers
    /// join it between rounds; with none free, or no desk free, it runs
    /// here alone.
    fn fan(&self, helpers: usize, job: &(dyn Fn() + Sync)) {
        let helpers = helpers.min(self.threads.len());
        let board = &self.shared.board;
        let free = |desk: &&Desk| {
            let taken = &desk.taken;
            let won = taken.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed);
            won.is_ok()
        };
        let Some(desk) = (helpers > 0)
            .then(|| board.desks.iter().find(free))
            .flatten()
        else {
            return job();
        };
        #[allow(unsafe_code)]
        // SAFETY: only the lifetime changes. The pointer is taken back out
        // of the desk below, after no call of it is running and none can
        // start, and `job` outlives this call.
        let erased: *const (dyn Fn() + Sync + 'static) = unsafe {
            std::mem::transmute::<*const (dyn Fn() + Sync + '_), *const (dyn Fn() + Sync + 'static)>(
                job,
            )
        };
        *lock(&desk.job) = Some(Job(erased));
        desk.calls.store(helpers as u64, Ordering::Release);
        board.open.fetch_add(1, Ordering::AcqRel);
        for thread in &self.threads {
            thread.unpark();
        }
        let mine = catch_unwind(AssertUnwindSafe(job));
        // Close the job to latecomers, and wait for those in it.
        desk.calls.fetch_and(!LEFT, Ordering::AcqRel);
        board.open.fetch_sub(1, Ordering::AcqRel);
        while desk.calls.load(Ordering::Acquire) != 0 {
            std::hint::spin_loop();
        }
        *lock(&desk.job) = None;
        let panicked = desk.panicked.swap(false, Ordering::AcqRel);
        desk.taken.store(false, Ordering::Release);
        if let Err(panic) = mine {
            std::panic::resume_unwind(panic);
        }
        if panicked {
            panic!("a voice worker panicked");
        }
    }
}

/// The pool as the generators take it, when it has workers.
pub(crate) fn workers() -> Option<&'static dyn plugins::Workers> {
    let pool = shared();
    (pool.workers() > 0).then_some(pool as &dyn plugins::Workers)
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
    // The workers fill the output beside the audio thread, so they want
    // the same real-time turn on the processor.
    crate::priority::promote_current();
    let mut seen = 0;
    loop {
        let mut spins = 0;
        loop {
            let round = shared.round.load(Ordering::Acquire);
            if round != seen {
                seen = round;
                break;
            }
            if shared.board.help() {
                spins = 0;
            } else if spins < SPIN {
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
    fn a_job_fanned_out_from_inside_a_round_takes_every_piece_once() {
        use plugins::Workers;
        let pool = Pool::new(3);
        let mut pieces = [[0u32; 40]; 6];
        for round in 1..=50 {
            // Each strip of the round fans its own pieces out, as a
            // plugin on a worker does with its voices.
            let strips = Mutex::new(pieces.iter_mut());
            pool.run(&|| {
                while let Some(strip) = lock(&strips).next() {
                    let left = Mutex::new(strip.iter_mut());
                    pool.run_fanned(&left);
                }
            });
            assert!(pieces.iter().flatten().all(|n| *n == round));
            // And from outside a round.
            let left = Mutex::new(pieces.iter_mut().flatten());
            Workers::run(&pool, 3, &|| {
                while let Some(piece) = lock(&left).next() {
                    *piece += 1;
                }
            });
            assert!(pieces.iter().flatten().all(|n| *n == round + 1));
            pieces.iter_mut().flatten().for_each(|n| *n -= 1);
        }
    }

    impl Pool {
        fn run_fanned<'a>(&self, left: &Mutex<impl Iterator<Item = &'a mut u32> + Send>) {
            plugins::Workers::run(self, 3, &|| {
                while let Some(piece) = lock(left).next() {
                    *piece += 1;
                }
            });
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
