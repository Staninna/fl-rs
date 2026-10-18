//! Fruity Convolver: the input convolved with an impulse response, as
//! `docs/plugins/fruity-convolver.md` and `Fruity Convolver_x64.dll` have
//! it.
//!
//! # The state
//!
//! An i32 version, 10 to 15, plain or in the high half of a word like
//! `0xf0003`. Then:
//!
//! - four i32: the dry level and the wet level, 0 to 1000, and the input's
//!   and the wet sound's stereo separation, -1000 to 1000;
//! - a byte that is 1 for an impulse in a file, and the file's path: a
//!   byte length, with `0xff` and a u32 for a long one, and that many
//!   8-bit characters;
//! - for an impulse that is not in a file, an i32 length and a WAV file;
//! - 36 bytes of the editor's view;
//! - five envelopes ([`crate::menvelope`]): the four layers the editor
//!   draws over the impulse, and the equaliser's curve;
//! - four bytes, the last of them "normalize";
//! - from version 15 on, an i32 latency in samples;
//! - four i32, and five from version 11 on: the tab that shows, the
//!   pre-delay (0 to 1000), the self-convolution (0 to 1000), the stretch
//!   (-1000 to 1000) and the equaliser's mix.
//!
//! # The impulse
//!
//! A path starts with `%FLStudioData%` or `%FLStudioFactoryData%`, which
//! both mean the install here, and names a WavPack or a WAV file under FL's
//! `Data` directory. [`Effect::set_resource_dir`] says where that is. A
//! file that is not where its path says is looked for by its name under
//! `Data/Patches/Impulses`, which is where FL keeps the impulses its
//! presets save under a path on their maker's own disk.
//!
//! FL makes the impulse ready in this order, and so does this:
//!
//! 1. With normalize on, the impulse is scaled so that the loudest 300 ms
//!    of it has an RMS of 0.05, over both sides.
//! 2. It is resampled to the song's rate times the stretch, which is
//!    `2^(v / 1000)`.
//! 3. A level equaliser curve at `y` scales it by `(3^(2y) - 1) / 2`, which
//!    is 1 at the half it starts at.
//! 4. What is under 2^-24 at its end is cut off, and what is under it at
//!    its start counts as delay, with the pre-delay: `(7^(v / 1000) - 1) /
//!    12` seconds.
//!
//! # The sound
//!
//! The input goes through its stereo separation, is delayed and convolved,
//! one side with the impulse's left and the other with its right, and comes
//! out through the wet level, `(1001^(v / 1000) - 1) / 1000` times
//! `44100 / rate`, and the wet separation, next to the input at the dry
//! level. The convolution is the sum it is in a textbook, worked out the
//! way FL's engine does: the first 128 taps one by one, and the rest in
//! blocks that are four times as long from one group of six to the next,
//! each block through an FFT. No block is late, so the plugin adds no
//! delay. What it says of itself in [`Effect::latency`] is the number in
//! its state, which FL tells its host and does nothing else with.
//!
//! What this does not do as FL does, and [`Effect::fidelity`] says when a
//! state asks for one of them:
//!
//! - An impulse whose file is not found, or is a kind of file that is not
//!   decoded, leaves the plugin out: its input passes as it is.
//! - The four layers, the self-convolution and an equaliser curve that is
//!   not level are left out.
//! - FL's resampler is not read. This one is a windowed sinc.
//! - FL sleeps after as much silence as the impulse is long.

use crate::menvelope::{Class, MEnvelope, Reader};
use crate::parts::{glide, stereo_gains};
use crate::{Effect, Fidelity, StateError, knob_gain, knob_value};
use std::f64::consts::PI;
use std::path::{Component, Path, PathBuf};

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Convolver";

/// The range of each parameter's wheel.
const RANGES: [(i32, i32); 4] = [(0, 1000), (0, 1000), (-1000, 1000), (-1000, 1000)];
const DEFAULTS: [i32; 4] = [1000, 500, 0, 0];

/// The impulse a new Convolver has.
const DEFAULT_PATH: &str = "%FLStudioData%\\Patches\\Impulses\\Default.wav";

/// Where the impulses are under FL's `Data` directory.
const IMPULSES: &str = "Patches/Impulses";

/// The most frames [`Convolver::process`] takes through at once.
const CHUNK: usize = 512;

/// FL's "too small to keep": 2^-24.
const SILENT: f32 = 5.960_464_5e-8;

/// The taps that are summed one by one, which is also the shortest block.
const HEAD: usize = 128;
/// How many blocks of one length there are, how much longer the next
/// ones are and the longest there is.
const BLOCKS: usize = 6;
const GROWTH: usize = 4;
const LONGEST: usize = 32_768;

/// A radix 2 FFT of one size, over a real and an imaginary block.
#[derive(Debug, Clone, PartialEq)]
struct Fft {
    /// The pairs the bit reversal swaps.
    swaps: Vec<(u32, u32)>,
    /// Half a turn of cosines and sines.
    cos: Vec<f32>,
    sin: Vec<f32>,
}

impl Fft {
    /// `size` is a power of 2.
    fn new(size: usize) -> Self {
        let bits = size.trailing_zeros();
        let reversed = |i: usize| i.reverse_bits() >> (usize::BITS - bits);
        let swaps = (0..size).filter(|i| *i < reversed(*i));
        let turn = |k: usize| 2.0 * PI * k as f64 / size as f64;
        Fft {
            swaps: swaps.map(|i| (i as u32, reversed(i) as u32)).collect(),
            cos: (0..size / 2).map(|k| turn(k).cos() as f32).collect(),
            sin: (0..size / 2).map(|k| turn(k).sin() as f32).collect(),
        }
    }

    /// Transform in place, with nothing scaled either way.
    fn run(&self, re: &mut [f32], im: &mut [f32], inverse: bool) {
        let size = self.cos.len() * 2;
        for (a, b) in &self.swaps {
            re.swap(*a as usize, *b as usize);
            im.swap(*a as usize, *b as usize);
        }
        let sign = if inverse { 1.0 } else { -1.0 };
        let (re, im) = (&mut re[..size], &mut im[..size]);
        // Plain loops over indexes: an unoptimised build runs these many
        // times faster than it runs a chain of iterators.
        let mut half = 1;
        while half < size {
            let stride = size / (2 * half);
            let mut start = 0;
            while start < size {
                for k in 0..half {
                    let (low, high) = (start + k, start + k + half);
                    let (cos, sin) = (self.cos[k * stride], self.sin[k * stride] * sign);
                    let turned_re = re[high] * cos - im[high] * sin;
                    let turned_im = re[high] * sin + im[high] * cos;
                    re[high] = re[low] - turned_re;
                    im[high] = im[low] - turned_im;
                    re[low] += turned_re;
                    im[low] += turned_im;
                }
                start += 2 * half;
            }
            half *= 2;
        }
    }
}

/// The first taps over a run: frame `j` of `out` becomes the sum of
/// `taps[k]` times the input `k` frames before it, added up from `k = 0`.
/// Frame `j` of the run is `line[HEAD - 1 + j]`, with the inputs before it
/// below. Sixteen frames are summed side by side, each in that order, so
/// every sum comes out as it does one frame at a time.
fn direct(taps: &[f32], line: &[f32], out: &mut [f32]) {
    const LANES: usize = 16;
    let mut j = 0;
    while j + LANES <= out.len() {
        let mut sums = [0.0f32; LANES];
        for (k, tap) in taps.iter().enumerate() {
            let from = HEAD - 1 + j - k;
            for (sum, input) in sums.iter_mut().zip(&line[from..from + LANES]) {
                *sum += tap * input;
            }
        }
        out[j..j + LANES].copy_from_slice(&sums);
        j += LANES;
    }
    for (j, out) in out.iter_mut().enumerate().skip(j) {
        let mut sum = 0.0;
        for (k, tap) in taps.iter().enumerate() {
            sum += tap * line[HEAD - 1 + j - k];
        }
        *out = sum;
    }
}

