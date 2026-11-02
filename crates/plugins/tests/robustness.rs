//! Every built plugin, from its default state, driven the ways a host may
//! drive it: odd block lengths, rates changed mid-play, a reset, the tempo
//! moving, every parameter at its ends. None may panic or play anything
//! but finite samples.
//!
//! Each plugin runs under `catch_unwind`, so one run lists every plugin
//! that fails, not only the first.
use std::panic::{AssertUnwindSafe, catch_unwind};

use plugins::port::PORTS;
use plugins::{Effect, Generator, NoteOn};

const BLOCKS: [usize; 7] = [0, 1, 7, 511, 512, 513, 4096];
const RATES: [f32; 4] = [22_050.0, 44_100.0, 48_000.0, 96_000.0];

/// A plugin as either kind, so one driver plays both.
enum Plugin {
    Effect(Box<dyn Effect>),
    Generator(Box<dyn Generator>),
}

impl Plugin {
    fn set_sample_rate(&mut self, hz: f32) {
        match self {
            Plugin::Effect(e) => e.set_sample_rate(hz),
            Plugin::Generator(g) => g.set_sample_rate(hz),
        }
    }

    fn set_tempo(&mut self, bpm: f64) {
        match self {
            Plugin::Effect(e) => e.set_tempo(bpm),
            Plugin::Generator(g) => g.set_tempo(bpm),
        }
    }

    fn set_ppq(&mut self, ppq: u16) {
        match self {
            Plugin::Effect(e) => e.set_ppq(ppq),
            Plugin::Generator(g) => g.set_ppq(ppq),
        }
    }

    fn reset(&mut self) {
        match self {
            Plugin::Effect(e) => e.reset(),
            Plugin::Generator(g) => g.reset(),
        }
    }

    fn param_count(&self) -> usize {
        match self {
            Plugin::Effect(e) => e.param_count(),
            Plugin::Generator(g) => g.param_count(),
        }
    }

    fn param(&self, index: usize) -> f32 {
        match self {
            Plugin::Effect(e) => e.param(index),
            Plugin::Generator(g) => g.param(index),
        }
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match self {
            Plugin::Effect(e) => e.set_param(index, value),
            Plugin::Generator(g) => g.set_param(index, value),
        }
    }
}

/// A step of a fixed LCG, as a sample in -0.5..0.5.
fn noise(seed: &mut u32) -> f32 {
    *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    (*seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
}

/// Plays a plugin and checks what comes out, keeping the song position.
struct Driver {
    plugin: Plugin,
    tick: u64,
    tempo: f64,
    seed: u32,
    voice: u32,
    left: Vec<f32>,
    right: Vec<f32>,
}

impl Driver {
    /// A driver for `plugin`, told the rate, a tempo and a PPQ.
    fn new(plugin: Plugin, rate: f32) -> Driver {
        let mut driver = Driver {
            plugin,
            tick: 0,
            tempo: 120.0,
            seed: 0x1234_5678,
            voice: 0,
            left: vec![0.0; 4096],
            right: vec![0.0; 4096],
        };
        driver.plugin.set_sample_rate(rate);
        driver.plugin.set_tempo(driver.tempo);
        driver.plugin.set_ppq(96);
        driver
    }

    /// A new note on a generator, with a key that walks the keyboard.
    fn note(&mut self) {
        let voice = self.voice;
        self.voice += 1;
        if let Plugin::Generator(g) = &mut self.plugin {
            g.note_on(NoteOn {
                voice,
                key: 36 + (voice % 48) as i16,
                velocity: 0.8,
                ..NoteOn::default()
            });
        }
    }

    /// Plays `frames` frames and fails on a sample that is not finite.
    fn play(&mut self, frames: usize, what: &str) {
        let mut left = std::mem::take(&mut self.left);
        let mut right = std::mem::take(&mut self.right);
        left.resize(frames.max(left.len()), 0.0);
        right.resize(frames.max(right.len()), 0.0);
        let (l, r) = (&mut left[..frames], &mut right[..frames]);
        match &mut self.plugin {
            Plugin::Effect(e) => {
                for sample in l.iter_mut().chain(r.iter_mut()) {
                    *sample = noise(&mut self.seed);
                }
                e.tick(self.tick, self.tempo);
                e.process(l, r);
            }
            Plugin::Generator(g) => {
                l.fill(0.0);
                r.fill(0.0);
                g.tick(self.tick, self.tempo);
                g.process(l, r);
            }
        }
        let bad = l.iter().chain(r.iter()).find(|s| !s.is_finite());
        if let Some(sample) = bad {
            panic!("{sample} after {what}");
        }
        self.tick += 8;
        self.left = left;
        self.right = right;
    }
}

/// Runs `test` on every plugin, handing it a way to make a fresh one,
/// and fails with the list of those it panicked on.
fn each(test: impl Fn(&dyn Fn() -> Plugin)) {
    let mut failed = Vec::new();
    for &(name, _) in PORTS {
        let effect = || plugins::by_name(name).map(Plugin::Effect);
        let generator = || plugins::generator_by_name(name).map(Plugin::Generator);
        let kinds: [(&str, &dyn Fn() -> Option<Plugin>); 2] =
            [("effect", &effect), ("generator", &generator)];
        for (kind, make) in kinds {
            if make().is_none() {
                continue;
            }
            let make = || make().unwrap();
            let run = catch_unwind(AssertUnwindSafe(|| test(&make)));
            if let Err(error) = run {
                let message = error
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| error.downcast_ref::<&str>().copied())
                    .unwrap_or("?");
                failed.push(format!("{kind} {name}: {message}"));
            }
        }
    }
    assert!(
        failed.is_empty(),
        "{} failed:\n{}",
        failed.len(),
        failed.join("\n")
    );
}

