//! Time every built effect and generator on input made here, with no FL
//! files, so CI can catch a plugin that got slower.
//!
//! Usage: cargo run --release -p plugins --example bench -- [options] [filter]
//!
//!   --seconds N   whole seconds of sound per round (1)
//!   --rounds N    timed rounds per plugin (5)
//!   --write FILE  write the run to FILE, as the baseline to check against
//!   --check FILE  compare the run with the baseline in FILE, and fail
//!                 where a plugin got more than TOLERANCE times slower or
//!                 panics where it did not
//!   filter        keep the plugins whose name contains it, in any case
//!
//! Every plugin in `port.rs` is built from its default state, as
//! `by_name` and `generator_by_name` make it: no project, no preset and no
//! resource directory. Some of FL's plugins sound only with a file of
//! FL's, so they play silent here; the line says so, and their time is
//! still the time of what they do without it. A plugin that panics is
//! skipped and listed at the end.
//!
//! At 44100 Hz in calls of 512 frames, an effect is handed a stereo signal
//! of noise and two sines, and a generator plays a fixed pattern: a note
//! every quarter second and a three-note chord every second. Each round
//! plays the same seconds of it, so each call does the same work in every
//! round. After one untimed round to warm up, the time is the sum of each
//! call's fastest over the timed rounds, which a busy machine moves little.
//!
//! Machines differ, so a time alone says little. Right before each plugin
//! the bench times a reference workload of its own, four biquads and a
//! `tanh` a channel over the same signal, and divides the plugin's time by
//! it. That cost, in reference units, is what the baseline keeps and the
//! check compares. A line per plugin: how many times faster than realtime
//! it plays, nanoseconds per stereo frame, its cost, and its kind and
//! name. After them each plugin comes again as tab-separated fields after
//! `bench`, the baseline's format.
//!
//! With `--check`, a plugin whose cost is over TOLERANCE times its
//! baseline plus SLACK is timed again, up to RETRIES times, and the run
//! fails when it stays over. A plugin more than TOLERANCE times faster
//! than its baseline is listed, so the baseline can be written again.
use std::collections::BTreeMap;
use std::hint::black_box;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use plugins::port::PORTS;
use plugins::{Effect, Generator, NoteOn};

const RATE: usize = 44_100;
const BLOCK: usize = 512;
const TEMPO: f64 = 120.0;
const PPQ: u16 = 96;

/// How many times its baseline a plugin's cost may grow before the check
/// fails. CI runners are not the machine the baseline was taken on, and a
/// plugin heavy on `sin` or on memory moves against the reference by more
/// than one that is all multiplies, so this is generous: it catches a
/// plugin that does more than twice the work, not one a tenth slower.
const TOLERANCE: f64 = 2.0;

/// What a cost may grow by on top of that, in reference units, so the
/// plugins that cost next to nothing do not fail on noise.
const SLACK: f64 = 0.05;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Effect,
    Generator,
}

impl Kind {
    fn word(self) -> &'static str {
        match self {
            Kind::Effect => "effect",
            Kind::Generator => "generator",
        }
    }
}

/// A stereo signal that is the same on every run: white noise from a
/// fixed LCG under a 110 Hz and a 1375 Hz sine, at about -6 dBFS.
fn signal(frames: usize) -> (Vec<f32>, Vec<f32>) {
    let mut seed: u32 = 0x1234_5678;
    let mut noise = move || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
    };
    let tau = std::f32::consts::TAU;
    let mut left = Vec::with_capacity(frames);
    let mut right = Vec::with_capacity(frames);
    for at in 0..frames {
        let t = at as f32 / RATE as f32;
        let low = (tau * 110.0 * t).sin();
        let high = (tau * 1375.0 * t).sin();
        left.push(0.3 * low + 0.1 * high + 0.1 * noise());
        right.push(0.3 * low - 0.1 * high + 0.1 * noise());
    }
    (left, right)
}

/// The reference workload: per channel four biquads in a row and a
/// `tanh`, the kind of work most of the plugins do.
struct Reference {
    state: [[f32; 2]; 8],
}