/// One group of blocks of one length: where in the impulse it starts and
/// how many blocks it has.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Group {
    size: usize,
    start: usize,
    blocks: usize,
}

/// What one side keeps for one group.
#[derive(Debug, Clone, PartialEq)]
struct Part {
    /// The input of the block that is filling up.
    block: Vec<f32>,
    fill: usize,
    /// The spectra of the last blocks of input, `size + 1` bins each, and
    /// which of them is the newest.
    input_re: Vec<f32>,
    input_im: Vec<f32>,
    newest: usize,
    /// The spectra of the impulse's blocks, scaled for the way back.
    impulse_re: Vec<f32>,
    impulse_im: Vec<f32>,
}

/// One side of the convolution.
#[derive(Debug, Clone, PartialEq)]
struct Side {
    /// The first taps, and the inputs in order: the `HEAD - 1` before the
    /// run being summed, then the run.
    taps: Vec<f32>,
    line: Vec<f32>,
    /// How much of the shortest block has come in.
    fill: usize,
    parts: Vec<Part>,
    /// What the blocks have put out for the frames to come, and the frame
    /// that is next.
    ahead: Vec<f32>,
    next: usize,
}

/// A convolution with an impulse of any length that is never late.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Engine {
    groups: Vec<Group>,
    ffts: Vec<Fft>,
    sides: [Side; 2],
    /// One block's transform, and the sum of a group's products.
    re: Vec<f32>,
    im: Vec<f32>,
    sum_re: Vec<f32>,
    sum_im: Vec<f32>,
}

impl Engine {
    /// The engine for an impulse, one side each. This asks for memory:
    /// about 16 bytes a tap.
    pub(crate) fn new(left: &[f32], right: &[f32]) -> Self {
        let len = left.len().max(right.len());
        let mut groups = Vec::new();
        let (mut start, mut size) = (HEAD, HEAD);
        while start < len {
            let rest = (len - start).div_ceil(size);
            let blocks = if size == LONGEST {
                rest
            } else {
                rest.min(BLOCKS)
            };
            groups.push(Group {
                size,
                start,
                blocks,
            });
            start += blocks * size;
            size = (size * GROWTH).min(LONGEST);
        }
        let ffts: Vec<Fft> = groups.iter().map(|g| Fft::new(2 * g.size)).collect();
        let longest = groups.last().map_or(0, |group| 2 * group.size);
        let (mut re, mut im) = (vec![0.0; longest], vec![0.0; longest]);
        // The frames a block's output reaches past the one that is next.
        let reach = groups.iter().map(|g| g.start + g.size).max().unwrap_or(0);
        let ahead = (reach + 1).next_power_of_two();
        let mut side = |impulse: &[f32]| {
            let mut parts = Vec::with_capacity(groups.len());
            for (group, fft) in groups.iter().zip(&ffts) {
                let bins = group.size + 1;
                let scale = 1.0 / (2 * group.size) as f32;
                let mut part = Part {
                    block: vec![0.0; group.size],
                    fill: 0,
                    input_re: vec![0.0; group.blocks * bins],
                    input_im: vec![0.0; group.blocks * bins],
                    newest: 0,
                    impulse_re: Vec::with_capacity(group.blocks * bins),
                    impulse_im: Vec::with_capacity(group.blocks * bins),
                };
                let (re, im) = (&mut re[..2 * group.size], &mut im[..2 * group.size]);
                for block in 0..group.blocks {
                    let from = (group.start + block * group.size).min(impulse.len());
                    let taps = &impulse[from..(from + group.size).min(impulse.len())];
                    re[..taps.len()].copy_from_slice(taps);
                    re[taps.len()..].fill(0.0);
                    im.fill(0.0);
                    fft.run(re, im, false);
                    part.impulse_re.extend(re[..bins].iter().map(|v| v * scale));
                    part.impulse_im.extend(im[..bins].iter().map(|v| v * scale));
                }
                parts.push(part);
            }
            Side {
                taps: impulse[..HEAD.min(impulse.len())].to_vec(),
                line: vec![0.0; 2 * HEAD],
                fill: 0,
                parts,
                ahead: vec![0.0; ahead],
                next: 0,
            }
        };
        let sides = [side(left), side(right)];
        let bins = longest / 2 + 1;
        Engine {
            groups,
            ffts,
            sides,
            re,
            im,
            sum_re: vec![0.0; bins],
            sum_im: vec![0.0; bins],
        }
    }

    /// Forget the input so far.
    pub(crate) fn reset(&mut self) {
        for side in &mut self.sides {
            side.line.fill(0.0);
            side.ahead.fill(0.0);
            (side.fill, side.next) = (0, 0);
            for part in &mut side.parts {
                part.block.fill(0.0);
                part.input_re.fill(0.0);
                part.input_im.fill(0.0);
                (part.fill, part.newest) = (0, 0);
            }
        }
    }

    /// Convolve a block of one side with that side's impulse, in place.
    /// This allocates nothing.
    pub(crate) fn process(&mut self, side: usize, block: &mut [f32]) {
        let Engine {
            groups,
            ffts,
            sides,
            re,
            im,
            sum_re,
            sum_im,
        } = self;
        let side = &mut sides[side];
        let mask = side.ahead.len() - 1;
        let mut at = 0;
        while at < block.len() {
            let run = (HEAD - side.fill).min(block.len() - at);
            let chunk = &mut block[at..at + run];
            for part in &mut side.parts {
                part.block[part.fill..part.fill + run].copy_from_slice(chunk);
                part.fill += run;
            }
            side.line[HEAD - 1..HEAD - 1 + run].copy_from_slice(chunk);
            direct(&side.taps, &side.line, chunk);
            for sample in chunk.iter_mut() {
                *sample += side.ahead[side.next];
                side.ahead[side.next] = 0.0;
                side.next = (side.next + 1) & mask;
            }
            side.line.copy_within(run..run + HEAD - 1, 0);
            side.fill += run;
            at += run;
            if side.fill < HEAD {
                continue;
            }
            side.fill = 0;
            for ((group, fft), part) in groups.iter().zip(ffts.iter()).zip(&mut side.parts) {
                if part.fill < group.size {
                    continue;
                }
                part.fill = 0;
                let (size, bins) = (group.size, group.size + 1);
                let (re, im) = (&mut re[..2 * size], &mut im[..2 * size]);
                re[..size].copy_from_slice(&part.block);
                re[size..].fill(0.0);
                im.fill(0.0);
                fft.run(re, im, false);
                part.newest = (part.newest + group.blocks - 1) % group.blocks;
                let slot = part.newest * bins;
                part.input_re[slot..slot + bins].copy_from_slice(&re[..bins]);
                part.input_im[slot..slot + bins].copy_from_slice(&im[..bins]);

                // Every block of the impulse times the input that is as
                // many blocks old.
                let (sum_re, sum_im) = (&mut sum_re[..bins], &mut sum_im[..bins]);
                sum_re.fill(0.0);
                sum_im.fill(0.0);
                for block in 0..group.blocks {
                    let input = ((part.newest + block) % group.blocks) * bins;
                    let impulse = block * bins;
                    let x_re = &part.input_re[input..input + bins];
                    let x_im = &part.input_im[input..input + bins];
                    let h_re = &part.impulse_re[impulse..impulse + bins];
                    let h_im = &part.impulse_im[impulse..impulse + bins];
                    for bin in 0..bins {
                        sum_re[bin] += x_re[bin] * h_re[bin] - x_im[bin] * h_im[bin];
                        sum_im[bin] += x_re[bin] * h_im[bin] + x_im[bin] * h_re[bin];
                    }
                }
                // The other half of the spectrum mirrors the first.
                re[..bins].copy_from_slice(sum_re);
                im[..bins].copy_from_slice(sum_im);
                for bin in 1..size {
                    re[2 * size - bin] = sum_re[bin];
                    im[2 * size - bin] = -sum_im[bin];
                }
                fft.run(re, im, true);
                // The block started `size` frames ago, and its output
                // starts `start` frames after that.
                let first = side.next + group.start - size;
                for (i, value) in re.iter().enumerate() {
                    side.ahead[(first + i) & mask] += value;
                }
            }
        }
    }
}

