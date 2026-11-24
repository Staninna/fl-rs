//! Threads a host lends a plugin for one block's work, so that a plugin
//! whose voices cost more than a block lasts can render them side by side.
//!
//! The host hands a generator a [`Workers`] with
//! [`Generator::set_workers`](crate::Generator::set_workers). A plugin then
//! renders its voices with [`for_each`], each voice into a buffer of its
//! own, and sums the buffers in its voice order afterwards, so that what
//! it plays is the same to the bit as when one thread renders every voice.
//! Without workers, `for_each` is a plain loop on the calling thread.
//!
//! Nothing here allocates, on any platform: `std::sync::Mutex` is a
//! pthread mutex boxed on its first lock on macOS, so the items are handed
//! out under a [`Spin`] lock instead.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, Ordering};

/// Threads that take part in one job beside the caller's.
pub trait Workers: Sync {
    /// How many threads besides the caller's could take part in a job.
    /// 0 makes every job run on the caller's thread alone.
    fn helpers(&self) -> usize;

    /// Call `job` on this thread and on up to `helpers` other threads at
    /// once, and return once every call of it is over. A job shares its
    /// work out itself: each call takes pieces until none are left, so a
    /// call that finds nothing left returns at once. The other threads may
    /// be busy: then fewer calls, or only the caller's, do the work. This
    /// must not allocate, and may be called from any thread, one of the
    /// workers included.
    fn run(&self, helpers: usize, job: &(dyn Fn() + Sync));
}

/// Call `each` on every item of `items`, of which there are `count`, on
/// the workers when there are any and at least `least` (and 2) items, and
/// on this thread one after the other otherwise. The items may be done in
/// any order and on any thread, so `each` must touch nothing but its item
/// and what it only reads.
pub fn for_each<T, I>(
    workers: Option<&dyn Workers>,
    items: (I, usize),
    least: usize,
    each: impl Fn(T) + Sync,
) where
    I: Iterator<Item = T> + Send,
    T: Send,
{
    for_each_with(workers, items, least, || (), |(), item| each(item));
}

/// [`for_each`], where every thread that takes part first makes itself
/// something to work with by `start` (a scratch buffer of its own, say),
/// and hands it to `each` with every item it takes. At most
/// [`Workers::helpers`] + 1 threads call `start` at once.
pub fn for_each_with<S, T, I>(
    workers: Option<&dyn Workers>,
    (items, count): (I, usize),
    least: usize,
    start: impl Fn() -> S + Sync,
    each: impl Fn(&mut S, T) + Sync,
) where
    I: Iterator<Item = T> + Send,
    T: Send,
{
    let helpers = workers
        .map_or(0, |w| w.helpers())
        .min(count.saturating_sub(1));
    match workers {
        Some(workers) if helpers > 0 && count >= least.max(2) => {
            let items = Spin::new(items);
            workers.run(helpers, &|| {
                let mut own = None;
                loop {
                    let item = items.with(Iterator::next);
                    let Some(item) = item else { break };
                    each(own.get_or_insert_with(&start), item);
                }
            });
        }
        _ => {
            let mut own = None;
            for item in items {
                each(own.get_or_insert_with(&start), item);
            }
        }
    }
}

/// A lock that spins, for the moment it takes to step an iterator. It
/// allocates nothing, where `std::sync::Mutex` allocates on its first lock
/// on some platforms.
struct Spin<T> {
    locked: AtomicBool,
    value: UnsafeCell<T>,
}

// SAFETY: the value is reached only in `with`, by one thread at a time,
// and moves between threads with the lock, so `T: Send` is enough.
#[allow(unsafe_code)]
unsafe impl<T: Send> Sync for Spin<T> {}

impl<T> Spin<T> {
    fn new(value: T) -> Self {
        Spin {
            locked: AtomicBool::new(false),
            value: UnsafeCell::new(value),
        }
    }

    /// Run `body` on the value with the lock held.
    fn with<R>(&self, body: impl FnOnce(&mut T) -> R) -> R {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            std::hint::spin_loop();
        }
        /// Lets the lock go however `body` ends, a panic included.
        struct Unlock<'a>(&'a AtomicBool);
        impl Drop for Unlock<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _unlock = Unlock(&self.locked);
        #[allow(unsafe_code)]
        // SAFETY: the lock is held, so no other thread has the value until
        // `_unlock` lets it go after `body`.
        let value = unsafe { &mut *self.value.get() };
        body(value)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Workers made of scoped threads spawned for each job: slow, and
    /// they allocate, but they run a job on other threads as a host's do.
    pub(crate) struct Spawned(pub usize);

    impl Workers for Spawned {
        fn helpers(&self) -> usize {
            self.0
        }

        fn run(&self, helpers: usize, job: &(dyn Fn() + Sync)) {
            std::thread::scope(|scope| {
                for _ in 0..helpers.min(self.0) {
                    scope.spawn(job);
                }
                job();
            });
        }
    }

    /// The workers the plugin tests hand out: three threads.
    pub(crate) static THREE: Spawned = Spawned(3);

    #[test]
    fn every_item_is_done_once_with_and_without_workers() {
        for workers in [None, Some(&THREE as &dyn Workers)] {
            let mut items = [0u32; 50];
            let calls = AtomicUsize::new(0);
            for_each(workers, (items.iter_mut(), 50), 2, |n| {
                *n += 1;
                calls.fetch_add(1, Ordering::Relaxed);
            });
            assert!(items.iter().all(|n| *n == 1));
            assert_eq!(calls.load(Ordering::Relaxed), 50);
        }
    }
}