#[test]
fn every_plugin_plays_any_block_length_at_any_rate() {
    each(|make| {
        // Each rate from the start, before anything is played.
        for rate in RATES {
            let mut d = Driver::new(make(), rate);
            d.note();
            for frames in BLOCKS {
                d.play(
                    frames,
                    &format!("{frames} frames from the start at {rate} Hz"),
                );
            }
            // A second more at the lowest rate, where a corner past
            // Nyquist makes a filter that runs away slowly.
            if rate == RATES[0] {
                for _ in 0..rate as usize / 4096 {
                    d.play(4096, &format!("a second at {rate} Hz"));
                }
            }
        }
        // And each rate after another, in the middle of the notes.
        let mut d = Driver::new(make(), 44_100.0);
        d.note();
        for rate in RATES.into_iter().chain(RATES.into_iter().rev()) {
            d.plugin.set_sample_rate(rate);
            for frames in BLOCKS {
                d.play(frames, &format!("{frames} frames at {rate} Hz mid-play"));
            }
            d.note();
        }
        d.plugin.reset();
        d.play(64, "a reset");
        d.note();
        d.play(64, "a note after a reset");
        for tempo in [10.0, 522.0, 60.0, 999.0, 120.0] {
            d.tempo = tempo;
            d.plugin.set_tempo(tempo);
            d.play(128, &format!("a tempo of {tempo}"));
        }
    });
}

#[test]
fn every_plugin_plays_with_every_parameter_at_its_ends() {
    each(|make| {
        let mut d = Driver::new(make(), 44_100.0);
        d.note();
        let count = d.plugin.param_count();
        let saved: Vec<f32> = (0..count).map(|i| d.plugin.param(i)).collect();
        for (i, &old) in saved.iter().enumerate() {
            for value in [f32::MIN, f32::MAX, 0.0, old] {
                d.plugin.set_param(i, value);
                d.play(16, &format!("parameter {i} at {value:e}"));
            }
        }
        for value in [f32::MIN, f32::MAX, 0.0] {
            for i in 0..count {
                d.plugin.set_param(i, value);
            }
            d.play(64, &format!("every parameter at {value:e}"));
            d.note();
        }
        for (i, &value) in saved.iter().enumerate() {
            d.plugin.set_param(i, value);
        }
        d.play(64, "the parameters set back");
    });
}
