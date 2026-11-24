//! A hash of what every state FL ships of a plugin puts out, and the time
//! it took, for work that must keep every bit: run it before and after a
//! change and compare. It is quicker than the engine's profile harness
//! over the whole corpus and covers every preset too.
//!
//! ```text
//! FL_RS_CORPUS=<FL Studio>/Data FL_RS_STATE_HASHES="Pitcher,Harmor" \
//!     cargo test --release -p plugins --lib -- --ignored state_hashes --nocapture
//! ```
//!
//! `FL_RS_STATE_HASHES` names the plugins, comma-separated, as
//! [`crate::by_name`] and [`crate::generator_by_name`] know them.
//! `FL_RS_STATE_SECONDS` (3) is how much each state plays, and
//! `FL_RS_STATE_ROUNDS` (1) how many times, the time being the fastest.
//! Each state, from the plugin's presets and the corpus projects, is
//! loaded at 44.1 kHz and 120 BPM and played in calls of 512 frames: an
//! effect a fixed stereo signal of noise and two sines that falls silent
//! a third of the time, a generator a four-note chord every 20 calls,
//! each held for 15. A line per state: `HASH`, the plugin, the state's
//! number, its file and the FNV-1a hash of its output; then a `TIME` line
//! per plugin with the time its states spent in `process`.
use std::time::{Duration, Instant};

use crate::NoteOn;

const RATE: usize = 44_100;
const BLOCK: usize = 512;

fn setting(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

/// The same signal on every run: noise from a fixed LCG under a 110 Hz and
/// a 1375 Hz sine, silent for every third half second.
fn signal(frames: usize) -> (Vec<f32>, Vec<f32>) {
    let mut seed: u32 = 0x1234_5678;
    let mut noise = move || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
    };
    let tau = std::f32::consts::TAU;
    let (mut left, mut right) = (Vec::new(), Vec::new());
    for at in 0..frames {
        let t = at as f32 / RATE as f32;
        let low = (tau * 110.0 * t).sin();
        let high = (tau * 1375.0 * t).sin();
        let on = if (at / (RATE / 2)) % 3 == 2 { 0.0 } else { 1.0 };
        left.push(on * (0.3 * low + 0.1 * high + 0.1 * noise()));
        right.push(on * (0.3 * low - 0.1 * high + 0.1 * noise()));
    }
    (left, right)
}

/// FNV-1a over the bits of every sample.
fn hash(h: &mut u64, samples: &[f32]) {
    for sample in samples {
        for byte in sample.to_bits().to_le_bytes() {
            *h = (*h ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
    }
}

/// One play of a state: the hash of its output and the time `process`
/// took. None where the plugin turns the state down.
fn play(
    name: &str,
    state: &[u8],
    frames: usize,
    input: &(Vec<f32>, Vec<f32>),
) -> Option<(u64, Duration)> {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut took = Duration::ZERO;
    if let Some(mut effect) = crate::by_name(name) {
        effect.set_sample_rate(RATE as f32);
        effect.set_tempo(120.0);
        effect.set_state(state).ok()?;
        let (mut left, mut right) = input.clone();
        for (l, r) in left.chunks_mut(BLOCK).zip(right.chunks_mut(BLOCK)) {
            let started = Instant::now();
            effect.process(l, r);
            took += started.elapsed();
        }
        hash(&mut h, &left);
        hash(&mut h, &right);
    } else {
        let mut generator = crate::generator_by_name(name)?;
        generator.set_sample_rate(RATE as f32);
        generator.set_tempo(120.0);
        generator.set_state(state).ok()?;
        let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
        let mut voice = 0u32;
        let mut held: Vec<(u32, usize)> = Vec::new();
        for (call, (l, r)) in left
            .chunks_mut(BLOCK)
            .zip(right.chunks_mut(BLOCK))
            .enumerate()
        {
            let at = call * BLOCK;
            held.retain(|&(id, off)| {
                let keep = off > at;
                if !keep {
                    generator.note_off(id);
                }
                keep
            });
            if call % 20 == 0 {
                for key in [48i16, 60, 64, 67] {
                    generator.note_on(NoteOn {
                        voice,
                        key: key + (call / 20 % 5) as i16,
                        velocity: 0.8,
                        ..NoteOn::default()
                    });
                    held.push((voice, at + BLOCK * 15));
                    voice += 1;
                }
            }
            let started = Instant::now();
            generator.process(l, r);
            took += started.elapsed();
        }
        hash(&mut h, &left);
        hash(&mut h, &right);
    }
    Some((h, took))
}

#[test]
#[ignore]
fn state_hashes() {
    let Ok(names) = std::env::var("FL_RS_STATE_HASHES") else {
        eprintln!("FL_RS_STATE_HASHES unset, nothing to hash");
        return;
    };
    let Some(root) = crate::testing::corpus() else {
        return;
    };
    let frames = RATE * setting("FL_RS_STATE_SECONDS", 3);
    let rounds = setting("FL_RS_STATE_ROUNDS", 1).max(1);
    let input = signal(frames);
    let projects = crate::testing::projects(&root);
    for name in names.split(',') {
        let presets = crate::testing::every_preset(&root, name);
        let ours = |saved: &&crate::testing::Saved| saved.plugin.eq_ignore_ascii_case(name);
        let saved = presets.iter().chain(projects.iter().filter(ours));
        let mut total = Duration::ZERO;
        for (i, saved) in saved.enumerate() {
            let mut best: Option<(u64, Duration)> = None;
            for _ in 0..rounds {
                let Some((h, took)) = play(name, &saved.state, frames, &input) else {
                    break;
                };
                best = Some((h, best.map_or(took, |(_, t)| t.min(took))));
            }
            let Some((h, took)) = best else {
                continue;
            };
            total += took;
            println!("HASH {name}\t{i}\t{}\t{h:016x}", saved.file);
        }
        println!("TIME {name} {:.1} ms", total.as_secs_f64() * 1000.0);
    }
}
