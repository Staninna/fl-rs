//! Time every project in the FL Studio corpus the way the app plays it,
//! and fingerprint what it puts out.
//!
//! Usage: FL_RS_CORPUS=<FL's Data> cargo run --release -p engine --example profile -- [options] [filter]
//!
//!   --frames N   frames per call, as the output device asks for them (512)
//!   --warmup N   untimed rounds between the cold round and the timed one (2)
//!   --whole      play each song once from start to end instead of the windows
//!   filter       keep the projects whose path contains it
//!
//! Each project decodes its samples and builds an engine, then plays four
//! windows round and round: 20 s from the start, and 10 s from a quarter,
//! a half and three quarters of the song. It plays them in rounds. The
//! first round is cold: every call of it is timed, as the app first plays
//! the song. The warm-up rounds are not timed. In the last round the first
//! second of each window is warm-up and not timed either.
//!
//! A line per project, slowest first: how many times faster than realtime
//! it plays in the last round, its 99th percentile and slowest call
//! against the time a call lasts, how many calls ran over that, where in
//! the song the slowest fell, the very first call, which the app pays once
//! at play, how fast the cold round played and its slowest call, and a
//! hash of every sample it put out in every round. A change that keeps the
//! sound keeps every hash. Compare hashes taken at the same `--frames`:
//! some plugins work once per block, and a note splits the blocks
//! differently for a different call size, so they sound a little
//! different.
//!
//! With `--features profile` each project also lists its six costliest
//! generators and effect slots: their share of its time, and how many of
//! its calls over budget each took the most of.
//!
//! Pin it to one core and compare medians of three runs: `taskset -c 2`.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use engine::{Engine, Part, Ticks};

const RATE: u32 = 48_000;

/// Where each window starts, as a part of the song, and how long it plays.
const WINDOWS: [(f64, f64); 4] = [(0.0, 20.0), (0.25, 10.0), (0.5, 10.0), (0.75, 10.0)];
const WARM_UP: f64 = 1.0;

fn find(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            find(&path, out);
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("flp"))
        {
            out.push(path);
        }
    }
}

/// FNV-1a over the bits of every sample.
struct Hash(u64);

impl Hash {
    fn new() -> Self {
        Hash(0xcbf2_9ce4_8422_2325)
    }

    /// Frame by frame, left then right, so that how the frames were split
    /// into calls does not change the hash.
    fn add(&mut self, left: &[f32], right: &[f32]) {
        for (l, r) in left.iter().zip(right) {
            for byte in l
                .to_bits()
                .to_le_bytes()
                .into_iter()
                .chain(r.to_bits().to_le_bytes())
            {
                self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
            }
        }
    }
}

struct Timing {
    name: String,
    first: Duration,
    /// Every call of the cold round, and the seconds it played.
    cold: Vec<Duration>,
    cold_played: f64,
    /// Every timed call of the last round, in order.
    calls: Vec<Duration>,
    /// The song second each timed call starts at.
    at: Vec<f64>,
    played: f64,
    hash: u64,
    /// With the `profile` feature: each generator and slot by its name,
    /// with the time it took in every timed call and how many of the calls
    /// over budget it took the most of.
    parts: BTreeMap<String, (Duration, usize)>,
}

struct Options {
    frames: usize,
    warmup: usize,
    whole: bool,
    filter: String,
}

/// Which round a project is playing.
#[derive(Clone, Copy, PartialEq)]
enum Round {
    /// The first: every call is timed.
    Cold,
    /// Not timed.
    Warm,
    /// The last: the calls after each window's warm-up are timed.
    Timed,
}

