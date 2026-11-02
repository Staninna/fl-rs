//! Best-effort real-time scheduling for the threads that fill the output:
//! the audio callback and the mixer's workers. A promoted thread keeps its
//! turn on the processor when the machine is busy, so a heavy project does
//! not drop out because the UI, another process or a worker took the core.
//!
//! Every call is best-effort: a system that will not grant it (no
//! privilege, an unsupported platform) leaves the thread as it was and
//! playback goes on, a little more at the mercy of the scheduler.

/// Ask the OS to schedule the current thread as a real-time audio thread.
/// Does nothing the OS refuses.
pub(crate) fn promote_current() {
    #[cfg(unix)]
    {
        use thread_priority::{
            RealtimeThreadSchedulePolicy, ThreadPriority, ThreadSchedulePolicy,
            set_thread_priority_and_policy, thread_native_id,
        };
        // SCHED_FIFO near the top, as JACK and PipeWire clients run. The OS
        // caps it to what this process may ask for.
        let _ = set_thread_priority_and_policy(
            thread_native_id(),
            ThreadPriority::Max,
            ThreadSchedulePolicy::Realtime(RealtimeThreadSchedulePolicy::Fifo),
        );
    }
    #[cfg(not(unix))]
    {
        use thread_priority::{ThreadPriority, set_current_thread_priority};
        let _ = set_current_thread_priority(ThreadPriority::Max);
    }
}