impl Reference {
    const COEFFS: [[f32; 5]; 4] = [
        [0.2929, 0.5858, 0.2929, -0.0, 0.1716],
        [0.9, -1.8, 0.9, -1.79, 0.81],
        [0.5, 0.0, -0.5, -1.2, 0.4],
        [0.1, 0.2, 0.1, -1.1, 0.5],
    ];

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (channel, samples) in [left, right].into_iter().enumerate() {
            for sample in samples.iter_mut() {
                let mut x = *sample;
                for (stage, [b0, b1, b2, a1, a2]) in Self::COEFFS.iter().enumerate() {
                    let [s1, s2] = &mut self.state[channel * 4 + stage];
                    let y = b0 * x + *s1;
                    *s1 = b1 * x - a1 * y + *s2;
                    *s2 = b2 * x - a2 * y;
                    x = y;
                }
                *sample = (x * 1.5).tanh();
            }
        }
    }
}

/// What is timed: a plugin, or the reference.
enum Subject {
    Effect(Box<dyn Effect>),
    Generator(Box<dyn Generator>, Pattern),
    Reference(Reference),
}

/// A note every quarter second and a chord every second, each as long as
/// the gap to the next, so two to five voices sound at once. Events fall
/// on the call that starts at or after their time.
#[derive(Default)]
struct Pattern {
    frame: usize,
    next_voice: u32,
    /// The voices sounding and the frame each is let go at.
    held: Vec<(u32, usize)>,
}

impl Pattern {
    const KEYS: [i16; 4] = [48, 55, 60, 63];
    const CHORD: [i16; 3] = [60, 64, 67];

    fn block(&mut self, generator: &mut dyn Generator) {
        let (start, end) = (self.frame, self.frame + BLOCK);
        self.held.retain(|&(voice, off)| {
            let keep = off >= end;
            if !keep {
                generator.note_off(voice);
            }
            keep
        });
        let quarter = RATE / 4;
        let mut at = start.div_ceil(quarter) * quarter;
        while at < end {
            let step = (at / quarter) % 4;
            let (next_voice, held) = (&mut self.next_voice, &mut self.held);
            let mut play = |key: i16, velocity: f32, length: usize| {
                let voice = *next_voice;
                *next_voice += 1;
                generator.note_on(NoteOn {
                    voice,
                    key,
                    velocity,
                    ..NoteOn::default()
                });
                held.push((voice, at + length));
            };
            play(Self::KEYS[step], 0.8, quarter * 3 / 2);
            if step == 0 {
                for key in Self::CHORD {
                    play(key, 0.6, quarter * 3);
                }
            }
            at += quarter;
        }
        self.frame = end;
    }
}

impl Subject {
    fn prepare(&mut self) {
        match self {
            Subject::Effect(effect) => {
                effect.set_sample_rate(RATE as f32);
                effect.set_tempo(TEMPO);
                effect.set_ppq(PPQ);
            }
            Subject::Generator(generator, _) => {
                generator.set_sample_rate(RATE as f32);
                generator.set_tempo(TEMPO);
                generator.set_ppq(PPQ);
            }
            Subject::Reference(_) => {}
        }
    }

    /// Plays `input` through in calls of `BLOCK` frames, keeps in `best`
    /// the fastest each call has been so far, and gives the loudest
    /// sample that came out.
    fn round(
        &mut self,
        input: &(Vec<f32>, Vec<f32>),
        ticks: &mut u64,
        best: &mut [Duration],
    ) -> f32 {
        let mut left = [0.0f32; BLOCK];
        let mut right = [0.0f32; BLOCK];
        let mut peak = 0.0f32;
        let ticks_per_block = BLOCK as f64 / RATE as f64 * TEMPO / 60.0 * f64::from(PPQ);
        let blocks = input.0.chunks(BLOCK).zip(input.1.chunks(BLOCK));
        for ((in_left, in_right), best) in blocks.zip(best) {
            let len = in_left.len();
            let (left, right) = (&mut left[..len], &mut right[..len]);
            let tick = (*ticks as f64 * ticks_per_block) as u64;
            *ticks += 1;
            // Only the plugin's own calls are timed, not filling its input
            // or reading its output.
            let start;
            match self {
                Subject::Effect(effect) => {
                    left.copy_from_slice(in_left);
                    right.copy_from_slice(in_right);
                    start = Instant::now();
                    effect.tick(tick, TEMPO);
                    effect.process(left, right);
                }
                Subject::Generator(generator, pattern) => {
                    left.fill(0.0);
                    right.fill(0.0);
                    start = Instant::now();
                    pattern.block(generator.as_mut());
                    generator.tick(tick, TEMPO);
                    generator.process(left, right);
                }
                Subject::Reference(reference) => {
                    left.copy_from_slice(in_left);
                    right.copy_from_slice(in_right);
                    start = Instant::now();
                    reference.process(left, right);
                }
            }
            *best = (*best).min(start.elapsed());
            let loudest = left.iter().chain(right.iter()).fold(0.0f32, |m, s| {
                if s.is_finite() {
                    m.max(s.abs())
                } else {
                    f32::INFINITY
                }
            });
            peak = peak.max(black_box(loudest));
        }
        peak
    }
}