/// An impulse as its file has it.
#[derive(Debug, Clone, PartialEq)]
struct Impulse {
    left: Vec<f32>,
    right: Vec<f32>,
    rate: u32,
}

/// The blocks of a WavPack file, without what follows the last of them:
/// one file FL ships ends in a tag.
fn wavpack_blocks(bytes: &[u8]) -> &[u8] {
    let mut at = 0;
    while let Some(header) = bytes.get(at..at + 8).filter(|h| h.starts_with(b"wvpk")) {
        let size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
        match at.checked_add(8).and_then(|body| body.checked_add(size)) {
            Some(end) if end <= bytes.len() => at = end,
            _ => return bytes,
        }
    }
    &bytes[..at]
}

/// Decode a WavPack file or a WAV file of plain samples. `None` for
/// anything else: a hybrid WavPack file, or a WAV file with a codec in it.
fn decode(bytes: &[u8]) -> Option<Impulse> {
    let (samples, channels, rate) = if bytes.starts_with(b"wvpk") {
        let stream = wavicle::decode_stream(wavpack_blocks(bytes)).ok()?;
        let full_scale = (1u64 << (stream.bits_per_sample.clamp(1, 32) - 1)) as f32;
        let sample = |value: &i32| {
            if stream.is_float {
                f32::from_bits(value.cast_unsigned())
            } else {
                *value as f32 / full_scale
            }
        };
        let samples: Vec<f32> = stream.samples.iter().map(sample).collect();
        (samples, stream.channels as usize, stream.sample_rate)
    } else {
        wav(bytes)?
    };
    if channels == 0 || rate == 0 {
        return None;
    }
    let side = |skip: usize| {
        samples
            .iter()
            .skip(skip)
            .step_by(channels)
            .copied()
            .collect()
    };
    let left: Vec<f32> = side(0);
    // FL makes two sides of a mono impulse.
    let right = if channels > 1 { side(1) } else { left.clone() };
    Some(Impulse { left, right, rate })
}

/// The samples of a WAV file, its channels and its rate.
fn wav(bytes: &[u8]) -> Option<(Vec<f32>, usize, u32)> {
    if !(bytes.starts_with(b"RIFF") && bytes.get(8..12)? == b"WAVE") {
        return None;
    }
    let word = |at: usize| Some(u32::from_le_bytes(*bytes.get(at..at + 4)?.first_chunk()?));
    let half = |at: usize| Some(u16::from_le_bytes(*bytes.get(at..at + 2)?.first_chunk()?));
    let mut format = None;
    let mut at = 12;
    loop {
        let id = bytes.get(at..at + 4)?;
        let size = word(at + 4)? as usize;
        let body = at + 8;
        if id == b"fmt " {
            let mut tag = half(body)?;
            if tag == 0xfffe {
                // The real tag is the start of the sub format.
                tag = half(body + 24)?;
            }
            format = Some((tag, half(body + 2)?, word(body + 4)?, half(body + 14)?));
        } else if id == b"data" {
            let (tag, channels, rate, bits) = format?;
            // A file cut short keeps what it has.
            let data = bytes.get(body..)?;
            let data = &data[..size.min(data.len())];
            let samples: Vec<f32> = match (tag, bits) {
                (1, 8) => data
                    .iter()
                    .map(|b| (f32::from(*b) - 128.0) / 128.0)
                    .collect(),
                (1, 16) => {
                    let (pairs, _) = data.as_chunks::<2>();
                    let value = |b: &[u8; 2]| f32::from(i16::from_le_bytes(*b)) / 32_768.0;
                    pairs.iter().map(value).collect()
                }
                (1, 24) => {
                    let (triples, _) = data.as_chunks::<3>();
                    let value = |b: &[u8; 3]| i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8;
                    triples
                        .iter()
                        .map(|b| value(b) as f32 / 8_388_608.0)
                        .collect()
                }
                (1, 32) => {
                    let (words, _) = data.as_chunks::<4>();
                    let value = |b: &[u8; 4]| i32::from_le_bytes(*b) as f32 / 2_147_483_648.0;
                    words.iter().map(value).collect()
                }
                (3, 32) => {
                    let (words, _) = data.as_chunks::<4>();
                    words.iter().map(|b| f32::from_le_bytes(*b)).collect()
                }
                _ => return None,
            };
            return Some((samples, usize::from(channels), rate));
        }
        // Chunks are padded to an even size.
        at = body.checked_add(size)?.checked_add(size & 1)?;
    }
}

/// Walk `parts` down from `root`. A part that does not exist as written is
/// looked up among its directory's entries without regard to case, because
/// the paths come from Windows.
fn find(root: &Path, parts: &[&str]) -> Option<PathBuf> {
    let mut at = root.to_path_buf();
    for part in parts {
        let plain = matches!(
            Path::new(part).components().next(),
            Some(Component::Normal(_))
        );
        if !plain {
            return None;
        }
        let exact = at.join(part);
        at = if exact.exists() {
            exact
        } else {
            let entries = std::fs::read_dir(&at).ok()?.filter_map(Result::ok);
            let same =
                |name: &std::ffi::OsString| name.to_string_lossy().eq_ignore_ascii_case(part);
            entries
                .map(|e| e.file_name())
                .find(same)
                .map(|n| at.join(n))?
        };
    }
    at.is_file().then_some(at)
}

/// The file of this name anywhere under `dir`, without regard to case.
fn by_name(dir: &Path, name: &str) -> Option<PathBuf> {
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)
            .ok()?
            .filter_map(|entry| Some(entry.ok()?.path()))
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                dirs.push(path);
            } else if path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(name))
            {
                return Some(path);
            }
        }
    }
    None
}

/// The file a state's path means, with FL's `Data` directory at `data`.
///
/// The placeholders a path starts with are dropped and the rest is looked
/// for under `data`, with and without a leading `Data`, and under the
/// install that `data` is in. A file that is at none of those is looked for
/// by its name among FL's impulses.
fn locate(data: &Path, path: &str) -> Option<PathBuf> {
    let path = path.replace('\\', "/");
    let placeholder = |part: &str| part.len() > 2 && part.starts_with('%') && part.ends_with('%');
    let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    let held = parts.iter().take_while(|part| placeholder(part)).count();
    let parts = &parts[held..];
    let name = parts.last()?;
    let mut found = find(data, parts);
    if let Some((first, rest)) = parts.split_first()
        && first.eq_ignore_ascii_case("data")
    {
        found = found.or_else(|| find(data, rest));
    }
    found
        .or_else(|| find(data.parent()?, parts))
        .or_else(|| by_name(&data.join(IMPULSES), name))
}

/// What became of the impulse a state names.
#[derive(Debug, Clone, PartialEq)]
enum Source {
    /// Nobody has said where FL's files are, or the file is not there.
    Missing,
    /// The file is there and is not one that is decoded.
    Unreadable,
    Loaded(Impulse),
}

