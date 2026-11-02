//! The audio path allocates nothing.
//!
//! `process`, `tick`, `set_tempo`, `set_param` and, for a generator,
//! `note_on`, `note_off` and `set_note_pitch` run on the audio thread. This
//! test binary counts the allocations a thread makes while it is inside an
//! audio section, builds every plugin `port.rs` names from its defaults,
//! sets it up outside the section and then calls each of those inside it.
//! Every plugin and call that allocates is reported at once.

use plugins::{Effect, Generator, NoteOn};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::panic::AssertUnwindSafe;

/// The system allocator, counting what a thread asks of it inside a
/// section.
struct Counting;

thread_local! {
    static INSIDE: Cell<bool> = const { Cell::new(false) };
    static COUNT: Cell<usize> = const { Cell::new(0) };
    static TRACE: Cell<bool> = const { Cell::new(false) };
}

/// Count an allocation inside a section. With `NO_ALLOC_TRACE` set, the
/// first in each section prints where it came from.
fn note() {
    // `try_with`: a thread being torn down has no locals left.
    let inside = INSIDE.try_with(Cell::get).unwrap_or(false);
    if !inside {
        return;
    }
    let first = COUNT
        .try_with(|count| {
            count.set(count.get() + 1);
            count.get() == 1
        })
        .unwrap_or(false);
    if first && TRACE.with(Cell::get) {
        // Printing allocates: step out of the section while it does.
        INSIDE.with(|inside| inside.set(false));
        eprintln!("{}", std::backtrace::Backtrace::force_capture());
        INSIDE.with(|inside| inside.set(true));
    }
}

// A global allocator is unsafe to write; this one only hands on to the
// system's.
#[allow(unsafe_code)]
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note();
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note();
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// How many allocations `run` makes on this thread.
fn allocations(run: impl FnOnce()) -> usize {
    COUNT.with(|count| count.set(0));
    INSIDE.with(|inside| inside.set(true));
    run();
    INSIDE.with(|inside| inside.set(false));
    COUNT.with(Cell::get)
}

/// Plugins and calls known to allocate, with the reason they are let be.
/// Each entry is `(plugin, call, reason)`.
const ALLOWED: &[(&str, &str, &str)] = &[(
    "Fruity Delay 2",
    "set_tempo",
    "a slower tempo makes the line longer than its memory, which is sized \
     for the tempo it has (delay_2's no_wheel_asks_for_memory asks for \
     that); room for FL's slowest, 10 BPM, would be 24 s of line",
)];

const RATE: f32 = 48_000.0;
/// The block lengths the audio section plays, up to the most a host asks.
const BLOCKS: [usize; 5] = [512, 1, 37, 256, 512];