/// The time of a round after one untimed one, and the loudest sample of
/// the timed ones. Every round plays the same input, and a generator the
/// same notes, so each call does the same work in each round: the time is
/// the sum of each call's fastest of `rounds`. A call that the system
/// stopped to run something else is slow in one round and not in the
/// others, so a busy machine moves this far less than the fastest round.
fn time(subject: &mut Subject, input: &(Vec<f32>, Vec<f32>), rounds: usize) -> (Duration, f32) {
    let mut ticks = 0;
    let mut best = vec![Duration::MAX; input.0.len().div_ceil(BLOCK)];
    subject.round(input, &mut ticks, &mut best);
    best.fill(Duration::MAX);
    let mut peak = 0.0f32;
    for _ in 0..rounds {
        peak = peak.max(subject.round(input, &mut ticks, &mut best));
    }
    (best.iter().sum(), peak)
}

struct Options {
    seconds: usize,
    rounds: usize,
    write: Option<PathBuf>,
    check: Option<PathBuf>,
    filter: Option<String>,
}

fn options() -> Options {
    let mut options = Options {
        seconds: 1,
        rounds: 5,
        write: None,
        check: None,
        filter: None,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().unwrap_or_else(|| panic!("{arg} wants a value"));
        match arg.as_str() {
            "--seconds" => options.seconds = value().parse().expect("--seconds N"),
            "--rounds" => options.rounds = value().parse().expect("--rounds N"),
            "--write" => options.write = Some(value().into()),
            "--check" => options.check = Some(value().into()),
            _ if arg.starts_with("--") => panic!("unknown option {arg}"),
            _ => options.filter = Some(arg.to_lowercase()),
        }
    }
    options
}

/// One plugin's result.
struct Timed {
    kind: Kind,
    name: &'static str,
    ns_per_frame: f64,
    realtime: f64,
    cost: f64,
    silent: bool,
}

impl Timed {
    fn key(&self) -> String {
        format!("{}\t{}", self.kind.word(), self.name)
    }
}

/// The plugins to time: every name in `PORTS`, as an effect, a generator
/// or both, as `by_name` and `generator_by_name` build it.
fn subjects() -> Vec<(Kind, &'static str)> {
    let mut all = Vec::new();
    for &(name, _) in PORTS {
        if plugins::by_name(name).is_some() {
            all.push((Kind::Effect, name));
        }
        if plugins::generator_by_name(name).is_some() {
            all.push((Kind::Generator, name));
        }
    }
    all.sort_by_key(|&(kind, name)| (kind, name.to_lowercase()));
    all
}

fn make(kind: Kind, name: &str) -> Subject {
    let mut subject = match kind {
        Kind::Effect => Subject::Effect(plugins::by_name(name).unwrap()),
        Kind::Generator => Subject::Generator(
            plugins::generator_by_name(name).unwrap(),
            Pattern::default(),
        ),
    };
    subject.prepare();
    subject
}

/// The input, and the time of the reference.
struct Bench {
    input: (Vec<f32>, Vec<f32>),
    rounds: usize,
    reference: Duration,
}

impl Bench {
    fn new(options: &Options) -> Bench {
        Bench {
            input: signal(options.seconds.max(1) * RATE),
            rounds: options.rounds.max(1),
            reference: Duration::MAX,
        }
    }

    fn frames(&self) -> usize {
        self.input.0.len()
    }

    /// Times the reference again. It is timed right before each plugin,
    /// so a clock that speeds up or slows down over the run moves both
    /// alike.
    fn reference(&mut self) {
        let mut reference = Subject::Reference(Reference {
            state: [[0.0; 2]; 8],
        });
        (self.reference, _) = time(&mut reference, &self.input, self.rounds);
    }