/// Play `seconds` from `start` in calls of `frames`, timing those the
/// round times.
#[allow(clippy::too_many_arguments)]
fn play(
    engine: &mut Engine,
    start: Ticks,
    seconds: f64,
    round: Round,
    timing: &mut Timing,
    hash: &mut Hash,
    frames: usize,
    label: &dyn Fn(Part) -> String,
) {
    #[cfg(not(feature = "profile"))]
    let _ = label;
    engine.seek(start);
    let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
    // Exactly this many samples whatever the frames.
    let len = (seconds * f64::from(RATE)) as usize;
    let warm = (WARM_UP * f64::from(RATE)) as usize;
    let mut done = 0;
    while done < len {
        let count = frames.min(len - done);
        let (left, right) = (&mut left[..count], &mut right[..count]);
        let at = engine.position();
        let started = Instant::now();
        engine.process(left, right);
        let took = started.elapsed();
        hash.add(left, right);
        if timing.first.is_zero() {
            timing.first = took;
        }
        if round == Round::Cold {
            timing.cold.push(took);
            timing.cold_played += count as f64 / f64::from(RATE);
        } else if round == Round::Timed && done >= warm && count == frames {
            timing.calls.push(took);
            timing.at.push(at as f64);
            timing.played += frames as f64 / f64::from(RATE);
            #[cfg(feature = "profile")]
            {
                let mut call: BTreeMap<String, Duration> = BTreeMap::new();
                for (part, spent) in engine.spent() {
                    *call.entry(label(*part)).or_default() += *spent;
                }
                for (name, spent) in &call {
                    timing.parts.entry(name.clone()).or_default().0 += *spent;
                }
                let budget = Duration::from_secs_f64(frames as f64 / f64::from(RATE));
                if took > budget
                    && let Some((name, _)) = call.iter().max_by_key(|(_, spent)| **spent)
                {
                    timing.parts.entry(name.clone()).or_default().1 += 1;
                }
            }
        }
        done += count;
    }
}

/// Play one project; `None` when the engine turns it down.
fn time(path: &Path, root: &Path, options: &Options) -> Option<Timing> {
    let file = flp::Project::parse(&std::fs::read(path).unwrap()).unwrap();
    let project = model::from_flp(&file);
    let samples = engine::prepare(&project, root).unwrap();
    let mut engine = Engine::new(&project, &samples, RATE).ok()?;
    let ticks = project.song_length_ticks();
    let mut timing = Timing {
        name: String::new(),
        first: Duration::ZERO,
        cold: Vec::new(),
        cold_played: 0.0,
        calls: Vec::new(),
        at: Vec::new(),
        played: 0.0,
        hash: 0,
        parts: BTreeMap::new(),
    };
    let label = |part: Part| match part {
        Part::Generator(channel) => {
            let channel = project.channel(channel);
            let name = channel.and_then(|c| c.plugin_name.as_deref());
            format!("generator {}", name.unwrap_or("?"))
        }
        Part::Slot { insert, slot } => {
            let slots = project.insert_mix(insert).insert().map(|i| &i.slots[..]);
            let slot_of = |slots: &[model::Slot]| {
                let found = slots.iter().find(|s| s.index == i32::from(slot));
                found.and_then(|s| s.plugin_name.clone())
            };
            let name = slots.and_then(slot_of).unwrap_or_else(|| "?".to_owned());
            format!("insert {insert} slot {slot} {name}")
        }
    };
    let mut hash = Hash::new();
    engine.play();
    let frames = options.frames;
    let windows = if options.whole {
        let seconds = engine::song_length(&project, RATE) as f64 / f64::from(RATE);
        vec![(0, seconds)]
    } else {
        engine.set_loop(true);
        let start = |part: f64| (ticks as f64 * part) as Ticks;
        WINDOWS
            .map(|(part, seconds)| (start(part), seconds))
            .to_vec()
    };
    let rounds = std::iter::once(Round::Cold)
        .chain(std::iter::repeat_n(Round::Warm, options.warmup))
        .chain(std::iter::once(Round::Timed));
    for round in rounds {
        for &(start, seconds) in &windows {
            let (timing, hash) = (&mut timing, &mut hash);
            play(
                &mut engine,
                start,
                seconds,
                round,
                timing,
                hash,
                frames,
                &label,
            );
        }
    }
    // Ticks to song seconds, through the tick each call started at.
    let per_tick =
        engine::song_length(&project, RATE) as f64 / f64::from(RATE) / ticks.max(1) as f64;
    timing.at.iter_mut().for_each(|at| *at *= per_tick);
    timing.hash = hash.0;
    Some(timing)
}