/// Resample by `ratio`, which is the new rate over the old: a sinc under a
/// Hann window, 16 zero crossings a side, and wider for a lower rate.
fn resample(source: &[f32], ratio: f64) -> Vec<f32> {
    let len = (source.len() as f64 * ratio).round() as usize;
    let cutoff = ratio.min(1.0);
    let reach = (16.0 / cutoff).ceil();
    let (turn, window) = (PI * cutoff, PI / reach);
    let (turn_sin, turn_cos) = turn.sin_cos();
    let (window_sin, window_cos) = window.sin_cos();
    let mut out = Vec::with_capacity(len);
    let taps = 2 * reach as i64;
    for i in 0..len {
        let place = i as f64 / ratio;
        let first = place.floor() as i64 - reach as i64 + 1;
        // The taps that are inside the source.
        let from = (-first).clamp(0, taps);
        let to = (source.len() as i64 - first).clamp(from, taps);
        // How far a tap is from the place, and the sines there, which
        // turn by one step from a tap to the next.
        let mut x = (first + from) as f64 - place;
        let (mut sin, mut cos) = (turn * x).sin_cos();
        let (mut wide_sin, mut wide_cos) = (window * x).sin_cos();
        let mut sum = 0.0;
        for tap in from..to {
            let sinc = if x.abs() < 1e-9 {
                cutoff
            } else {
                sin / (PI * x)
            };
            sum += f64::from(source[(first + tap) as usize]) * sinc * (0.5 + 0.5 * wide_cos);
            x += 1.0;
            (sin, cos) = (
                sin * turn_cos + cos * turn_sin,
                cos * turn_cos - sin * turn_sin,
            );
            (wide_sin, wide_cos) = (
                wide_sin * window_cos + wide_cos * window_sin,
                wide_cos * window_cos - wide_sin * window_sin,
            );
        }
        out.push(sum as f32);
    }
    out
}

/// The loudest RMS of both sides of an impulse over a window of 300 ms,
/// which is what FL's normalize brings to 0.05.
fn loudest(impulse: &Impulse) -> f32 {
    let window = (300.0 * f64::from(impulse.rate) * 0.001)
        .round_ties_even()
        .max(1.0) as usize;
    let scale = (1.0 / (window * 2) as f64) as f32;
    let mut held = vec![0f32; window];
    let (mut at, mut sum, mut most) = (0, 0f64, 0f32);
    for (l, r) in impulse.left.iter().zip(&impulse.right) {
        let power = l * l + r * r;
        at = (at + 1) % window;
        sum = (sum - f64::from(held[at])) + f64::from(power);
        held[at] = power;
        most = most.max((sum * f64::from(scale)) as f32);
    }
    most.max(0.0).sqrt()
}

/// Fruity Convolver. A new one has FL's defaults: its default impulse at
/// the wet level's half, next to the input.
#[derive(Debug, Clone, PartialEq)]
pub struct Convolver {
    params: [i32; 4],
    by_file: bool,
    path: String,
    /// The WAV file of an impulse that is in the state.
    embedded: Vec<u8>,
    /// Whether the four layers are level lines, and the level of the
    /// equaliser's curve when it is one.
    layers_flat: bool,
    equaliser: Option<f32>,
    normalize: bool,
    /// The latency the state reports, in frames.
    reported: i32,
    predelay: i32,
    self_convolution: i32,
    stretch: i32,

    hz: f64,
    /// FL's `Data` directory.
    data: Option<PathBuf>,
    source: Source,
    /// The impulse was resampled.
    resampled: bool,
    engine: Engine,
    /// The frames the input waits before it is convolved, and the line
    /// they wait in, one side each.
    lead: usize,
    line: [Vec<f32>; 2],
    line_at: usize,
    /// The gains as the last block left them: the input's separation, the
    /// dry level and the wet mix.
    width: [f32; 4],
    dry: f32,
    wet: [f32; 4],
    /// The next block puts every gain where it belongs with no glide.
    snap: bool,
    scratch: [Vec<f32>; 2],
}

impl Default for Convolver {
    fn default() -> Self {
        Convolver {
            params: DEFAULTS,
            by_file: true,
            path: DEFAULT_PATH.to_owned(),
            embedded: Vec::new(),
            layers_flat: true,
            equaliser: Some(0.5),
            normalize: true,
            reported: 0,
            predelay: 0,
            self_convolution: 0,
            stretch: 0,
            hz: 44_100.0,
            data: None,
            source: Source::Missing,
            resampled: false,
            engine: Engine::new(&[], &[]),
            lead: 0,
            line: [Vec::new(), Vec::new()],
            line_at: 0,
            width: [1.0, 1.0, 0.0, 0.0],
            dry: 1.0,
            wet: [0.0; 4],
            snap: true,
            scratch: [vec![0.0; CHUNK], vec![0.0; CHUNK]],
        }
    }
}

impl Convolver {
    /// Find and decode the impulse the state names.
    fn load(&mut self) {
        self.source = if self.by_file {
            let file = self
                .data
                .as_deref()
                .and_then(|data| locate(data, &self.path));
            match file.and_then(|file| std::fs::read(file).ok()) {
                None => Source::Missing,
                Some(bytes) => decode(&bytes).map_or(Source::Unreadable, Source::Loaded),
            }
        } else {
            decode(&self.embedded).map_or(Source::Unreadable, Source::Loaded)
        };
        self.build();
    }

    /// Make the impulse ready for the rate and build the engine.
    fn build(&mut self) {
        self.resampled = false;
        self.lead = 0;
        let Source::Loaded(impulse) = &self.source else {
            self.engine = Engine::new(&[], &[]);
            self.line = [Vec::new(), Vec::new()];
            return;
        };
        let mut gain = 1.0f32;
        if self.normalize {
            let peak = loudest(impulse);
            if peak > 0.0 {
                gain = (0.05 / f64::from(peak)) as f32;
            }
        }
        let stretch = (f64::from(self.stretch as f32 / 1000.0) * 2f64.ln()).exp() as f32;
        let target = f64::from(self.hz as f32 * stretch);
        let ratio = target / f64::from(impulse.rate);
        self.resampled = (ratio - 1.0).abs() > 1e-9;
        let ready = |side: &[f32]| -> Vec<f32> {
            let scaled: Vec<f32> = side.iter().map(|v| v * gain).collect();
            if self.resampled {
                resample(&scaled, ratio)
            } else {
                scaled
            }
        };
        let (mut left, mut right) = (ready(&impulse.left), ready(&impulse.right));
        // A level equaliser curve is a gain: 1 at a half and 4 at the top.
        if let Some(level) = self.equaliser {
            let x = level * 2.0;
            if x != 1.0 {
                let curve = (f64::from(x) * 3f64.ln()).exp() - 1.0;
                let gain = (curve * 0.5) as f32;
                left.iter_mut().chain(&mut right).for_each(|v| *v *= gain);
            }
        }
        // What is too small to hear at the end is cut off, and at the
        // start it is delay.
        let loud = |(l, r): (&f32, &f32)| l.abs() >= SILENT || r.abs() >= SILENT;
        let end = left
            .iter()
            .zip(&right)
            .rposition(loud)
            .map_or(0, |at| at + 1);
        let start = left[..end]
            .iter()
            .zip(&right[..end])
            .position(loud)
            .unwrap_or(end);
        let knob = self.predelay as f32 / 1000.0;
        let seconds = f64::from(((f64::from(knob) * 7f64.ln()).exp() - 1.0) as f32);
        let predelay = (seconds * 83.333_333_333_333_33 * 0.001 * self.hz).round_ties_even();
        self.lead = predelay as usize + start;
        self.engine = Engine::new(&left[start..end], &right[start..end]);
        self.line = [vec![0.0; self.lead], vec![0.0; self.lead]];
        self.line_at = 0;
    }