/// Fills a block with something that is not silence.
fn signal(left: &mut [f32], right: &mut [f32], at: &mut u32) {
    for (l, r) in left.iter_mut().zip(right.iter_mut()) {
        *at = at.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let x = (*at >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
        *l = x;
        *r = -0.7 * x;
    }
}

/// What allocated, as "plugin: call (count)".
struct Findings(Vec<(String, &'static str, usize)>);

impl Findings {
    fn check(&mut self, name: &str, call: &'static str, run: impl FnOnce()) {
        let count = allocations(run);
        if count > 0 {
            self.0.push((name.to_owned(), call, count));
        }
    }
}

fn effect(name: &str, mut effect: Box<dyn Effect>, found: &mut Findings) {
    let mut left = vec![0.0f32; 512];
    let mut right = vec![0.0f32; 512];
    let mut at = 1u32;
    effect.set_ppq(96);
    effect.set_sample_rate(RATE);
    effect.set_tempo(120.0);
    signal(&mut left, &mut right, &mut at);
    effect.process(&mut left, &mut right);
    effect.tick(0, 120.0);
    let params: Vec<(usize, f32)> = (0..effect.param_count())
        .map(|index| (index, effect.param(index)))
        .collect();

    found.check(name, "process", || {
        for frames in BLOCKS {
            signal(&mut left[..frames], &mut right[..frames], &mut at);
            effect.process(&mut left[..frames], &mut right[..frames]);
        }
    });
    found.check(name, "set_param", || {
        for &(index, value) in &params {
            effect.set_param(index, value + 1.0);
            effect.set_param(index, value);
        }
    });
    found.check(name, "set_tempo", || {
        effect.set_tempo(97.5);
        effect.set_tempo(120.0);
    });
    found.check(name, "tick", || {
        for tick in 1..48 {
            effect.tick(tick, 120.0);
        }
        let _ = effect.controller_changed();
    });
    found.check(name, "process after", || {
        for frames in BLOCKS {
            signal(&mut left[..frames], &mut right[..frames], &mut at);
            effect.process(&mut left[..frames], &mut right[..frames]);
        }
    });
}

fn generator(name: &str, mut generator: Box<dyn Generator>, found: &mut Findings) {
    let mut left = vec![0.0f32; 512];
    let mut right = vec![0.0f32; 512];
    generator.set_ppq(96);
    generator.set_sample_rate(RATE);
    generator.set_tempo(120.0);
    generator.process(&mut left, &mut right);
    generator.tick(0, 120.0);
    let params: Vec<(usize, f32)> = (0..generator.param_count())
        .map(|index| (index, generator.param(index)))
        .collect();
    let mut play = |generator: &mut Box<dyn Generator>| {
        for frames in BLOCKS {
            left[..frames].fill(0.0);
            right[..frames].fill(0.0);
            generator.process(&mut left[..frames], &mut right[..frames]);
        }
    };

    found.check(name, "process", || play(&mut generator));
    found.check(name, "note_on", || {
        for (voice, key) in [(1, 60), (2, 64), (3, 67), (4, 48)] {
            generator.note_on(NoteOn {
                voice,
                key,
                ..NoteOn::default()
            });
        }
    });
    found.check(name, "process notes", || play(&mut generator));
    found.check(name, "set_note_pitch", || {
        generator.set_note_pitch(1, 250.0);
        generator.set_note_pitch(2, 400.0);
    });
    found.check(name, "set_param", || {
        for &(index, value) in &params {
            generator.set_param(index, value + 1.0);
            generator.set_param(index, value);
        }
    });
    found.check(name, "set_tempo", || {
        generator.set_tempo(97.5);
        generator.set_tempo(120.0);
    });
    found.check(name, "tick", || {
        for tick in 1..48 {
            generator.tick(tick, 120.0);
        }
        let _ = generator.controller_changed();
    });
    found.check(name, "process bent", || play(&mut generator));
    found.check(name, "note_off", || {
        for voice in 1..=4 {
            generator.note_off(voice);
        }
    });
    found.check(name, "process released", || play(&mut generator));
}

#[test]
fn no_plugin_allocates_on_the_audio_thread() {
    let trace = std::env::var_os("NO_ALLOC_TRACE").is_some();
    TRACE.with(|on| on.set(trace));
    let mut found = Findings(Vec::new());
    for (name, _) in plugins::port::PORTS {
        // A plugin that panics is reported with the rest, and the others
        // still run.
        let played = std::panic::catch_unwind(AssertUnwindSafe(|| {
            if let Some(built) = plugins::by_name(name) {
                effect(name, built, &mut found);
            }
            if let Some(built) = plugins::generator_by_name(name) {
                generator(name, built, &mut found);
            }
        }));
        INSIDE.with(|inside| inside.set(false));
        if played.is_err() {
            found.0.push((name.to_string(), "panicked", 0));
        }
    }
    let allowed = |(name, call, _): &&(String, &str, usize)| {
        ALLOWED
            .iter()
            .any(|(plugin, known, _)| plugin == name && known == call)
    };
    let (excused, failed): (Vec<_>, Vec<_>) = found.0.iter().partition(allowed);
    for (name, call, count) in &excused {
        eprintln!("allowed: {name}: {call} ({count})");
    }
    let mut report: Vec<String> = failed
        .iter()
        .map(|(name, call, count)| format!("{name}: {call} ({count})"))
        .collect();
    // An entry for a call that allocates no more comes off the list.
    for (plugin, call, _) in ALLOWED {
        let still = found
            .0
            .iter()
            .any(|(name, known, _)| name == plugin && known == call);
        if !still {
            report.push(format!("{plugin}: {call} is allowed and allocates no more"));
        }
    }
    assert!(
        report.is_empty(),
        "{} calls allocate on the audio thread:\n{}",
        report.len(),
        report.join("\n")
    );
}