    /// Times one plugin; `None` when it panics.
    fn one(&mut self, kind: Kind, name: &'static str) -> Option<Timed> {
        self.reference();
        let (input, rounds) = (&self.input, self.rounds);
        let (took, peak) = catch_unwind(AssertUnwindSafe(|| {
            let mut subject = make(kind, name);
            time(&mut subject, input, rounds)
        }))
        .ok()?;
        let frames = self.frames() as f64;
        Some(Timed {
            kind,
            name,
            ns_per_frame: took.as_nanos() as f64 / frames,
            realtime: frames / RATE as f64 / took.as_secs_f64(),
            cost: took.as_secs_f64() / self.reference.as_secs_f64(),
            silent: peak == 0.0,
        })
    }
}

fn line(timed: &Timed) -> String {
    format!(
        "{:>9.1}x {:>9.1} ns/frame {:>8.3} ref  {:<9} {}{}",
        timed.realtime,
        timed.ns_per_frame,
        timed.cost,
        timed.kind.word(),
        timed.name,
        if timed.silent { "  (silent)" } else { "" },
    )
}

/// Times every plugin the filter keeps, and lists the ones that panic.
fn run(bench: &mut Bench, options: &Options) -> (Vec<Timed>, Vec<(Kind, &'static str)>) {
    let mut timed = Vec::new();
    let mut skipped = Vec::new();
    let wanted = |name: &str| {
        let filter = options.filter.as_deref();
        filter.is_none_or(|filter| name.to_lowercase().contains(filter))
    };
    for (kind, name) in subjects().into_iter().filter(|(_, name)| wanted(name)) {
        match bench.one(kind, name) {
            Some(entry) => {
                println!("{}", line(&entry));
                timed.push(entry);
            }
            None => skipped.push((kind, name)),
        }
    }
    (timed, skipped)
}

/// Whether a cost is over what its baseline allows.
fn over(cost: f64, was: f64) -> bool {
    cost > was * TOLERANCE + SLACK
}

/// Times again, up to `RETRIES` times, each plugin that came out over its
/// baseline, and keeps its fastest, so that one busy moment on a shared
/// runner does not fail the check.
fn retry(bench: &mut Bench, timed: &mut [Timed], base: &BTreeMap<String, Option<f64>>) {
    for entry in timed.iter_mut() {
        let Some(Some(was)) = base.get(&entry.key()).copied() else {
            continue;
        };
        for _ in 0..RETRIES {
            if !over(entry.cost, was) {
                break;
            }
            let Some(again) = bench.one(entry.kind, entry.name) else {
                break;
            };
            println!("again: {}", line(&again));
            if again.cost < entry.cost {
                *entry = again;
            }
        }
    }
}

/// How many more times a plugin over its baseline is timed before the
/// check fails it.
const RETRIES: usize = 2;

fn tsv(timed: &Timed) -> String {
    format!(
        "{}\t{:.4}\t{:.1}\t{:.1}\t{}",
        timed.key(),
        timed.cost,
        timed.ns_per_frame,
        timed.realtime,
        if timed.silent { "silent" } else { "sound" },
    )
}

/// What the baseline holds for a plugin that panics on its default state.
const PANICS: &str = "panics";

const WRITE: &str =
    "cargo run --release -p plugins --example bench -- --write crates/plugins/bench.tsv";

/// The baseline's costs by kind and name; `None` for a plugin that
/// panicked when it was written.
fn baseline(text: &str) -> BTreeMap<String, Option<f64>> {
    let rows = text.lines().filter(|line| !line.starts_with('#'));
    rows.filter_map(|line| {
        let fields: Vec<&str> = line.split('\t').collect();
        let cost = match *fields.get(2)? {
            PANICS => None,
            cost => Some(cost.parse().ok()?),
        };
        Some((format!("{}\t{}", fields[0], fields[1]), cost))
    })
    .collect()
}

/// Compares the run with the baseline and says whether every plugin is
/// within it.
fn check(
    timed: &[Timed],
    skipped: &[(Kind, &str)],
    base: &BTreeMap<String, Option<f64>>,
    filtered: bool,
) -> bool {
    let key = |kind: Kind, name: &str| format!("{}\t{name}", kind.word());
    let mut slower = Vec::new();
    let mut faster = Vec::new();
    for timed in timed {
        let name = format!("{} {}", timed.kind.word(), timed.name);
        let was = match base.get(&key(timed.kind, timed.name)) {
            None => {
                println!("new: {name} has no baseline");
                continue;
            }
            Some(None) => {
                println!("fixed: {name} panicked in the baseline and runs now");
                continue;
            }
            Some(Some(was)) => *was,
        };
        let line = format!(
            "{name}: {:.3} ref, baseline {was:.3} ({:+.0}%)",
            timed.cost,
            (timed.cost / was - 1.0) * 100.0
        );
        if over(timed.cost, was) {
            slower.push(line);
        } else if timed.cost < (was - SLACK) / TOLERANCE {
            faster.push(line);
        }
    }
    let broken: Vec<String> = skipped
        .iter()
        .filter(|&&(kind, name)| base.get(&key(kind, name)) != Some(&None))
        .map(|&(kind, name)| format!("{} {name}", kind.word()))
        .collect();
    if !filtered {
        let run = timed.iter().map(|timed| (timed.kind, timed.name));
        let run: Vec<String> = run
            .chain(skipped.iter().copied())
            .map(|(kind, name)| key(kind, name))
            .collect();
        for gone in base.keys().filter(|known| !run.contains(known)) {
            println!(
                "gone: {} is in the baseline but not run",
                gone.replace('\t', " ")
            );
        }
    }
    for line in &faster {
        println!("faster: {line}");
    }
    for line in &slower {
        println!("SLOWER: {line}");
    }
    for name in &broken {
        println!("PANICS: {name} panics on its default state and did not before");
    }
    println!(
        "{} plugins checked against the baseline: {} over {TOLERANCE}x slower, {} over {TOLERANCE}x faster, {} newly panicking.",
        timed.len(),
        slower.len(),
        faster.len(),
        broken.len(),
    );
    if !faster.is_empty() || !slower.is_empty() {
        println!("Where the change is meant, write the baseline again: {WRITE}");
    }
    slower.is_empty() && broken.is_empty()
}

fn main() -> ExitCode {
    let options = options();
    // A plugin that panics is listed and skipped; one line says why.
    std::panic::set_hook(Box::new(|info| eprintln!("panic: {info}")));
    // The app plays with denormals flushed to zero, so the bench does too.
    #[allow(unsafe_code)]
    // SAFETY: the flags change only how numbers below f32's normal range
    // round, nothing the plugins rely on, and they are put back as they
    // were when the closure returns or unwinds.
    let mut bench = unsafe { no_denormals::no_denormals(|| Bench::new(&options)) };
    #[allow(unsafe_code)]
    // SAFETY: as above.
    let (mut timed, skipped) = unsafe { no_denormals::no_denormals(|| run(&mut bench, &options)) };
    let base = options.check.as_ref().map(|path| {
        let text = std::fs::read_to_string(path).expect("read the baseline");
        baseline(&text)
    });
    if let Some(base) = &base {
        #[allow(unsafe_code)]
        // SAFETY: as above.
        unsafe {
            no_denormals::no_denormals(|| retry(&mut bench, &mut timed, base))
        };
    }
    for timed in &timed {
        println!("bench\t{}", tsv(timed));
    }
    let silent = timed.iter().filter(|timed| timed.silent).count();
    println!(
        "{} plugins timed at {RATE} Hz in calls of {BLOCK} frames, {silent} of them silent on their default state.",
        timed.len(),
    );
    for (kind, name) in &skipped {
        println!(
            "skipped: {} {name} panics on its default state",
            kind.word()
        );
    }
    if let Some(path) = &options.write {
        let mut text = format!(
            "# The plugin bench's baseline. Written by: {WRITE}\n\
             # kind\tname\tcost in reference units\tns per frame\ttimes realtime\tsound\n",
        );
        for timed in &timed {
            text.push_str(&tsv(timed));
            text.push('\n');
        }
        for (kind, name) in &skipped {
            text.push_str(&format!("{}\t{name}\t{PANICS}\n", kind.word()));
        }
        std::fs::write(path, text).expect("write the baseline");
        println!("wrote {}", path.display());
    }
    if let Some(base) = &base
        && !check(&timed, &skipped, base, options.filter.is_some())
    {
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