    /// The gains the parameters come to: the input's separation, the dry
    /// level and the wet mix.
    fn gains(&self) -> ([f32; 4], f32, [f32; 4]) {
        let [dry, wet, width, circle] = self.params.map(|value| value as f32 / 1000.0);
        let dry = if dry == 1.0 {
            1.0
        } else {
            knob_gain(f64::from(dry)) as f32
        };
        let wet = ((f64::from(wet) * 1001f64.ln()).exp() - 1.0) as f32;
        let wet = (f64::from(wet) * 0.001) as f32;
        let wet = (f64::from(wet) * (44_100.0 / self.hz)) as f32;
        (
            stereo_gains(0.0, 1.0, width),
            dry,
            stereo_gains(0.0, wet, circle),
        )
    }

    /// Whether there is an impulse to convolve with.
    fn ready(&self) -> bool {
        matches!(self.source, Source::Loaded(_))
    }

    /// One block of no more than [`CHUNK`] frames.
    fn chunk(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len();
        let (width, dry, wet) = self.gains();
        if self.snap {
            (self.width, self.dry, self.wet) = (width, dry, wet);
            self.snap = false;
        }
        let cap = (44_100.0 / self.hz * 0.001) as f32;
        let mut gains = [(0.0, 0.0); 4];
        for k in 0..4 {
            gains[k] = glide(&mut self.width[k], width[k], cap, len);
        }
        let [wet_left, wet_right] = &mut self.scratch;
        let (wet_left, wet_right) = (&mut wet_left[..len], &mut wet_right[..len]);
        let mut now = gains.map(|gain| gain.0);
        for frame in 0..len {
            let (l, r) = (left[frame], right[frame]);
            wet_left[frame] = l * now[0] + r * now[2];
            wet_right[frame] = r * now[1] + l * now[3];
            for (gain, step) in now.iter_mut().zip(gains) {
                *gain += step.1;
            }
        }
        if self.lead > 0 {
            for (side, block) in [&mut *wet_left, &mut *wet_right].into_iter().enumerate() {
                let mut at = self.line_at;
                for sample in block.iter_mut() {
                    std::mem::swap(sample, &mut self.line[side][at]);
                    at = (at + 1) % self.lead;
                }
            }
            self.line_at = (self.line_at + len) % self.lead;
        }
        self.engine.process(0, wet_left);
        self.engine.process(1, wet_right);
        let (mut dry_now, dry_step) = glide(&mut self.dry, dry, cap, len);
        for k in 0..4 {
            gains[k] = glide(&mut self.wet[k], wet[k], cap, len);
        }
        let mut now = gains.map(|gain| gain.0);
        for frame in 0..len {
            let (l, r) = (wet_left[frame], wet_right[frame]);
            left[frame] = left[frame] * dry_now + l * now[0] + r * now[2];
            right[frame] = right[frame] * dry_now + r * now[1] + l * now[3];
            dry_now += dry_step;
            for (gain, step) in now.iter_mut().zip(gains) {
                *gain += step.1;
            }
        }
    }
}

impl Effect for Convolver {
    /// FL takes versions 10 to 15, and so does this. This decodes the
    /// impulse and builds the engine, so it reads a file and asks for
    /// memory.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let mut reader = Reader::new(bytes);
        let word = reader.i32()?;
        let version = if word >= 0x10000 { word >> 16 } else { word };
        if !(10..=15).contains(&version) {
            return Err(StateError::Version { found: word });
        }
        let mut params = [0; 4];
        for (param, range) in RANGES.iter().enumerate() {
            let value = reader.i32()?;
            if value < range.0 || value > range.1 {
                return Err(StateError::Range { param, value });
            }
            params[param] = value;
        }
        let by_file = reader.u8()? != 0;
        let mut len = usize::from(reader.u8()?);
        if len == 0xff {
            len = reader.u32()? as usize;
        }
        let path: String = reader.bytes(len)?.iter().map(|b| char::from(*b)).collect();
        let mut embedded = Vec::new();
        if !by_file {
            let len = usize::try_from(reader.i32()?).unwrap_or(usize::MAX);
            embedded = reader.bytes(len)?.to_vec();
        }
        reader.bytes(36)?;
        let mut layers_flat = true;
        for _ in 0..4 {
            let layer = MEnvelope::read(&mut reader, Class::Map)?;
            layers_flat &= layer.flat() == Some(0.5);
        }
        let equaliser = MEnvelope::read(&mut reader, Class::Map)?.flat();
        let flags = reader.bytes(4)?;
        let normalize = flags[3] != 0;
        let reported = if version >= 15 { reader.i32()? } else { 0 };
        let _tab = reader.i32()?;
        let predelay = reader.i32()?.clamp(0, 1000);
        let self_convolution = reader.i32()?.clamp(0, 1000);
        let stretch = reader.i32()?.clamp(-1000, 1000);
        if version >= 11 {
            let _equaliser_mix = reader.i32()?;
        }
        reader.end()?;