fn options() -> Options {
    let mut options = Options {
        frames: 512,
        warmup: 2,
        whole: false,
        filter: String::new(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--frames" => {
                options.frames = args
                    .next()
                    .and_then(|n| n.parse().ok())
                    .expect("--frames N")
            }
            "--warmup" => {
                options.warmup = args
                    .next()
                    .and_then(|n| n.parse().ok())
                    .expect("--warmup N")
            }
            "--whole" => options.whole = true,
            _ => options.filter = arg,
        }
    }
    options
}

fn main() {
    let root = PathBuf::from(std::env::var_os("FL_RS_CORPUS").expect("set FL_RS_CORPUS"));
    let options = options();
    let mut files = Vec::new();
    find(&root, &mut files);
    files.sort();
    let budget = Duration::from_secs_f64(options.frames as f64 / f64::from(RATE));
    let mut timings = Vec::new();
    for path in &files {
        let name = path.strip_prefix(&root).unwrap().display().to_string();
        if !name.contains(&options.filter) {
            continue;
        }
        // A panic, which a dev build's overflow checks raise, is reported
        // and the next project goes on.
        match std::panic::catch_unwind(|| time(path, &root, &options)) {
            Ok(Some(timing)) => timings.push(Timing { name, ..timing }),
            Ok(None) => eprintln!("{name}: no engine"),
            Err(_) => eprintln!("{name}: panicked"),
        }
    }
    let total = |t: &Timing| t.calls.iter().sum::<Duration>();
    timings.sort_by_key(|t| std::cmp::Reverse(total(t)));
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let share = |d: Duration| 100.0 * d.as_secs_f64() / budget.as_secs_f64();
    let mut over_projects = 0;
    for t in &timings {
        let mut sorted = t.calls.clone();
        sorted.sort();
        let p99 = sorted
            .get(sorted.len() * 99 / 100)
            .copied()
            .unwrap_or_default();
        let (worst_at, worst) = t
            .calls
            .iter()
            .enumerate()
            .max_by_key(|(_, d)| **d)
            .map_or((0.0, Duration::ZERO), |(i, d)| (t.at[i], *d));
        let over = t.calls.iter().filter(|d| **d > budget).count();
        over_projects += usize::from(over > 0);
        let cold = t.cold.iter().sum::<Duration>().as_secs_f64().max(1e-9);
        let cold_worst = t.cold.iter().max().copied().unwrap_or_default();
        println!(
            "{:>6.1}x  p99 {:>5.2} ms ({:>3.0}%)  worst {:>6.2} ms ({:>4.0}%) at {:>5.1} s  over {:>4}  first {:>6.2} ms  cold {:>5.1}x worst {:>6.2} ms ({:>4.0}%)  {:016x}  {}",
            t.played / total(t).as_secs_f64().max(1e-9),
            ms(p99),
            share(p99),
            ms(worst),
            share(worst),
            worst_at,
            over,
            ms(t.first),
            t.cold_played / cold,
            ms(cold_worst),
            share(cold_worst),
            t.hash,
            t.name,
        );
        let mut parts: Vec<_> = t.parts.iter().collect();
        parts.sort_by_key(|(_, (spent, _))| std::cmp::Reverse(*spent));
        for (name, (spent, blamed)) in parts.iter().take(6) {
            let share = 100.0 * spent.as_secs_f64() / total(t).as_secs_f64().max(1e-9);
            println!("        {share:>4.1}%  most of {blamed:>4} over  {name}");
        }
    }
    let all: Duration = timings.iter().map(total).sum();
    println!(
        "{} projects at {} frames ({:.2} ms a call), {} warm-up rounds: {over_projects} with a call over, {:.1} s timed",
        timings.len(),
        options.frames,
        ms(budget),
        options.warmup,
        all.as_secs_f64(),
    );
}