        *self = Convolver {
            params,
            by_file,
            path,
            embedded,
            layers_flat,
            equaliser,
            normalize,
            reported,
            predelay,
            self_convolution,
            stretch,
            hz: self.hz,
            data: self.data.take(),
            ..Convolver::default()
        };
        self.load();
        Ok(())
    }

    /// The impulse is made ready anew for the rate, which asks for memory.
    /// A rate that is not a number above 0 is not taken.
    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz > 0.0 && f64::from(hz) != self.hz {
            self.hz = f64::from(hz);
            self.build();
            self.snap = true;
        }
    }

    /// `dir` is FL's `Plugins/Fruity/Generators` directory, and FL's
    /// `Data` directory is the one next to `Plugins`. A directory that has
    /// `Patches` in it is taken for `Data` itself. This reads the impulse's
    /// file.
    fn set_resource_dir(&mut self, dir: &Path) {
        let install = dir.parent().and_then(Path::parent).and_then(Path::parent);
        let beside = install.map(|install| install.join("Data"));
        let own = dir.join("Patches").is_dir().then(|| dir.to_path_buf());
        self.data = own.or(beside.filter(|data| data.is_dir()));
        if self.by_file {
            self.load();
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if !self.ready() {
            return;
        }
        let len = left.len().min(right.len());
        let blocks = left[..len].chunks_mut(CHUNK);
        for (left, right) in blocks.zip(right[..len].chunks_mut(CHUNK)) {
            self.chunk(left, right);
        }
    }

    fn param_count(&self) -> usize {
        RANGES.len()
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |value| *value as f32)
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(range) = RANGES.get(index) {
            self.params[index] = knob_value(value, *range);
        }
    }

    /// Forgets the input so far.
    fn reset(&mut self) {
        self.engine.reset();
        for line in &mut self.line {
            line.fill(0.0);
        }
        self.line_at = 0;
        self.snap = true;
    }

    fn fidelity(&self) -> Fidelity {
        match self.source {
            Source::Missing => {
                return Fidelity::Approximate(
                    "Convolver's impulse file is not found, so its input passes as it is",
                );
            }
            Source::Unreadable => {
                return Fidelity::Approximate(
                    "Convolver's impulse is a kind of file that is not decoded, so its input \
                     passes as it is",
                );
            }
            Source::Loaded(_) => {}
        }
        if !self.layers_flat || self.self_convolution != 0 || self.equaliser.is_none() {
            Fidelity::Approximate(
                "Convolver's envelope layers, self-convolution and equaliser curve are left out",
            )
        } else if self.resampled {
            Fidelity::Approximate(
                "Convolver's impulse is resampled by a windowed sinc and not by FL's resampler",
            )
        } else {
            Fidelity::Exact
        }
    }

    /// The number the state has, which is no delay of the plugin's own: FL
    /// hands it to its host, and the host holds everything else back by as
    /// much.
    fn latency(&self) -> usize {
        let most = (5.0 * self.hz).round_ties_even() as i32;
        self.reported.clamp(0, most) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menvelope::tests::stream;
    use crate::testing::{corpus, every_preset, partial, projects, sine};
    use dsp::curve::Point;

    /// Numbers from -1 to 1 that look random and are the same every time.
    fn noise(len: usize, seed: u32) -> Vec<f32> {
        let mut state = seed;
        let mut next = || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 8) as f32 / 8_388_608.0 - 1.0
        };
        (0..len).map(|_| next()).collect()
    }

    /// The sum a convolution is, one tap at a time.
    fn direct(input: &[f32], impulse: &[f32]) -> Vec<f32> {
        let mut out = vec![0f64; input.len()];
        for (n, out) in out.iter_mut().enumerate() {
            for (k, tap) in impulse.iter().enumerate().take(n + 1) {
                *out += f64::from(*tap) * f64::from(input[n - k]);
            }
        }
        out.iter().map(|v| *v as f32).collect()
    }

    /// Run a block through one side of an engine, in pieces of odd sizes.
    fn convolved(engine: &mut Engine, side: usize, input: &[f32]) -> Vec<f32> {
        let mut out = input.to_vec();
        let mut at = 0;
        for size in [1, 7, 128, 300, 129, 64, 1000, 5].iter().cycle() {
            if at >= out.len() {
                break;
            }
            let end = (at + size).min(out.len());
            engine.process(side, &mut out[at..end]);
            at = end;
        }
        out
    }

    #[test]
    fn the_fft_goes_there_and_back_and_finds_a_tone_in_its_bin() {
        let fft = Fft::new(64);
        let mut re: Vec<f32> = sine(5.0 / 64.0, 64);
        let mut im = vec![0.0; 64];
        let kept = re.clone();
        fft.run(&mut re, &mut im, false);
        for bin in 0..64 {
            let size = re[bin].hypot(im[bin]);
            let wanted = if bin == 5 || bin == 59 { 32.0 } else { 0.0 };
            assert!((size - wanted).abs() < 1e-3, "{bin}: {size}");
        }
        fft.run(&mut re, &mut im, true);
        for (back, kept) in re.iter().zip(kept) {
            assert!((back / 64.0 - kept).abs() < 1e-5);
        }
    }

    #[test]
    fn a_unit_impulse_gives_the_input_back() {
        let mut engine = Engine::new(&[1.0], &[0.5]);
        let input = noise(3000, 1);
        assert_eq!(convolved(&mut engine, 0, &input), input);
        let half: Vec<f32> = input.iter().map(|v| v * 0.5).collect();
        assert_eq!(convolved(&mut engine, 1, &input), half);
        // And no impulse gives nothing.
        let mut none = Engine::new(&[], &[]);
        assert!(convolved(&mut none, 0, &input).iter().all(|v| *v == 0.0));
    }

    #[test]
    fn a_short_impulse_matches_the_sum_tap_by_tap() {
        // Long enough for the taps that are summed, the blocks of 128 and
        // the first of 512.
        let impulse = noise(1500, 2);
        let input = noise(6000, 3);
        let mut engine = Engine::new(&impulse, &impulse[..100]);
        let wanted = direct(&input, &impulse);
        let heard = convolved(&mut engine, 0, &input);
        for (n, (heard, wanted)) in heard.iter().zip(&wanted).enumerate() {
            // The sum is of up to 1500 products of numbers up to 1.
            assert!(
                (heard - wanted).abs() < 1e-5 * 30.0,
                "{n}: {heard} {wanted}"
            );
        }
        // The other side has its own impulse.
        let short = direct(&input, &impulse[..100]);
        let heard = convolved(&mut engine, 1, &input);
        for (heard, wanted) in heard.iter().zip(&short) {
            assert!((heard - wanted).abs() < 1e-5, "{heard} {wanted}");
        }
        // After a reset it does the same again.
        engine.reset();
        let again = convolved(&mut engine, 1, &input);
        assert_eq!(again, heard);
    }

    #[test]
    fn every_group_of_blocks_puts_its_taps_on_the_right_frame() {
        // A tap at either end of every group, and the last past them all.
        let places = [
            0, 127, 128, 895, 896, 3967, 3968, 16_255, 16_256, 65_407, 65_408, 99_999,
        ];
        let mut impulse = vec![0.0; 100_000];
        for (k, place) in places.iter().enumerate() {
            impulse[*place] = 0.5 + 0.03125 * k as f32;
        }
        let mut engine = Engine::new(&impulse, &[]);
        let sizes: Vec<usize> = engine.groups.iter().map(|group| group.size).collect();
        assert_eq!(sizes, [128, 512, 2048, 8192, 32_768]);
        assert_eq!(engine.groups[4].blocks, 2);
        let input = noise(140_000, 4);
        let heard = convolved(&mut engine, 0, &input);
        for n in (0..input.len()).step_by(37) {
            let mut wanted = 0.0;
            for place in places.iter().filter(|place| **place <= n) {
                wanted += impulse[*place] * input[n - place];
            }
            assert!(
                (heard[n] - wanted).abs() < 1e-4,
                "{n}: {} {wanted}",
                heard[n]
            );
        }
    }

    /// A WAV file of 16-bit samples, both sides the same.
    fn wav_file(rate: u32, samples: &[f32]) -> Vec<u8> {
        let data: Vec<u8> = samples
            .iter()
            .flat_map(|v| [(v * 32_768.0) as i16; 2])
            .flat_map(i16::to_le_bytes)
            .collect();
        let mut bytes = b"RIFF".to_vec();
        bytes.extend((36 + data.len() as u32).to_le_bytes());
        bytes.extend(b"WAVEfmt ");
        bytes.extend(16u32.to_le_bytes());
        bytes.extend([1u16, 2].iter().flat_map(|v| v.to_le_bytes()));
        bytes.extend(rate.to_le_bytes());
        bytes.extend((rate * 4).to_le_bytes());
        bytes.extend([4u16, 16].iter().flat_map(|v| v.to_le_bytes()));
        bytes.extend(b"data");
        bytes.extend((data.len() as u32).to_le_bytes());
        bytes.extend(data);
        bytes
    }

    /// What a state says of its impulse and its wheels.
    struct Saved {
        params: [i32; 4],
        path: Option<&'static str>,
        wav: Vec<u8>,
        normalize: bool,
        latency: i32,
        /// Pre-delay, self-convolution and stretch.
        wheels: [i32; 3],
        equaliser: f64,
    }

    impl Saved {
        /// An impulse in the state, all wet, with nothing done to it.
        fn new(wav: Vec<u8>) -> Self {
            Saved {
                params: [0, 1000, 0, 0],
                path: None,
                wav,
                normalize: false,
                latency: 0,
                wheels: [0; 3],
                equaliser: 0.5,
            }
        }

        fn bytes(&self) -> Vec<u8> {
            let mut bytes = 0xf0003i32.to_le_bytes().to_vec();
            for param in self.params {
                bytes.extend(param.to_le_bytes());
            }
            let path = self.path.unwrap_or("");
            bytes.push(u8::from(self.path.is_some()));
            bytes.push(path.len() as u8);
            bytes.extend(path.bytes());
            if self.path.is_none() {
                bytes.extend((self.wav.len() as i32).to_le_bytes());
                bytes.extend(&self.wav);
            }
            bytes.extend([0; 36]);
            let level = |y: f64| [Point::new(0.0, y), Point::new(1.0, y)];
            for _ in 0..4 {
                bytes.extend(stream(6, 3, &level(0.5), [0, -1, -1, -1, -1], &[]));
            }
            let curve = level(self.equaliser);
            bytes.extend(stream(3, 3, &curve, [0, -1, -1, -1, -1], &[]));
            bytes.extend([0, 0, 1, u8::from(self.normalize)]);
            bytes.extend(self.latency.to_le_bytes());
            let [predelay, convolution, stretch] = self.wheels;
            for word in [0, predelay, convolution, stretch, 1000] {
                bytes.extend(word.to_le_bytes());
            }
            bytes
        }

        fn load(&self) -> Convolver {
            let mut convolver = Convolver::default();
            convolver.set_sample_rate(44_100.0);
            convolver.set_state(&self.bytes()).unwrap();
            convolver
        }
    }

    /// What a click of 1 on both sides comes out as, on the left.
    fn click(convolver: &mut Convolver, len: usize) -> Vec<f32> {
        let mut left = vec![0.0; len];
        left[0] = 1.0;
        let mut right = left.clone();
        for (l, r) in left.chunks_mut(300).zip(right.chunks_mut(300)) {
            convolver.process(l, r);
        }
        assert_eq!(left, right);
        left
    }

    #[test]
    fn an_impulse_in_the_state_is_what_a_click_comes_out_as() {
        let impulse = [0.5, -0.25, 0.0, 0.125];
        let mut convolver = Saved::new(wav_file(44_100, &impulse)).load();
        assert_eq!(convolver.fidelity(), Fidelity::Exact);
        assert_eq!(convolver.latency(), 0);
        let heard = click(&mut convolver, 8);
        assert_eq!(heard[..4], impulse);
        assert!(heard[4..].iter().all(|v| *v == 0.0));

        // The dry level adds the input, and the wet level's half is 0.0306.
        let mut both = Saved::new(wav_file(44_100, &impulse));
        both.params = [1000, 500, 0, 0];
        let heard = click(&mut both.load(), 4);
        let wet = ((1001f64.sqrt() - 1.0) * 0.001) as f32;
        assert!((heard[0] - (1.0 + 0.5 * wet)).abs() < 1e-6, "{}", heard[0]);
        assert!((heard[1] + 0.25 * wet).abs() < 1e-6);

        // At another rate the wet level makes up for the longer impulse.
        let mut fast = Saved::new(wav_file(88_200, &[0.5])).load();
        fast.set_sample_rate(88_200.0);
        assert_eq!(click(&mut fast, 2), [0.25, 0.0]);
    }

    #[test]
    fn normalize_brings_the_loudest_300_ms_to_an_rms_of_a_twentieth() {
        // A second of a steady 0.5 on both sides: its RMS is 0.5.
        let mut loud = Saved::new(wav_file(1000, &[0.5; 1000]));
        loud.normalize = true;
        let mut convolver = loud.load();
        convolver.set_sample_rate(1000.0);
        let heard = click(&mut convolver, 4);
        // 44.1 is what the wet level makes of the rate.
        assert!((heard[0] - 0.05 * 44.1).abs() < 1e-3, "{}", heard[0]);

        // Shorter than the window, it is the whole of it over the window.
        let short = Impulse {
            left: vec![1.0; 30],
            right: vec![1.0; 30],
            rate: 1000,
        };
        assert!((loudest(&short) - 0.1f32.sqrt()).abs() < 1e-6);
    }

    #[test]
    fn the_wheels_delay_stretch_and_scale_the_impulse() {
        let mut late = Saved::new(wav_file(44_100, &[0.0, 0.0, 0.5]));
        late.wheels[0] = 500;
        let mut convolver = late.load();
        // (sqrt(7) - 1) / 12 of a second, and the two silent taps.
        let predelay = ((7f64.sqrt() - 1.0) / 12.0 * 44_100.0).round() as usize;
        assert_eq!(convolver.lead, predelay + 2);
        let heard = click(&mut convolver, predelay + 8);
        assert_eq!(heard[predelay + 2], 0.5);
        assert_eq!(heard.iter().filter(|v| **v != 0.0).count(), 1);

        // A stretch of 1000 is twice as long.
        let tone = sine(0.05, 2000);
        let mut long = Saved::new(wav_file(44_100, &tone));
        long.wheels[2] = 1000;
        let mut convolver = long.load();
        assert!(matches!(convolver.fidelity(), Fidelity::Approximate(_)));
        let heard = click(&mut convolver, 4000);
        assert!((partial(&heard[400..3600], 0.025) - 1.0).abs() < 0.02);

        // The top of a level equaliser curve is 12 dB.
        let mut bright = Saved::new(wav_file(44_100, &[0.125]));
        bright.equaliser = 1.0;
        assert_eq!(click(&mut bright.load(), 2), [0.5, 0.0]);

        // And the latency is the state's number, up to five seconds.
        let mut told = Saved::new(wav_file(44_100, &[0.5]));
        told.latency = 512;
        assert_eq!(told.load().latency(), 512);
        told.latency = 1 << 30;
        assert_eq!(told.load().latency(), 220_500);
    }

    #[test]
    fn the_separations_merge_the_input_and_widen_the_wet_sound() {
        let mut mono = Saved::new(wav_file(44_100, &[0.5]));
        mono.params = [0, 1000, 1000, 0];
        let mut convolver = mono.load();
        let (mut left, mut right) = ([1.0f32, 0.0], [0.0f32, 0.0]);
        convolver.process(&mut left, &mut right);
        assert!((left[0] - 0.25).abs() < 1e-6 && (right[0] - 0.25).abs() < 1e-6);

        let mut wide = Saved::new(wav_file(44_100, &[0.5]));
        wide.params = [0, 1000, 0, -1000];
        let mut convolver = wide.load();
        let (mut left, mut right) = ([1.0f32, 0.0], [0.0f32, 0.0]);
        convolver.process(&mut left, &mut right);
        assert!((left[0] - 0.75).abs() < 1e-6 && (right[0] + 0.25).abs() < 1e-6);
    }

    #[test]
    fn a_resampled_tone_keeps_its_pitch_and_its_level() {
        let tone = sine(0.02, 4000);
        let up = resample(&tone, 48_000.0 / 44_100.0);
        assert_eq!(up.len(), 4354);
        let turns = 0.02 * 44_100.0 / 48_000.0;
        let whole = (1.0 / turns * 40.0f64).round() as usize;
        assert!((partial(&up[500..500 + whole], 40.0 / whole as f64) - 1.0).abs() < 1e-2);
        // Down to half the rate, a tone above the new band is gone.
        let high = sine(0.4, 4000);
        let down = resample(&high, 0.5);
        assert!(down[200..1800].iter().all(|v| v.abs() < 0.01));
        let low = resample(&tone, 0.5);
        assert!((partial(&low[500..1500], 0.04) - 1.0).abs() < 1e-2);
    }

    /// A directory laid out as FL's install is, gone when the test is.
    struct Install(PathBuf);

    impl Install {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("fl-rs-{name}-{}", std::process::id()));
            std::fs::create_dir_all(dir.join("Plugins/Fruity/Generators")).unwrap();
            std::fs::create_dir_all(dir.join("Data/Patches/Impulses/Halls")).unwrap();
            Install(dir)
        }

        fn generators(&self) -> PathBuf {
            self.0.join("Plugins/Fruity/Generators")
        }
    }

    impl Drop for Install {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    #[test]
    fn an_impulse_in_a_file_is_found_from_fl_s_plugin_directory() {
        let install = Install::new("convolver");
        let halls = install.0.join("Data/Patches/Impulses/Halls");
        std::fs::write(halls.join("IMP Hall.wav"), wav_file(44_100, &[0.5, 0.25])).unwrap();
        let paths = [
            "%FLStudioData%\\Patches\\Impulses\\Halls\\IMP Hall.wav",
            "%FLStudioFactoryData%\\Data\\Patches\\Impulses\\Halls\\IMP Hall.wav",
            "%FLStudioData%\\patches\\impulses\\HALLS\\imp hall.WAV",
            "%FLStudioData%\\%FLStudioData%\\Patches\\Impulses\\Halls\\IMP Hall.wav",
            // A path on its maker's disk is found by its name.
            "%USERPROFILE%\\Dropbox\\Impulses\\IMP Hall.wav",
        ];
        for path in paths {
            let mut saved = Saved::new(Vec::new());
            saved.path = Some(path);
            // Told the directory before the state, and after it.
            let mut first = Convolver::default();
            first.set_resource_dir(&install.generators());
            first.set_state(&saved.bytes()).unwrap();
            let mut second = Convolver::default();
            second.set_state(&saved.bytes()).unwrap();
            assert!(matches!(second.fidelity(), Fidelity::Approximate(_)));
            second.set_resource_dir(&install.generators());
            for convolver in [&mut first, &mut second] {
                assert_eq!(convolver.fidelity(), Fidelity::Exact, "{path}");
                assert_eq!(click(convolver, 3), [0.5, 0.25, 0.0], "{path}");
            }
        }
        // `Data` itself will do as well.
        let mut saved = Saved::new(Vec::new());
        saved.path = Some(paths[0]);
        let mut direct = Convolver::default();
        direct.set_resource_dir(&install.0.join("Data"));
        direct.set_state(&saved.bytes()).unwrap();
        assert_eq!(direct.fidelity(), Fidelity::Exact);
    }

    #[test]
    fn without_its_impulse_the_plugin_passes_its_input_and_says_so() {
        let install = Install::new("convolver-missing");
        let halls = install.0.join("Data/Patches/Impulses/Halls");
        std::fs::write(halls.join("noise.wav"), b"RIFF____WAVEnothing").unwrap();
        for (path, what) in [
            ("%FLStudioData%\\Patches\\Impulses\\gone.wv", "not found"),
            ("%FLStudioData%\\..\\..\\secret.wav", "not found"),
            (
                "%FLStudioData%\\Patches\\Impulses\\Halls\\noise.wav",
                "not decoded",
            ),
        ] {
            let mut saved = Saved::new(Vec::new());
            saved.path = Some(path);
            let mut convolver = Convolver::default();
            convolver.set_resource_dir(&install.generators());
            convolver.set_state(&saved.bytes()).unwrap();
            let Fidelity::Approximate(said) = convolver.fidelity() else {
                panic!("{path}");
            };
            assert!(said.contains(what), "{path}: {said}");
            let (mut left, mut right) = ([0.5f32, -0.25], [0.125f32, 1.0]);
            convolver.process(&mut left, &mut right);
            assert_eq!((left, right), ([0.5, -0.25], [0.125, 1.0]));
        }
        // A new one waits for its default impulse.
        assert!(crate::by_name("fruity convolver").is_some());
        let mut new = Convolver::default();
        assert!(matches!(new.fidelity(), Fidelity::Approximate(_)));
        assert_eq!(new.param(1), 500.0);
        new.set_param(1, 2000.0);
        assert_eq!((new.param(1), new.param_count()), (1000.0, 4));
    }

    #[test]
    fn the_kinds_of_wav_file_decode_and_a_bad_state_is_turned_down() {
        // 32-bit floats, one side.
        let mut float = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
        float.extend(16u32.to_le_bytes());
        float.extend([3u16, 1].iter().flat_map(|v| v.to_le_bytes()));
        float.extend(48_000u32.to_le_bytes());
        float.extend(192_000u32.to_le_bytes());
        float.extend([4u16, 32].iter().flat_map(|v| v.to_le_bytes()));
        float.extend(b"JUNK");
        float.extend(3u32.to_le_bytes());
        float.extend([0; 4]);
        float.extend(b"data");
        float.extend(8u32.to_le_bytes());
        float.extend([0.5f32, -1.5].iter().flat_map(|v| v.to_le_bytes()));
        let impulse = decode(&float).unwrap();
        assert_eq!(impulse.rate, 48_000);
        assert_eq!(
            (impulse.left.clone(), impulse.right),
            (vec![0.5, -1.5], impulse.left)
        );
        assert_eq!(decode(b"OggS"), None);
        assert_eq!(decode(&float[..40]), None);

        let good = Saved::new(wav_file(44_100, &[0.5])).bytes();
        let mut convolver = Convolver::default();
        assert!(convolver.set_state(&good).is_ok());
        let before = convolver.clone();
        for len in [0, 3, 20, good.len() - 1] {
            let cut = convolver.set_state(&good[..len]);
            assert!(matches!(cut, Err(StateError::Length { .. })), "{len}");
        }
        let mut long = good.clone();
        long.push(0);
        assert!(convolver.set_state(&long).is_err());
        let mut old = good.clone();
        old[..4].copy_from_slice(&9i32.to_le_bytes());
        assert_eq!(
            convolver.set_state(&old),
            Err(StateError::Version { found: 9 })
        );
        let mut loud = good.clone();
        loud[4..8].copy_from_slice(&1001i32.to_le_bytes());
        let range = StateError::Range {
            param: 0,
            value: 1001,
        };
        assert_eq!(convolver.set_state(&loud), Err(range));
        assert_eq!(convolver, before);
    }

    #[test]
    fn every_state_fl_ships_reads_to_its_last_byte_and_finds_its_impulse() {
        let Some(root) = corpus() else {
            eprintln!("FL_RS_CORPUS unset, the states of Convolver skipped");
            return;
        };
        let mut saved = every_preset(&root, NAME);
        let shipped = saved.len();
        let named = |saved: &crate::testing::Saved| saved.plugin.eq_ignore_ascii_case(NAME);
        saved.extend(projects(&root).into_iter().filter(named));
        assert_eq!((shipped, saved.len()), (264, 285));
        let (mut found, mut exact, mut late) = (0, 0, 0);
        for saved in &saved[..shipped] {
            // A preset is read and its impulse is looked for. Decoding 264
            // of them is left to the songs that load them.
            let mut convolver = Convolver::default();
            if let Err(error) = convolver.set_state(&saved.state) {
                panic!("{}: {error}", saved.file);
            }
            assert!(convolver.by_file, "{}", saved.file);
            late += usize::from(convolver.latency() > 0);
            found += usize::from(locate(&root, &convolver.path).is_some());
        }
        assert_eq!(found, 264);
        let generators = root.join("../Plugins/Fruity/Generators");
        let mut ready = 0;
        for saved in &saved[shipped..] {
            let mut convolver = Convolver::default();
            convolver.set_resource_dir(&generators);
            if let Err(error) = convolver.set_state(&saved.state) {
                panic!("{}: {error}", saved.file);
            }
            late += usize::from(convolver.latency() > 0);
            ready += usize::from(convolver.ready());
            exact += usize::from(convolver.fidelity() == Fidelity::Exact);
            let (mut left, mut right) = (sine(0.01, 2048), sine(0.037, 2048));
            for (l, r) in left.chunks_mut(300).zip(right.chunks_mut(300)) {
                convolver.process(l, r);
            }
            let sound = left.iter().chain(&right);
            assert!(
                sound.clone().all(|v| v.is_finite() && v.abs() < 100.0),
                "{}",
                saved.file
            );
        }
        assert_eq!(late, 2);
        eprintln!("convolver: {ready} of 21 ready, {exact} exact");
    }
}
